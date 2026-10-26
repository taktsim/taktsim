//! The YAML subset of ADR-0002, read with `saphyr-parser` (ADR-0020). This
//! is the only module that uses the parser crate.
//!
//! The parser produces events with spans; this module builds the [`Node`]
//! tree from them and enforces the subset: one document, no anchors,
//! aliases, tags or merge keys, text keys only, each key once per mapping,
//! and plain scalars resolved by [`scalar::plain`].

use std::collections::BTreeMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Span};

use super::{Entry, MAX_NESTING, Node, SyntaxError, Value, scalar};
use crate::error::{FieldPath, LoadErrorKind, Location, Segment};

/// saphyr's lines count from 1 and its columns from 0, in characters.
fn location(marker: saphyr_parser::Marker) -> Location {
    Location::new(marker.line(), marker.col() + 1)
}

/// A list or mapping being built.
enum Frame {
    List {
        start: Location,
        items: Vec<Node>,
    },
    Map {
        start: Location,
        entries: Vec<Entry>,
        seen: BTreeMap<String, Location>,
        /// The key read, waiting for its value; `None` if the next scalar
        /// is a key.
        key: Option<Key>,
    },
}

/// A mapping key waiting for its value.
struct Key {
    name: String,
    at: Location,
    /// `false` for a repeated key: its value is read but dropped, so the
    /// first entry stays.
    keep: bool,
}

struct Builder {
    stack: Vec<Frame>,
    root: Option<Node>,
    errors: Vec<SyntaxError>,
}

impl Builder {
    /// The path to the value being read: the keys and indices of the open
    /// frames.
    fn path(&self) -> FieldPath {
        let segments = self
            .stack
            .iter()
            .filter_map(|frame| match frame {
                Frame::List { items, .. } => Some(Segment::Index(items.len())),
                Frame::Map { key, .. } => key.as_ref().map(|k| Segment::Key(k.name.clone())),
            })
            .collect::<Vec<_>>();
        FieldPath::from(segments)
    }

    fn error(&mut self, at: Location, kind: LoadErrorKind) {
        let path = self.path();
        self.errors.push(SyntaxError {
            location: Some(at),
            path,
            kind,
        });
    }

    /// Whether the next node is a mapping key.
    fn expects_key(&self) -> bool {
        matches!(self.stack.last(), Some(Frame::Map { key: None, .. }))
    }

    /// Adds a finished value to the open frame, or makes it the document.
    fn push(&mut self, node: Node) {
        match self.stack.last_mut() {
            None => self.root = Some(node),
            Some(Frame::List { items, .. }) => items.push(node),
            Some(Frame::Map { entries, key, .. }) => {
                if let Some(k) = key.take().filter(|k| k.keep) {
                    entries.push(Entry::new(k.name, k.at, node));
                }
            }
        }
    }

    /// Reads a scalar as a mapping key.
    fn key(&mut self, text: String, style: ScalarStyle, at: Location) {
        let text = if style == ScalarStyle::Plain {
            if text == "<<" {
                self.error(at, LoadErrorKind::MergeKey);
            }
            match scalar::plain(&text) {
                Ok(Value::Str(s)) => s,
                Ok(value) => {
                    let found = Node::new(value, at, at).found();
                    self.error(at, LoadErrorKind::NonStringKey { found });
                    text
                }
                Err(kind) => {
                    self.error(at, kind);
                    text
                }
            }
        } else {
            text
        };
        let Some(Frame::Map { seen, key, .. }) = self.stack.last_mut() else {
            return;
        };
        let first = seen.get(&text).copied();
        if first.is_none() {
            seen.insert(text.clone(), at);
        }
        // The key is in the path of any error about it or its value.
        *key = Some(Key {
            name: text.clone(),
            at,
            keep: first.is_none(),
        });
        if let Some(first) = first {
            self.error(at, LoadErrorKind::DuplicateKey { key: text, first });
        }
    }

    fn scalar(&mut self, text: String, style: ScalarStyle, span: Span) {
        let (start, end) = (location(span.start), location(span.end));
        if self.expects_key() {
            self.key(text, style, start);
            return;
        }
        let value = if style == ScalarStyle::Plain {
            scalar::plain(&text).unwrap_or_else(|kind| {
                self.error(start, kind);
                Value::Str(text)
            })
        } else {
            Value::Str(text)
        };
        self.push(Node::new(value, start, end));
    }

    /// Opens a list or mapping; `false` if that is not possible.
    fn open(&mut self, frame: Frame, at: Location) -> bool {
        if self.expects_key() {
            self.error(at, LoadErrorKind::ComplexKey);
            return false;
        }
        if self.stack.len() >= MAX_NESTING {
            self.error(at, LoadErrorKind::TooDeep { limit: MAX_NESTING });
            return false;
        }
        self.stack.push(frame);
        true
    }

