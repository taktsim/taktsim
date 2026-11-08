//! The model document (spec §4; roadmap M4): its sections, as read from a
//! file, and their normalized form.

use taktsim_units::{Ratio, SimDuration};

use crate::component::{
    Catalog, Component, ComponentEntry, ComponentRef, ID_PATTERN, RoutingWords, read_component,
    read_id,
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
#[derive(Debug, Clone)]
pub struct Document {
    pub(crate) model: ModelInfo,
    pub(crate) experiment: Experiment,
    pub(crate) components: Vec<Component>,
    pub(crate) flow: Vec<Edge>,
    pub(crate) observe: Vec<Observable>,
    pub(crate) control: Control,
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

    /// The `control` section: read from the model file only, empty when it is
    /// absent (ADR-0023 §2).
    ///
    /// ```
    /// fn rules(doc: &taktsim_model::Document) -> usize {
    ///     doc.control().dispatch().len()
    /// }
    /// ```
    #[must_use]
    pub const fn control(&self) -> &Control {
        &self.control
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
        let doc = if self.observe.is_empty() {
            doc
        } else {
            doc.with("observe", &self.observe)
        };
        Ok(if self.control.dispatch.is_empty() {
            doc
        } else {
            doc.with("control", &self.control)
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
#[derive(Debug, Clone)]
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
    pub(crate) fn set_horizon(&mut self, horizon: SimDuration) {
        self.horizon = horizon;
    }

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
#[derive(Debug, Clone)]
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

/// How a part chooses among the targets of an edge (spec §4, ADR-0023 §5). The
/// list order is the preference, and the last term of every rank. Matched
/// exhaustively, with no wildcard arm, so a new rule has to be handled everywhere
/// (ADR-0023 §2).
///
/// ```
/// use taktsim_model::{ChoiceRule, Format, decode_str};
///
/// let rule: ChoiceRule = decode_str("shortest_queue", Format::Yaml)?;
/// assert_eq!(rule, ChoiceRule::ShortestQueue);
/// assert_eq!(rule.as_str(), "shortest_queue");
/// assert!(decode_str::<ChoiceRule>("shortest", Format::Yaml).is_err());
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChoiceRule {
    /// Offers to every target; the first `Accept` takes the part. When room
    /// opens at several at one instant, event order decides.
    FirstFree,
    /// Has room, then the fewest units at the target, then list order.
    ShortestQueue,
    /// Has room, then the least work per place so far, then list order.
    LowestUtilization,
    /// Has room, then list order: preferred with fallback.
    Preferred,
}

impl ChoiceRule {
    /// Every rule, in the order of the documents.
    pub const ALL: [Self; 4] = [
        Self::FirstFree,
        Self::ShortestQueue,
        Self::LowestUtilization,
        Self::Preferred,
    ];

    /// The word in a model.
    ///
    /// ```
    /// assert_eq!(taktsim_model::ChoiceRule::FirstFree.as_str(), "first_free");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FirstFree => "first_free",
            Self::ShortestQueue => "shortest_queue",
            Self::LowestUtilization => "lowest_utilization",
            Self::Preferred => "preferred",
        }
    }
}

impl Encode for ChoiceRule {
    fn encode(&self) -> Json {
        self.as_str().into()
    }
}

impl Decode for ChoiceRule {
    const EXPECTED: &'static str =
        "a choice rule: first_free, shortest_queue, lowest_utilization or preferred";
    const EXAMPLE: &'static str = "shortest_queue";
    const SCHEMA_NAME: Option<&'static str> = Some("ChoiceRule");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let options = Self::ALL.map(|r| (r.as_str(), r));
        one_of(node, cx, "choice rule", &options)
    }

    fn schema(_: &mut Schemas) -> Json {
        let words = Self::ALL.iter().map(|r| Json::from(r.as_str())).collect();
        Json::object([
            ("enum", Json::Array(words)),
            (
                "description",
                Json::from(
                    "How a part chooses among the targets of an edge, the list being the \
                     preference. Default first_free.",
                ),
            ),
        ])
    }
}

/// Which waiting unit a station takes next (spec §4, ADR-0023 §6). Lower key
/// wins; ties go to the oldest ready unit. Matched exhaustively, with no wildcard
/// arm.
///
/// ```
/// use taktsim_model::{DispatchRule, Format, decode_str};
///
/// let rule: DispatchRule = decode_str("earliest_due_date", Format::Yaml)?;
/// assert_eq!(rule, DispatchRule::EarliestDueDate);
/// assert_eq!(rule.as_str(), "earliest_due_date");
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DispatchRule {
    /// The oldest ready unit, across every upstream.
    Fifo,
    /// The earliest due date; a unit with none comes after all others.
    EarliestDueDate,
    /// The shortest mean time on this machine.
    ShortestProcessingTime,
    /// The same variant first, then the cheapest change.
    MinimumSetup,
    /// The highest priority.
    PriorityClass,
}

impl DispatchRule {
    /// Every rule, in the order of the documents.
    pub const ALL: [Self; 5] = [
        Self::Fifo,
        Self::EarliestDueDate,
        Self::ShortestProcessingTime,
        Self::MinimumSetup,
        Self::PriorityClass,
    ];

    /// The word in a model.
    ///
    /// ```
    /// assert_eq!(taktsim_model::DispatchRule::Fifo.as_str(), "fifo");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fifo => "fifo",
            Self::EarliestDueDate => "earliest_due_date",
            Self::ShortestProcessingTime => "shortest_processing_time",
            Self::MinimumSetup => "minimum_setup",
            Self::PriorityClass => "priority_class",
        }
    }
}

