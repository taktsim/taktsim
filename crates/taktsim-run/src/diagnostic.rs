//! What goes wrong, in one typed value: every client renders it, as a line of
//! JSON or as text (ADR-0019 item 3, ADR-0024 §9).

use taktsim_core::{RecentEvent, RunError};
use taktsim_model::{
    Decode, Decoder, Document, Encode, Fields, Json, LoadError, LoadErrors, Node, Record, Schemas,
    Value,
};
use taktsim_units::{SimDuration, SimTime};

/// What a [`Diagnostic`] line is.
///
/// ```
/// use taktsim_run::DiagnosticKind;
///
/// assert_eq!(DiagnosticKind::Warning.as_str(), "warning");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// Something failed; the command's exit code says what follows.
    Error,
    /// Something the caller should know; the command goes on.
    Warning,
    /// How the command is doing.
    Progress,
}

impl DiagnosticKind {
    /// The word on the line.
    ///
    /// ```
    /// assert_eq!(taktsim_run::DiagnosticKind::Error.as_str(), "error");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Progress => "progress",
        }
    }
}

impl Encode for DiagnosticKind {
    fn encode(&self) -> Json {
        self.as_str().into()
    }
}

impl Decode for DiagnosticKind {
    const EXPECTED: &'static str = "`error`, `warning` or `progress`";
    const EXAMPLE: &'static str = "error";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Str(text) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        match text.as_str() {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "progress" => Some(Self::Progress),
            other => {
                cx.not_allowed(node, other, Self::EXPECTED, Self::EXAMPLE);
                None
            }
        }
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([(
            "enum",
            Json::Array(vec![
                Json::from("error"),
                Json::from("warning"),
                Json::from("progress"),
            ]),
        )])
    }
}

/// One line of what goes to stderr: an error, a warning or a progress report.
///
/// `type_name` is a stable word in a namespace: `args.*`, `model.*`, `run.*`,
/// `store.*`, `internal.*`. An error of exit code 1 says what was found, what is
/// allowed and, where the loader has one, one valid example; an error of exit
/// code 2 says when and what happened last instead (ADR-0024 §9).
///
/// ```
/// use taktsim_model::{Encode, Format, decode_str};
/// use taktsim_run::Diagnostic;
///
/// let error = Diagnostic::error("run.no_seed", "this model draws random numbers and has no seed")
///     .at("model.seed")
///     .found("no seed")
///     .allowed("a seed from 0 to 9007199254740991")
///     .example("--seed 42");
/// assert_eq!(error.encode()["type"], "run.no_seed");
/// let back: Diagnostic = decode_str(&error.encode().to_string(), Format::Json)?;
/// assert_eq!(back, error);
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Error, warning or progress.
    pub kind: DiagnosticKind,
    /// The stable word for this kind of line.
    pub type_name: String,
    /// One sentence for a person.
    pub message: String,
    /// A model path, or the flag or argument it concerns.
    pub path: Option<String>,
    /// The file it was found in.
    pub file: Option<String>,
    /// The line in that file, from 1.
    pub line: Option<u64>,
    /// The column in that file, from 1.
    pub column: Option<u64>,
    /// What was found.
    pub found: Option<String>,
    /// What is allowed.
    pub allowed: Option<String>,
    /// One valid value.
    pub example: Option<String>,
    /// The simulation time, for a failed run.
    pub time: Option<SimDuration>,
    /// Items in the system, for a run that stalled.
    pub items_alive: Option<u64>,
    /// The last events, oldest first, for a failed run.
    pub recent: Vec<String>,
    /// Where the run store is, for a progress line.
    pub store: Option<String>,
    /// Whether the run was reused, for a progress line.
    pub reused: Option<bool>,
}

