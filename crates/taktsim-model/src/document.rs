//! The model document (spec §4; roadmap M4): its sections, as read from a
//! file, and their normalized form.

use taktsim_units::{Ratio, SimDuration};

use crate::component::{
    Catalog, Component, ComponentEntry, ComponentRef, ID_PATTERN, read_component, read_id,
};
use crate::decode::{Decode, Decoder, Fields, Record, Schemas};
use crate::error::{FieldPath, LoadErrorKind, Location, Place, Segment};
use crate::expr::Expr;
use crate::include::{IncludeSpec, Includes};
use crate::json::{Encode, Json};
use crate::syntax::{Node, Value};

/// The format version this engine reads (ADR-0008: the header is
/// `format: "1.0"`).
pub const FORMAT_VERSION: &str = "1.0";

/// A loaded model document: every section, typed, with includes resolved
/// and shorthands expanded.
///
/// ```
/// use taktsim_model::expr::{Arity, Dimension, Signature, Type};
/// use taktsim_model::{Catalog, ComponentParams, Distribution, Encode, Fields, Format, Json, Loader};
///
/// #[derive(Debug)]
/// struct Source {
///     interarrival: Distribution,
/// }
/// impl Encode for Source {
///     fn encode(&self) -> Json {
///         Json::object([]).with("interarrival", &self.interarrival)
///     }
/// }
/// impl ComponentParams for Source {
///     const TYPE: &'static str = "Source";
///     const EXAMPLE: &'static str = "{id: source, type: Source, interarrival: 1min}";
///     fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self { interarrival: fields.required("interarrival")? })
///     }
/// }
/// #[derive(Debug)]
/// struct Sink {}
/// impl Encode for Sink {
///     fn encode(&self) -> Json {
///         Json::object([])
///     }
/// }
/// impl ComponentParams for Sink {
///     const TYPE: &'static str = "Sink";
///     const EXAMPLE: &'static str = "{id: sink, type: Sink}";
///     fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self {})
///     }
/// }
///
/// let throughput = Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate));
/// let catalog = Catalog::new()
///     .with::<Source>()?
///     .with::<Sink>()?
///     .with_observable("throughput", throughput)?;
/// let model = Loader::new(&catalog).load_str(
///     r#"
/// format: "1.0"
/// model: {name: line-a, time_unit: s, seed: 42}
/// experiment: {type: terminating, horizon: 8h}
/// components:
///   - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}
///   - {id: sink, type: Sink}
/// flow: [[source, sink]]
/// observe: [throughput(sink)]
/// "#,
///     Format::Yaml,
/// )?;
/// let doc = model.document();
/// assert_eq!(doc.model().name(), "line-a");
/// assert_eq!(doc.experiment().horizon().to_string(), "8h");
/// assert_eq!(doc.components().len(), 2);
/// assert_eq!(doc.flow()[0].to()[0].id(), "sink");
/// assert_eq!(doc.observe()[0].text(), "throughput(sink)");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Document {
    pub(crate) model: ModelInfo,
    pub(crate) experiment: Experiment,
    pub(crate) components: Vec<Component>,
    pub(crate) flow: Vec<Edge>,
    pub(crate) observe: Vec<Observable>,
}

impl Document {
    /// The `model` section.
    ///
    /// ```
    /// fn name(doc: &taktsim_model::Document) -> &str {
    ///     doc.model().name()
    /// }
    /// ```
    #[must_use]
    pub const fn model(&self) -> &ModelInfo {
        &self.model
    }

    /// The `experiment` section.
    ///
    /// ```
    /// fn horizon(doc: &taktsim_model::Document) -> taktsim_units::SimDuration {
    ///     doc.experiment().horizon()
    /// }
    /// ```
    #[must_use]
    pub const fn experiment(&self) -> &Experiment {
        &self.experiment
    }

    /// The components: those of each included file in `include:` order,
    /// then the model file's own, each in the order written.
    ///
    /// ```
    /// fn ids(doc: &taktsim_model::Document) -> Vec<&str> {
    ///     doc.components().iter().map(|c| c.id()).collect()
    /// }
    /// ```
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The flow as edges: every chain expanded to its edges, in the order
    /// written (included files first, as for components).
    ///
    /// ```
    /// fn edge_count(doc: &taktsim_model::Document) -> usize {
    ///     doc.flow().len()
    /// }
    /// ```
    #[must_use]
    pub fn flow(&self) -> &[Edge] {
        &self.flow
    }