    fn close(&mut self, end: Location) {
        let node = match self.stack.pop() {
            Some(Frame::List { start, items }) => Node::new(Value::List(items), start, end),
            Some(Frame::Map { start, entries, .. }) => Node::new(Value::Map(entries), start, end),
            None => return,
        };
        self.push(node);
    }
}

/// Parses YAML `text` into a tree.
pub(super) fn parse(text: &str) -> Result<Node, Vec<SyntaxError>> {
    let mut b = Builder {
        stack: Vec::new(),
        root: None,
        errors: Vec::new(),
    };
    let mut documents = 0;
    for event in Parser::new_from_str(text) {
        let (event, span) = match event {
            Ok(event) => event,
            Err(e) => {
                let at = location(*e.marker());
                b.error(
                    at,
                    LoadErrorKind::Yaml {
                        problem: e.info().to_owned(),
                    },
                );
                return Err(b.errors);
            }
        };
        let start = location(span.start);
        match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
            Event::DocumentStart(_) => {
                documents += 1;
                if documents > 1 {
                    b.error(start, LoadErrorKind::MultipleDocuments);
                    return Err(b.errors);
                }
            }
            Event::Alias(_) => {
                b.error(start, LoadErrorKind::Alias);
                if b.expects_key() {
                    return Err(b.errors);
                }
                b.push(Node::new(Value::Null, start, location(span.end)));
            }
            Event::Scalar(text, style, anchor, tag) => {
                if anchor != 0 {
                    b.error(start, LoadErrorKind::Anchor);
                }
                if let Some(tag) = tag {
                    let tag = format!("{}{}", tag.handle, tag.suffix);
                    b.error(start, LoadErrorKind::Tag { tag });
                }
                b.scalar(text.into_owned(), style, span);
            }
            Event::SequenceStart(anchor, ref tag) | Event::MappingStart(anchor, ref tag) => {
                if anchor != 0 {
                    b.error(start, LoadErrorKind::Anchor);
                }
                if let Some(tag) = tag {
                    let tag = format!("{}{}", tag.handle, tag.suffix);
                    b.error(start, LoadErrorKind::Tag { tag });
                }
                let frame = if matches!(event, Event::SequenceStart(..)) {
                    Frame::List {
                        start,
                        items: Vec::new(),
                    }
                } else {
                    Frame::Map {
                        start,
                        entries: Vec::new(),
                        seen: BTreeMap::new(),
                        key: None,
                    }
                };
                if !b.open(frame, start) {
                    return Err(b.errors);
                }
            }
            Event::SequenceEnd | Event::MappingEnd => b.close(location(span.end)),
        }
    }
    match (b.root, b.errors.is_empty()) {
        (Some(root), true) => Ok(root),
        (None, true) => Err(vec![SyntaxError {
            location: None,
            path: FieldPath::default(),
            kind: LoadErrorKind::Empty,
        }]),
        (_, false) => Err(b.errors),
    }
}