impl Encode for DispatchRule {
    fn encode(&self) -> Json {
        self.as_str().into()
    }
}

impl Decode for DispatchRule {
    const EXPECTED: &'static str = "a dispatching rule: fifo, earliest_due_date, \
         shortest_processing_time, minimum_setup or priority_class";
    const EXAMPLE: &'static str = "earliest_due_date";
    const SCHEMA_NAME: Option<&'static str> = Some("DispatchRule");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let options = Self::ALL.map(|r| (r.as_str(), r));
        one_of(node, cx, "dispatching rule", &options)
    }

    fn schema(_: &mut Schemas) -> Json {
        let words = Self::ALL.iter().map(|r| Json::from(r.as_str())).collect();
        Json::object([
            ("enum", Json::Array(words)),
            (
                "description",
                Json::from("Which waiting unit the station takes next."),
            ),
        ])
    }
}

/// The `control` section: the dispatching rule declared for each station, in the
/// order of their ids. `release` waits for v0.2.
///
/// ```
/// fn rule_of<'a>(doc: &'a taktsim_model::Document, id: &str) -> Option<taktsim_model::DispatchRule> {
///     doc.control().dispatch().iter().find(|(r, _)| r.id() == id).map(|(_, rule)| *rule)
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Control {
    dispatch: Vec<(ComponentRef, DispatchRule)>,
}

impl Control {
    /// Each station with a declared rule, sorted by id.
    ///
    /// ```
    /// assert!(taktsim_model::Control::default().dispatch().is_empty());
    /// ```
    #[must_use]
    pub fn dispatch(&self) -> &[(ComponentRef, DispatchRule)] {
        &self.dispatch
    }
}

impl Encode for Control {
    /// `{dispatch: {id: rule}}`.
    fn encode(&self) -> Json {
        let rules = Json::Object(
            self.dispatch
                .iter()
                .map(|(id, rule)| (id.id().to_owned(), rule.encode()))
                .collect(),
        );
        Json::object([]).with("dispatch", &rules)
    }
}

impl Record for Control {
    const EXPECTED: &'static str = "the control section: a mapping with `dispatch`";
    const EXAMPLE: &'static str = "{dispatch: {weld-1: earliest_due_date}}";
    const SCHEMA_NAME: Option<&'static str> = Some("Control");
    const DESCRIPTION: Option<&'static str> =
        Some("Control logic: the dispatching rule of each station (ADR-0023).");

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let dispatch = fields.optional::<DispatchRules>("dispatch");
        Some(Self {
            dispatch: dispatch?.map(|d| d.0).unwrap_or_default(),
        })
    }
}