    /// The observables, in the order written (included files first).
    ///
    /// ```
    /// fn observables(doc: &taktsim_model::Document) -> Vec<&str> {
    ///     doc.observe().iter().map(|o| o.text()).collect()
    /// }
    /// ```
    #[must_use]
    pub fn observe(&self) -> &[Observable] {
        &self.observe
    }

    /// The normalized form, before canonicalization: every value typed and
    /// in canonical text, shorthands expanded, `include:` resolved, and
    /// absent or empty optional sections left out.
    pub(crate) fn to_json(&self) -> Result<Json, (&Component, LoadErrorKind)> {
        let components = self
            .components
            .iter()
            .map(|c| c.to_json().map_err(|kind| (c, kind)))
            .collect::<Result<Vec<_>, _>>()?;
        let doc = Json::object([
            ("format", Json::from(FORMAT_VERSION)),
            ("components", Json::Array(components)),
        ])
        .with("model", &self.model)
        .with("experiment", &self.experiment)
        .with("flow", &self.flow);
        Ok(if self.observe.is_empty() {
            doc
        } else {
            doc.with("observe", &self.observe)
        })
    }
}

/// The `model` section: the model's name and defaults.
///
/// ```
/// fn seed(doc: &taktsim_model::Document) -> Option<u64> {
///     doc.model().seed()
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    name: String,
    time_unit: Option<TimeUnit>,
    seed: Option<u64>,
}

impl Encode for ModelInfo {
    fn encode(&self) -> Json {
        Json::object([("name", Json::from(self.name.as_str()))])
            .with_optional("time_unit", self.time_unit.as_ref())
            .with_optional("seed", self.seed.as_ref())
    }
}

impl ModelInfo {
    /// The name, of the same form as a component id (`line-a`).
    ///
    /// ```
    /// fn name(doc: &taktsim_model::Document) -> &str {
    ///     doc.model().name()
    /// }
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The unit results are reported in, if given (spec §4's example
    /// writes `time_unit: s`). It is only that: it never changes how a
    /// model reads, and a bare number never takes it as its unit; every
    /// quantity carries its own (ADR-0020, answer 8).
    ///
    /// ```
    /// use taktsim_model::TimeUnit;
    ///
    /// fn in_hours(doc: &taktsim_model::Document) -> bool {
    ///     doc.model().time_unit() == Some(TimeUnit::Hours)
    /// }
    /// ```
    #[must_use]
    pub const fn time_unit(&self) -> Option<TimeUnit> {
        self.time_unit
    }

    /// The default seed, if given: at most 2^53 − 1, the integer range of a
    /// model.
    ///
    /// ```
    /// fn seed(doc: &taktsim_model::Document) -> Option<u64> {
    ///     doc.model().seed()
    /// }
    /// ```
    #[must_use]
    pub const fn seed(&self) -> Option<u64> {
        self.seed
    }
}

/// A unit of time for `model.time_unit`: the units of a duration.
///
/// ```
/// use taktsim_model::TimeUnit;
///
/// assert_eq!(TimeUnit::Minutes.as_str(), "min");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TimeUnit {
    /// `d`
    Days,
    /// `h`
    Hours,
    /// `min`
    Minutes,
    /// `s`
    Seconds,
    /// `ms`
    Milliseconds,
}

impl TimeUnit {
    /// Every time unit, longest first.
    pub const ALL: [Self; 5] = [
        Self::Days,
        Self::Hours,
        Self::Minutes,
        Self::Seconds,
        Self::Milliseconds,
    ];

    /// The unit as written: `d`, `h`, `min`, `s` or `ms`.
    ///
    /// ```
    /// assert_eq!(taktsim_model::TimeUnit::Seconds.as_str(), "s");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Days => "d",
            Self::Hours => "h",
            Self::Minutes => "min",
            Self::Seconds => "s",
            Self::Milliseconds => "ms",
        }
    }
}

