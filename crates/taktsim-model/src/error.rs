//! Load errors: where (file, line and column, field path), what was found,
//! what is allowed and one valid example (spec §5, Contracts).

use std::fmt;
use std::io;

use taktsim_core::random::DistributionError;
use taktsim_units::UnitError;

use crate::expr::ExprError;

/// A position in a model file: line and column, both counted from 1. The
/// column counts characters (Unicode scalar values), not bytes, so it
/// matches what an editor shows for text without tabs.
///
/// ```
/// use taktsim_model::Location;
///
/// let at = Location::new(3, 7);
/// assert_eq!((at.line(), at.column()), (3, 7));
/// assert_eq!(at.to_string(), "3:7");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
    line: usize,
    column: usize,
}

impl Location {
    /// Line `line`, column `column`, both from 1.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Location::new(1, 1).line(), 1);
    /// ```
    #[must_use]
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    /// The line, from 1.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Location::new(4, 2).line(), 4);
    /// ```
    #[must_use]
    pub const fn line(self) -> usize {
        self.line
    }

    /// The column in characters, from 1.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Location::new(4, 2).column(), 2);
    /// ```
    #[must_use]
    pub const fn column(self) -> usize {
        self.column
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A location in a named file, for errors that point at an earlier
/// definition (a first id, a first key), which may be in another file.
///
/// ```
/// use taktsim_model::{Location, Place};
///
/// let place = Place::new(Some("cells.yaml".into()), Location::new(2, 5));
/// assert_eq!(place.to_string(), "cells.yaml:2:5");
/// assert_eq!(Place::new(None, Location::new(2, 5)).to_string(), "2:5");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Place {
    file: Option<String>,
    location: Location,
}

impl Place {
    /// `location` in `file` (`None`: the text given to the loader).
    ///
    /// ```
    /// use taktsim_model::{Location, Place};
    ///
    /// assert_eq!(Place::new(None, Location::new(1, 1)).file(), None);
    /// ```
    #[must_use]
    pub const fn new(file: Option<String>, location: Location) -> Self {
        Self { file, location }
    }

    /// The file, as named in the load call or the `include:` list.
    ///
    /// ```
    /// use taktsim_model::{Location, Place};
    ///
    /// let place = Place::new(Some("a.yaml".into()), Location::new(1, 1));
    /// assert_eq!(place.file(), Some("a.yaml"));
    /// ```
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    /// The line and column.
    ///
    /// ```
    /// use taktsim_model::{Location, Place};
    ///
    /// assert_eq!(Place::new(None, Location::new(3, 1)).location().line(), 3);
    /// ```
    #[must_use]
    pub const fn location(&self) -> Location {
        self.location
    }
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
        }
        self.location.fmt(f)
    }
}

/// One step of a [`FieldPath`]: a mapping key or a list index.
///
/// ```
/// use taktsim_model::Segment;
///
/// assert_eq!(Segment::Index(0), Segment::Index(0));
/// assert_ne!(Segment::Key("a".into()), Segment::Index(0));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    /// A key of a mapping.
    Key(String),
    /// A position in a list, from 0.
    Index(usize),
}

/// Where in the document a value is: `components[0].cycle_time.sigma`.
///
/// Keys that are not plain identifiers are written in brackets and quotes
/// (`experiment.precision["throughput(sink)"]`). The empty path is the
/// document itself, written `(document)`. [`pointer`](Self::pointer) gives
/// the same path as a JSON Pointer (RFC 6901) for tools.
///
/// ```
/// use taktsim_model::{FieldPath, Segment};
///
/// let path = FieldPath::from(vec![
///     Segment::Key("components".into()),
///     Segment::Index(0),
///     Segment::Key("cycle_time".into()),
/// ]);
/// assert_eq!(path.to_string(), "components[0].cycle_time");
/// assert_eq!(path.pointer(), "/components/0/cycle_time");
/// assert_eq!(FieldPath::default().to_string(), "(document)");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldPath(Vec<Segment>);

impl FieldPath {
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }

    /// The segments, outermost first.
    ///
    /// ```
    /// use taktsim_model::{FieldPath, Segment};
    ///
    /// let path = FieldPath::from(vec![Segment::Key("flow".into())]);
    /// assert_eq!(path.segments(), &[Segment::Key("flow".into())]);
    /// ```
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.0
    }

    /// The path as a JSON Pointer (RFC 6901): `/components/0/id`; `""` for
    /// the document.
    ///
    /// ```
    /// use taktsim_model::{FieldPath, Segment};
    ///
    /// let path = FieldPath::from(vec![Segment::Key("a/b~c".into())]);
    /// assert_eq!(path.pointer(), "/a~1b~0c");
    /// ```
    #[must_use]
    pub fn pointer(&self) -> String {
        let mut out = String::new();
        for segment in &self.0 {
            out.push('/');
            match segment {
                Segment::Key(key) => out.push_str(&key.replace('~', "~0").replace('/', "~1")),
                Segment::Index(i) => out.push_str(&i.to_string()),
            }
        }
        out
    }

    pub(crate) fn push(&mut self, segment: Segment) {
        self.0.push(segment);
    }

    pub(crate) fn pop(&mut self) {
        self.0.pop();
    }

    pub(crate) fn child(&self, segment: Segment) -> Self {
        let mut path = self.clone();
        path.push(segment);
        path
    }
}