impl Diagnostic {
    fn new(kind: DiagnosticKind, type_name: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            type_name: type_name.to_owned(),
            message: message.into(),
            path: None,
            file: None,
            line: None,
            column: None,
            found: None,
            allowed: None,
            example: None,
            time: None,
            items_alive: None,
            recent: Vec::new(),
            store: None,
            reused: None,
        }
    }

    /// An error of this type.
    ///
    /// ```
    /// let e = taktsim_run::Diagnostic::error("run.stalled", "items are left and no event");
    /// assert_eq!(e.type_name, "run.stalled");
    /// ```
    #[must_use]
    pub fn error(type_name: &str, message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Error, type_name, message)
    }

    /// A warning of this type.
    ///
    /// ```
    /// use taktsim_run::{Diagnostic, DiagnosticKind};
    ///
    /// let w = Diagnostic::warning("run.seed_unused", "the seed is not used");
    /// assert_eq!(w.kind, DiagnosticKind::Warning);
    /// ```
    #[must_use]
    pub fn warning(type_name: &str, message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Warning, type_name, message)
    }

    /// A progress line of this type.
    ///
    /// ```
    /// use taktsim_run::{Diagnostic, DiagnosticKind};
    ///
    /// let p = Diagnostic::progress("run.store", "using the store");
    /// assert_eq!(p.kind, DiagnosticKind::Progress);
    /// ```
    #[must_use]
    pub fn progress(type_name: &str, message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Progress, type_name, message)
    }

    /// With the path it concerns.
    ///
    /// ```
    /// let e = taktsim_run::Diagnostic::error("run.x", "m").at("--seed");
    /// assert_eq!(e.path.as_deref(), Some("--seed"));
    /// ```
    #[must_use]
    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// With what was found.
    ///
    /// ```
    /// let e = taktsim_run::Diagnostic::error("run.x", "m").found("7");
    /// assert_eq!(e.found.as_deref(), Some("7"));
    /// ```
    #[must_use]
    pub fn found(mut self, found: impl Into<String>) -> Self {
        self.found = Some(found.into());
        self
    }

    /// With what is allowed.
    ///
    /// ```
    /// let e = taktsim_run::Diagnostic::error("run.x", "m").allowed("0 or 1");
    /// assert_eq!(e.allowed.as_deref(), Some("0 or 1"));
    /// ```
    #[must_use]
    pub fn allowed(mut self, allowed: impl Into<String>) -> Self {
        self.allowed = Some(allowed.into());
        self
    }

    /// With one valid value.
    ///
    /// ```
    /// let e = taktsim_run::Diagnostic::error("run.x", "m").example("1");
    /// assert_eq!(e.example.as_deref(), Some("1"));
    /// ```
    #[must_use]
    pub fn example(mut self, example: impl Into<String>) -> Self {
        self.example = Some(example.into());
        self
    }

    /// With the simulation time.
    ///
    /// ```
    /// let e = taktsim_run::Diagnostic::error("run.x", "m").at_time("1h".parse()?);
    /// assert_eq!(e.time.map(|t| t.to_string()).as_deref(), Some("1h"));
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn at_time(mut self, time: SimTime) -> Self {
        self.time = Some(SimDuration::from_nanos(time.as_nanos()));
        self
    }
}

impl Encode for Diagnostic {
    fn encode(&self) -> Json {
        let recent = (!self.recent.is_empty()).then_some(&self.recent);
        Json::object([])
            .with("kind", &self.kind)
            .with("type", &self.type_name)
            .with("message", &self.message)
            .with_optional("path", self.path.as_ref())
            .with_optional("file", self.file.as_ref())
            .with_optional("line", self.line.as_ref())
            .with_optional("column", self.column.as_ref())
            .with_optional("found", self.found.as_ref())
            .with_optional("allowed", self.allowed.as_ref())
            .with_optional("example", self.example.as_ref())
            .with_optional("time", self.time.as_ref())
            .with_optional("items_alive", self.items_alive.as_ref())
            .with_optional("recent", recent)
            .with_optional("store", self.store.as_ref())
            .with_optional("reused", self.reused.as_ref())
    }
}

impl Record for Diagnostic {
    const EXPECTED: &'static str = "a line such as `{kind: error, type: run.no_seed, message: …}`";
    const EXAMPLE: &'static str = "{kind: error, type: run.no_seed, message: no seed}";
    const SCHEMA_NAME: Option<&'static str> = Some("Diagnostic");
    const DESCRIPTION: Option<&'static str> =
        Some("One line of stderr with `--format json`: an error, a warning or a progress report.");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let kind = fields.required("kind");
        let type_name = fields.required("type");
        let message = fields.required("message");
        let path = fields.optional("path");
        let file = fields.optional("file");
        let line = fields.optional("line");
        let column = fields.optional("column");
        let found = fields.optional("found");
        let allowed = fields.optional("allowed");
        let example = fields.optional("example");
        let time = fields.optional("time");
        let items_alive = fields.optional("items_alive");
        let recent = fields.optional("recent");
        let store = fields.optional("store");
        let reused = fields.optional("reused");
        Some(Self {
            kind: kind?,
            type_name: type_name?,
            message: message?,
            path: path?,
            file: file?,
            line: line?,
            column: column?,
            found: found?,
            allowed: allowed?,
            example: example?,
            time: time?,
            items_alive: items_alive?,
            recent: recent?.unwrap_or_default(),
            store: store?,
            reused: reused?,
        })
    }
}

