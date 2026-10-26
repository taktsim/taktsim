//! The model document (spec §4; roadmap M4): its sections, as read from a
//! file, and their normalized form.

use serde::Serialize;
use taktsim_units::{Ratio, SimDuration};

use crate::component::{Catalog, Component, ComponentRef, read_component, read_id};
use crate::decode::{Decode, Decoder, Fields};
use crate::error::{FieldPath, LoadErrorKind, Location, Place, Segment};
use crate::expr::Expr;
use crate::include::{IncludeSpec, read_includes};
use crate::syntax::{Node, Value};

/// The format version this engine reads (ADR-0008: the header is
/// `format: "1.0"`).
pub const FORMAT_VERSION: &str = "1.0";

/// A loaded model document: every section, typed, with includes resolved
/// and shorthands expanded.
///
/// ```
/// use taktsim_model::expr::{Arity, Dimension, Signature, Type};
/// use taktsim_model::{Catalog, ComponentParams, Distribution, Fields, Format, Loader};
///
/// #[derive(Debug, serde::Serialize)]
/// struct Source {
///     interarrival: Distribution,
/// }
/// impl ComponentParams for Source {
///     const TYPE: &'static str = "Source";
///     const EXAMPLE: &'static str = "{id: source, type: Source, interarrival: 1min}";
///     fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self { interarrival: fields.required("interarrival")? })
///     }
/// }
/// #[derive(Debug, serde::Serialize)]
/// struct Sink {}
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
    pub(crate) fn to_json(&self) -> Result<serde_json::Value, (Option<&Component>, LoadErrorKind)> {
        let plain = |value: Result<serde_json::Value, serde_json::Error>| {
            value.map_err(|e| {
                (
                    None,
                    LoadErrorKind::Normalize {
                        type_name: "(document)".to_owned(),
                        problem: crate::error::NormalizeProblem::Serialize(e.to_string()),
                    },
                )
            })
        };
        let mut doc = serde_json::Map::new();
        doc.insert("format".to_owned(), FORMAT_VERSION.into());
        doc.insert(
            "model".to_owned(),
            plain(serde_json::to_value(&self.model))?,
        );
        doc.insert(
            "experiment".to_owned(),
            plain(serde_json::to_value(&self.experiment))?,
        );
        let components = self
            .components
            .iter()
            .map(|c| c.to_json().map_err(|kind| (Some(c), kind)))
            .collect::<Result<Vec<_>, _>>()?;
        doc.insert(
            "components".to_owned(),
            serde_json::Value::Array(components),
        );
        doc.insert("flow".to_owned(), plain(serde_json::to_value(&self.flow))?);
        if !self.observe.is_empty() {
            doc.insert(
                "observe".to_owned(),
                plain(serde_json::to_value(&self.observe))?,
            );
        }
        Ok(serde_json::Value::Object(doc))
    }
}

/// The `model` section: the model's name and defaults.
///
/// ```
/// fn seed(doc: &taktsim_model::Document) -> Option<u64> {
///     doc.model().seed()
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    time_unit: Option<TimeUnit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
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

    /// The model's time unit, if given (spec §4's example writes
    /// `time_unit: s`). Every quantity carries its own unit, so this does
    /// not change how a model reads; what it is used for (for example the
    /// unit of reported results) is decided with the CLI and the statistics
    /// (M7, M8).
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum TimeUnit {
    /// `d`
    #[serde(rename = "d")]
    Days,
    /// `h`
    #[serde(rename = "h")]
    Hours,
    /// `min`
    #[serde(rename = "min")]
    Minutes,
    /// `s`
    #[serde(rename = "s")]
    Seconds,
    /// `ms`
    #[serde(rename = "ms")]
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

impl Decode for TimeUnit {
    const EXPECTED: &'static str = "a time unit: d, h, min, s or ms";
    const EXAMPLE: &'static str = "s";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let options = Self::ALL.map(|u| (u.as_str(), u));
        one_of(node, cx, "time unit", &options)
    }
}