impl From<Vec<Segment>> for FieldPath {
    fn from(segments: Vec<Segment>) -> Self {
        Self(segments)
    }
}

/// Whether `key` can be written after a dot: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_plain_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("(document)");
        }
        for (i, segment) in self.0.iter().enumerate() {
            match segment {
                Segment::Key(key) if is_plain_key(key) => {
                    if i > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(key)?;
                }
                Segment::Key(key) => write!(f, "[{}]", Quoted(key))?,
                Segment::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

/// `text` in double quotes, with `"`, `\` and control characters escaped
/// as in JSON.
struct Quoted<'a>(&'a str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        for c in self.0.chars() {
            match c {
                '"' => f.write_str("\\\"")?,
                '\\' => f.write_str("\\\\")?,
                c if c.is_control() => write!(f, "\\u{{{:04x}}}", u32::from(c))?,
                c => write!(f, "{c}")?,
            }
        }
        f.write_str("\"")
    }
}

/// What a value turned out to be, for "expected …, found …" messages.
///
/// ```
/// use taktsim_model::Found;
///
/// assert_eq!(Found::Int(42).to_string(), "the number 42");
/// assert_eq!(Found::Str("yes".into()).to_string(), "the text `yes`");
/// assert_eq!(Found::Map.to_string(), "a mapping");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Found {
    /// `null`, `~` or nothing.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A number with a fraction or exponent.
    Float(f64),
    /// Text.
    Str(String),
    /// A list.
    List,
    /// A mapping.
    Map,
}

impl fmt::Display for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("null (no value)"),
            Self::Bool(b) => write!(f, "the boolean {b}"),
            Self::Int(n) => write!(f, "the number {n}"),
            Self::Float(x) => write!(f, "the number {x}"),
            Self::Str(s) => write!(f, "the text `{s}`"),
            Self::List => f.write_str("a list"),
            Self::Map => f.write_str("a mapping"),
        }
    }
}

/// Why a plain YAML scalar is rejected: YAML 1.1 and YAML 1.2 parsers, or
/// the YAML and JSON forms of a model, would read it differently
/// (ADR-0002).
///
/// ```
/// use taktsim_model::Ambiguity;
///
/// assert_eq!(Ambiguity::Boolean.to_string(), "a boolean");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ambiguity {
    /// `yes`, `No`, `ON`, `True`, …: a boolean in YAML 1.1 or 1.2, text
    /// in the other.
    Boolean,
    /// `012`, `0x1F`, `+1`, `.5`, `1_000`, `.inf`, …: a number to some
    /// YAML parsers, but not a JSON number.
    Number,
    /// `2026-10-26`: a date to YAML 1.1 parsers.
    Timestamp,
}

impl fmt::Display for Ambiguity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Boolean => "a boolean",
            Self::Number => "a number",
            Self::Timestamp => "a date",
        })
    }
}

/// What is wrong with JSON text.
///
/// ```
/// use taktsim_model::JsonProblem;
///
/// assert_eq!(JsonProblem::TrailingComma.to_string(), "a trailing comma");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JsonProblem {
    /// The text ends inside a value.
    UnexpectedEnd,
    /// A character that cannot start or continue a value here.
    UnexpectedChar(char),
    /// A comma before `]` or `}`.
    TrailingComma,
    /// A number that does not follow the JSON grammar (`01`, `1.`, `.5`,
    /// `+1`).
    InvalidNumber,
    /// An escape other than `\" \\ \/ \b \f \n \r \t \uXXXX`.
    InvalidEscape,
    /// A `\u` escape of half a surrogate pair.
    LoneSurrogate,
    /// A control character (below U+0020) inside a string.
    ControlCharacter,
    /// A literal other than `true`, `false`, `null`.
    InvalidLiteral,
    /// More text after the value.
    TrailingContent,
}

impl fmt::Display for JsonProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd => f.write_str("the text ends inside a value"),
            Self::UnexpectedChar(c) => write!(f, "unexpected character {c:?}"),
            Self::TrailingComma => f.write_str("a trailing comma"),
            Self::InvalidNumber => f.write_str("a number that is not valid JSON"),
            Self::InvalidEscape => f.write_str("an invalid escape in a string"),
            Self::LoneSurrogate => f.write_str("half of a UTF-16 surrogate pair"),
            Self::ControlCharacter => f.write_str("a control character inside a string"),
            Self::InvalidLiteral => f.write_str("an unknown literal"),
            Self::TrailingContent => f.write_str("more text after the document"),
        }
    }
}

/// What is wrong with a path in `include:`.
///
/// ```
/// use taktsim_model::IncludePathProblem;
///
/// assert!(IncludePathProblem::ParentDirectory.to_string().contains(".."));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IncludePathProblem {
    /// An absolute path (`/x.yaml`, `C:x.yaml`).
    Absolute,
    /// A `\` separator.
    Backslash,
    /// A `..` segment.
    ParentDirectory,
    /// An empty or `.` segment (`a//b.yaml`, `./a.yaml`).
    EmptySegment,
    /// A character outside `A-Z a-z 0-9 . _ -`.
    Character(char),
    /// Not a `.yaml`, `.yml` or `.json` file.
    Extension,
}

