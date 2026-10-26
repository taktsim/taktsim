//! Model text → a tree of values with source locations (ADR-0002, step 1
//! of the pipeline).
//!
//! YAML and JSON both become the same [`Node`] tree, so everything after
//! parsing is shared. The YAML parser crate is used only in [`yaml`]
//! (ADR-0002: "wrapped in one module so it can be replaced"); JSON has its
//! own strict parser in [`json`], because YAML 1.2 parsers accept text that
//! is not JSON and reject a little that is.

mod json;
mod scalar;
mod yaml;

use std::fmt;

use crate::error::{Found, Location};

/// The deepest nesting of lists and mappings a model file may have. Real
/// models nest about six levels; the limit keeps every pass over the tree
/// within a small stack.
pub const MAX_NESTING: usize = 64;

/// The syntax of a model file.
///
/// ```
/// use std::path::Path;
/// use taktsim_model::Format;
///
/// assert_eq!(Format::from_path(Path::new("line-a.yaml")), Some(Format::Yaml));
/// assert_eq!(Format::from_path(Path::new("line-a.JSON")), Some(Format::Json));
/// assert_eq!(Format::from_path(Path::new("line-a.txt")), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Format {
    /// The YAML 1.2 subset of ADR-0002.
    Yaml,
    /// JSON (RFC 8259).
    Json,
}

impl Format {
    /// The format of a file by its extension: `.yaml` or `.yml` is YAML,
    /// `.json` is JSON, in any case.
    ///
    /// ```
    /// use std::path::Path;
    /// use taktsim_model::Format;
    ///
    /// assert_eq!(Format::from_path(Path::new("cells/weld.yml")), Some(Format::Yaml));
    /// ```
    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "yaml" | "yml" => Some(Self::Yaml),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Yaml => "YAML",
            Self::Json => "JSON",
        })
    }
}

/// A value read from a model file, with where it starts and ends.
///
/// Component types read their parameters from nodes through
/// [`Decode`](crate::Decode); nothing else in a node can be changed.
///
/// ```
/// use taktsim_model::{Decode, Decoder, Node, Value};
///
/// // A component type's own value type, read from a node.
/// struct Even(i64);
///
/// impl Decode for Even {
///     const EXPECTED: &'static str = "an even integer";
///     const EXAMPLE: &'static str = "2";
///
///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
///         match node.value() {
///             Value::Int(n) if n % 2 == 0 => Some(Even(*n)),
///             _ => {
///                 cx.not_allowed(node, node.found().to_string(), "an even integer", "2");
///                 None
///             }
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    value: Value,
    start: Location,
    end: Location,
}

impl Node {
    pub(crate) const fn new(value: Value, start: Location, end: Location) -> Self {
        Self { value, start, end }
    }

    /// The value.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Node, Value};
    ///
    /// fn is_text(node: &Node) -> bool {
    ///     matches!(node.value(), Value::Str(_))
    /// }
    /// ```
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.value
    }

    /// Where the value starts: its first character, or for a tagged or
    /// quoted YAML scalar, the tag or the opening quote.
    ///
    /// ```
    /// use taktsim_model::{Location, Node};
    ///
    /// fn line(node: &Node) -> usize {
    ///     node.start().line()
    /// }
    /// ```
    #[must_use]
    pub const fn start(&self) -> Location {
        self.start
    }

    /// Where the value ends: the position after its last character.
    ///
    /// ```
    /// use taktsim_model::{Location, Node};
    ///
    /// fn single_line(node: &Node) -> bool {
    ///     node.start().line() == node.end().line()
    /// }
    /// ```
    #[must_use]
    pub const fn end(&self) -> Location {
        self.end
    }

    /// What the value is, for error messages.
    ///
    /// ```
    /// use taktsim_model::{Found, Node};
    ///
    /// fn is_list(node: &Node) -> bool {
    ///     node.found() == Found::List
    /// }
    /// ```
    #[must_use]
    pub fn found(&self) -> Found {
        match &self.value {
            Value::Null => Found::Null,
            Value::Bool(b) => Found::Bool(*b),
            Value::Int(n) => Found::Int(*n),
            Value::Float(x) => Found::Float(*x),
            Value::Str(s) => Found::Str(s.clone()),
            Value::List(_) => Found::List,
            Value::Map(_) => Found::Map,
        }
    }
}

