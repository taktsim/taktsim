//! `Flow`: the domain of production lines, and what it checks across a
//! document (ADR-0022 §3, §5; ADR-0023 §1, §8, §13).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use taktsim_model::expr::{Arity, Condition, Dimension, EnvError, Signature, Type, TypeEnv};
use taktsim_model::{
    CatalogError, ChoiceRule, Component as Entry, ComponentRef, DispatchRule, Document, Edge,
    FieldPath, Findings, LoadErrorKind, Place, RoutingWords,
};
use taktsim_units::SimDuration;

use crate::buffer::Buffer;
use crate::dispatch::TakerPlan;
use crate::machine::Machine;
use crate::message::{FlowMessage, VariantId};
use crate::route::{Pick, RouteEdge, RouteTable, StatusRule, TargetPlan};
use crate::run::{BuildError, Domain, Ports, Registry};
use crate::sink::Sink;
use crate::source::Source;

/// `Roles` of this domain.
type Roles = crate::run::Roles<StationFacts>;

/// The variant of a Source that declares none.
const DEFAULT_VARIANT: &str = "default";

/// The most variant names in a model: a `VariantId` is a `u16`.
const MAX_VARIANTS: usize = 65_535;

/// The most edges leaving one component, and the most targets of one edge, so a
/// route record stays far below the largest record (ADR-0023 §1, §9).
const MAX_ROUTES: usize = 1024;

/// How far shares may be from 100 %, in percentage points, as for `variants`.
const SHARE_TOLERANCE: f64 = 1e-9;

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

/// What every builder of a flow model reads: the sorted variant names, which
/// stations are `idle` rather than `starved` when empty, and, all built from what the
/// components state, how parts leave each component that is not on a plain edge, the
/// rule of each station that has one, who hears from each target and the bound on each
/// return lane.
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
    variants: Arc<Vec<String>>,
    idle: BTreeSet<String>,
    /// How parts leave, for each component whose edges are not plain.
    routes: BTreeMap<String, Arc<RouteTable>>,
    /// The declared rule of each station that has one.
    takers: BTreeMap<String, Arc<TakerPlan>>,
    /// Who hears from each target, in the order `flow` names them.
    subscribers: BTreeMap<String, Vec<String>>,
    /// Each loop's return target, with its bound on the lane: C (ADR-0023 §8).
    returns: BTreeMap<String, u64>,
}