impl fmt::Display for IncludePathProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute => f.write_str("it is absolute"),
            Self::Backslash => f.write_str("it uses `\\`; separate directories with `/`"),
            Self::ParentDirectory => f.write_str(
                "it has a `..` segment; included files are in the model's directory or below",
            ),
            Self::EmptySegment => f.write_str("it has an empty or `.` segment"),
            Self::Character(c) => write!(
                f,
                "it contains {c:?}; file names use only letters, digits, `.`, `_` and `-`"
            ),
            Self::Extension => f.write_str("it does not end in `.yaml`, `.yml` or `.json`"),
        }
    }
}

/// Why a component's parameters have no normalized form. This is a defect
/// of the component type's Rust code, not of the model, but it is still
/// reported as an error rather than a panic.
///
/// ```
/// use taktsim_model::NormalizeProblem;
///
/// assert!(NormalizeProblem::NotAMapping.to_string().contains("mapping"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NormalizeProblem {
    /// The parameters did not serialize to a mapping.
    NotAMapping,
    /// The parameters serialized a key the entry itself uses (`id`,
    /// `type`).
    ReservedKey(String),
    /// serde reported an error.
    Serialize(String),
    /// A number outside what canonical JSON can represent exactly.
    Number(String),
}

impl fmt::Display for NormalizeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAMapping => f.write_str("the parameters did not serialize to a mapping"),
            Self::ReservedKey(key) => {
                write!(f, "the parameters serialize the reserved key `{key}`")
            }
            Self::Serialize(message) => write!(f, "serialization failed: {message}"),
            Self::Number(n) => write!(f, "the number {n} has no exact canonical JSON form"),
        }
    }
}

/// The integer range of a model document: ±(2^53 − 1), the integers every
/// JSON parser reads exactly (I-JSON, RFC 7493), so canonical JSON
/// (RFC 8785) represents each one exactly.
pub const MAX_INTEGER: i64 = (1 << 53) - 1;

