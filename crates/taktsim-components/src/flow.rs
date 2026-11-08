//! `Flow`: the domain of production lines, and what it checks across a
//! document (ADR-0022 §3, §5).

use std::collections::{BTreeMap, BTreeSet};

use taktsim_model::expr::{Arity, Dimension, Signature, Type};
use taktsim_model::{
    CatalogError, Component as Entry, ComponentRef, Document, FieldPath, Findings, LoadErrorKind,
    Place, RoutingWords,
};
use taktsim_units::SimDuration;

use crate::buffer::Buffer;
use crate::machine::Machine;
use crate::message::{FlowMessage, VariantId};
use crate::run::{BuildError, Domain, Ports, Registry};
use crate::sink::Sink;
use crate::source::Source;

/// `Roles` of this domain.
type Roles = crate::run::Roles<StationFacts>;

/// The variant of a Source that declares none.
const DEFAULT_VARIANT: &str = "default";

/// The most variant names in a model: a `VariantId` is a `u16`.
const MAX_VARIANTS: usize = 65_535;

/// The domain of production lines: Source, Machine, Buffer, Sink and what
/// follows them.
///
/// ```
/// use taktsim_components::{Domain, Flow, FlowMessage};
/// use taktsim_trace::TraceMessage;
///
/// assert_eq!(<<Flow as Domain>::Message as TraceMessage>::FORMAT, FlowMessage::FORMAT);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Flow;

/// What a target says it does for an upstream's rule (ADR-0023 §13).
///
/// ```
/// assert_eq!(taktsim_components::StatusFact::default(), taktsim_components::StatusFact::None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFact {
    /// It says nothing, so an upstream cannot use a status rule on it.
    #[default]
    None,
    /// It always has room and an empty queue, and sends nothing: a Sink.
    AlwaysRoom,
    /// It sends `Status` to its subscribers at the end of any event that changed it.
    Reports,
}

/// The mean time of a unit by variant: `default` for a variant not listed
/// (ADR-0023 §6).
///
/// ```
/// use taktsim_components::MeanTimes;
///
/// let times = MeanTimes { default: "45s".parse()?, by_variant: Default::default() };
/// assert_eq!(times.of("premium").to_string(), "45s");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeanTimes {
    /// For a variant not listed.
    pub default: SimDuration,
    /// By variant name.
    pub by_variant: BTreeMap<String, SimDuration>,
}

impl MeanTimes {
    /// The mean time of `variant`.
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use taktsim_components::MeanTimes;
    ///
    /// let times = MeanTimes {
    ///     default: "45s".parse()?,
    ///     by_variant: BTreeMap::from([("premium".to_owned(), "60s".parse()?)]),
    /// };
    /// assert_eq!(times.of("premium").to_string(), "1min");
    /// assert_eq!(times.of("basic").to_string(), "45s");
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn of(&self, variant: &str) -> SimDuration {
        self.by_variant
            .get(variant)
            .copied()
            .unwrap_or(self.default)
    }
}

/// The mean time of a change of variant (ADR-0023 §6). The default states that no
/// change takes time, which is a fact about a machine with no `setup`.
///
/// ```
/// use taktsim_components::SetupMeans;
///
/// assert_eq!(SetupMeans::default().of("a", "b"), None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SetupMeans {
    /// For a change the matrix does not list.
    pub per_variant: Option<SimDuration>,
    /// By `(from, to)`; `0s` for a listed change that costs nothing.
    pub matrix: BTreeMap<(String, String), SimDuration>,
}

impl SetupMeans {
    /// The mean time of the change from `from` to `to`; `None` if it takes no
    /// changeover.
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use taktsim_components::SetupMeans;
    /// use taktsim_units::SimDuration;
    ///
    /// let setup = SetupMeans {
    ///     per_variant: Some("4min".parse()?),
    ///     matrix: BTreeMap::from([(("a".to_owned(), "b".to_owned()), SimDuration::ZERO)]),
    /// };
    /// assert_eq!(setup.of("a", "b"), Some(SimDuration::ZERO));
    /// assert_eq!(setup.of("b", "a").map(|d| d.to_string()).as_deref(), Some("4min"));
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn of(&self, from: &str, to: &str) -> Option<SimDuration> {
        self.matrix
            .get(&(from.to_owned(), to.to_owned()))
            .copied()
            .or(self.per_variant)
    }
}