impl Decode for ModelInfo {
    const EXPECTED: &'static str = "the model section: a mapping with `name`";
    const EXAMPLE: &'static str = "{name: line-a, time_unit: s, seed: 42}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        /// The model name.
        struct Name(String);
        impl Decode for Name {
            const EXPECTED: &'static str = "a name such as `line-a`";
            const EXAMPLE: &'static str = "line-a";
            fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
                read_id(node, cx, "model name").map(Self)
            }
        }

        let mut fields = cx.fields::<Self>(node)?;
        let name = fields.required::<Name>("name");
        let time_unit = fields.optional("time_unit");
        let seed = fields.optional("seed");
        fields.finish();
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum ExperimentKind {
    /// A run with a natural end, such as one shift.
    #[serde(rename = "terminating")]
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
}

/// The `experiment` section: how the model is run and analysed.
///
/// ```
/// fn horizon(doc: &taktsim_model::Document) -> String {
///     doc.experiment().horizon().to_string()
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct Experiment {
    #[serde(rename = "type")]
    kind: ExperimentKind,
    horizon: SimDuration,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "precision_map"
    )]
    precision: Vec<Precision>,
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

/// Serializes precision targets as a mapping from observable to target.
fn precision_map<S: serde::Serializer>(
    targets: &[Precision],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(targets.len()))?;
    for target in targets {
        map.serialize_entry(target.observable.text(), &target.target)?;
    }
    map.end()
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

impl Serialize for Observable {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
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
#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    from: Vec<ComponentRef>,
    to: Vec<ComponentRef>,
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

impl Decode for RawExperiment {
    const EXPECTED: &'static str = "the experiment section: a mapping with `type` and `horizon`";
    const EXAMPLE: &'static str = "{type: terminating, horizon: 8h}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
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
        }

        let mut fields = cx.fields::<Self>(node)?;
        let kind = fields.required::<ExperimentKind>("type");
        let horizon = fields.required::<Horizon>("horizon");
        let precision = match fields.take("precision") {
            None => Some(Vec::new()),
            Some(node) => fields
                .decoder()
                .within(Segment::Key("precision".to_owned()), |cx| {
                    read_precision(node, cx)
                }),
        };
        fields.finish();
        Some(Self {
            kind: kind?,
            horizon: horizon?.0,
            precision: precision?,
        })
    }
}