/// Reads one of a fixed set of words.
fn one_of<T: Copy>(
    node: &Node,
    cx: &mut Decoder,
    what: &'static str,
    options: &[(&'static str, T)],
) -> Option<T> {
    let text = String::decode(node, cx)?;
    if let Some((_, value)) = options.iter().find(|(name, _)| *name == text) {
        return Some(*value);
    }
    let names = options.iter().map(|(name, _)| *name);
    let kind = LoadErrorKind::UnknownValue {
        what,
        suggestion: crate::error::suggest(&text, names.clone()),
        found: text,
        allowed: names.map(str::to_owned).collect(),
    };
    cx.error(node.start(), kind);
    None
}

impl Encode for TimeUnit {
    fn encode(&self) -> Json {
        self.as_str().into()
    }
}

impl Decode for TimeUnit {
    const EXPECTED: &'static str = "a time unit: d, h, min, s or ms";
    const EXAMPLE: &'static str = "s";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let options = Self::ALL.map(|u| (u.as_str(), u));
        one_of(node, cx, "time unit", &options)
    }

    fn schema(_: &mut Schemas) -> Json {
        let units = Self::ALL.iter().map(|u| Json::from(u.as_str())).collect();
        Json::object([("enum", Json::Array(units))])
    }
}

/// The model name.
struct Name(String);

impl Decode for Name {
    const EXPECTED: &'static str = "a name such as `line-a`";
    const EXAMPLE: &'static str = "line-a";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        read_id(node, cx, "model name").map(Self)
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("string")),
            ("pattern", Json::from(ID_PATTERN)),
            ("examples", Json::Array(vec![Json::from("line-a")])),
        ])
    }
}

impl Record for ModelInfo {
    const EXPECTED: &'static str = "the model section: a mapping with `name`";
    const EXAMPLE: &'static str = "{name: line-a, time_unit: s, seed: 42}";
    const SCHEMA_NAME: Option<&'static str> = Some("Model");
    const DESCRIPTION: Option<&'static str> = Some("The model's name and defaults.");

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let name = fields.required::<Name>("name");
        let time_unit = fields.optional("time_unit");
        let seed = fields.optional("seed");
        Some(Self {
            name: name?.0,
            time_unit: time_unit?,
            seed: seed?,
        })
    }
}

/// The type of an experiment (spec §6, "Output analysis").
///
/// Only `terminating` exists in this format version: `steady_state` needs a
/// warm-up rule, which arrives with steady-state experiments (roadmap M9),
/// so the loader rejects it with that reason.
///
/// ```
/// use taktsim_model::ExperimentKind;
///
/// assert_eq!(ExperimentKind::Terminating.as_str(), "terminating");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExperimentKind {
    /// A run with a natural end, such as one shift.
    Terminating,
}

impl ExperimentKind {
    /// The kind as written.
    ///
    /// ```
    /// assert_eq!(taktsim_model::ExperimentKind::Terminating.as_str(), "terminating");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Terminating => "terminating",
        }
    }
}

impl Decode for ExperimentKind {
    const EXPECTED: &'static str = "an experiment type: terminating";
    const EXAMPLE: &'static str = "terminating";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if let Value::Str(text) = node.value()
            && text == "steady_state"
        {
            cx.error(node.start(), LoadErrorKind::SteadyStateUnsupported);
            return None;
        }
        one_of(
            node,
            cx,
            "experiment type",
            &[("terminating", Self::Terminating)],
        )
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([("const", Json::from(Self::Terminating.as_str()))])
    }
}

/// The `experiment` section: how the model is run and analysed.
///
/// ```
/// fn horizon(doc: &taktsim_model::Document) -> String {
///     doc.experiment().horizon().to_string()
/// }
/// ```
#[derive(Debug)]
pub struct Experiment {
    kind: ExperimentKind,
    horizon: SimDuration,
    precision: Vec<Precision>,
}

impl Encode for Experiment {
    /// `{type, horizon, precision?}`, precision as a mapping from
    /// observable to target, left out when there is none.
    fn encode(&self) -> Json {
        let value =
            Json::object([("type", Json::from(self.kind.as_str()))]).with("horizon", &self.horizon);
        if self.precision.is_empty() {
            return value;
        }
        let targets = Json::Object(
            self.precision
                .iter()
                .map(|p| (p.observable.text().to_owned(), p.target.encode()))
                .collect(),
        );
        value.with("precision", &targets)
    }
}

impl Experiment {
    /// The experiment type.
    ///
    /// ```
    /// use taktsim_model::ExperimentKind;
    ///
    /// fn terminating(doc: &taktsim_model::Document) -> bool {
    ///     doc.experiment().kind() == ExperimentKind::Terminating
    /// }
    /// ```
    #[must_use]
    pub const fn kind(&self) -> ExperimentKind {
        self.kind
    }