/// What a component states for the rules of a flow (ADR-0023 §13). A rule on a type
/// lacking a fact it needs is an error at load, naming the type, the fact and the
/// rule, so a new station states its facts and never silently gets a default.
///
/// ```
/// use taktsim_components::{StationFacts, StatusFact};
///
/// let facts = StationFacts::default();
/// assert!(!facts.routes && !facts.work && !facts.takes_rule && !facts.takes_returns);
/// assert_eq!(facts.status, StatusFact::None);
/// assert_eq!(facts.places, None);
/// ```
// Four yes-or-no facts among others, each stated by itself: the names are the point.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StationFacts {
    /// It uses the route table for parts leaving. Needed for a leaving `when`,
    /// `split`, or any `choose` but `first_free`.
    pub routes: bool,
    /// What it says as a target. A target of `preferred` or `shortest_queue` needs
    /// more than [`StatusFact::None`].
    pub status: StatusFact,
    /// It keeps `worked` and `running`. Needed by a target of `lowest_utilization`.
    pub work: bool,
    /// How many places it has, however derived. Needed by a target of
    /// `lowest_utilization`, and by every component of a loop.
    pub places: Option<u64>,
    /// It picks among described offers. Needed for any `control.dispatch`.
    pub takes_rule: bool,
    /// The mean time of a unit by variant. Needed for `shortest_processing_time`.
    pub mean_time: Option<MeanTimes>,
    /// The mean time of a change of variant. Needed for `minimum_setup`.
    pub setup: Option<SetupMeans>,
    /// It has a return lane. Needed by a loop's return target.
    pub takes_returns: bool,
}

/// What every builder of a flow model reads: the sorted variant names, and which
/// stations are `idle` rather than `starved` when empty.
///
/// ```
/// use taktsim_components::{Domain, Flow, FlowShared};
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components:\n\
///      - {id: source, type: Source, variants: {b: 50%, a: 50%}}\n\
///      - {id: cut, type: Machine, cycle_time: 1min}\n\
///      - {id: weld, type: Machine, cycle_time: 1min}\n\
///      - {id: sink, type: Sink}\n\
///      flow: [[source, cut, weld, sink]]\n",
///     Format::Yaml,
/// )?;
/// let shared: FlowShared = Flow::prepare(model.document(), registry.roles())?;
/// assert_eq!(shared.variants(), ["a", "b"]);
/// assert!(shared.is_idle_when_empty("cut"));
/// assert!(!shared.is_idle_when_empty("weld"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct FlowShared {
    variants: Vec<String>,
    idle: BTreeSet<String>,
}

impl FlowShared {
    /// The model's variant names, sorted: a [`VariantId`] is a position here.
    ///
    /// ```
    /// assert!(taktsim_components::FlowShared::default().variants().is_empty());
    /// ```
    #[must_use]
    pub fn variants(&self) -> &[String] {
        &self.variants
    }

    /// Whether the station `id` is `idle` when it waits for a part: a Source
    /// reaches it through non-stations only. Otherwise it is `starved`.
    ///
    /// ```
    /// assert!(!taktsim_components::FlowShared::default().is_idle_when_empty("cut"));
    /// ```
    #[must_use]
    pub fn is_idle_when_empty(&self, id: &str) -> bool {
        self.idle.contains(id)
    }
}

/// The id of variant `name`.
pub(crate) fn variant_id(
    shared: &FlowShared,
    name: &str,
    path: &FieldPath,
) -> Result<VariantId, BuildError> {
    shared
        .variants
        .binary_search_by(|v| v.as_str().cmp(name))
        .ok()
        .and_then(|index| u16::try_from(index).ok())
        .map(VariantId::new)
        .ok_or_else(|| BuildError::UnknownVariant {
            path: path.clone(),
            name: name.to_owned(),
        })
}