/// The value of a tree as text, for tests: `Found` for scalars.
#[cfg(test)]
fn describe(node: &Node) -> String {
    match node.value() {
        Value::List(items) => format!(
            "[{}]",
            items.iter().map(describe).collect::<Vec<_>>().join(", ")
        ),
        Value::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|e| format!("{}: {}", e.key(), describe(e.value())))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
            node.found().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Found;

    fn kinds(text: &str) -> Vec<(String, String, LoadErrorKind)> {
        parse(text)
            .unwrap_err()
            .into_iter()
            .map(|e| {
                (
                    e.location.map(|l| l.to_string()).unwrap_or_default(),
                    e.path.to_string(),
                    e.kind,
                )
            })
            .collect()
    }

    fn one(text: &str) -> (String, String, LoadErrorKind) {
        let mut all = kinds(text);
        assert_eq!(all.len(), 1, "{all:?}");
        all.remove(0)
    }

    #[test]
    fn builds_trees_with_locations() {
        let node = parse("a: [1, x]\nb:\n  c: 'd'\n").unwrap();
        assert_eq!(
            describe(&node),
            "{a: [the number 1, the text `x`], b: {c: the text `d`}}"
        );
        let Value::Map(entries) = node.value() else {
            panic!()
        };
        assert_eq!(entries[1].key_start(), Location::new(2, 1));
        let Value::Map(inner) = entries[1].value().value() else {
            panic!()
        };
        assert_eq!(inner[0].value().start(), Location::new(3, 6));
        assert_eq!(inner[0].value().end(), Location::new(3, 9));
    }

    #[test]
    fn quoted_and_block_scalars_are_text() {
        let node = parse("a: '42'\nb: \"true\"\nc: |\n  x\nd: >-\n  y\n").unwrap();
        assert_eq!(
            describe(&node),
            "{a: the text `42`, b: the text `true`, c: the text `x\n`, d: the text `y`}"
        );
    }

    #[test]
    fn duplicate_keys_are_errors_with_both_places() {
        let (at, path, kind) = one("a: 1\nb: 2\na: 3\n");
        assert_eq!((at.as_str(), path.as_str()), ("3:1", "a"));
        assert_eq!(
            kind,
            LoadErrorKind::DuplicateKey {
                key: "a".into(),
                first: Location::new(1, 1)
            }
        );
        // Quoting does not make a key different.
        assert!(matches!(
            one("a: 1\n'a': 2\n").2,
            LoadErrorKind::DuplicateKey { .. }
        ));
    }

    #[test]
    fn anchors_aliases_tags_and_merge_keys_are_errors() {
        let all = kinds("x: &a [1]\nw: *a\nz: !!str 3\n<<: {}\n");
        let found: Vec<_> = all
            .iter()
            .map(|(at, path, k)| (at.as_str(), path.as_str(), k))
            .collect();
        assert_eq!(
            found,
            [
                ("1:4", "x", &LoadErrorKind::Anchor),
                ("2:4", "w", &LoadErrorKind::Alias),
                (
                    "3:4",
                    "z",
                    &LoadErrorKind::Tag {
                        tag: "tag:yaml.org,2002:str".into()
                    }
                ),
                ("4:1", "(document)", &LoadErrorKind::MergeKey),
            ]
        );
        assert!(matches!(
            one("a: !custom {}\n").2,
            LoadErrorKind::Tag { .. }
        ));
        assert_eq!(one("a: &x 1\n").2, LoadErrorKind::Anchor);
        let keys: Vec<LoadErrorKind> = kinds("k: &x 1\n*x : 2\n")
            .into_iter()
            .map(|e| e.2)
            .collect();
        assert_eq!(keys, [LoadErrorKind::Anchor, LoadErrorKind::Alias]);
    }

    #[test]
    fn keys_must_be_text() {
        assert_eq!(
            one("1: a\n").2,
            LoadErrorKind::NonStringKey {
                found: Found::Int(1)
            }
        );
        assert_eq!(one("? [a]\n: b\n").2, LoadErrorKind::ComplexKey);
        assert_eq!(one("? {a: 1}\n: b\n").2, LoadErrorKind::ComplexKey);
        assert!(matches!(one("yes: 1\n").2, LoadErrorKind::Ambiguous { .. }));
        assert_eq!(
            parse("'1': a\n").map(|n| describe(&n)),
            Ok("{1: the text `a`}".into())
        );
    }

    #[test]
    fn ambiguous_scalars_are_errors_with_their_path() {
        let (at, path, kind) = one("model:\n  flag: no\n");
        assert_eq!((at.as_str(), path.as_str()), ("2:9", "model.flag"));
        assert!(matches!(kind, LoadErrorKind::Ambiguous { .. }));
        let (at, path, _) = one("list: [1, 2, 0x10]\n");
        assert_eq!((at.as_str(), path.as_str()), ("1:14", "list[2]"));
    }

    #[test]
    fn one_document_per_file() {
        assert_eq!(one("--- 1\n--- 2\n").2, LoadErrorKind::MultipleDocuments);
        assert!(parse("--- 1\n...\n").is_ok());
    }

    #[test]
    fn syntax_errors_carry_the_parser_message_and_location() {
        let (at, _, kind) = one("a: [b, c\n");
        assert_eq!(at, "2:1");
        assert!(matches!(kind, LoadErrorKind::Yaml { .. }));
    }

    #[test]
    fn nesting_is_limited() {
        let deep = format!(
            "{}{}",
            "[".repeat(MAX_NESTING + 1),
            "]".repeat(MAX_NESTING + 1)
        );
        assert_eq!(one(&deep).2, LoadErrorKind::TooDeep { limit: MAX_NESTING });
        let ok = format!("{}{}", "[".repeat(MAX_NESTING), "]".repeat(MAX_NESTING));
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn columns_count_characters() {
        let node = parse("é: [x]\n").unwrap();
        let Value::Map(entries) = node.value() else {
            panic!()
        };
        assert_eq!(entries[0].value().start(), Location::new(1, 4));
    }

    #[test]
    fn an_explicit_empty_document_is_null() {
        // YAML gives `---` without content a null node; the decoder then
        // reports that the document is not a mapping.
        assert_eq!(parse("---\n...\n").unwrap().value(), &Value::Null);
    }
}