impl FlowShared {
    /// The model's variant names, sorted: a [`VariantId`] is a position here.
    ///
    /// ```
    /// assert!(taktsim_components::FlowShared::default().variants().is_empty());
    /// ```
    #[must_use]
    pub fn variants(&self) -> &[String] {
        self.variants.as_slice()
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

    /// How parts leave `id`; `None` for a plain edge.
    pub(crate) fn route_table(&self, id: &str) -> Option<Arc<RouteTable>> {
        self.routes.get(id).cloned()
    }

    /// The declared rule of the station `id`.
    pub(crate) fn taker(&self, id: &str) -> Option<Arc<TakerPlan>> {
        self.takers.get(id).cloned()
    }

    /// Who hears from the target `id`.
    pub(crate) fn subscribers(&self, id: &str) -> &[String] {
        self.subscribers.get(id).map_or(&[], Vec::as_slice)
    }

    /// The bound on the return lane of `id`, if it is a loop's return target.
    pub(crate) fn return_bound(&self, id: &str) -> Option<u64> {
        self.returns.get(id).copied()
    }
}

/// The id of variant `name`.
pub(crate) fn variant_id(
    shared: &FlowShared,
    name: &str,
    path: &FieldPath,
) -> Result<VariantId, BuildError> {
    position_of(&shared.variants, name).ok_or_else(|| BuildError::UnknownVariant {
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

/// The edges leaving each component, in written order, with the reference naming
/// the component on each.
fn leaving_edges(document: &Document) -> BTreeMap<&str, Vec<(&Edge, &ComponentRef)>> {
    let mut leaving: BTreeMap<&str, Vec<(&Edge, &ComponentRef)>> = BTreeMap::new();
    for edge in document.flow() {
        for from in edge.from() {
            leaving.entry(from.id()).or_default().push((edge, from));
        }
    }
    leaving
}

/// Edges leaving one component are tried in the order written (ADR-0023 §1). The
/// last has no `when`, so every part has a way out; an edge after one without `when`
/// can never be used; at most 1,024 leave one component.
fn check_leaving_edges(document: &Document, findings: &mut Findings) {
    for (id, edges) in leaving_edges(document) {
        if let Some((_, from)) = edges.get(MAX_ROUTES) {
            not_allowed(
                findings,
                (from.place(), from.path()),
                format!("{} edges leaving `{id}`", edges.len()),
                "at most 1024 edges leaving one component",
                "{from: cut, to: [weld-1, weld-2]}",
            );
        }
        let mut closed = false;
        for (edge, from) in &edges {
            if closed {
                not_allowed(
                    findings,
                    (from.place(), from.path()),
                    format!("a second edge leaving `{id}`, after one that takes every part"),
                    "edges tried in the order written, each but the last with a `when`: an edge \
                     after one without `when` can never be used. A branch is one edge with several \
                     targets",
                    "{from: cut, to: [weld-1, weld-2]}",
                );
            }
            closed |= edge.when().is_none();
        }
        if !closed
            && let Some((edge, _)) = edges.last()
            && let Some((_, place, path)) = edge.when_written()
        {
            not_allowed(
                findings,
                (place, path),
                format!("a `when` on the last edge leaving `{id}`"),
                "a last edge without `when`, so every part has a way out",
                "{from: qc, to: pack}",
            );
        }
    }
}

/// The path of what holds `path`: the `split` of a share's `split.pack`.
fn parent(path: &FieldPath) -> FieldPath {
    let segments = path.segments();
    FieldPath::from(segments[..segments.len().saturating_sub(1)].to_vec())
}

/// `choose` needs at least two targets; an edge has at most 1,024 (ADR-0023 §1).
fn check_choose(document: &Document, findings: &mut Findings) {
    for edge in document.flow() {
        if let Some(extra) = edge.to().get(MAX_ROUTES) {
            not_allowed(
                findings,
                (extra.place(), extra.path()),
                format!("{} targets on one edge", edge.to().len()),
                "at most 1024 targets on one edge",
                "{from: cut, to: [weld-1, weld-2]}",
            );
        }
        if let (Some(rule), [only]) = (edge.choose(), edge.to()) {
            not_allowed(
                findings,
                (only.place(), only.path()),
                format!("`choose: {}` with one target", rule.as_str()),
                "`choose` with at least two targets",
                "{from: cut, to: [weld-1, weld-2], choose: shortest_queue}",
            );
        }
    }
}

/// `split`: at least two targets, each share above 0 %, summing to 100 % within
/// 10⁻⁹ percentage points, as `variants` does (ADR-0023 §1).
fn check_split(document: &Document, findings: &mut Findings) {
    let example = "{from: qc, split: {cut: 5%, pack: 95%}}";
    for edge in document.flow() {
        let Some(shares) = edge.split() else {
            continue;
        };
        let Some((first, _)) = shares.first() else {
            continue;
        };
        let whole = parent(first.path());
        if shares.len() < 2 {
            not_allowed(
                findings,
                (first.place(), &whole),
                format!("a split with {} target", shares.len()),
                "a split with at least two targets; one target is an edge with `to`",
                example,
            );
            continue;
        }
        let mut positive = true;
        for (target, share) in shares {
            if share.as_percent() <= 0.0 {
                positive = false;
                not_allowed(
                    findings,
                    (target.place(), target.path()),
                    share.to_string(),
                    "a share above 0%; leave out a target that is never taken",
                    example,
                );
            }
        }
        let total: f64 = shares.iter().map(|(_, share)| share.as_percent()).sum();
        if positive && (total - 100.0).abs() > SHARE_TOLERANCE {
            not_allowed(
                findings,
                (first.place(), &whole),
                format!("shares summing to {total}%"),
                "shares summing to 100%",
                example,
            );
        }
    }
}

/// What a condition can read: `variant` as an enum of the model's names and
/// `priority` as an int, no functions and no components. So conditions are about the
/// unit, never about observables, and none can compare a float (ADR-0023 §2).
fn condition_env(document: &Document) -> Result<TypeEnv, EnvError> {
    TypeEnv::new()
        .with_enum("variant", declared_variants(document))?
        .with_path("variant", Type::Enum("variant".to_owned()))?
        .with_path("priority", Type::Int)
}

/// Each `when` has type bool in that environment, and names a declared variant.
fn check_conditions(document: &Document, findings: &mut Findings) {
    let env = match condition_env(document) {
        Ok(env) => env,
        Err(error) => {
            if let Some(first) = document.components().first() {
                not_allowed(
                    findings,
                    (first.place(), first.path()),
                    format!("the model's variant names: {error}"),
                    "variant names a condition can compare",
                    "{basic: 80%, premium: 20%}",
                );
            }
            return;
        }
    };
    for edge in document.flow() {
        let (Some(expr), Some((text, place, path))) = (edge.when(), edge.when_written()) else {
            continue;
        };
        if let Err(error) = Condition::new(expr, &env) {
            let kind = LoadErrorKind::Expression {
                text: text.to_owned(),
                error,
            };
            findings.error(place, path, kind);
        }
    }
}

/// What each component of a document states.
fn facts_of<'a>(
    document: &'a Document,
    roles: &Roles,
) -> BTreeMap<&'a str, (&'a str, StationFacts)> {
    document
        .components()
        .iter()
        .map(|c| (c.id(), (c.type_name(), roles.facts(c))))
        .collect()
}

/// A rule on a type lacking a fact: found, with the fix.
fn lacks_fact(
    findings: &mut Findings,
    at: (&Place, &FieldPath),
    (id, type_name): (&str, &str),
    fact: &str,
    rule: &str,
) {
    not_allowed(
        findings,
        at,
        format!("`{rule}` on `{id}`, whose type `{type_name}` does not state `{fact}`"),
        &format!(
            "`{rule}` on a component whose type states `{fact}`; a type that needs it states it \
             in `ComponentType::facts`"
        ),
        "{id: weld, type: Machine, cycle_time: 45s}",
    );
}

/// A rule on a type that does not state what the rule needs is an error, naming the
/// type, the fact and the rule (ADR-0023 §13). A plain edge and `first_free` need
/// nothing.
fn check_edge_facts(
    document: &Document,
    facts: &BTreeMap<&str, (&str, StationFacts)>,
    findings: &mut Findings,
) {
    for edge in document.flow() {
        let leaving_rule = if edge.split().is_some() {
            Some("split")
        } else if edge.when().is_some() {
            Some("when")
        } else {
            match edge.choose() {
                Some(ChoiceRule::FirstFree) | None => None,
                Some(rule) => Some(rule.as_str()),
            }
        };
        if let Some(rule) = leaving_rule {
            for from in edge.from() {
                if let Some((type_name, stated)) = facts.get(from.id())
                    && !stated.routes
                {
                    lacks_fact(
                        findings,
                        (from.place(), from.path()),
                        (from.id(), type_name),
                        "routes",
                        rule,
                    );
                }
            }
        }
        let Some(rule) = edge.choose() else {
            continue;
        };
        for to in edge.to() {
            let Some((type_name, stated)) = facts.get(to.id()) else {
                continue;
            };
            let at = (to.place(), to.path());
            let name = (to.id(), *type_name);
            match rule {
                ChoiceRule::FirstFree => {}
                ChoiceRule::Preferred | ChoiceRule::ShortestQueue => {
                    if stated.status == StatusFact::None {
                        lacks_fact(findings, at, name, "status", rule.as_str());
                    }
                }
                ChoiceRule::LowestUtilization => {
                    if stated.status != StatusFact::Reports {
                        lacks_fact(findings, at, name, "status", rule.as_str());
                    } else if !stated.work {
                        lacks_fact(findings, at, name, "work", rule.as_str());
                    } else if stated.places.is_none() {
                        lacks_fact(findings, at, name, "places", rule.as_str());
                    }
                }
            }
        }
    }
}

/// `control.dispatch`: each rule on a station that states what it needs, and fed by
/// components that can describe what they offer (ADR-0023 §6, §13).
fn check_dispatch(
    document: &Document,
    facts: &BTreeMap<&str, (&str, StationFacts)>,
    findings: &mut Findings,
) {
    for (station, rule) in document.control().dispatch() {
        let Some((type_name, stated)) = facts.get(station.id()) else {
            continue;
        };
        let at = (station.place(), station.path());
        let name = (station.id(), *type_name);
        let word = rule.as_str();
        if !stated.takes_rule {
            lacks_fact(findings, at, name, "takes_rule", word);
        }
        match rule {
            DispatchRule::ShortestProcessingTime => {
                if stated.mean_time.is_none() {
                    lacks_fact(findings, at, name, "mean_time", word);
                }
            }
            DispatchRule::MinimumSetup => {
                if stated.setup.is_none() {
                    lacks_fact(findings, at, name, "setup", word);
                }
            }
            DispatchRule::Fifo | DispatchRule::EarliestDueDate | DispatchRule::PriorityClass => {}
        }
        // A rule ranks what its upstreams describe; a type that cannot would leave
        // the station with offers it cannot rank.
        let feeders = document
            .flow()
            .iter()
            .filter(|edge| edge.to().iter().any(|to| to.id() == station.id()))
            .flat_map(Edge::from);
        for from in feeders {
            if let Some((type_name, stated)) = facts.get(from.id())
                && !stated.routes
            {
                lacks_fact(
                    findings,
                    (from.place(), from.path()),
                    (from.id(), type_name),
                    "routes",
                    word,
                );
            }
        }
    }
}

/// The parts of the graph where every component reaches every other, of two or more
/// components, by Tarjan's algorithm without recursion.
fn strongly_connected<'a>(document: &'a Document, graph: &Graph<'a>) -> Vec<BTreeSet<&'a str>> {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut low: BTreeMap<&str, usize> = BTreeMap::new();
    let mut on_stack: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut parts = Vec::new();
    for start in document.components().iter().map(Entry::id) {
        if index.contains_key(start) {
            continue;
        }
        // Each entry: a component on the walk, and its next target to look at.
        let mut walk: Vec<(&str, usize)> = vec![(start, 0)];
        while let Some(&(id, next)) = walk.last() {
            if next == 0 && !index.contains_key(id) {
                let number = index.len();
                index.insert(id, number);
                low.insert(id, number);
                stack.push(id);
                on_stack.insert(id);
            }
            if let Some(target) = graph.down.get(id).and_then(|t| t.get(next)) {
                if let Some(top) = walk.last_mut() {
                    top.1 += 1;
                }
                if let Some(&seen) = index.get(target.id()) {
                    if on_stack.contains(target.id())
                        && let Some(mine) = low.get_mut(id)
                    {
                        *mine = (*mine).min(seen);
                    }
                } else {
                    walk.push((target.id(), 0));
                }
                continue;
            }
            walk.pop();
            let mine = low.get(id).copied().unwrap_or(0);
            if let Some(&(caller, _)) = walk.last()
                && let Some(theirs) = low.get_mut(caller)
            {
                *theirs = (*theirs).min(mine);
            }
            if index.get(id) == Some(&mine) {
                let mut part = BTreeSet::new();
                while let Some(member) = stack.pop() {
                    on_stack.remove(member);
                    part.insert(member);
                    if member == id {
                        break;
                    }
                }
                if part.len() > 1 {
                    parts.push(part);
                }
            }
        }
    }
    parts
}

/// One loop of a flow: its components, its gates and its return target (ADR-0023
/// §8).
struct LoopPart<'a> {
    members: BTreeSet<&'a str>,
    /// The components whose leaving edges are all exit edges.
    gates: BTreeSet<&'a str>,
}

impl<'a> LoopPart<'a> {
    fn new(document: &'a Document, members: BTreeSet<&'a str>) -> Self {
        let is_exit = |edge: &Edge| {
            let outside = |to: &ComponentRef| !members.contains(to.id());
            edge.to().iter().all(outside)
                || (edge.split().is_some() && edge.to().iter().any(outside))
        };
        let leaving = leaving_edges(document);
        let gates = members
            .iter()
            .copied()
            .filter(|id| {
                leaving
                    .get(id)
                    .is_some_and(|edges| edges.iter().all(|(edge, _)| is_exit(edge)))
            })
            .collect();
        Self { members, gates }
    }

