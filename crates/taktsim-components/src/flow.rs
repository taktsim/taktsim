//! `Flow`: the domain of production lines, and what it checks across a
//! document (ADR-0022 §3, §5).

use std::collections::{BTreeMap, BTreeSet};

use taktsim_model::expr::{Arity, Dimension, Signature, Type};
use taktsim_model::{
    CatalogError, Component as Entry, ComponentRef, Document, FieldPath, Findings, Json,
    LoadErrorKind, Location, Place, Segment, model_schema,
};

use crate::buffer::Buffer;
use crate::machine::Machine;
use crate::message::{FlowMessage, VariantId};
use crate::params::MAX_UNITS;
use crate::run::{BuildError, Domain, Ports, Registry, Roles};
use crate::sink::Sink;
use crate::source::Source;

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
            declared: shared.variants.clone(),
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
        add_names(&mut names, source);
    }
    names
}

/// Adds the variant names `source` makes to `names`: its own, or `default`.
fn add_names<'a>(names: &mut BTreeSet<&'a str>, source: &'a Source) {
    match source.variant_names() {
        Some(declared) => names.extend(declared),
        None => {
            names.insert(DEFAULT_VARIANT);
        }
    }
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
    // The example at a reference is an id: one the rule allows there.
    let first_with = |allowed: fn(Ports) -> bool, fallback: &'static str| {
        document
            .components()
            .iter()
            .map(Entry::id)
            .find(|id| ports_of(id).is_some_and(allowed))
            .unwrap_or(fallback)
    };
    let with_outlet = first_with(Ports::has_outlet, "cut");
    let with_inlet = first_with(Ports::has_inlet, "cut");
    for edge in document.flow() {
        for from in edge.from() {
            if ports_of(from.id()).is_some_and(|p| !p.has_outlet()) {
                not_allowed(
                    findings,
                    (from.place(), from.path()),
                    format!("an edge leaving `{}`", from.id()),
                    "edges leave components with an outlet: a Source, a Machine or a Buffer",
                    with_outlet,
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
                    with_inlet,
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
///
/// Reported at the second edge, pointing at its source, with the one edge that
/// replaces both as the example: no id at the reference alone would be valid.
fn check_one_edge_per_source(document: &Document, graph: &Graph<'_>, findings: &mut Findings) {
    let mut first: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, edge) in document.flow().iter().enumerate() {
        for from in edge.from() {
            let seen = *first.entry(from.id()).or_insert(index);
            if seen != index {
                let mut targets: Vec<&str> = Vec::new();
                for target in graph.targets(from.id()).map(ComponentRef::id) {
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
                // `flow[i]`: the entry that holds the reference.
                let segments = from.path().segments();
                let path = FieldPath::from(segments.get(..2).unwrap_or(segments).to_vec());
                not_allowed(
                    findings,
                    (from.place(), &path),
                    format!("a second edge leaving `{}`", from.id()),
                    "one edge leaving each component; a branch is one edge with several targets",
                    &format!("{{from: {}, to: [{}]}}", from.id(), targets.join(", ")),
                );
            }
        }
    }
}

/// No loop: a loop with no condition or ratio would send parts round for ever,
/// and finite buffers in a loop can deadlock. M6 lifts the rule with conditions.
fn check_no_loops(document: &Document, roles: &Roles, graph: &Graph<'_>, findings: &mut Findings) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        OnPath,
        Done,
    }
    // The example at the reference that closes a loop: an id that ends a path
    // there, like a Sink, so it closes none.
    let end = document
        .components()
        .iter()
        .find(|c| roles.ports(c.type_name()) == Some(Ports::Inlet))
        .map_or("sink", Entry::id);
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
                        end,
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

/// The buffers an on-demand Source reaches through buffers alone hold at most
/// 65,535 places together: it fills them all at time zero, so more could exhaust
/// memory on one PC and not on another (ADR-0022, "The vocabulary"). A buffer
/// behind a timed Source, or behind a station, may be larger.
///
/// Reported once per buffer, at the `capacity` of the largest buffer the Source
/// fills (the first in document order of equal ones), with the capacity that
/// would bring the total to the limit as the example.
fn check_on_demand_places(document: &Document, graph: &Graph<'_>, findings: &mut Findings) {
    let buffers: BTreeSet<&str> = document
        .components()
        .iter()
        .filter(|c| c.params::<Buffer>().is_some())
        .map(Entry::id)
        .collect();
    let mut reported = BTreeSet::new();
    for source in document.components() {
        if !source.params::<Source>().is_some_and(Source::on_demand) {
            continue;
        }
        let reached = graph.reached(source.id(), |id| buffers.contains(id));
        // In document order, with their places and where `capacity` is written.
        let filled: Vec<(&Entry, u64, Location)> = document
            .components()
            .iter()
            .filter(|c| reached.contains(c.id()))
            .filter_map(|c| {
                let (places, at) = c.params::<Buffer>()?.capacity();
                Some((c, places.get(), at))
            })
            .collect();
        let total = filled
            .iter()
            .fold(0_u64, |sum, (_, places, _)| sum.saturating_add(*places));
        let largest = filled
            .iter()
            .copied()
            .reduce(|best, next| if next.1 > best.1 { next } else { best });
        let Some((entry, places, at)) = largest else {
            continue;
        };
        if total <= MAX_UNITS || !reported.insert(entry.id()) {
            continue;
        }
        let names: Vec<String> = filled
            .iter()
            .map(|(c, _, _)| format!("`{}`", c.id()))
            .collect();
        let fits = MAX_UNITS.saturating_sub(total - places).max(1);
        let place = Place::new(entry.place().file().map(str::to_owned), at);
        let path = entry.path().child(Segment::Key("capacity".to_owned()));
        not_allowed(
            findings,
            (&place, &path),
            format!(
                "{total} places in {}, which on-demand `{}` fills at time zero",
                names.join(", "),
                source.id()
            ),
            "at most 65535 places in the buffers an on-demand Source reaches through buffers \
             alone, since it fills them at time zero; a Source with `interarrival` may feed a \
             larger buffer",
            &format!("{fits} parts"),
        );
    }
}

/// Matrix names are declared by some Source, and the model's names fit a
/// `VariantId`.
/// Reports `total` variant names, more than a model holds, at the Source whose
/// names, in document order, pass the limit: at its `variants`, or at the entry
/// of a Source that declares none and so adds `default`.
fn too_many_variants(document: &Document, total: usize, findings: &mut Findings) {
    let mut names = BTreeSet::new();
    for c in document.components() {
        let Some(source) = c.params::<Source>() else {
            continue;
        };
        add_names(&mut names, source);
        if names.len() <= MAX_VARIANTS {
            continue;
        }
        let allowed = "at most 65535 variant names in a model";
        match source.variants_at() {
            Some(at) => {
                let place = Place::new(c.place().file().map(str::to_owned), at);
                let path = c.path().child(Segment::Key("variants".to_owned()));
                not_allowed(
                    findings,
                    (&place, &path),
                    format!("{total} variant names in the model"),
                    allowed,
                    "{basic: 80%, premium: 20%}",
                );
            }
            None => not_allowed(
                findings,
                (c.place(), c.path()),
                format!(
                    "{total} variant names in the model, counting `{DEFAULT_VARIANT}` for this \
                     Source, which declares none"
                ),
                allowed,
                "{id: source, type: Source, interarrival: 1min, variants: {basic: 100%}}",
            ),
        }
        return;
    }
}

fn check_variants(document: &Document, findings: &mut Findings) {
    let declared = declared_variants(document);
    if declared.len() > MAX_VARIANTS {
        too_many_variants(document, declared.len(), findings);
    }
    for c in document.components() {
        let Some(setup) = c.params::<Machine>().and_then(Machine::setup) else {
            continue;
        };
        for change in setup.changes() {
            for name in [&change.from, &change.to] {
                if !declared.contains(name.id()) {
                    let known: Vec<&str> = declared.iter().copied().collect();
                    not_allowed(
                        findings,
                        (name.place(), name.path()),
                        format!("the variant `{}`, which no Source declares", name.id()),
                        &format!("a variant some Source declares: {}", known.join(", ")),
                        known.first().copied().unwrap_or("basic"),
                    );
                }
            }
        }
    }
}

impl Domain for Flow {
    type Message = FlowMessage;
    type Shared = FlowShared;

    const PACKAGE: &'static str = "taktsim-components";
    const PACKAGE_VERSION: &'static str = env!("CARGO_PKG_VERSION");

    fn check(document: &Document, roles: &Roles, findings: &mut Findings) {
        let graph = Graph::new(document);
        check_ports(document, roles, &graph, findings);
        check_one_edge_per_source(document, &graph, findings);
        check_no_loops(document, roles, &graph, findings);
        check_on_demand(document, roles, &graph, findings);
        check_on_demand_places(document, &graph, findings);
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

    /// Nothing outside the components' parameters draws until M6's `split` on
    /// an edge.
    fn draws_random(_: &Document) -> bool {
        false
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

/// The JSON Schema of a model of the standard catalog: the bytes of
/// `schemas/model/<format>.json`, once written with [`Json::to_pretty`] and a
/// line end. `taktsim schema` and the schema generator print this one value.
///
/// # Errors
///
/// As [`standard`].
///
/// ```
/// let schema = taktsim_components::standard_schema()?;
/// assert_eq!(schema["properties"]["format"]["const"], "1.0");
/// assert!(schema["$defs"]["Component"].as_object().is_some());
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
pub fn standard_schema() -> Result<Json, CatalogError> {
    let registry = standard()?;
    let catalog = registry.catalog();
    // The types named from the catalog too, so the description cannot fall
    // behind it.
    let types: Vec<&str> = catalog.types().collect();
    let description = format!(
        "A taktsim model (spec §4) of the standard production catalog: {}. Generated from \
         the Rust types; do not edit.",
        types.join(", ")
    );
    Ok(model_schema(catalog).with("description", &description.as_str()))
}