impl From<&LoadError> for Diagnostic {
    fn from(error: &LoadError) -> Self {
        let kind = error.kind();
        // A model file that cannot be read has no path inside it yet.
        let path = if kind.is_io() && error.path().segments().is_empty() {
            "<model>".to_owned()
        } else {
            error.path().to_string()
        };
        let mut line = Self::error(kind.name(), kind.to_string()).at(path);
        line.file = error.file().map(str::to_owned);
        if let Some(at) = error.location() {
            line.line = Some(at.line() as u64);
            line.column = Some(at.column() as u64);
        }
        line.found = kind.found();
        line.allowed = kind.allowed();
        line.example = kind.example();
        line
    }
}

/// Why a failed command failed: one of the three things a caller acts on.
///
/// ```
/// use taktsim_run::FailureClass;
///
/// assert_ne!(FailureClass::Invalid, FailureClass::Io);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    /// Invalid input: nothing ran (exit code 1).
    Invalid,
    /// The run failed: a broken invariant, an engine error, a differing trace
    /// (exit code 2).
    RunFailed,
    /// A file or the store cannot be read or written (exit code 7).
    Io,
}

/// A failed command: what kind of failure, and every error found.
///
/// ```
/// use taktsim_run::{Diagnostic, Failure, FailureClass};
///
/// let failure = Failure::new(FailureClass::Invalid, vec![Diagnostic::error("run.no_seed", "no seed")]);
/// assert_eq!(failure.to_string(), "no seed");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.diagnostics.first().map_or("the command failed", |d| d.message.as_str()))]
pub struct Failure {
    /// What kind of failure.
    pub class: FailureClass,
    /// Every error found, in reading order.
    pub diagnostics: Vec<Diagnostic>,
}

impl Failure {
    /// A failure with these errors.
    ///
    /// ```
    /// use taktsim_run::{Failure, FailureClass};
    ///
    /// assert_eq!(Failure::new(FailureClass::Io, vec![]).class, FailureClass::Io);
    /// ```
    #[must_use]
    pub const fn new(class: FailureClass, diagnostics: Vec<Diagnostic>) -> Self {
        Self { class, diagnostics }
    }

    pub(crate) fn one(class: FailureClass, diagnostic: Diagnostic) -> Self {
        Self::new(class, vec![diagnostic])
    }

    /// Every error of a model that did not load. A file that cannot be read is
    /// an I/O failure; everything else is invalid input.
    pub(crate) fn from_load(errors: &LoadErrors) -> Self {
        let class = if errors.iter().any(|e| e.kind().is_io()) {
            FailureClass::Io
        } else {
            FailureClass::Invalid
        };
        Self::new(class, errors.iter().map(Diagnostic::from).collect())
    }
}

/// The path of the component called `id`, if `document` has one.
fn component_path(document: &Document, id: &str) -> Option<String> {
    document
        .components()
        .iter()
        .find(|c| c.id() == id)
        .map(|c| c.path().to_string())
}

/// The line of a run error: its type, the component's path, the time and the last
/// events. There is no example of a valid value for a broken invariant
/// (ADR-0024 §9).
pub(crate) fn from_run_error(error: &RunError, document: &Document) -> Diagnostic {
    let (type_name, component, time, recent): (&str, &str, SimTime, &[RecentEvent]) = match error {
        RunError::Component {
            component,
            time,
            recent,
            ..
        } => ("run.component_failed", component, *time, recent),
        RunError::Invariant {
            component,
            time,
            recent,
            ..
        } => ("run.invariant_violated", component, *time, recent),
        RunError::Conservation {
            component,
            time,
            recent,
            ..
        } => ("run.conservation_violated", component, *time, recent),
        RunError::NotReceived {
            component,
            time,
            recent,
            ..
        } => ("run.item_not_received", component, *time, recent),
        RunError::AlreadyFailed | RunError::HorizonInPast { .. } => {
            return Diagnostic::error("run.engine", error.to_string());
        }
        // A variant of a later engine.
        _ => return Diagnostic::error("run.failed", error.to_string()),
    };
    let mut line = Diagnostic::error(type_name, error.to_string()).at_time(time);
    line.path = component_path(document, component);
    line.recent = recent.iter().map(ToString::to_string).collect();
    line
}
