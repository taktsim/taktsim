//! A strict JSON parser (RFC 8259) that keeps locations and rejects
//! duplicate keys (ADR-0020).
//!
//! YAML 1.2 is nearly a superset of JSON, but YAML parsers reject some JSON
//! (a tab after `:`) and accept much that is not JSON, so JSON gets its own
//! parser. It is recursive, with depth limited to [`MAX_NESTING`].

use std::collections::BTreeMap;

use super::{Entry, MAX_NESTING, Node, SyntaxError, Value, scalar};
use crate::error::{FieldPath, JsonProblem, LoadErrorKind, Location, Segment};

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    /// The column of `pos`, counted in characters from 1.
    column: usize,
    path: FieldPath,
}

/// A problem found by the parser, before it is placed in the document.
type Problem = (Location, LoadErrorKind);

impl Parser<'_> {
    fn location(&self) -> Location {
        Location::new(self.line, self.column)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Moves past one byte, counting characters by their first byte.
    fn bump(&mut self) {
        if let Some(&b) = self.bytes.get(self.pos) {
            self.pos += 1;
            if b == b'\n' {
                self.line += 1;
                self.column = 1;
            } else if b & 0xC0 != 0x80 {
                self.column += 1;
            }
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    fn problem(&self, problem: JsonProblem) -> Problem {
        (self.location(), LoadErrorKind::Json { problem })
    }

    /// The problem at the current position: the end of the text, or an
    /// unexpected character.
    fn unexpected(&self) -> Problem {
        match self.text[self.pos..].chars().next() {
            Some(c) => self.problem(JsonProblem::UnexpectedChar(c)),
            None => self.problem(JsonProblem::UnexpectedEnd),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, Problem> {
        let start = self.location();
        let value = match self.peek() {
            Some(b'{') => self.object(depth)?,
            Some(b'[') => self.array(depth)?,
            Some(b'"') => Value::Str(self.string()?),
            Some(b'-' | b'0'..=b'9') => self.number()?,
            Some(b't' | b'f' | b'n') => self.literal()?,
            Some(_) | None => return Err(self.unexpected()),
        };
        Ok(Node::new(value, start, self.location()))
    }

    fn enter(&self, depth: usize) -> Result<usize, Problem> {
        if depth >= MAX_NESTING {
            return Err((
                self.location(),
                LoadErrorKind::TooDeep { limit: MAX_NESTING },
            ));
        }
        Ok(depth + 1)
    }

    fn object(&mut self, depth: usize) -> Result<Value, Problem> {
        let depth = self.enter(depth)?;
        self.bump();
        let mut entries = Vec::new();
        let mut seen = BTreeMap::new();
        self.whitespace();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Value::Map(entries));
        }
        loop {
            self.whitespace();
            if self.peek() != Some(b'"') {
                return Err(if self.peek() == Some(b'}') {
                    self.problem(JsonProblem::TrailingComma)
                } else {
                    self.unexpected()
                });
            }
            let key_start = self.location();
            let key = self.string()?;
            self.path.push(Segment::Key(key.clone()));
            if let Some(&first) = seen.get(&key) {
                return Err((key_start, LoadErrorKind::DuplicateKey { key, first }));
            }
            seen.insert(key.clone(), key_start);
            self.whitespace();
            if self.peek() != Some(b':') {
                return Err(self.unexpected());
            }
            self.bump();
            self.whitespace();
            let value = self.value(depth)?;
            self.path.pop();
            entries.push(Entry::new(key, key_start, value));
            self.whitespace();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    return Ok(Value::Map(entries));
                }
                Some(_) | None => return Err(self.unexpected()),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Problem> {
        let depth = self.enter(depth)?;
        self.bump();
        let mut items = Vec::new();
        self.whitespace();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(Value::List(items));
        }
        loop {
            self.whitespace();
            if self.peek() == Some(b']') {
                return Err(self.problem(JsonProblem::TrailingComma));
            }
            self.path.push(Segment::Index(items.len()));
            let item = self.value(depth)?;
            self.path.pop();
            items.push(item);
            self.whitespace();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b']') => {
                    self.bump();
                    return Ok(Value::List(items));
                }
                Some(_) | None => return Err(self.unexpected()),
            }
        }
    }

    fn string(&mut self) -> Result<String, Problem> {
        self.bump();
        let mut out = String::new();
        loop {
            let run_start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.bump();
            }
            out.push_str(&self.text[run_start..self.pos]);
            match self.peek() {
                None => return Err(self.problem(JsonProblem::UnexpectedEnd)),
                Some(b'"') => {
                    self.bump();
                    return Ok(out);
                }
                Some(b'\\') => {
                    let at = self.location();
                    self.bump();
                    out.push(self.escape(at)?);
                }
                Some(_) => return Err(self.problem(JsonProblem::ControlCharacter)),
            }
        }
    }

    /// The character of the escape after a `\` at `at`.
    fn escape(&mut self, at: Location) -> Result<char, Problem> {
        let invalid = |problem| (at, LoadErrorKind::Json { problem });
        let c = match self.peek() {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            Some(b'u') => {
                self.bump();
                let high = self
                    .hex4()
                    .ok_or_else(|| invalid(JsonProblem::InvalidEscape))?;
                return match high {
                    0xD800..=0xDBFF => {
                        if self.peek() != Some(b'\\') {
                            return Err(invalid(JsonProblem::LoneSurrogate));
                        }
                        self.bump();
                        if self.peek() != Some(b'u') {
                            return Err(invalid(JsonProblem::LoneSurrogate));
                        }
                        self.bump();
                        let low = self
                            .hex4()
                            .ok_or_else(|| invalid(JsonProblem::InvalidEscape))?;
                        if !(0xDC00..=0xDFFF).contains(&low) {
                            return Err(invalid(JsonProblem::LoneSurrogate));
                        }
                        let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                        char::from_u32(code).ok_or_else(|| invalid(JsonProblem::LoneSurrogate))
                    }
                    0xDC00..=0xDFFF => Err(invalid(JsonProblem::LoneSurrogate)),
                    _ => char::from_u32(high).ok_or_else(|| invalid(JsonProblem::InvalidEscape)),
                };
            }
            Some(_) | None => return Err(invalid(JsonProblem::InvalidEscape)),
        };
        self.bump();
        Ok(c)
    }

    /// Four hex digits.
    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.pos..self.pos + 4)?;
        let text = std::str::from_utf8(digits).ok()?;
        if !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from_str_radix(text, 16).ok()?;
        for _ in 0..4 {
            self.bump();
        }
        Some(value)
    }

    fn number(&mut self) -> Result<Value, Problem> {
        let start = self.location();
        let from = self.pos;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.bump();
        }
        let text = &self.text[from..self.pos];
        if !scalar::is_json_number(text) {
            return Err((
                start,
                LoadErrorKind::Json {
                    problem: JsonProblem::InvalidNumber,
                },
            ));
        }
        scalar::number(text).map_err(|kind| (start, kind))
    }

    fn literal(&mut self) -> Result<Value, Problem> {
        let start = self.location();
        let from = self.pos;
        while self.peek().is_some_and(|b| b.is_ascii_alphabetic()) {
            self.bump();
        }
        match &self.text[from..self.pos] {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "null" => Ok(Value::Null),
            _ => Err((
                start,
                LoadErrorKind::Json {
                    problem: JsonProblem::InvalidLiteral,
                },
            )),
        }
    }
}