/// The variants the Sources of `document` declare, with `default` if some Source
/// declares none.
fn declared_variants(document: &Document) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    for source in document
        .components()
        .iter()
        .filter_map(|c| c.params::<Source>())
    {
        match source.variant_names() {
            Some(declared) => names.extend(declared),
            None => {
                names.insert(DEFAULT_VARIANT);
            }
        }
    }
    names
}

/// The flow as a graph.
struct Graph<'a> {
    /// Targets of each component, in declared order, with the reference that
    /// names each.
    down: BTreeMap<&'a str, Vec<&'a ComponentRef>>,
    entered: BTreeSet<&'a str>,
}

impl<'a> Graph<'a> {
    fn new(document: &'a Document) -> Self {
        let mut down: BTreeMap<&str, Vec<&ComponentRef>> = BTreeMap::new();
        let mut entered = BTreeSet::new();
        for edge in document.flow() {
            for from in edge.from() {
                for to in edge.to() {
                    down.entry(from.id()).or_default().push(to);
                    entered.insert(to.id());
                }
            }
        }
        Self { down, entered }
    }

    fn targets(&self, id: &str) -> impl Iterator<Item = &'a ComponentRef> + '_ {
        self.down.get(id).into_iter().flatten().copied()
    }

    /// Every component reachable from `start`, passing only what `through`
    /// lets through; `start` counts only if a path comes back to it.
    fn reached(&self, start: &'a str, through: impl Fn(&str) -> bool) -> BTreeSet<&'a str> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            for target in self.targets(id) {
                if seen.insert(target.id()) && through(target.id()) {
                    stack.push(target.id());
                }
            }
        }
        seen
    }
}

/// One rule broken: where, what was found, what is allowed and an example.
fn not_allowed(
    findings: &mut Findings,
    (place, path): (&Place, &FieldPath),
    found: String,
    allowed: &str,
    example: &str,
) {
    let kind = LoadErrorKind::NotAllowed {
        found,
        allowed: allowed.to_owned(),
        example: example.to_owned(),
    };
    findings.error(place, path, kind);
}

/// Each type's ports say which edges it may have, and a type with ports must be
/// connected.
fn check_ports(document: &Document, roles: &Roles, graph: &Graph<'_>, findings: &mut Findings) {
    let types: BTreeMap<&str, &str> = document
        .components()
        .iter()
        .map(|c| (c.id(), c.type_name()))
        .collect();
    let ports_of = |id: &str| types.get(id).and_then(|t| roles.ports(t));
    for edge in document.flow() {
        for from in edge.from() {
            if ports_of(from.id()).is_some_and(|p| !p.has_outlet()) {
                not_allowed(
                    findings,
                    (from.place(), from.path()),
                    format!("an edge leaving `{}`", from.id()),
                    "edges leave components with an outlet: a Source, a Machine or a Buffer",
                    "[source, cut, sink]",
                );
            }
        }
        for to in edge.to() {
            if ports_of(to.id()).is_some_and(|p| !p.has_inlet()) {
                not_allowed(
                    findings,
                    (to.place(), to.path()),
                    format!("an edge entering `{}`", to.id()),
                    "edges enter components with an inlet: a Machine, a Buffer or a Sink",
                    "[source, cut, sink]",
                );
            }
        }
    }
    for c in document.components() {
        let Some(ports) = roles.ports(c.type_name()) else {
            not_allowed(
                findings,
                (c.place(), c.path()),
                format!("a component of type `{}`", c.type_name()),
                "a component type this catalog registers",
                "{id: cut, type: Machine, cycle_time: 42s}",
            );
            continue;
        };
        let entered = graph.entered.contains(c.id());
        let leaves = graph.down.contains_key(c.id());
        if ports.has_inlet() && !entered {
            not_allowed(
                findings,
                (c.place(), c.path()),
                format!("no edge entering `{}`", c.id()),
                "a component with an inlet is connected",
                "[source, cut, sink]",
            );
        }
        if ports.has_outlet() && !leaves {
            not_allowed(
                findings,
                (c.place(), c.path()),
                format!("no edge leaving `{}`", c.id()),
                "a component with an outlet is connected",
                "[source, cut, sink]",
            );
        }
    }
}