/// The value of a [`Node`].
///
/// Scalars are resolved by the YAML 1.2 core schema, restricted so that the
/// YAML and JSON forms of a model always agree (ADR-0002, ADR-0020):
/// integers are within ±(2^53 − 1), numbers are finite, and plain scalars
/// that some YAML parser would read as something else are errors.
///
/// ```
/// use taktsim_model::Value;
///
/// assert_ne!(Value::Int(1), Value::Float(1.0));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`, `~` or nothing.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer within ±(2^53 − 1).
    Int(i64),
    /// A finite number written with a fraction or an exponent.
    Float(f64),
    /// Text: a quoted or block scalar, or a plain one that is not a
    /// boolean, null or number.
    Str(String),
    /// A list.
    List(Vec<Node>),
    /// A mapping, in the order written; its keys are unique.
    Map(Vec<Entry>),
}

/// One key and value of a mapping.
///
/// ```
/// use taktsim_model::{Entry, Value};
///
/// fn keys(value: &Value) -> Vec<&str> {
///     match value {
///         Value::Map(entries) => entries.iter().map(Entry::key).collect(),
///         _ => Vec::new(),
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    key: String,
    key_start: Location,
    value: Node,
}

impl Entry {
    pub(crate) const fn new(key: String, key_start: Location, value: Node) -> Self {
        Self {
            key,
            key_start,
            value,
        }
    }

    /// The key.
    ///
    /// ```
    /// use taktsim_model::Entry;
    ///
    /// fn is_id(entry: &Entry) -> bool {
    ///     entry.key() == "id"
    /// }
    /// ```
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Where the key starts.
    ///
    /// ```
    /// use taktsim_model::Entry;
    ///
    /// fn line(entry: &Entry) -> usize {
    ///     entry.key_start().line()
    /// }
    /// ```
    #[must_use]
    pub const fn key_start(&self) -> Location {
        self.key_start
    }

    /// The value.
    ///
    /// ```
    /// use taktsim_model::Entry;
    ///
    /// fn value_line(entry: &Entry) -> usize {
    ///     entry.value().start().line()
    /// }
    /// ```
    #[must_use]
    pub const fn value(&self) -> &Node {
        &self.value
    }
}

/// A syntax-level problem: where, the path of keys and indices to it, and
/// what.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SyntaxError {
    pub(crate) location: Option<Location>,
    pub(crate) path: crate::error::FieldPath,
    pub(crate) kind: crate::error::LoadErrorKind,
}

/// Parses `text` into a tree, or every problem found on the way.
pub(crate) fn parse(text: &str, format: Format) -> Result<Node, Vec<SyntaxError>> {
    // A byte order mark is not part of the document (RFC 8259 §8.1 allows
    // ignoring it; YAML 1.2 §5.2 allows it).
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format {
        Format::Yaml => yaml::parse(text),
        Format::Json => json::parse(text).map_err(|e| vec![*e]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::LoadErrorKind;

    fn tree(text: &str, format: Format) -> Value {
        parse(text, format).unwrap().value
    }

    fn errors(text: &str, format: Format) -> Vec<LoadErrorKind> {
        parse(text, format)
            .unwrap_err()
            .into_iter()
            .map(|e| e.kind)
            .collect()
    }

    #[test]
    fn a_byte_order_mark_is_ignored() {
        assert_eq!(tree("\u{feff}1", Format::Yaml), Value::Int(1));
        assert_eq!(tree("\u{feff}1", Format::Json), Value::Int(1));
    }

    #[test]
    fn yaml_and_json_give_the_same_tree() {
        let yaml = tree("a: [1, 2.5, x, true, null]\nb: {c: \"d\"}\n", Format::Yaml);
        let json = tree(
            r#"{"a": [1, 2.5, "x", true, null], "b": {"c": "d"}}"#,
            Format::Json,
        );
        assert_eq!(strip(&yaml), strip(&json));
    }

    #[test]
    fn an_empty_text_has_no_document() {
        assert_eq!(errors("", Format::Yaml), [LoadErrorKind::Empty]);
        assert_eq!(
            errors("# only a comment\n", Format::Yaml),
            [LoadErrorKind::Empty]
        );
    }

    /// The value without locations, for comparing trees from different
    /// texts.
    fn strip(value: &Value) -> String {
        match value {
            Value::List(items) => {
                let items: Vec<String> = items.iter().map(|n| strip(&n.value)).collect();
                format!("[{}]", items.join(","))
            }
            Value::Map(entries) => {
                let entries: Vec<String> = entries
                    .iter()
                    .map(|e| format!("{}:{}", e.key, strip(&e.value.value)))
                    .collect();
                format!("{{{}}}", entries.join(","))
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
                format!("{value:?}")
            }
        }
    }

    #[test]
    fn formats_display_their_names() {
        assert_eq!(Format::Yaml.to_string(), "YAML");
        assert_eq!(Format::Json.to_string(), "JSON");
        assert_eq!(Format::from_path(std::path::Path::new("noext")), None);
    }
}