/// `control.dispatch`: a mapping from station to rule, sorted by id.
struct DispatchRules(Vec<(ComponentRef, DispatchRule)>);

impl Decode for DispatchRules {
    const EXPECTED: &'static str = "a mapping from station to dispatching rule";
    const EXAMPLE: &'static str = "{weld-1: earliest_due_date}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Map(entries) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        let mut rules = Vec::with_capacity(entries.len());
        let mut ok = true;
        for entry in entries {
            let read = cx.within(Segment::Key(entry.key().to_owned()), |cx| {
                let station = ComponentRef::from_key(entry, cx);
                let rule = DispatchRule::decode(entry.value(), cx);
                Some((station?, rule?))
            });
            match read {
                Some(rule) => rules.push(rule),
                None => ok = false,
            }
        }
        rules.sort_by(|a, b| a.0.id().cmp(b.0.id()));
        ok.then_some(Self(rules))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("object")),
            ("propertyNames", schemas.of::<ComponentRef>()),
            ("additionalProperties", schemas.of::<DispatchRule>()),
        ])
    }
}

/// A condition on an edge: as written until the loader parses it, then its
/// expression, whose canonical text is the normalized form.
#[derive(Debug, Clone)]
struct When {
    written: String,
    expr: Option<Expr>,
    place: Place,
    path: FieldPath,
}

/// One edge of the flow graph: units go from any component in `from` to a
/// component in `to` (spec §4, "Routing"). A single id is a one-element
/// list; a chain `[a, b, c]` is the edges `a → b` and `b → c`.
///
/// Edges leaving one component are tried in the order written; the first whose
/// condition holds is the part's way out (ADR-0023 §3). With a [`split`](Self::split),
/// [`to`](Self::to) lists the split's targets, sorted by id.
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
    when: Option<When>,
    choose: Option<ChoiceRule>,
    split: Option<Vec<(ComponentRef, Ratio)>>,
}

impl Encode for Edge {
    /// `{from: [ids], to: [ids], when?, choose?}`, or `{from: [ids], split: {id:
    /// share}, when?}`; what is absent is left out, so no pinned hash moves.
    fn encode(&self) -> Json {
        let value = Json::object([]).with("from", &self.from);
        let value = match &self.split {
            Some(shares) => {
                let shares = Json::Object(
                    shares
                        .iter()
                        .map(|(id, share)| (id.id().to_owned(), share.encode()))
                        .collect(),
                );
                value.with("split", &shares)
            }
            None => value.with("to", &self.to),
        };
        let when = self.when.as_ref().map(|when| {
            when.expr
                .as_ref()
                .map_or_else(|| when.written.clone(), ToString::to_string)
        });
        value
            .with_optional("when", when.as_ref())
            .with_optional("choose", self.choose.as_ref())
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

    /// The components units go to: for a [`split`](Self::split), its targets
    /// sorted by id.
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

    /// The condition, if the edge has one. Its [`Display`](std::fmt::Display) is
    /// the canonical text.
    ///
    /// ```
    /// fn condition(e: &taktsim_model::Edge) -> Option<String> {
    ///     e.when().map(ToString::to_string)
    /// }
    /// ```
    #[must_use]
    pub fn when(&self) -> Option<&Expr> {
        self.when.as_ref().and_then(|when| when.expr.as_ref())
    }

    /// The condition as written, where it is written and its field path, for an
    /// error a catalog's check reports.
    ///
    /// ```
    /// fn path(e: &taktsim_model::Edge) -> Option<String> {
    ///     e.when_written().map(|(_, _, path)| path.to_string())
    /// }
    /// ```
    #[must_use]
    pub fn when_written(&self) -> Option<(&str, &Place, &FieldPath)> {
        self.when
            .as_ref()
            .map(|when| (when.written.as_str(), &when.place, &when.path))
    }

    /// The written choice rule. Absent, a part takes the first target free.
    ///
    /// ```
    /// use taktsim_model::{ChoiceRule, Edge};
    ///
    /// fn rule(e: &Edge) -> ChoiceRule {
    ///     e.choose().unwrap_or(ChoiceRule::FirstFree)
    /// }
    /// ```
    #[must_use]
    pub const fn choose(&self) -> Option<ChoiceRule> {
        self.choose
    }

    /// The share of each target, sorted by id, if the edge is a split.
    ///
    /// ```
    /// fn total(e: &taktsim_model::Edge) -> f64 {
    ///     e.split().map_or(0.0, |shares| shares.iter().map(|(_, s)| s.as_percent()).sum())
    /// }
    /// ```
    #[must_use]
    pub fn split(&self) -> Option<&[(ComponentRef, Ratio)]> {
        self.split.as_deref()
    }

    /// Replaces the condition as written by what the loader made of it.
    pub(crate) fn take_when(&mut self) -> Option<(&str, &mut Option<Expr>, &Place, &FieldPath)> {
        self.when.as_mut().map(|when| {
            (
                when.written.as_str(),
                &mut when.expr,
                &when.place,
                &when.path,
            )
        })
    }

    /// Whether the edge has a condition, parsed or not.
    pub(crate) const fn has_when(&self) -> bool {
        self.when.is_some()
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
            ("propertyNames", schemas.of::<ObservableRef>()),
            ("additionalProperties", schemas.of::<Target>()),
        ])
    }
}