    /// The simulated time of one replication; longer than zero.
    ///
    /// ```
    /// fn horizon(doc: &taktsim_model::Document) -> taktsim_units::SimDuration {
    ///     doc.experiment().horizon()
    /// }
    /// ```
    #[must_use]
    pub const fn horizon(&self) -> SimDuration {
        self.horizon
    }

    /// The precision targets: replications continue until each KPI's
    /// confidence interval half-width is within its relative target
    /// (ADR-0014). Sorted by the observable's canonical text.
    ///
    /// ```
    /// fn targets(doc: &taktsim_model::Document) -> usize {
    ///     doc.experiment().precision().len()
    /// }
    /// ```
    #[must_use]
    pub fn precision(&self) -> &[Precision] {
        &self.precision
    }
}

/// One precision target: `throughput(sink): 2%`.
///
/// ```
/// fn describe(p: &taktsim_model::Precision) -> String {
///     format!("{}: {}", p.observable().text(), p.target())
/// }
/// ```
#[derive(Debug)]
pub struct Precision {
    observable: Observable,
    target: Ratio,
}

impl Precision {
    /// The observable whose confidence interval is controlled.
    ///
    /// ```
    /// fn text(p: &taktsim_model::Precision) -> &str {
    ///     p.observable().text()
    /// }
    /// ```
    #[must_use]
    pub const fn observable(&self) -> &Observable {
        &self.observable
    }

    /// The relative half-width to reach, above 0 % and at most 100 %.
    ///
    /// ```
    /// fn target(p: &taktsim_model::Precision) -> f64 {
    ///     p.target().as_fraction()
    /// }
    /// ```
    #[must_use]
    pub const fn target(&self) -> Ratio {
        self.target
    }
}

/// An observable: a built-in function of component ids, such as
/// `throughput(sink)` (ADR-0009, ADR-0015), type-checked against the
/// catalog's observables and the model's components.
///
/// Its text is the canonical form of the expression, so
/// `throughput( sink )` and `throughput(sink)` are the same observable.
///
/// ```
/// fn function(o: &taktsim_model::Observable) -> &str {
///     o.text().split('(').next().unwrap_or_default()
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Observable {
    text: String,
    expr: Expr,
    place: Place,
}

impl Observable {
    /// The canonical text of the expression.
    ///
    /// ```
    /// fn text(o: &taktsim_model::Observable) -> &str {
    ///     o.text()
    /// }
    /// ```
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The parsed expression, with spans into the text as written.
    ///
    /// ```
    /// fn depth(o: &taktsim_model::Observable) -> usize {
    ///     o.expr().depth()
    /// }
    /// ```
    #[must_use]
    pub const fn expr(&self) -> &Expr {
        &self.expr
    }

    /// Where the observable is written.
    ///
    /// ```
    /// fn line(o: &taktsim_model::Observable) -> usize {
    ///     o.place().location().line()
    /// }
    /// ```
    #[must_use]
    pub const fn place(&self) -> &Place {
        &self.place
    }

    pub(crate) const fn new(text: String, expr: Expr, place: Place) -> Self {
        Self { text, expr, place }
    }
}

impl Encode for Observable {
    /// The canonical text.
    fn encode(&self) -> Json {
        self.text.as_str().into()
    }
}

/// One edge of the flow graph: units go from any component in `from` to a
/// component in `to` (spec §4, "Routing"). A single id is a one-element
/// list; a chain `[a, b, c]` is the edges `a → b` and `b → c`.
///
/// Conditions (`when`), choice rules (`choose`) and split ratios arrive
/// with routing (roadmap M6); until then an edge has only `from` and `to`.
///
/// ```
/// fn pairs(doc: &taktsim_model::Document) -> Vec<(String, String)> {
///     doc.flow()
///         .iter()
///         .flat_map(|e| {
///             e.from().iter().flat_map(move |f| e.to().iter().map(move |t| (f.id().to_owned(), t.id().to_owned())))
///         })
///         .collect()
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Edge {
    from: Vec<ComponentRef>,
    to: Vec<ComponentRef>,
}

impl Encode for Edge {
    /// `{from: [ids], to: [ids]}`.
    fn encode(&self) -> Json {
        Json::object([])
            .with("from", &self.from)
            .with("to", &self.to)
    }
}