/// What is wrong, with what was found, what is allowed and an example.
///
/// Every variant gives [`found`](Self::found), [`allowed`](Self::allowed)
/// and, where one exists, an [`example`](Self::example), so the CLI can
/// print them as separate fields.
///
/// ```
/// use taktsim_model::{Found, LoadErrorKind};
///
/// let kind = LoadErrorKind::WrongType {
///     expected: "a duration such as `42s`",
///     example: "42s",
///     found: Found::Int(42),
/// };
/// assert_eq!(kind.to_string(), "expected a duration such as `42s`, found the number 42");
/// assert_eq!(kind.example().as_deref(), Some("42s"));
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LoadErrorKind {
    /// A file could not be read.
    #[error("cannot read the file: {error}")]
    Io {
        /// What went wrong.
        error: io::ErrorKind,
    },
    /// The file is not UTF-8.
    #[error("the file is not UTF-8 text (first invalid byte at offset {offset})")]
    NotUtf8 {
        /// The byte offset of the first invalid byte.
        offset: usize,
    },
    /// The YAML parser rejected the text.
    #[error("YAML syntax error: {problem}")]
    Yaml {
        /// The parser's description.
        problem: String,
    },
    /// The JSON text is not valid JSON (RFC 8259).
    #[error("JSON syntax error: {problem}")]
    Json {
        /// What is wrong.
        problem: JsonProblem,
    },
    /// The document nests deeper than the loader allows.
    #[error("the document nests deeper than {limit} levels")]
    TooDeep {
        /// The deepest nesting allowed.
        limit: usize,
    },
    /// The file has no document.
    #[error("the file contains no document")]
    Empty,
    /// A YAML stream with more than one document.
    #[error("a model file holds one YAML document; this is a second one")]
    MultipleDocuments,
    /// A YAML anchor (`&name`).
    #[error("YAML anchors (`&name`) are not allowed; reuse parts of a model with `include:`")]
    Anchor,
    /// A YAML alias (`*name`).
    #[error("YAML aliases (`*name`) are not allowed; reuse parts of a model with `include:`")]
    Alias,
    /// A YAML tag (`!!str`, `!custom`).
    #[error("YAML tags such as `{tag}` are not allowed; write the value without it")]
    Tag {
        /// The tag as written.
        tag: String,
    },
    /// A YAML merge key (`<<`).
    #[error("YAML merge keys (`<<`) are not allowed; write the keys out or use `include:`")]
    MergeKey,
    /// A mapping key that is a list or a mapping.
    #[error("a mapping key must be text, not a list or a mapping")]
    ComplexKey,
    /// A mapping key that is a number, boolean or null.
    #[error("the mapping key {found} is not text; keys are text: quote it")]
    NonStringKey {
        /// The key as it was read.
        found: Found,
    },
    /// The same key twice in one mapping.
    #[error("the key `{key}` appears twice in this mapping; the first is at {first}")]
    DuplicateKey {
        /// The key.
        key: String,
        /// The first occurrence.
        first: Location,
    },
    /// A plain scalar that YAML parsers read differently.
    #[error(
        "`{text}` is read as {reading} by some YAML parsers and as text by others; \
         quote it for text, or write a JSON {}",
        match reading {
            Ambiguity::Boolean => "boolean: true or false",
            Ambiguity::Number => "number such as 12, 0.5 or 1e-3",
            Ambiguity::Timestamp => "string: '2026-10-26'",
        }
    )]
    Ambiguous {
        /// The scalar.
        text: String,
        /// How some parsers read it.
        reading: Ambiguity,
    },
    /// An integer outside ±(2^53 − 1).
    #[error(
        "the integer {text} is outside ±{MAX_INTEGER} (2^53 − 1), the range JSON represents exactly"
    )]
    IntegerRange {
        /// The number as written.
        text: String,
    },
    /// A number too large for a 64-bit float.
    #[error("the number {text} is not finite")]
    NotFinite {
        /// The number as written.
        text: String,
    },
    /// A value of the wrong kind.
    #[error("expected {expected}, found {found}")]
    WrongType {
        /// What was expected, e.g. "a duration such as `42s`".
        expected: &'static str,
        /// One valid value.
        example: &'static str,
        /// What was found.
        found: Found,
    },
    /// A required key is missing.
    #[error("the required key `{field}` is missing; it takes {expected}")]
    MissingField {
        /// The key.
        field: &'static str,
        /// What it takes.
        expected: &'static str,
        /// One valid value.
        example: &'static str,
    },
    /// A key the mapping does not have.
    #[error(
        "unknown key `{field}`{}; allowed: {}",
        suggestion.as_ref().map(|s| format!(" (did you mean `{s}`?)")).unwrap_or_default(),
        allowed.join(", ")
    )]
    UnknownField {
        /// The key.
        field: String,
        /// The keys the mapping takes.
        allowed: Vec<String>,
        /// The closest allowed key, if one is close.
        suggestion: Option<String>,
    },
    /// A value outside a fixed set: a distribution, an experiment type, a
    /// component type, a time unit.
    #[error(
        "unknown {what} `{found}`{}; allowed: {}",
        suggestion.as_ref().map(|s| format!(" (did you mean `{s}`?)")).unwrap_or_default(),
        if allowed.is_empty() { "none (no component types are registered)".to_owned() } else { allowed.join(", ") }
    )]
    UnknownValue {
        /// What kind of value, e.g. "distribution".
        what: &'static str,
        /// The value.
        found: String,
        /// The values allowed.
        allowed: Vec<String>,
        /// The closest allowed value, if one is close.
        suggestion: Option<String>,
    },
    /// A quantity that does not parse.
    #[error("{0}")]
    Unit(UnitError),
    /// Distribution parameters that cannot be sampled (ADR-0003).
    #[error("{0}")]
    Distribution(DistributionError),
    /// An expression that does not parse or type-check (ADR-0009,
    /// ADR-0015).
    #[error("in `{text}`: {error}")]
    Expression {
        /// The expression.
        text: String,
        /// What is wrong; its span is a byte range of `text`.
        error: ExprError,
    },
    /// A value outside its allowed range, or breaking a rule of its field.
    #[error("{found} is not allowed; allowed: {allowed}")]
    NotAllowed {
        /// The value found.
        found: String,
        /// What is allowed.
        allowed: String,
        /// One valid value.
        example: String,
    },
    /// A format version this engine does not read.
    #[error("format version `{found}` is not supported; supported: \"1.0\"")]
    UnsupportedFormat {
        /// The version found.
        found: String,
    },
    /// An id or name that does not have the required form.
    #[error(
        "`{found}` is not a valid {what}: use letters, digits and `_`, \
         starting with a letter or `_`, optionally joined by single `-` (e.g. `weld-1`)"
    )]
    InvalidId {
        /// What the text names, e.g. "component id".
        what: &'static str,
        /// The text.
        found: String,
    },
    /// Two components with the same id.
    #[error("the component id `{id}` is used twice; the first is at {first}")]
    DuplicateId {
        /// The id.
        id: String,
        /// The first component with it.
        first: Place,
    },
    /// Two component ids that differ only in case.
    #[error("the component ids `{id}` and `{other}` differ only in case; the other is at {first}")]
    CaseClash {
        /// The later id.
        id: String,
        /// The earlier id.
        other: String,
        /// Where the earlier one is.
        first: Place,
    },
    /// A reference to a component that does not exist.
    #[error(
        "there is no component `{id}`{}",
        suggestion.as_ref().map(|s| format!("; did you mean `{s}`?")).unwrap_or_default()
    )]
    UnknownComponent {
        /// The id referred to.
        id: String,
        /// The closest existing id, if one is close.
        suggestion: Option<String>,
    },
    /// A list with too few entries.
    #[error("{what} needs at least {min} entries, found {found}")]
    TooFew {
        /// What the list is, e.g. "a chain".
        what: &'static str,
        /// The smallest number allowed.
        min: usize,
        /// The number found.
        found: usize,
        /// One valid value.
        example: &'static str,
    },
    /// The same id twice in one list.
    #[error("`{id}` is listed twice")]
    Repeated {
        /// The id.
        id: String,
    },
    /// An edge from a component to itself.
    #[error("an edge from `{id}` to itself; route rework to another component")]
    SelfLoop {
        /// The component.
        id: String,
    },
    /// An edge that an earlier entry already declares.
    #[error("the edge `{from}` → `{to}` is declared twice; the first is at {first}")]
    DuplicateEdge {
        /// The source component.
        from: String,
        /// The target component.
        to: String,
        /// The first declaration.
        first: Place,
    },
    /// The same observable twice.
    #[error("`{text}` is listed twice; the first is at {first}")]
    DuplicateObservable {
        /// The observable, in canonical form.
        text: String,
        /// The first occurrence.
        first: Place,
    },
    /// An expression where an observable reference is required.
    #[error("`{text}` is not an observable such as `throughput(sink)`")]
    NotAnObservable {
        /// The expression.
        text: String,
    },
    /// A steady-state experiment, which needs a warm-up rule (spec §6).
    #[error(
        "steady-state experiments are not supported yet: they need a warm-up rule, \
         which this format version does not have; use `type: terminating`"
    )]
    SteadyStateUnsupported,
    /// A path in `include:` that is not allowed.
    #[error("the include path `{path}` is not allowed: {problem}")]
    IncludePath {
        /// The path.
        path: String,
        /// What is wrong with it.
        problem: IncludePathProblem,
    },
    /// `include:` in a model given as text, with nothing to read files from.
    #[error("`include:` needs a model file to resolve paths against; load the model from a file")]
    IncludeUnavailable,
    /// `include:` in an included file.
    #[error(
        "an included file cannot include other files; list every file in the model's `include:`"
    )]
    NestedInclude,
    /// A model file that includes itself.
    #[error("the model includes itself (`{path}`); list only other files in `include:`")]
    IncludeSelf {
        /// The path.
        path: String,
    },
    /// A file included twice.
    #[error("`{path}` is included twice; the first is at {first}")]
    DuplicateInclude {
        /// The path.
        path: String,
        /// The first occurrence.
        first: Place,
    },
    /// An included file whose content does not match its pinned hash.
    #[error("`{path}` has SHA-256 {actual}, but `include:` pins {pinned}")]
    IncludeHash {
        /// The path.
        path: String,
        /// The hash in the model.
        pinned: String,
        /// The hash of the file.
        actual: String,
    },
    /// A component type's parameters have no normalized form.
    #[error("the parameters of component type `{type_name}` cannot be normalized: {problem}")]
    Normalize {
        /// The component type.
        type_name: String,
        /// What went wrong.
        problem: NormalizeProblem,
    },
}