/// A precision key: a reference to an observable, opaque to the schema.
///
/// The experiment section belongs to the envelope, which every engine
/// shares, so its schema must not depend on a domain's catalog (owner's
/// decision, 2026-11-04; ADR-0020 §11). The schema only requires the
/// shape of a call; the loader checks that the catalog declares the
/// function, and its arguments and type, as for `observe`.
struct ObservableRef;

impl Decode for ObservableRef {
    const EXPECTED: &'static str = String::EXPECTED;
    const EXAMPLE: &'static str = "throughput(sink)";
    const SCHEMA_NAME: Option<&'static str> = Some("ObservableRef");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        String::decode(node, cx).map(|_| Self)
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("string")),
            (
                "description",
                Json::from(
                    "A reference to an observable: a call on component ids, e.g. \
                     `throughput(sink)`. The schema only requires the shape of a \
                     call; the loader checks that the catalog declares the function, \
                     and its arguments and type.",
                ),
            ),
            (
                "pattern",
                Json::from(r"^[ \t]*[A-Za-z_][A-Za-z0-9_]*[ \t]*\("),
            ),
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
    /// The `control` section; `None` if it could not be read.
    pub(crate) control: Option<Control>,
    /// `None` if `include:` could not be read: the files it names may hold
    /// the sections the checks across files would report as missing.
    pub(crate) includes: Option<Vec<IncludeSpec>>,
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
    // Only where the catalog honours it; otherwise an unknown key (ADR-0023 §2).
    let control = if catalog.routing().dispatch {
        fields.optional::<Control>("control")
    } else {
        Some(None)
    };
    Some(MainFile {
        model,
        experiment,
        control: control.map(Option::unwrap_or_default),
        includes: includes.map(|read| read.map(|i| i.0).unwrap_or_default()),
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

impl FlowList {
    /// Reads the list, with the routing words the catalog honours.
    fn read(node: &Node, cx: &mut Decoder, words: RoutingWords) -> Option<Self> {
        if !matches!(node.value(), Value::List(_)) {
            cx.wrong_type::<Self>(node);
            return None;
        }
        let entries = cx.list(node, |item, cx| FlowEntry::read(item, cx, words))?;
        Some(Self(entries.into_iter().flat_map(|e| e.0).collect()))
    }
}

impl Decode for FlowList {
    const EXPECTED: &'static str = "a list of chains and edges";
    const EXAMPLE: &'static str = "[[source, cut, sink]]";

    /// Reads with no routing words: the loader uses [`FlowList::read`].
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        Self::read(node, cx, RoutingWords::NONE)
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
    let words = catalog.routing();
    let flow =
        fields.optional_with::<FlowList, _>("flow", |node, cx| FlowList::read(node, cx, words));
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

/// An edge's condition as written: parsed once the whole document is read.
struct WhenText(When);

impl Decode for WhenText {
    const EXPECTED: &'static str = "a condition on a part, such as `variant == 'premium'`";
    const EXAMPLE: &'static str = "variant == 'premium'";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let written = String::decode(node, cx)?;
        Some(Self(When {
            written,
            expr: None,
            place: Place::new(cx.file().map(str::to_owned), node.start()),
            path: cx.path().clone(),
        }))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("string")),
            (
                "description",
                Json::from(
                    "A boolean expression over the part's attributes (ADR-0009, ADR-0023). A part \
                     takes the first edge whose condition holds, in the order written, and the \
                     last edge leaving a component has none. The loader checks its type.",
                ),
            ),
            (
                "examples",
                Json::Array(vec![Json::from("variant == 'premium'")]),
            ),
        ])
    }
}