impl Edge {
    /// The components units come from.
    ///
    /// ```
    /// fn sources(e: &taktsim_model::Edge) -> Vec<&str> {
    ///     e.from().iter().map(|r| r.id()).collect()
    /// }
    /// ```
    #[must_use]
    pub fn from(&self) -> &[ComponentRef] {
        &self.from
    }

    /// The components units go to.
    ///
    /// ```
    /// fn targets(e: &taktsim_model::Edge) -> Vec<&str> {
    ///     e.to().iter().map(|r| r.id()).collect()
    /// }
    /// ```
    #[must_use]
    pub fn to(&self) -> &[ComponentRef] {
        &self.to
    }
}

/// An observable as written, checked once the components are known.
#[derive(Debug, Clone)]
pub(crate) struct RawExpr {
    pub(crate) text: String,
    pub(crate) place: Place,
    pub(crate) path: FieldPath,
}

/// A precision target as written.
#[derive(Debug, Clone)]
pub(crate) struct RawPrecision {
    pub(crate) observable: RawExpr,
    pub(crate) target: Ratio,
}

/// The experiment as written.
#[derive(Debug)]
pub(crate) struct RawExperiment {
    pub(crate) kind: ExperimentKind,
    pub(crate) horizon: SimDuration,
    pub(crate) precision: Vec<RawPrecision>,
}

impl RawExperiment {
    pub(crate) fn finish(self, precision: Vec<Precision>) -> Experiment {
        Experiment {
            kind: self.kind,
            horizon: self.horizon,
            precision,
        }
    }
}

impl Precision {
    pub(crate) const fn new(observable: Observable, target: Ratio) -> Self {
        Self { observable, target }
    }
}

/// The horizon: a duration longer than zero.
struct Horizon(SimDuration);

impl Decode for Horizon {
    const EXPECTED: &'static str = SimDuration::EXPECTED;
    const EXAMPLE: &'static str = "8h";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let horizon = SimDuration::decode(node, cx)?;
        if horizon == SimDuration::ZERO {
            cx.not_allowed(node, horizon.to_string(), "a duration longer than 0s", "8h");
            return None;
        }
        Some(Self(horizon))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        schemas.of::<SimDuration>()
    }
}

impl Record for RawExperiment {
    const EXPECTED: &'static str = "the experiment section: a mapping with `type` and `horizon`";
    const EXAMPLE: &'static str = "{type: terminating, horizon: 8h}";
    const SCHEMA_NAME: Option<&'static str> = Some("Experiment");
    const DESCRIPTION: Option<&'static str> = Some(
        "How the model is run: a terminating experiment over `horizon`, with optional \
         relative precision targets per observable (ADR-0014).",
    );

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let kind = fields.required::<ExperimentKind>("type");
        let horizon = fields.required::<Horizon>("horizon");
        let precision = fields.optional::<Targets>("precision");
        Some(Self {
            kind: kind?,
            horizon: horizon?.0,
            precision: precision?.map(|t| t.0).unwrap_or_default(),
        })
    }
}

/// A relative precision: above 0 %, at most 100 %.
struct Target(Ratio);

impl Decode for Target {
    const EXPECTED: &'static str = Ratio::EXPECTED;
    const EXAMPLE: &'static str = "2%";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let ratio = Ratio::decode(node, cx)?;
        if !(ratio.as_percent() > 0.0 && ratio.as_percent() <= 100.0) {
            cx.not_allowed(
                node,
                ratio.to_string(),
                "a percentage above 0% and at most 100%",
                "2%",
            );
            return None;
        }
        Some(Self(ratio))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        schemas.of::<Ratio>()
    }
}

/// `precision: {observable: ratio, …}`.
struct Targets(Vec<RawPrecision>);

impl Decode for Targets {
    const EXPECTED: &'static str = "a mapping from observable to relative precision";
    const EXAMPLE: &'static str = "{throughput(sink): 2%}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Map(entries) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        let mut targets = Vec::with_capacity(entries.len());
        let mut ok = true;
        for entry in entries {
            let read = cx.within(Segment::Key(entry.key().to_owned()), |cx| {
                let target = Target::decode(entry.value(), cx)?;
                Some(RawPrecision {
                    observable: RawExpr {
                        text: entry.key().to_owned(),
                        place: Place::new(cx.file().map(str::to_owned), entry.key_start()),
                        path: cx.path().clone(),
                    },
                    target: target.0,
                })
            });
            match read {
                Some(target) => targets.push(target),
                None => ok = false,
            }
        }
        ok.then_some(Self(targets))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("object")),
            ("propertyNames", schemas.of::<ObservableText>()),
            ("additionalProperties", schemas.of::<Target>()),
        ])
    }
}