/// At most one edge leaves a component in M5, in any spelling: M6 makes separate
/// edges first-match in order (ADR-0022 §3).
fn check_one_edge_per_source(document: &Document, findings: &mut Findings) {
    let mut first: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, edge) in document.flow().iter().enumerate() {
        for from in edge.from() {
            let seen = *first.entry(from.id()).or_insert(index);
            if seen != index {
                not_allowed(
                    findings,
                    (from.place(), from.path()),
                    format!("a second edge leaving `{}`", from.id()),
                    "one edge leaving each component; a branch is one edge with several targets",
                    "{from: cut, to: [weld-1, weld-2]}",
                );
            }
        }
    }
}

/// No loop: a loop with no condition or ratio would send parts round for ever,
/// and finite buffers in a loop can deadlock. M6 lifts the rule with conditions.
fn check_no_loops(document: &Document, graph: &Graph<'_>, findings: &mut Findings) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        OnPath,
        Done,
    }
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    for start in document.components().iter().map(Entry::id) {
        if marks.contains_key(start) {
            continue;
        }
        // Each entry: a component on the path, and its next target to look at.
        let mut path: Vec<(&str, usize)> = vec![(start, 0)];
        marks.insert(start, Mark::OnPath);
        while let Some(&(id, next)) = path.last() {
            let target = graph.down.get(id).and_then(|t| t.get(next)).copied();
            let Some(target) = target else {
                marks.insert(id, Mark::Done);
                path.pop();
                continue;
            };
            if let Some(top) = path.last_mut() {
                top.1 += 1;
            }
            match marks.get(target.id()) {
                Some(Mark::OnPath) => {
                    let names: Vec<&str> = path
                        .iter()
                        .map(|(name, _)| *name)
                        .skip_while(|name| *name != target.id())
                        .chain([target.id()])
                        .collect();
                    not_allowed(
                        findings,
                        (target.place(), target.path()),
                        format!("a loop: {}", names.join(" -> ")),
                        "a flow without loops: a loop with no condition or ratio would send \
                         parts round for ever, and routing with conditions arrives in M6",
                        "[source, cut, sink]",
                    );
                }
                Some(Mark::Done) => {}
                None => {
                    marks.insert(target.id(), Mark::OnPath);
                    path.push((target.id(), 0));
                }
            }
        }
    }
}

/// Every path from an on-demand Source to a Sink passes a station. Otherwise
/// parts would reach the Sink for ever at one instant.
fn check_on_demand(document: &Document, roles: &Roles, graph: &Graph<'_>, findings: &mut Findings) {
    let types: BTreeMap<&str, &str> = document
        .components()
        .iter()
        .map(|c| (c.id(), c.type_name()))
        .collect();
    let is_station = |id: &str| types.get(id).is_some_and(|t| roles.is_station(t));
    for c in document.components() {
        if !c.params::<Source>().is_some_and(Source::on_demand) {
            continue;
        }
        let reached = graph.reached(c.id(), |id| !is_station(id));
        let sink = reached.iter().copied().find(|id| {
            !is_station(id)
                && types
                    .get(id)
                    .and_then(|t| roles.ports(t))
                    .is_some_and(|ports| ports == Ports::Inlet)
        });
        if let Some(sink) = sink {
            not_allowed(
                findings,
                (c.place(), c.path()),
                format!(
                    "a path from on-demand `{}` to `{sink}` that passes no station",
                    c.id()
                ),
                "a Machine on every path, or `interarrival` on the Source: otherwise parts reach \
                 the sink for ever at one instant",
                "{id: source, type: Source, interarrival: 40s}",
            );
        }
    }
}