/// `split`: a mapping from target to share, sorted by id.
struct SplitShares(Vec<(ComponentRef, Ratio)>);

impl Decode for SplitShares {
    const EXPECTED: &'static str = "a mapping from target to share, such as `{cut: 5%, pack: 95%}`";
    const EXAMPLE: &'static str = "{cut: 5%, pack: 95%}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Map(entries) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        let mut shares = Vec::with_capacity(entries.len());
        let mut ok = true;
        for entry in entries {
            let read = cx.within(Segment::Key(entry.key().to_owned()), |cx| {
                let target = ComponentRef::from_key(entry, cx);
                let share = Ratio::decode(entry.value(), cx);
                Some((target?, share?))
            });
            match read {
                Some(share) => shares.push(share),
                None => ok = false,
            }
        }
        shares.sort_by(|a, b| a.0.id().cmp(b.0.id()));
        ok.then_some(Self(shares))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("object")),
            ("minProperties", Json::from(2)),
            ("propertyNames", schemas.of::<ComponentRef>()),
            ("additionalProperties", schemas.of::<Ratio>()),
            (
                "description",
                Json::from(
                    "The share of each target, each above 0 %, summing to 100 %. A part takes one \
                     target by one draw and waits for it when it is full.",
                ),
            ),
        ])
    }
}

/// An edge as a mapping, in either closed shape: `{from, to, when?, choose?}` or
/// `{from, split, when?}`.
struct EdgeMap {
    from: Vec<ComponentRef>,
    to: Vec<ComponentRef>,
    when: Option<When>,
    choose: Option<ChoiceRule>,
    split: Option<Vec<(ComponentRef, Ratio)>>,
}

impl Decode for EdgeMap {
    const EXPECTED: &'static str = FlowEntry::EXPECTED;
    const EXAMPLE: &'static str = "{from: cut, to: [weld-1, weld-2]}";

    /// Reads with no routing words: the loader uses [`EdgeMap::read`].
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        Self::read(node, cx, RoutingWords::NONE)
    }

    /// The closed shapes the catalog offers (ADR-0023 §2): with `split`, two,
    /// and a value can match only one of them.
    fn schema(schemas: &mut Schemas) -> Json {
        let words = schemas.routing();
        let to = schemas.describe(|fields| {
            let _ = Self::read_to(fields, words);
        });
        if !words.split {
            return to;
        }
        let split = schemas.describe(|fields| {
            let _ = Self::read_split(fields, words);
        });
        Json::object([("oneOf", Json::Array(vec![to, split]))])
    }
}