impl LoadErrorKind {
    /// What was found, as text.
    ///
    /// ```
    /// use taktsim_model::LoadErrorKind;
    ///
    /// let kind = LoadErrorKind::UnsupportedFormat { found: "2.0".into() };
    /// assert_eq!(kind.found().as_deref(), Some("2.0"));
    /// ```
    #[must_use]
    pub fn found(&self) -> Option<String> {
        match self {
            Self::Io { error } => Some(error.to_string()),
            Self::NotUtf8 { offset } => Some(format!("an invalid byte at offset {offset}")),
            Self::Yaml { problem } => Some(problem.clone()),
            Self::Json { problem } => Some(problem.to_string()),
            Self::TooDeep { limit } => Some(format!("more than {limit} levels")),
            Self::Empty => Some("no document".to_owned()),
            Self::MultipleDocuments => Some("a second document".to_owned()),
            Self::Anchor => Some("an anchor".to_owned()),
            Self::Alias => Some("an alias".to_owned()),
            Self::Tag { tag } => Some(tag.clone()),
            Self::MergeKey => Some("<<".to_owned()),
            Self::ComplexKey => Some("a list or mapping as a key".to_owned()),
            Self::NonStringKey { found } | Self::WrongType { found, .. } => Some(found.to_string()),
            Self::DuplicateKey { key: text, .. }
            | Self::Ambiguous { text, .. }
            | Self::IntegerRange { text }
            | Self::NotFinite { text }
            | Self::Expression { text, .. }
            | Self::NotAnObservable { text }
            | Self::DuplicateObservable { text, .. }
            | Self::UnknownField { field: text, .. }
            | Self::UnknownValue { found: text, .. }
            | Self::NotAllowed { found: text, .. }
            | Self::UnsupportedFormat { found: text }
            | Self::InvalidId { found: text, .. }
            | Self::DuplicateId { id: text, .. }
            | Self::CaseClash { id: text, .. }
            | Self::UnknownComponent { id: text, .. }
            | Self::Repeated { id: text }
            | Self::SelfLoop { id: text }
            | Self::IncludePath { path: text, .. }
            | Self::IncludeSelf { path: text }
            | Self::DuplicateInclude { path: text, .. } => Some(text.clone()),
            Self::MissingField { .. } => Some("nothing".to_owned()),
            Self::Unit(error) => Some(unit_input(error)),
            Self::Distribution(error) => Some(error.to_string()),
            Self::TooFew { found, .. } => Some(format!("{found} entries")),
            Self::DuplicateEdge { from, to, .. } => Some(format!("{from} → {to}")),
            Self::SteadyStateUnsupported => Some("steady_state".to_owned()),
            Self::IncludeUnavailable | Self::NestedInclude => Some("include".to_owned()),
            Self::IncludeHash { actual, .. } => Some(actual.clone()),
            Self::Normalize { problem, .. } => Some(problem.to_string()),
        }
    }