/// Matrix names and a machine's times per variant are declared by some Source, and
/// the model's names fit a `VariantId`.
fn check_variants(document: &Document, findings: &mut Findings) {
    let declared = declared_variants(document);
    if declared.len() > MAX_VARIANTS
        && let Some(first) = document.components().first()
    {
        not_allowed(
            findings,
            (first.place(), first.path()),
            format!("{} variant names in the model", declared.len()),
            "at most 65535 variant names in a model",
            "{basic: 80%, premium: 20%}",
        );
    }
    for machine in document
        .components()
        .iter()
        .filter_map(|c| c.params::<Machine>())
    {
        let changes = machine.setup().into_iter().flat_map(|setup| {
            setup
                .changes()
                .iter()
                .flat_map(|change| [&change.from, &change.to])
        });
        for name in changes.chain(machine.variants_timed()) {
            if !declared.contains(name.id()) {
                let known: Vec<&str> = declared.iter().copied().collect();
                not_allowed(
                    findings,
                    (name.place(), name.path()),
                    format!("the variant `{}`, which no Source declares", name.id()),
                    &format!("a variant some Source declares: {}", known.join(", ")),
                    "{id: source, type: Source, variants: {basic: 80%, premium: 20%}}",
                );
            }
        }
    }
}

impl Domain for Flow {
    type Message = FlowMessage;
    type Shared = FlowShared;
    type Facts = StationFacts;

    const ROUTING: RoutingWords = RoutingWords::ALL;

    fn check(document: &Document, roles: &Roles, findings: &mut Findings) {
        let graph = Graph::new(document);
        check_ports(document, roles, &graph, findings);
        check_one_edge_per_source(document, findings);
        check_no_loops(document, &graph, findings);
        check_on_demand(document, roles, &graph, findings);
        check_variants(document, findings);
    }

    fn prepare(document: &Document, roles: &Roles) -> Result<FlowShared, BuildError> {
        let graph = Graph::new(document);
        let types: BTreeMap<&str, &str> = document
            .components()
            .iter()
            .map(|c| (c.id(), c.type_name()))
            .collect();
        let is_station = |id: &str| types.get(id).is_some_and(|t| roles.is_station(t));
        let mut idle = BTreeSet::new();
        for c in document.components() {
            if roles.ports(c.type_name()) == Some(Ports::Outlet) {
                idle.extend(
                    graph
                        .reached(c.id(), |id| !is_station(id))
                        .into_iter()
                        .filter(|id| is_station(id))
                        .map(str::to_owned),
                );
            }
        }
        Ok(FlowShared {
            variants: declared_variants(document)
                .into_iter()
                .map(str::to_owned)
                .collect(),
            idle,
        })
    }
}

/// The standard catalog of production components: Source, Machine, Buffer and
/// Sink, and the observables of spec §4, whose result types M8 may refine
/// (ADR-0022, "Observables"). The schema generator calls it, so it picks up every
/// registered type with no list of its own.
///
/// # Errors
///
/// A [`CatalogError`] only if a type here broke a catalog rule, which the tests
/// rule out.
///
/// ```
/// let registry = taktsim_components::standard()?;
/// assert_eq!(registry.catalog().types().collect::<Vec<_>>(), ["Buffer", "Machine", "Sink", "Source"]);
/// assert_eq!(
///     registry.catalog().observables().collect::<Vec<_>>(),
///     ["lead_time", "state_shares", "throughput", "wip"]
/// );
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
pub fn standard() -> Result<Registry<Flow>, CatalogError> {
    let throughput =
        Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate)).accepting(["Sink"]);
    let lead_time =
        Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Time)).accepting(["Sink"]);
    let wip = Signature::new(Arity::Exactly(1), Type::Number).accepting(["Buffer"]);
    let state_shares = Signature::new(Arity::AtLeast(1), Type::Number).accepting(["Machine"]);
    Registry::new()
        .with::<Source>()?
        .with::<Machine>()?
        .with::<Buffer>()?
        .with::<Sink>()?
        .with_observable("throughput", throughput)?
        .with_observable("lead_time", lead_time)?
        .with_observable("wip", wip)?
        .with_observable("state_shares", state_shares)
}