/// An observable as written in `observe`.
struct ObservableText(RawExpr);

impl Decode for ObservableText {
    const EXPECTED: &'static str = String::EXPECTED;
    const EXAMPLE: &'static str = "throughput(sink)";
    const SCHEMA_NAME: Option<&'static str> = Some("Observable");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let text = String::decode(node, cx)?;
        Some(Self(RawExpr {
            text,
            place: Place::new(cx.file().map(str::to_owned), node.start()),
            path: cx.path().clone(),
        }))
    }

    /// A call of one of the catalog's observables; nothing if it declares
    /// none. The loader checks the arguments and the type.
    fn schema(schemas: &mut Schemas) -> Json {
        let functions = schemas.observables();
        let Some(first) = functions.first() else {
            return Json::object([
                (
                    "description",
                    Json::from("An observable; this catalog declares none, so none is valid."),
                ),
                ("not", Json::object([])),
            ]);
        };
        Json::object([
            ("type", Json::from("string")),
            (
                "description",
                Json::from(format!(
                    "An observable: a call of {} on component ids, e.g. `{first}(sink)` \
                     (ADR-0009, ADR-0015). The loader checks its arguments and type.",
                    functions.join(", "),
                )),
            ),
            (
                "pattern",
                Json::from(format!(r"^[ \t]*({})[ \t]*\(", functions.join("|"))),
            ),
        ])
    }
}

/// The sections a file may contribute to the model's lists.
#[derive(Debug)]
pub(crate) struct Parts {
    pub(crate) components: Option<Vec<Component>>,
    pub(crate) flow: Option<Vec<Edge>>,
    /// Where each edge was written: the entry, then each `to` reference.
    pub(crate) flow_places: Vec<EdgePlaces>,
    pub(crate) observe: Option<Vec<RawExpr>>,
    /// Whether the file has the key `components`, `flow`.
    pub(crate) has_components: bool,
    pub(crate) has_flow: bool,
    /// Whether every section present was read without errors; if not, the
    /// checks across sections are skipped, as they would report errors
    /// that follow from the first ones.
    pub(crate) complete: bool,
}

/// The model file, as written.
#[derive(Debug)]
pub(crate) struct MainFile {
    pub(crate) model: Option<ModelInfo>,
    pub(crate) experiment: Option<RawExperiment>,
    pub(crate) includes: Vec<IncludeSpec>,
    pub(crate) parts: Parts,
    /// Where the document starts, for errors about missing sections.
    pub(crate) start: Location,
}

/// Reads the model file's top-level mapping; `None` if it cannot be read
/// at all (not a mapping, or a format version this engine does not read).
pub(crate) fn read_main(node: &Node, cx: &mut Decoder, catalog: &Catalog) -> Option<MainFile> {
    let mut fields = cx.fields::<MainFile>(node)?;
    let Some(main) = read_main_fields(&mut fields, catalog) else {
        // Another format version: its sections may mean something else, so
        // reporting errors in them would mislead.
        fields.ignore_rest();
        return None;
    };
    fields.finish();
    Some(MainFile {
        start: node.start(),
        ..main
    })
}

/// The keys of the model file; `None` for another format version. Also
/// run to describe the model file in its schema.
pub(crate) fn read_main_fields(fields: &mut Fields<'_, '_>, catalog: &Catalog) -> Option<MainFile> {
    if !read_format(fields) {
        return None;
    }
    let model = fields.required::<ModelInfo>("model");
    let experiment = fields.required::<RawExperiment>("experiment");
    let includes = fields.optional::<Includes>("include");
    let parts = read_parts(fields, catalog);
    Some(MainFile {
        model,
        experiment,
        includes: includes.flatten().map(|i| i.0).unwrap_or_default(),
        parts,
        start: Location::new(1, 1),
    })
}

impl Decode for MainFile {
    const EXPECTED: &'static str =
        "a model: a mapping with `format`, `model`, `experiment`, `components` and `flow`";
    const EXAMPLE: &'static str = "format: \"1.0\"";

    /// Not used: the model file is read with [`read_main`], which needs
    /// the catalog.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        cx.wrong_type::<Self>(node);
        None
    }

    /// Not used: the schema describes [`read_main_fields`].
    fn schema(_: &mut Schemas) -> Json {
        Json::object([("type", Json::from("object"))])
    }
}