    /// What is allowed, as text.
    ///
    /// ```
    /// use taktsim_model::LoadErrorKind;
    ///
    /// assert_eq!(LoadErrorKind::Anchor.allowed().as_deref(), Some("plain values; `include:` for reuse"));
    /// ```
    #[must_use]
    pub fn allowed(&self) -> Option<String> {
        let text = match self {
            Self::Io { .. } => "a readable file",
            Self::NotUtf8 { .. } => "UTF-8 text",
            Self::Yaml { .. } => "YAML 1.2 (the subset of ADR-0002)",
            Self::Json { .. } => "JSON (RFC 8259)",
            Self::TooDeep { limit } => return Some(format!("at most {limit} levels")),
            Self::Empty => "one document",
            Self::MultipleDocuments => "one document per file",
            Self::Anchor | Self::Alias | Self::MergeKey => "plain values; `include:` for reuse",
            Self::Tag { .. } => "values without tags",
            Self::ComplexKey | Self::NonStringKey { .. } => "text keys",
            Self::DuplicateKey { .. } => "each key once per mapping",
            Self::Ambiguous { reading, .. } => match reading {
                Ambiguity::Boolean => "true, false, or quoted text",
                Ambiguity::Number => "a JSON number, or quoted text",
                Ambiguity::Timestamp => "quoted text",
            },
            Self::IntegerRange { .. } => return Some(format!("-{MAX_INTEGER} to {MAX_INTEGER}")),
            Self::NotFinite { .. } => "a finite number",
            Self::WrongType { expected, .. } | Self::MissingField { expected, .. } => expected,
            Self::UnknownField { allowed, .. } | Self::UnknownValue { allowed, .. } => {
                return Some(allowed.join(", "));
            }
            Self::Unit(error) => return Some(unit_allowed(error)),
            Self::Distribution(_) => "the parameter rules of ADR-0003",
            Self::Expression { .. } => "an expression of the right type (ADR-0009)",
            Self::NotAllowed { allowed, .. } => return Some(allowed.clone()),
            Self::UnsupportedFormat { .. } => "\"1.0\"",
            Self::InvalidId { .. } => "[A-Za-z_][A-Za-z0-9_]*(-[A-Za-z0-9_]+)*",
            Self::DuplicateId { .. } | Self::CaseClash { .. } => {
                "ids unique in the model, ignoring case"
            }
            Self::UnknownComponent { .. } => "the id of a component in the model",
            Self::TooFew { min, .. } => return Some(format!("at least {min} entries")),
            Self::Repeated { .. } => "each id once",
            Self::SelfLoop { .. } => "an edge to another component",
            Self::DuplicateEdge { .. } => "each edge once",
            Self::DuplicateObservable { .. } => "each observable once",
            Self::NotAnObservable { .. } => "an observable: a built-in function of component ids",
            Self::SteadyStateUnsupported => "terminating",
            Self::IncludePath { .. } => {
                "a relative path with `/` separators to a .yaml, .yml or .json file in the model's directory or below"
            }
            Self::IncludeUnavailable => "a model loaded from a file",
            Self::NestedInclude => "`include:` in the model file only",
            Self::IncludeSelf { .. } => "other files",
            Self::DuplicateInclude { .. } => "each file once",
            Self::IncludeHash { pinned, .. } => return Some(pinned.clone()),
            Self::Normalize { .. } => {
                "parameters that serialize to a mapping of canonical JSON values"
            }
        };
        Some(text.to_owned())
    }