impl EdgeMap {
    /// Reads the mapping, with the routing words the catalog honours. A word it
    /// does not honour is an unknown key.
    fn read(node: &Node, cx: &mut Decoder, words: RoutingWords) -> Option<Self> {
        let mut fields = cx.fields::<Self>(node)?;
        let edge = if words.split && fields.entry("split").is_some() {
            Self::report_mixed(&mut fields, words);
            Self::read_split(&mut fields, words)
        } else {
            let edge = Self::read_to(&mut fields, words);
            // Known, so the message about an unknown key lists it.
            if words.split {
                let _ = fields.take("split");
            }
            edge
        };
        fields.finish();
        edge
    }

    /// `split` with `to` or `choose`: one error for each, at the other key.
    fn report_mixed(fields: &mut Fields<'_, '_>, words: RoutingWords) {
        for (key, allowed) in [
            ("to", "a split names its targets itself; leave out `to`"),
            (
                "choose",
                "a split chooses by its shares; leave out `choose`, or use `to` with `choose`",
            ),
        ] {
            let offered = key == "to" || words.choose;
            if let (true, Some(entry)) = (offered, fields.entry(key)) {
                fields.skip(key);
                fields.decoder().within(Segment::Key(key.to_owned()), |cx| {
                    cx.not_allowed(
                        entry.value(),
                        format!("`{key}` together with `split`"),
                        allowed,
                        "{from: qc, split: {cut: 5%, pack: 95%}}",
                    );
                });
            }
        }
    }

    /// `{from, to, when?, choose?}`.
    fn read_to(fields: &mut Fields<'_, '_>, words: RoutingWords) -> Option<Self> {
        let from = fields.required::<IdList>("from");
        let to = fields.required::<IdList>("to");
        let when = if words.when {
            fields.optional::<WhenText>("when")
        } else {
            Some(None)
        };
        let choose = if words.choose {
            fields.optional("choose")
        } else {
            Some(None)
        };
        Some(Self {
            from: from?.0,
            to: to?.0,
            when: when?.map(|w| w.0),
            choose: choose?,
            split: None,
        })
    }

    /// `{from, split, when?}`.
    fn read_split(fields: &mut Fields<'_, '_>, words: RoutingWords) -> Option<Self> {
        let from = fields.required::<IdList>("from");
        let split = fields.required::<SplitShares>("split");
        let when = if words.when {
            fields.optional::<WhenText>("when")
        } else {
            Some(None)
        };
        let shares = split?.0;
        Some(Self {
            from: from?.0,
            to: shares.iter().map(|(target, _)| target.clone()).collect(),
            when: when?.map(|w| w.0),
            choose: None,
            split: Some(shares),
        })
    }
}

/// One flow entry: a chain (a list of ids) or an edge (a mapping with
/// `from` and `to` or `split`), as edges.
struct FlowEntry(Vec<(Edge, EdgePlaces)>);

impl FlowEntry {
    fn read(node: &Node, cx: &mut Decoder, words: RoutingWords) -> Option<Self> {
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
                            when: None,
                            choose: None,
                            split: None,
                        };
                        (edge, places)
                    })
                    .collect();
                Some(Self(edges))
            }
            Value::Map(_) => {
                let EdgeMap {
                    from,
                    to,
                    when,
                    choose,
                    split,
                } = EdgeMap::read(node, cx, words)?;
                let mut places = vec![(place, cx.path().clone())];
                places.extend(to.iter().map(|r| (r.place().clone(), r.path().clone())));
                let edge = Edge {
                    from,
                    to,
                    when,
                    choose,
                    split,
                };
                Some(Self(vec![(edge, places)]))
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
                cx.wrong_type::<Self>(node);
                None
            }
        }
    }
}

impl Decode for FlowEntry {
    const EXPECTED: &'static str = "a chain such as `[source, cut, sink]` or an edge such as `{from: cut, to: [weld-1, weld-2]}`";
    const EXAMPLE: &'static str = "[source, cut, sink]";
    const SCHEMA_NAME: Option<&'static str> = Some("FlowEntry");

    /// Reads with no routing words: the loader uses [`FlowEntry::read`].
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        Self::read(node, cx, RoutingWords::NONE)
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