/// Reads `format`; `false` if it names a version this engine does not
/// read. A missing or mistyped header is an error, but the rest is still
/// read.
fn read_format(fields: &mut Fields<'_, '_>) -> bool {
    /// The format version.
    struct Version;
    impl Decode for Version {
        const EXPECTED: &'static str = "the format version as text: \"1.0\" (quoted)";
        const EXAMPLE: &'static str = "\"1.0\"";
        fn schema(_: &mut Schemas) -> Json {
            Json::object([
                ("const", Json::from(FORMAT_VERSION)),
                (
                    "description",
                    Json::from("The format version (ADR-0008); quote it."),
                ),
            ])
        }
        fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
            let version = String::decode(node, cx)?;
            if version != FORMAT_VERSION {
                cx.error(
                    node.start(),
                    LoadErrorKind::UnsupportedFormat { found: version },
                );
                return None;
            }
            Some(Self)
        }
    }

    let unsupported = |fields: &Fields<'_, '_>| {
        fields
            .entry("format")
            .is_some_and(|e| matches!(e.value().value(), Value::Str(s) if s != FORMAT_VERSION))
    };
    let other_version = unsupported(fields);
    fields.required::<Version>("format");
    !other_version
}

/// The `components` section: a list of entries.
struct ComponentList;

impl Decode for ComponentList {
    const EXPECTED: &'static str = "a list of components";
    const EXAMPLE: &'static str = "[{id: cut, type: Machine, cycle_time: 42s}]";

    /// Not used: the entries are read with the catalog.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        cx.wrong_type::<Self>(node);
        None
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<ComponentEntry>()),
        ])
    }
}

/// The `flow` section: chains and edges.
struct FlowList(Vec<(Edge, EdgePlaces)>);

impl Decode for FlowList {
    const EXPECTED: &'static str = "a list of chains and edges";
    const EXAMPLE: &'static str = "[[source, cut, sink]]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if !matches!(node.value(), Value::List(_)) {
            cx.wrong_type::<Self>(node);
            return None;
        }
        let entries = cx.list(node, FlowEntry::decode)?;
        Some(Self(entries.into_iter().flat_map(|e| e.0).collect()))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<FlowEntry>()),
        ])
    }
}

/// The `observe` section.
struct ObserveList(Vec<RawExpr>);

impl Decode for ObserveList {
    const EXPECTED: &'static str = "a list of observables";
    const EXAMPLE: &'static str = "[throughput(sink)]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if !matches!(node.value(), Value::List(_)) {
            cx.wrong_type::<Self>(node);
            return None;
        }
        let items = cx.list(node, ObservableText::decode)?;
        Some(Self(items.into_iter().map(|o| o.0).collect()))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<ObservableText>()),
        ])
    }
}

/// Reads the list sections (`components`, `flow`, `observe`), which the
/// model file and included files share.
pub(crate) fn read_parts(fields: &mut Fields<'_, '_>, catalog: &Catalog) -> Parts {
    let has_components = fields.entry("components").is_some();
    let has_flow = fields.entry("flow").is_some();
    let components = fields.optional_with::<ComponentList, _>("components", |node, cx| {
        if !matches!(node.value(), Value::List(_)) {
            cx.wrong_type::<ComponentList>(node);
            return None;
        }
        cx.list(node, |item, cx| read_component(item, cx, catalog))
    });
    let flow = fields.optional::<FlowList>("flow");
    let observe = fields.optional::<ObserveList>("observe");
    let complete = components.is_some() && flow.is_some() && observe.is_some();
    let (flow, flow_places) = match flow.flatten() {
        Some(FlowList(entries)) => {
            let (edges, places): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
            (Some(edges), places)
        }
        None => (None, Vec::new()),
    };
    Parts {
        components: components.flatten(),
        flow,
        flow_places,
        observe: observe.flatten().map(|o| o.0),
        has_components,
        has_flow,
        complete,
    }
}

/// The places of an edge's parts: the entry, then each `to` reference.
pub(crate) type EdgePlaces = Vec<(Place, FieldPath)>;

/// An edge as a mapping: `{from, to}`.
struct EdgeMap {
    from: IdList,
    to: IdList,
}