    /// One valid value, where there is one.
    ///
    /// ```
    /// use taktsim_model::LoadErrorKind;
    ///
    /// let kind = LoadErrorKind::UnsupportedFormat { found: "2.0".into() };
    /// assert_eq!(kind.example().as_deref(), Some("format: \"1.0\""));
    /// ```
    #[must_use]
    pub fn example(&self) -> Option<String> {
        let text = match self {
            Self::Io { .. }
            | Self::NotUtf8 { .. }
            | Self::Yaml { .. }
            | Self::Json { .. }
            | Self::TooDeep { .. }
            | Self::MultipleDocuments
            | Self::Anchor
            | Self::Alias
            | Self::MergeKey
            | Self::ComplexKey
            | Self::DuplicateKey { .. }
            | Self::Repeated { .. }
            | Self::SelfLoop { .. }
            | Self::DuplicateEdge { .. }
            | Self::DuplicateObservable { .. }
            | Self::DuplicateId { .. }
            | Self::CaseClash { .. }
            | Self::IncludeUnavailable
            | Self::NestedInclude
            | Self::IncludeSelf { .. }
            | Self::DuplicateInclude { .. }
            | Self::Normalize { .. } => return None,
            Self::Empty | Self::UnsupportedFormat { .. } => "format: \"1.0\"",
            Self::Tag { .. } => "cycle_time: 42s",
            Self::NonStringKey { .. } => "\"1\": value",
            Self::Ambiguous { reading, .. } => match reading {
                Ambiguity::Boolean => "true",
                Ambiguity::Number => "12",
                Ambiguity::Timestamp => "'2026-10-26'",
            },
            Self::IntegerRange { .. } => "42",
            Self::NotFinite { .. } => "1e300",
            Self::WrongType { example, .. }
            | Self::MissingField { example, .. }
            | Self::TooFew { example, .. } => example,
            Self::UnknownField {
                suggestion,
                allowed,
                ..
            }
            | Self::UnknownValue {
                suggestion,
                allowed,
                ..
            } => {
                return suggestion.clone().or_else(|| allowed.first().cloned());
            }
            Self::UnknownComponent { suggestion, .. } => return suggestion.clone(),
            Self::Unit(error) => return unit_example(error),
            Self::Distribution(_) => "{dist: lognormal, mean: 42s, sigma: 0.08}",
            Self::Expression { .. } | Self::NotAnObservable { .. } => "throughput(sink)",
            Self::NotAllowed { example, .. } => return Some(example.clone()),
            Self::InvalidId { .. } => "weld-1",
            Self::SteadyStateUnsupported => "type: terminating",
            Self::IncludePath { .. } => "include: [cells/welding.yaml]",
            Self::IncludeHash { actual, path, .. } => {
                return Some(format!("{{path: {path}, sha256: {actual}}}"));
            }
        };
        Some(text.to_owned())
    }

    /// Whether the error is a failure to read a file (CLI exit code 7,
    /// ADR-0019) rather than invalid input (exit code 1).
    ///
    /// ```
    /// use taktsim_model::LoadErrorKind;
    ///
    /// assert!(LoadErrorKind::Io { error: std::io::ErrorKind::NotFound }.is_io());
    /// assert!(!LoadErrorKind::Empty.is_io());
    /// ```
    #[must_use]
    pub const fn is_io(&self) -> bool {
        matches!(self, Self::Io { .. })
    }
}

/// The input of a unit error, which every parse error carries.
fn unit_input(error: &UnitError) -> String {
    if let UnitError::InvalidNumber { input, .. }
    | UnitError::UnknownUnit { input, .. }
    | UnitError::Negative { input, .. }
    | UnitError::NotWhole { input, .. }
    | UnitError::TooPrecise { input } = error
    {
        input.clone()
    } else {
        error.to_string()
    }
}

/// What a unit error allows: its units, or its message.
fn unit_allowed(error: &UnitError) -> String {
    if let UnitError::UnknownUnit { allowed, .. } = error {
        allowed.join(", ")
    } else {
        error.to_string()
    }
}

/// The example a unit error carries, if any.
fn unit_example(error: &UnitError) -> Option<String> {
    if let UnitError::InvalidNumber { example, .. }
    | UnitError::UnknownUnit { example, .. }
    | UnitError::Negative { example, .. } = error
    {
        Some((*example).to_owned())
    } else if let UnitError::NotWhole { .. } = error {
        Some("20 parts".to_owned())
    } else if let UnitError::TooPrecise { .. } = error {
        Some("0.000000001s".to_owned())
    } else {
        None
    }
}

/// One problem found while loading a model: where it is, and what is
/// wrong.
///
/// ```
/// use taktsim_model::{Catalog, Format, Loader};
///
/// let catalog = Catalog::new();
/// let errors = Loader::new(&catalog)
///     .load_str("format: \"2.0\"\n", Format::Yaml)
///     .unwrap_err();
/// let first = errors.first();
/// assert_eq!(first.path().to_string(), "format");
/// assert_eq!(first.location().map(|l| l.to_string()).as_deref(), Some("1:9"));
/// assert!(first.to_string().starts_with("1:9: format: format version `2.0` is not supported"));
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub struct LoadError {
    file: Option<String>,
    location: Option<Location>,
    path: FieldPath,
    kind: Box<LoadErrorKind>,
}

impl LoadError {
    pub(crate) fn new(
        file: Option<String>,
        location: Option<Location>,
        path: FieldPath,
        kind: LoadErrorKind,
    ) -> Self {
        Self {
            file,
            location,
            path,
            kind: Box::new(kind),
        }
    }

    /// The file, as named in the load call or the `include:` list; `None`
    /// for text given to [`Loader::load_str`](crate::Loader::load_str).
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("[", Format::Yaml).unwrap_err();
    /// assert_eq!(errors.first().file(), None);
    /// ```
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    /// The line and column, where the error has one.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("a: 1\na: 2\n", Format::Yaml).unwrap_err();
    /// assert_eq!(errors.first().location().map(|l| l.line()), Some(2));
    /// ```
    #[must_use]
    pub const fn location(&self) -> Option<Location> {
        self.location
    }