/// Parses JSON `text` into a tree; the first problem stops the parser.
pub(super) fn parse(text: &str) -> Result<Node, Box<SyntaxError>> {
    let mut p = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        line: 1,
        column: 1,
        path: FieldPath::default(),
    };
    p.whitespace();
    if p.peek().is_none() {
        return Err(Box::new(SyntaxError {
            location: None,
            path: FieldPath::default(),
            kind: LoadErrorKind::Empty,
        }));
    }
    let result = p.value(0).and_then(|node| {
        p.whitespace();
        if p.peek().is_some() {
            return Err(p.problem(JsonProblem::TrailingContent));
        }
        Ok(node)
    });
    result.map_err(|(location, kind)| {
        Box::new(SyntaxError {
            location: Some(location),
            path: p.path.clone(),
            kind,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(text: &str) -> (String, String, LoadErrorKind) {
        let e = parse(text).unwrap_err();
        (
            e.location.map(|l| l.to_string()).unwrap_or_default(),
            e.path.to_string(),
            e.kind,
        )
    }

    fn json(problem: JsonProblem) -> LoadErrorKind {
        LoadErrorKind::Json { problem }
    }

    fn string(text: &str) -> String {
        let Value::Str(s) = parse(text).unwrap().value else {
            panic!("not a string: {text}")
        };
        s
    }

    #[test]
    fn values_and_locations() {
        let node =
            parse("{\n  \"a\":\t[1, -2.5e1, true, false, null, \"é\"],\n  \"b\": {}\n}").unwrap();
        let Value::Map(entries) = node.value() else {
            panic!()
        };
        assert_eq!(entries[0].key_start(), Location::new(2, 3));
        let Value::List(items) = entries[0].value().value() else {
            panic!()
        };
        assert_eq!(
            items.iter().map(|n| n.value().clone()).collect::<Vec<_>>(),
            [
                Value::Int(1),
                Value::Float(-25.0),
                Value::Bool(true),
                Value::Bool(false),
                Value::Null,
                Value::Str("é".into()),
            ]
        );
        assert_eq!(items[1].start(), Location::new(2, 12));
        assert_eq!(items[1].end(), Location::new(2, 18));
        assert_eq!(entries[1].value().value(), &Value::Map(Vec::new()));
        assert_eq!(node.end(), Location::new(4, 2));
    }

    #[test]
    fn escapes() {
        assert_eq!(string(r#""\"\\\/\b\f\n\r\t""#), "\"\\/\u{8}\u{c}\n\r\t");
        assert_eq!(string(r#""€\u000F""#), "€\u{f}");
        assert_eq!(string(r#""😀""#), "😀");
        assert_eq!(error(r#""\x""#).2, json(JsonProblem::InvalidEscape));
        assert_eq!(error(r#""\u12""#).2, json(JsonProblem::InvalidEscape));
        assert_eq!(error(r#""\u+123""#).2, json(JsonProblem::InvalidEscape));
        assert_eq!(error(r#""\ud83d""#).2, json(JsonProblem::LoneSurrogate));
        assert_eq!(error(r#""\ud83dx""#).2, json(JsonProblem::LoneSurrogate));
        assert_eq!(error(r#""\ud83d\n""#).2, json(JsonProblem::LoneSurrogate));
        assert_eq!(error(r#""\ud83dA""#).2, json(JsonProblem::LoneSurrogate));
        assert_eq!(
            error(r#""\ud83d\uZZZZ""#).2,
            json(JsonProblem::InvalidEscape)
        );
        assert_eq!(error(r#""\ude00""#).2, json(JsonProblem::LoneSurrogate));
        assert_eq!(error("\"a\tb\"").2, json(JsonProblem::ControlCharacter));
        assert_eq!(error("\"abc").2, json(JsonProblem::UnexpectedEnd));
    }

    #[test]
    fn duplicate_keys_are_errors() {
        let (at, path, kind) = error(r#"{"a": 1, "b": {"c": 1, "c": 2}}"#);
        assert_eq!((at.as_str(), path.as_str()), ("1:24", "b.c"));
        assert_eq!(
            kind,
            LoadErrorKind::DuplicateKey {
                key: "c".into(),
                first: Location::new(1, 16)
            }
        );
    }

    #[test]
    fn grammar_errors() {
        assert_eq!(error("[1,]").2, json(JsonProblem::TrailingComma));
        assert_eq!(error(r#"{"a": 1,}"#).2, json(JsonProblem::TrailingComma));
        assert_eq!(error("[01]").2, json(JsonProblem::InvalidNumber));
        assert_eq!(error("[1.]").2, json(JsonProblem::InvalidNumber));
        assert_eq!(error("[-]").2, json(JsonProblem::InvalidNumber));
        assert_eq!(error("[+1]").2, json(JsonProblem::UnexpectedChar('+')));
        assert_eq!(error("[.5]").2, json(JsonProblem::UnexpectedChar('.')));
        assert_eq!(error("[True]").2, json(JsonProblem::UnexpectedChar('T')));
        assert_eq!(error("[nul]").2, json(JsonProblem::InvalidLiteral));
        assert_eq!(error("{a: 1}").2, json(JsonProblem::UnexpectedChar('a')));
        assert_eq!(
            error(r#"{"a" 1}"#).2,
            json(JsonProblem::UnexpectedChar('1'))
        );
        assert_eq!(
            error(r#"{"a": 1 "b": 2}"#).2,
            json(JsonProblem::UnexpectedChar('"'))
        );
        assert_eq!(error("[1 2]").2, json(JsonProblem::UnexpectedChar('2')));
        assert_eq!(error("[1").2, json(JsonProblem::UnexpectedEnd));
        assert_eq!(error("1 2").2, json(JsonProblem::TrailingContent));
        assert_eq!(error("// c\n1").2, json(JsonProblem::UnexpectedChar('/')));
        assert_eq!(error("   ").2, LoadErrorKind::Empty);
    }

    #[test]
    fn numbers_follow_the_model_range() {
        assert!(matches!(
            error("[9007199254740992]").2,
            LoadErrorKind::IntegerRange { .. }
        ));
        assert!(matches!(error("1e999").2, LoadErrorKind::NotFinite { .. }));
        assert_eq!(parse("-0").unwrap().value, Value::Int(0));
    }

    #[test]
    fn nesting_is_limited() {
        let deep = format!(
            "{}{}",
            "[".repeat(MAX_NESTING + 1),
            "]".repeat(MAX_NESTING + 1)
        );
        assert_eq!(
            error(&deep).2,
            LoadErrorKind::TooDeep { limit: MAX_NESTING }
        );
        let deep = format!(
            "{}1{}",
            "{\"a\":".repeat(MAX_NESTING + 1),
            "}".repeat(MAX_NESTING + 1)
        );
        assert_eq!(
            error(&deep).2,
            LoadErrorKind::TooDeep { limit: MAX_NESTING }
        );
        let ok = format!("{}{}", "[".repeat(MAX_NESTING), "]".repeat(MAX_NESTING));
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn errors_name_the_path_inside_lists() {
        let (at, path, _) = error("{\"a\": [1, 2, x]}");
        assert_eq!((at.as_str(), path.as_str()), ("1:14", "a[2]"));
    }

    #[test]
    fn columns_count_characters_and_lines_restart() {
        let (at, ..) = error("{\"é\": x}");
        assert_eq!(at, "1:7");
        let (at, ..) = error("[\r\n  x]");
        assert_eq!(at, "2:3");
    }
}