impl Record for EdgeMap {
    const EXPECTED: &'static str = FlowEntry::EXPECTED;
    const EXAMPLE: &'static str = "{from: cut, to: [weld-1, weld-2]}";

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let from = fields.required("from");
        let to = fields.required("to");
        Some(Self {
            from: from?,
            to: to?,
        })
    }
}

/// One flow entry: a chain (a list of ids) or an edge (a mapping with
/// `from` and `to`), as edges.
struct FlowEntry(Vec<(Edge, EdgePlaces)>);

impl Decode for FlowEntry {
    const EXPECTED: &'static str = "a chain such as `[source, cut, sink]` or an edge such as `{from: cut, to: [weld-1, weld-2]}`";
    const EXAMPLE: &'static str = "[source, cut, sink]";
    const SCHEMA_NAME: Option<&'static str> = Some("FlowEntry");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let place = Place::new(cx.file().map(str::to_owned), node.start());
        match node.value() {
            Value::List(items) => {
                let ids = cx.list(node, ComponentRef::decode)?;
                if ids.len() < 2 {
                    let kind = LoadErrorKind::TooFew {
                        what: "a chain",
                        min: 2,
                        found: items.len(),
                        example: "[source, cut, sink]",
                    };
                    cx.error(node.start(), kind);
                    return None;
                }
                let edges = ids
                    .windows(2)
                    .map(|pair| {
                        let places = vec![
                            (place.clone(), cx.path().clone()),
                            (pair[1].place().clone(), pair[1].path().clone()),
                        ];
                        let edge = Edge {
                            from: vec![pair[0].clone()],
                            to: vec![pair[1].clone()],
                        };
                        (edge, places)
                    })
                    .collect();
                Some(Self(edges))
            }
            Value::Map(_) => {
                let EdgeMap { from, to } = EdgeMap::decode(node, cx)?;
                let (from, to) = (from.0, to.0);
                let mut places = vec![(place, cx.path().clone())];
                places.extend(to.iter().map(|r| (r.place().clone(), r.path().clone())));
                Some(Self(vec![(Edge { from, to }, places)]))
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
                cx.wrong_type::<Self>(node);
                None
            }
        }
    }

    fn schema(schemas: &mut Schemas) -> Json {
        let chain = Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<ComponentRef>()),
            ("minItems", Json::from(2)),
        ]);
        Json::object([
            (
                "description",
                Json::from(
                    "A chain of at least two components, or an edge from one or several components \
                     to one or several components (branch, merge, loop-back).",
                ),
            ),
            ("oneOf", Json::Array(vec![chain, schemas.of::<EdgeMap>()])),
        ])
    }
}

/// One id or a list of distinct ids: the `from` or `to` of an edge.
struct IdList(Vec<ComponentRef>);

impl Decode for IdList {
    const EXPECTED: &'static str = "a component id or a list of them";
    const EXAMPLE: &'static str = "[weld-1, weld-2]";
    const SCHEMA_NAME: Option<&'static str> = Some("EdgeEnd");

    fn schema(schemas: &mut Schemas) -> Json {
        let id = schemas.of::<ComponentRef>();
        let list = Json::object([
            ("type", Json::from("array")),
            ("items", id.clone()),
            ("minItems", Json::from(1)),
            ("uniqueItems", Json::from(true)),
        ]);
        Json::object([
            (
                "description",
                Json::from("A component id, or a list of different ones."),
            ),
            ("oneOf", Json::Array(vec![id, list])),
        ])
    }

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        match node.value() {
            Value::Str(_) => ComponentRef::decode(node, cx).map(|r| Self(vec![r])),
            Value::List(items) => {
                let ids = cx.list(node, ComponentRef::decode)?;
                if ids.is_empty() {
                    let kind = LoadErrorKind::TooFew {
                        what: "an edge end",
                        min: 1,
                        found: 0,
                        example: Self::EXAMPLE,
                    };
                    cx.error(node.start(), kind);
                    return None;
                }
                let mut ok = true;
                for (i, id) in ids.iter().enumerate() {
                    if ids[..i].contains(id) {
                        let at = items[i].start();
                        let kind = LoadErrorKind::Repeated {
                            id: id.id().to_owned(),
                        };
                        cx.within(Segment::Index(i), |cx| cx.error(at, kind));
                        ok = false;
                    }
                }
                ok.then_some(Self(ids))
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Map(_) => {
                cx.wrong_type::<Self>(node);
                None
            }
        }
    }
}