    /// The field path in the file.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("model: 1\n", Format::Yaml).unwrap_err();
    /// assert!(errors.iter().any(|e| e.path().to_string() == "model"));
    /// ```
    #[must_use]
    pub const fn path(&self) -> &FieldPath {
        &self.path
    }

    /// What is wrong.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, LoadErrorKind, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("", Format::Yaml).unwrap_err();
    /// assert_eq!(errors.first().kind(), &LoadErrorKind::Empty);
    /// ```
    #[must_use]
    pub fn kind(&self) -> &LoadErrorKind {
        &self.kind
    }

    /// The location, for sorting within a file; errors without one sort
    /// first.
    pub(crate) const fn sort_key(&self) -> Option<Location> {
        self.location
    }
}

impl fmt::Display for LoadError {
    /// `file:line:column: path: message`, leaving out what is unknown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
        }
        if let Some(location) = self.location {
            write!(f, "{location}:")?;
        }
        if self.file.is_some() || self.location.is_some() {
            f.write_str(" ")?;
        }
        write!(f, "{}: {}", self.path, self.kind)
    }
}

/// Every problem found in a model, in reading order: by file (the model
/// first, then its includes in order), then line and column. Never empty.
///
/// ```
/// use taktsim_model::{Catalog, Format, Loader};
///
/// let text = "format: \"1.0\"\nmodel: {name: a, sed: 1}\n";
/// let errors = Loader::new(&Catalog::new()).load_str(text, Format::Yaml).unwrap_err();
/// assert!(errors.len() >= 2);
/// assert!(errors.to_string().lines().count() >= 2);
/// assert!(errors.iter().any(|e| e.to_string().contains("did you mean `seed`?")));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LoadErrors {
    first: LoadError,
    rest: Vec<LoadError>,
}

impl LoadErrors {
    /// The errors in `errors`, or `None` if there are none.
    pub(crate) fn from_vec(errors: Vec<LoadError>) -> Option<Self> {
        let mut iter = errors.into_iter();
        let first = iter.next()?;
        Some(Self {
            first,
            rest: iter.collect(),
        })
    }

    pub(crate) const fn single(error: LoadError) -> Self {
        Self {
            first: error,
            rest: Vec::new(),
        }
    }

    /// The first error in reading order.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("1", Format::Yaml).unwrap_err();
    /// assert_eq!(errors.first().path().to_string(), "(document)");
    /// ```
    #[must_use]
    pub const fn first(&self) -> &LoadError {
        &self.first
    }

    /// The number of errors; at least 1.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("1", Format::Yaml).unwrap_err();
    /// assert_eq!(errors.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        1 + self.rest.len()
    }

    /// Always `false`: there is at least one error.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("1", Format::Yaml).unwrap_err();
    /// assert!(!errors.is_empty());
    /// ```
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// The errors in reading order.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("1", Format::Yaml).unwrap_err();
    /// assert_eq!(errors.iter().count(), errors.len());
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &LoadError> {
        std::iter::once(&self.first).chain(&self.rest)
    }
}

impl fmt::Display for LoadErrors {
    /// One error per line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, error) in self.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            error.fmt(f)?;
        }
        Ok(())
    }
}

impl std::error::Error for LoadErrors {}

/// The closest of `candidates` to `word`, if one is close enough to be a
/// likely typo: at most a third of the word's length (at least 1) edits
/// away. Ties go to the candidate that comes first.
pub(crate) fn suggest<'a, I>(word: &str, candidates: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let limit = (word.chars().count() / 3).max(1);
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let distance = edit_distance(word, candidate);
        if distance <= limit && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, c)| c.to_owned())
}

/// Levenshtein distance over characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = diagonal + usize::from(ca != *cb);
            diagonal = row[j + 1];
            row[j + 1] = substitution.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_counts_insertions_deletions_and_substitutions() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("seed", "sed"), 1);
        assert_eq!(edit_distance("sed", "seed"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }

    #[test]
    fn suggestions_are_close_and_deterministic() {
        assert_eq!(suggest("sed", ["seed", "name"]), Some("seed".to_owned()));
        assert_eq!(suggest("xyz", ["seed", "name"]), None);
        // Ties: the first candidate wins.
        assert_eq!(suggest("ab", ["aa", "bb"]), Some("aa".to_owned()));
    }

    #[test]
    fn paths_quote_keys_that_are_not_identifiers() {
        let path = FieldPath::from(vec![
            Segment::Key("experiment".into()),
            Segment::Key("precision".into()),
            Segment::Key("throughput(sink)".into()),
        ]);
        assert_eq!(
            path.to_string(),
            "experiment.precision[\"throughput(sink)\"]"
        );
        let path = FieldPath::from(vec![Segment::Key("a\"b\\\n".into())]);
        assert_eq!(path.to_string(), "[\"a\\\"b\\\\\\u{000a}\"]");
        let path = FieldPath::from(vec![Segment::Index(2), Segment::Key("x".into())]);
        assert_eq!(path.to_string(), "[2].x");
    }
}