/// Reads `precision: {observable: ratio, …}`.
fn read_precision(node: &Node, cx: &mut Decoder) -> Option<Vec<RawPrecision>> {
    /// For the error when the section is not a mapping.
    struct Targets;
    impl Decode for Targets {
        const EXPECTED: &'static str = "a mapping from observable to relative precision";
        const EXAMPLE: &'static str = "{throughput(sink): 2%}";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
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
    }

    let Value::Map(entries) = node.value() else {
        cx.wrong_type::<Targets>(node);
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
    ok.then_some(targets)
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
    if !read_format(&mut fields) {
        // Another format version: its sections may mean something else, so
        // reporting errors in them would mislead.
        fields.ignore_rest();
        return None;
    }
    let model = fields.required::<ModelInfo>("model");
    let experiment = fields.required::<RawExperiment>("experiment");
    let includes = match fields.take("include") {
        None => Some(Vec::new()),
        Some(node) => fields
            .decoder()
            .within(Segment::Key("include".to_owned()), |cx| {
                read_includes(node, cx)
            }),
    };
    let parts = read_parts(&mut fields, catalog);
    fields.finish();
    Some(MainFile {
        model,
        experiment,
        includes: includes.unwrap_or_default(),
        parts,
        start: node.start(),
    })
}

impl Decode for MainFile {
    const EXPECTED: &'static str =
        "a model: a mapping with `format`, `model`, `experiment`, `components` and `flow`";
    const EXAMPLE: &'static str = "format: \"1.0\"";

    /// Not used: the model file is read with [`read_main`], which needs
    /// the catalog.
    fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
        None
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

/// Reads the list sections (`components`, `flow`, `observe`), which the
/// model file and included files share.
pub(crate) fn read_parts(fields: &mut Fields<'_, '_>, catalog: &Catalog) -> Parts {
    /// For the error when a section is not a list.
    struct Components;
    impl Decode for Components {
        const EXPECTED: &'static str = "a list of components";
        const EXAMPLE: &'static str = "[{id: cut, type: Machine, cycle_time: 42s}]";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
        }
    }
    /// For the error when a section is not a list.
    struct Flow;
    impl Decode for Flow {
        const EXPECTED: &'static str = "a list of chains and edges";
        const EXAMPLE: &'static str = "[[source, cut, sink]]";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
        }
    }
    /// For the error when a section is not a list.
    struct Observe;
    impl Decode for Observe {
        const EXPECTED: &'static str = "a list of observables";
        const EXAMPLE: &'static str = "[throughput(sink)]";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
        }
    }

    let mut parts = Parts {
        components: None,
        flow: None,
        flow_places: Vec::new(),
        observe: None,
        has_components: fields.entry("components").is_some(),
        has_flow: fields.entry("flow").is_some(),
        complete: true,
    };
    if let Some(node) = fields.take("components") {
        parts.components = fields
            .decoder()
            .within(Segment::Key("components".to_owned()), |cx| {
                if !matches!(node.value(), Value::List(_)) {
                    cx.wrong_type::<Components>(node);
                    return None;
                }
                cx.list(node, |item, cx| read_component(item, cx, catalog))
            });
        parts.complete &= parts.components.is_some();
    }
    if let Some(node) = fields.take("flow") {
        let read = fields
            .decoder()
            .within(Segment::Key("flow".to_owned()), |cx| {
                if !matches!(node.value(), Value::List(_)) {
                    cx.wrong_type::<Flow>(node);
                    return None;
                }
                cx.list(node, read_flow_entry)
            });
        if let Some(entries) = read {
            let (edges, places): (Vec<_>, Vec<_>) = entries.into_iter().flatten().unzip();
            parts.flow = Some(edges);
            parts.flow_places = places;
        } else {
            parts.complete = false;
        }
    }
    if let Some(node) = fields.take("observe") {
        parts.observe = fields
            .decoder()
            .within(Segment::Key("observe".to_owned()), |cx| {
                if !matches!(node.value(), Value::List(_)) {
                    cx.wrong_type::<Observe>(node);
                    return None;
                }
                cx.list(node, |item, cx| {
                    let text = String::decode(item, cx)?;
                    Some(RawExpr {
                        text,
                        place: Place::new(cx.file().map(str::to_owned), item.start()),
                        path: cx.path().clone(),
                    })
                })
            });
        parts.complete &= parts.observe.is_some();
    }
    parts
}

/// The places of an edge's parts: the entry, then each `to` reference.
pub(crate) type EdgePlaces = Vec<(Place, FieldPath)>;

/// Reads one flow entry: a chain (a list of ids) or an edge (a mapping
/// with `from` and `to`), as edges.
fn read_flow_entry(node: &Node, cx: &mut Decoder) -> Option<Vec<(Edge, EdgePlaces)>> {
    /// For the error when the entry is neither.
    struct Entry;
    impl Decode for Entry {
        const EXPECTED: &'static str = "a chain such as `[source, cut, sink]` or an edge such as `{from: cut, to: [weld-1, weld-2]}`";
        const EXAMPLE: &'static str = "[source, cut, sink]";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
        }
    }

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
            Some(edges)
        }
        Value::Map(_) => {
            let mut fields = cx.fields::<Entry>(node)?;
            let from = fields.required::<IdList>("from");
            let to = fields.required::<IdList>("to");
            fields.finish();
            let (from, to) = (from?.0, to?.0);
            let mut places = vec![(place, cx.path().clone())];
            places.extend(to.iter().map(|r| (r.place().clone(), r.path().clone())));
            Some(vec![(Edge { from, to }, places)])
        }
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_) => {
            cx.wrong_type::<Entry>(node);
            None
        }
    }
}

/// One id or a list of distinct ids: the `from` or `to` of an edge.
struct IdList(Vec<ComponentRef>);

impl Decode for IdList {
    const EXPECTED: &'static str = "a component id or a list of them";
    const EXAMPLE: &'static str = "[weld-1, weld-2]";

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