    /// The edges from gates back into the part: the references naming their targets.
    fn back_edges(&self, document: &'a Document) -> Vec<&'a ComponentRef> {
        document
            .flow()
            .iter()
            .filter(|edge| edge.from().iter().any(|f| self.gates.contains(f.id())))
            .flat_map(Edge::to)
            .filter(|to| self.members.contains(to.id()))
            .collect()
    }

    /// The return target: where the edges from gates come back.
    fn return_target(&self, document: &'a Document) -> Option<&'a str> {
        self.back_edges(document).first().map(|to| to.id())
    }
}

/// Every cycle of `part` passes a gate: with the gates' edges left out, nothing goes
/// round. Reports the reference that closes each cycle found.
fn check_cycles_pass_a_gate(
    document: &Document,
    graph: &Graph<'_>,
    part: &LoopPart<'_>,
    findings: &mut Findings,
) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        OnPath,
        Done,
    }
    let mut ok = true;
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    let members = document
        .components()
        .iter()
        .map(Entry::id)
        .filter(|id| part.members.contains(id));
    for start in members {
        if marks.contains_key(start) {
            continue;
        }
        // Each entry: a component on the path, and its next target to look at.
        let mut path: Vec<(&str, usize)> = vec![(start, 0)];
        marks.insert(start, Mark::OnPath);
        while let Some(&(id, next)) = path.last() {
            let targets = if part.gates.contains(id) {
                None
            } else {
                graph.down.get(id)
            };
            let Some(target) = targets.and_then(|t| t.get(next)).copied() else {
                marks.insert(id, Mark::Done);
                path.pop();
                continue;
            };
            if let Some(top) = path.last_mut() {
                top.1 += 1;
            }
            if !part.members.contains(target.id()) {
                continue;
            }
            match marks.get(target.id()) {
                Some(Mark::OnPath) => {
                    let names: Vec<&str> = path
                        .iter()
                        .map(|(name, _)| *name)
                        .skip_while(|name| *name != target.id())
                        .chain([target.id()])
                        .collect();
                    ok = false;
                    not_allowed(
                        findings,
                        (target.place(), target.path()),
                        format!("a loop without an exit: {}", names.join(" -> ")),
                        "every loop through a gate: a component whose leaving edges all leave the \
                         loop, or are a `split` with a target outside it. A `when` alone is no \
                         exit, since a part's attributes never change, and a first-free edge back \
                         could leave a part circling for ever",
                        "{from: qc, split: {weld: 5%, pack: 95%}}",
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
    ok
}

/// The rules of ADR-0023 §8, over the parts of the graph where every component
/// reaches every other: every cycle passes a gate, all edges from gates back go to
/// one station that takes returns, every edge from outside enters there, and C, the
/// sum of the places of the part, fits a count.
fn check_loops(
    document: &Document,
    graph: &Graph<'_>,
    facts: &BTreeMap<&str, (&str, StationFacts)>,
    findings: &mut Findings,
) {
    for members in strongly_connected(document, graph) {
        let part = LoopPart::new(document, members);
        if !check_cycles_pass_a_gate(document, graph, &part, findings) {
            continue;
        }
        let Some(target) = part.return_target(document) else {
            continue;
        };
        let example = "{from: qc, split: {weld: 5%, pack: 95%}}";
        for back in part.back_edges(document) {
            if back.id() != target {
                not_allowed(
                    findings,
                    (back.place(), back.path()),
                    format!("a way back to `{}`, after one to `{target}`", back.id()),
                    "all ways back into a loop going to one station, the return target; nested \
                     loops with two return targets are not allowed yet",
                    example,
                );
            }
        }
        for edge in document.flow() {
            let from_outside = edge.from().iter().any(|f| !part.members.contains(f.id()));
            if !from_outside {
                continue;
            }
            for to in edge.to() {
                if part.members.contains(to.id()) && to.id() != target {
                    not_allowed(
                        findings,
                        (to.place(), to.path()),
                        format!("an edge entering a loop at `{}`", to.id()),
                        &format!(
                            "new parts entering a loop at its return target, here `{target}`, so a \
                             part coming back always finds its lane ahead of them"
                        ),
                        example,
                    );
                }
            }
        }
        if let Some((type_name, stated)) = facts.get(target)
            && !stated.takes_returns
            && let Some(back) = part.back_edges(document).first()
        {
            not_allowed(
                findings,
                (back.place(), back.path()),
                format!(
                    "a loop returning to `{target}`, whose type `{type_name}` does not state                      `takes_returns`"
                ),
                "a return target with a return lane; for a Buffer, send the way back to the                  machine after the buffer, since a Buffer's fill must stay what its events say",
                example,
            );
        }
        let mut total: Option<u64> = Some(0);
        for id in &part.members {
            if let Some((type_name, stated)) = facts.get(id) {
                match stated.places {
                    Some(places) => total = total.and_then(|t| t.checked_add(places)),
                    None => {
                        if let Some(c) = document.components().iter().find(|c| c.id() == *id) {
                            lacks_fact(
                                findings,
                                (c.place(), c.path()),
                                (id, type_name),
                                "places",
                                "a loop",
                            );
                        }
                    }
                }
            }
        }
        if total.is_none()
            && let Some(back) = part.back_edges(document).first()
        {
            not_allowed(
                findings,
                (back.place(), back.path()),
                format!("a loop returning to `{target}` whose places do not fit a count"),
                "a loop whose capacities sum to at most 2^64 - 1, the bound on its return lane",
                example,
            );
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
        let facts = facts_of(document, roles);
        check_ports(document, roles, &graph, findings);
        check_leaving_edges(document, findings);
        check_choose(document, findings);
        check_split(document, findings);
        check_conditions(document, findings);
        check_edge_facts(document, &facts, findings);
        check_dispatch(document, &facts, findings);
        check_loops(document, &graph, &facts, findings);
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
        let variants: Arc<Vec<String>> = Arc::new(
            declared_variants(document)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );
        let facts = facts_of(document, roles);
        let takers = taker_plans(document, &facts, &variants);
        let mut returns = BTreeMap::new();
        let mut gate_returns: BTreeSet<(&str, &str)> = BTreeSet::new();
        for members in strongly_connected(document, &graph) {
            let part = LoopPart::new(document, members);
            let Some(target) = part.return_target(document) else {
                continue;
            };
            let places = part
                .members
                .iter()
                .filter_map(|id| facts.get(id).and_then(|(_, stated)| stated.places))
                .fold(0_u64, u64::saturating_add);
            returns.insert(target.to_owned(), places);
            for gate in &part.gates {
                gate_returns.insert((gate, target));
            }
        }
        let routes = route_tables(document, &facts, &takers, &gate_returns, &variants)?;
        let subscribers = status_subscribers(document, &facts, &takers);
        Ok(FlowShared {
            variants,
            idle,
            routes,
            takers,
            subscribers,
            returns,
        })
    }
}

/// The position of `name` in the sorted variant names.
fn position_of(variants: &[String], name: &str) -> Option<VariantId> {
    variants
        .binary_search_by(|v| v.as_str().cmp(name))
        .ok()
        .and_then(|index| u16::try_from(index).ok())
        .map(VariantId::new)
}

/// The plan of each station with a declared rule, built from its facts: never from a
/// Machine's parameters (ADR-0023 §13).
fn taker_plans(
    document: &Document,
    facts: &BTreeMap<&str, (&str, StationFacts)>,
    variants: &[String],
) -> BTreeMap<String, Arc<TakerPlan>> {
    let mut plans = BTreeMap::new();
    for (station, rule) in document.control().dispatch() {
        let Some((_, stated)) = facts.get(station.id()) else {
            continue;
        };
        let mut plan = TakerPlan {
            rule: *rule,
            default_time: 0,
            times: BTreeMap::new(),
            per_variant: None,
            changes: BTreeMap::new(),
        };
        if let Some(times) = &stated.mean_time {
            plan.default_time = times.default.as_nanos();
            for name in variants {
                if let Some(id) = position_of(variants, name) {
                    plan.times.insert(id, times.of(name).as_nanos());
                }
            }
        }
        if let Some(setup) = &stated.setup {
            plan.per_variant = setup.per_variant.map(SimDuration::as_nanos);
            for ((from, to), mean) in &setup.matrix {
                if let (Some(from), Some(to)) =
                    (position_of(variants, from), position_of(variants, to))
                {
                    plan.changes.insert((from, to), mean.as_nanos());
                }
            }
        }
        plans.insert(station.id().to_owned(), Arc::new(plan));
    }
    plans
}

/// How parts leave each component whose edges are not plain: a plain edge has no
/// `when`, no `split`, no status rule, no target with a rule and no way back into a
/// return lane, and keeps M5's path (ADR-0023 §3).
fn route_tables(
    document: &Document,
    facts: &BTreeMap<&str, (&str, StationFacts)>,
    takers: &BTreeMap<String, Arc<TakerPlan>>,
    gate_returns: &BTreeSet<(&str, &str)>,
    variants: &Arc<Vec<String>>,
) -> Result<BTreeMap<String, Arc<RouteTable>>, BuildError> {
    let env = condition_env(document).map_err(|source| BuildError::Conditions {
        source: Box::new(source),
    })?;
    let mut tables = BTreeMap::new();
    for (id, edges) in leaving_edges(document) {
        let plain = matches!(edges.as_slice(), [(edge, _)]
        if edge.when().is_none()
            && edge.split().is_none()
            && matches!(edge.choose(), None | Some(ChoiceRule::FirstFree))
            && edge.to().iter().all(|to| {
                !takers.contains_key(to.id()) && !gate_returns.contains(&(id, to.id()))
            }));
        if plain {
            continue;
        }
        let mut names: Vec<&str> = Vec::new();
        let mut table_edges = Vec::with_capacity(edges.len());
        let mut has_when = false;
        let mut has_split = false;
        for (edge, _) in &edges {
            let when = match (edge.when(), edge.when_written()) {
                (Some(expr), Some((text, _, path))) => {
                    has_when = true;
                    let condition =
                        Condition::new(expr, &env).map_err(|error| BuildError::Condition {
                            path: path.clone(),
                            text: text.to_owned(),
                            source: Box::new(error),
                        })?;
                    Some((condition, expr.to_string()))
                }
                (Some(_) | None, Some(_) | None) => None,
            };
            let mut targets = Vec::with_capacity(edge.to().len());
            for to in edge.to() {
                let position = names.iter().position(|n| *n == to.id()).unwrap_or_else(|| {
                    names.push(to.id());
                    names.len() - 1
                });
                targets.push(position);
            }
            let pick = if let Some(shares) = edge.split() {
                has_split = true;
                let shares: Vec<f64> = shares.iter().map(|(_, s)| s.as_fraction()).collect();
                let mut total = 0.0;
                let cumulative = shares
                    .iter()
                    .map(|share| {
                        total += *share;
                        total
                    })
                    .collect();
                Pick::Split { shares, cumulative }
            } else if edge.to().len() == 1 {
                Pick::One
            } else {
                match edge.choose() {
                    None | Some(ChoiceRule::FirstFree) => Pick::FirstFree,
                    Some(ChoiceRule::Preferred) => Pick::Status(StatusRule::Preferred),
                    Some(ChoiceRule::ShortestQueue) => Pick::Status(StatusRule::ShortestQueue),
                    Some(ChoiceRule::LowestUtilization) => {
                        Pick::Status(StatusRule::LowestUtilization)
                    }
                }
            };
            table_edges.push(RouteEdge {
                when,
                targets,
                pick,
            });
        }
        let targets = names
            .iter()
            .map(|name| TargetPlan {
                taker: takers.get(*name).cloned(),
                places: facts
                    .get(name)
                    .and_then(|(_, stated)| stated.places)
                    .unwrap_or(1),
                returns: gate_returns.contains(&(id, *name)),
            })
            .collect();
        tables.insert(
            id.to_owned(),
            Arc::new(RouteTable {
                edges: table_edges,
                targets,
                has_when,
                has_split,
                variants: Arc::clone(variants),
            }),
        );
    }
    Ok(tables)
}

/// Who hears from each target (ADR-0023 §5): each upstream that uses a status rule on
/// it, or each upstream if its rule is `minimum_setup`. A target that always has room
/// sends nothing.
fn status_subscribers(
    document: &Document,
    facts: &BTreeMap<&str, (&str, StationFacts)>,
    takers: &BTreeMap<String, Arc<TakerPlan>>,
) -> BTreeMap<String, Vec<String>> {
    let mut subscribers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in document.flow() {
        let status_rule = matches!(
            edge.choose(),
            Some(ChoiceRule::Preferred | ChoiceRule::ShortestQueue | ChoiceRule::LowestUtilization)
        );
        for to in edge.to() {
            let reports = facts
                .get(to.id())
                .is_some_and(|(_, stated)| stated.status == StatusFact::Reports);
            let follows = takers
                .get(to.id())
                .is_some_and(|plan| plan.rule == DispatchRule::MinimumSetup);
            if reports && (status_rule || follows) {
                let list = subscribers.entry(to.id().to_owned()).or_default();
                for from in edge.from() {
                    if !list.iter().any(|known| known == from.id()) {
                        list.push(from.id().to_owned());
                    }
                }
            }
        }
    }
    subscribers
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
