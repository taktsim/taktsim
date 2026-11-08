//! Properties on generated models (roadmap M5): chains and acyclic graphs with
//! branches, merges, failures, capacities and every discipline.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use proptest::prelude::*;
use support::{load, run_model};
use taktsim_components::{FlowMessage, MachineState};
use taktsim_core::{Disposal, EngineConfig, RunOutcome};
use taktsim_trace::Record;

/// A station or a buffer of a generated model.
#[derive(Debug, Clone)]
enum Node {
    Machine {
        cycle: u32,
        capacity: u32,
        failing: bool,
        variant_setup: bool,
    },
    Buffer {
        capacity: u32,
        discipline: &'static str,
    },
}

fn node() -> impl Strategy<Value = Node> {
    prop_oneof![
        3 => (1_u32..60, 1_u32..4, any::<bool>(), any::<bool>()).prop_map(
            |(cycle, capacity, failing, variant_setup)| Node::Machine {
                cycle,
                capacity,
                failing,
                variant_setup,
            }
        ),
        2 => (1_u32..4, prop_oneof![Just("fifo"), Just("lifo"), Just("priority")])
            .prop_map(|(capacity, discipline)| Node::Buffer { capacity, discipline }),
    ]
}

/// A generated model: layers of nodes; each node feeds at least one node of the
/// next layer, and each node of a later layer has a feeder.
#[derive(Debug, Clone)]
struct Shape {
    on_demand: bool,
    interval: u32,
    batch: u32,
    layers: Vec<Vec<Node>>,
    /// For each node (in order), which next-layer nodes it also feeds.
    links: Vec<u32>,
    seed: u64,
}

fn chain() -> impl Strategy<Value = Shape> {
    (
        any::<bool>(),
        1_u32..30,
        1_u32..4,
        proptest::collection::vec(node(), 1..6),
        any::<u64>(),
    )
        .prop_map(|(on_demand, interval, batch, nodes, seed)| Shape {
            on_demand,
            interval,
            batch,
            layers: nodes.into_iter().map(|n| vec![n]).collect(),
            links: Vec::new(),
            seed,
        })
}

fn graph() -> impl Strategy<Value = Shape> {
    (
        any::<bool>(),
        1_u32..30,
        1_u32..4,
        proptest::collection::vec(proptest::collection::vec(node(), 1..4), 1..5),
        proptest::collection::vec(any::<u32>(), 16),
        any::<u64>(),
    )
        .prop_map(|(on_demand, interval, batch, layers, links, seed)| Shape {
            on_demand,
            interval,
            batch,
            layers,
            links,
            seed,
        })
}

/// The model of `shape`, as a document. A station goes first on an on-demand
/// path, so parts cannot reach a sink at one instant.
fn yaml(shape: &Shape) -> String {
    let mut components = String::new();
    let source = if shape.on_demand {
        "{id: source, type: Source, variants: {red: 70%, blue: 30%}}".to_owned()
    } else {
        format!(
            "{{id: source, type: Source, interarrival: {{dist: exponential, mean: {}s}}, batch: {} parts, variants: {{red: 70%, blue: 30%}}, priority: 3}}",
            shape.interval, shape.batch
        )
    };
    let _ = writeln!(components, "  - {source}");
    components
        .push_str("  - {id: other, type: Source, interarrival: 17s, variants: {red: 100%}}\n");
    let mut names: Vec<Vec<String>> = Vec::new();
    for (l, layer) in shape.layers.iter().enumerate() {
        let mut layer_names = Vec::new();
        for (i, node) in layer.iter().enumerate() {
            let name = format!("n{l}-{i}");
            let line = match node {
                Node::Machine {
                    cycle,
                    capacity,
                    failing,
                    variant_setup,
                } => {
                    let failures = if *failing {
                        ", mtbf: {dist: exponential, mean: 200s}, mttr: {dist: exponential, mean: 20s}"
                    } else {
                        ""
                    };
                    let setup = if *variant_setup {
                        ", setup: {per_variant: 5s, matrix: [{from: red, to: blue, time: 0s}]}"
                    } else {
                        ""
                    };
                    format!(
                        "  - {{id: {name}, type: Machine, cycle_time: {{dist: exponential, mean: {cycle}s}}, capacity: {capacity} parts{failures}{setup}}}\n"
                    )
                }
                Node::Buffer {
                    capacity,
                    discipline,
                } => format!(
                    "  - {{id: {name}, type: Buffer, capacity: {capacity} parts, discipline: {discipline}}}\n"
                ),
            };
            components.push_str(&line);
            layer_names.push(name);
        }
        names.push(layer_names);
    }
    // A station before any buffer on an on-demand path: the guard in front.
    components
        .push_str("  - {id: guard, type: Machine, cycle_time: {dist: exponential, mean: 5s}}\n");
    components.push_str("  - {id: sink, type: Sink}\n  - {id: sink2, type: Sink}\n");
    let mut flow = String::new();
    flow.push_str("  - [source, guard]\n");
    let first = names[0].join(", ");
    let _ = writeln!(flow, "  - {{from: [guard, other], to: [{first}]}}");
    let mut links = shape.links.iter().copied().cycle();
    for pair in names.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        // Each target has a feeder: the i-th source feeds the i-th target, the
        // last source any left.
        let mut targets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); from.len()];
        for t in 0..to.len() {
            targets[t.min(from.len() - 1)].insert(t);
        }
        for (s, set) in targets.iter_mut().enumerate() {
            if set.is_empty() {
                set.insert(s % to.len());
            }
            let extra = links.next().unwrap_or(0) as usize % (to.len() + 1);
            if extra < to.len() {
                set.insert(extra);
            }
        }
        for (s, set) in targets.iter().enumerate() {
            let list: Vec<&str> = set.iter().map(|t| to[*t].as_str()).collect();
            let _ = writeln!(flow, "  - {{from: {}, to: [{}]}}", from[s], list.join(", "));
        }
    }
    if let Some(last) = names.last() {
        for name in last {
            let _ = writeln!(flow, "  - {{from: {name}, to: [sink, sink2]}}");
        }
    }
    support::document("20min", components.trim_end(), flow.trim_end())
}

/// What a run's trace says about who holds what, as it is read.
#[derive(Default)]
struct Books {
    capacity: BTreeMap<String, u64>,
    buffers: BTreeSet<String>,
    /// Items held per component, per the events.
    fill: BTreeMap<String, u64>,
    /// Parts each holder can give: what the protocol must have on offer.
    to_give: BTreeMap<String, i64>,
    down: BTreeSet<String>,
    /// (upstream, target) with an entry at the target.
    entries: BTreeSet<(String, String)>,
}

impl Books {
    fn new(shape: &Shape) -> Self {
        let mut books = Self::default();
        for (l, layer) in shape.layers.iter().enumerate() {
            for (i, node) in layer.iter().enumerate() {
                let name = format!("n{l}-{i}");
                let cap = match node {
                    Node::Machine { capacity, .. } => *capacity,
                    Node::Buffer { capacity, .. } => {
                        books.buffers.insert(name.clone());
                        *capacity
                    }
                };
                books.capacity.insert(name, u64::from(cap));
            }
        }
        books.capacity.insert("guard".to_owned(), 1);
        books
    }

    fn arrive(&mut self, by: &str, to: &str) -> Result<(), TestCaseError> {
        *self.fill.entry(to.to_owned()).or_default() += 1;
        if let Some(held) = self.fill.get_mut(by) {
            *held = held.saturating_sub(1);
        }
        if self.buffers.contains(to) {
            *self.to_give.entry(to.to_owned()).or_default() += 1;
        }
        *self.to_give.entry(by.to_owned()).or_default() -= 1;
        if let Some(cap) = self.capacity.get(to) {
            prop_assert!(self.fill[to] <= *cap, "{} holds {}", to, self.fill[to]);
        }
        Ok(())
    }

    /// An event delivered from `by` to `to`.
    fn event(&mut self, by: &str, to: &str, message: &FlowMessage) -> Result<(), TestCaseError> {
        if matches!(message, FlowMessage::Arrive(_)) {
            self.arrive(by, to)?;
        }
        if matches!(message, FlowMessage::CycleDone { .. }) {
            *self.to_give.entry(to.to_owned()).or_default() += 1;
        }
        if matches!(message, FlowMessage::Offer) {
            self.entries.insert((by.to_owned(), to.to_owned()));
        }
        if matches!(message, FlowMessage::Withdraw) {
            self.entries.remove(&(by.to_owned(), to.to_owned()));
        }
        if matches!(message, FlowMessage::Accept) {
            self.entries.remove(&(to.to_owned(), by.to_owned()));
        }
        if matches!(message, FlowMessage::Failure) {
            self.down.insert(to.to_owned());
        }
        if matches!(message, FlowMessage::Repaired) {
            self.down.remove(to);
        }
        Ok(())
    }

    /// Between instants, a target holds an entry exactly when its upstream has
    /// an offer outstanding, which it has exactly when it has a part to give.
    fn check_entries(
        &self,
        edges: &BTreeSet<(String, String)>,
        on_demand: bool,
        time: taktsim_units::SimTime,
    ) -> Result<(), TestCaseError> {
        for (from, to) in edges {
            if self.down.contains(from) {
                continue;
            }
            let has =
                (from == "source" && on_demand) || self.to_give.get(from).copied().unwrap_or(0) > 0;
            prop_assert_eq!(
                self.entries.contains(&(from.clone(), to.clone())),
                has,
                "{} -> {} at {}: the entry and the offer disagree",
                from,
                to,
                time
            );
        }
        Ok(())
    }
}

/// Every property that a run's trace must show, read from the records.
fn check_trace(shape: &Shape, r: &support::Run) -> Result<(), TestCaseError> {
    let mut books = Books::new(shape);
    let edges: BTreeSet<(String, String)> = r
        .events()
        .iter()
        .filter(|e| matches!(e.message, FlowMessage::Offer))
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    let mut now = None;
    for record in &r.records {
        let time = record.time();
        if now != Some(time) {
            if let Some(previous) = now {
                books.check_entries(&edges, shape.on_demand, previous)?;
            }
            now = Some(time);
        }
        match record {
            Record::Event {
                target,
                from,
                message,
                ..
            } => books.event(r.name(*from), r.name(*target), message)?,
            Record::ItemCreated { by, .. } => {
                *books.to_give.entry(r.name(*by).to_owned()).or_default() += 1;
            }
            Record::ItemDisposed { by, how, .. } => {
                prop_assert_eq!(*how, Disposal::Delivered);
                if let Some(held) = books.fill.get_mut(r.name(*by)) {
                    *held = held.saturating_sub(1);
                }
            }
            Record::Note {
                by,
                message: FlowMessage::MachineState(note),
                ..
            } => {
                let machine = r.name(*by);
                // Never more than `capacity` units in process.
                if let Some(cap) = books.capacity.get(machine) {
                    prop_assert!(u64::from(note.units_in_cycle) <= *cap);
                }
                prop_assert_eq!(
                    note.state == MachineState::Down,
                    books.down.contains(machine)
                );
            }
            Record::Note { .. } | Record::Decision { .. } => {}
        }
    }
    if let Some(last) = now {
        books.check_entries(&edges, shape.on_demand, last)?;
    }
    Ok(())
}

fn check(shape: &Shape) -> Result<(), TestCaseError> {
    let text = yaml(shape);
    // A model that loads with the standard catalog always builds.
    let model = load(&text);
    let r = run_model(&model, shape.seed);
    // Conservation.
    let c = r.report.items;
    prop_assert_eq!(
        c.created.get(),
        c.held.get() + c.in_transit.get() + c.delivered.get() + c.scrapped.get() + c.consumed.get()
    );
    prop_assert_eq!(c.in_transit.get(), 0);
    // No deadlock: something is still to come at the horizon.
    prop_assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    check_trace(shape, &r)?;
    // Determinism: the same run, byte for byte.
    let again = run_model(&model, shape.seed);
    prop_assert!(r.bytes == again.bytes);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// A model that loads with the standard catalog always builds: what fails
    /// must fail at load time, with a path (principle 2).
    #[test]
    fn a_loaded_model_always_builds(shape in graph()) {
        let model = load(&yaml(&shape));
        let built = support::registry().build(model.document(), EngineConfig::new(shape.seed));
        prop_assert!(built.is_ok());
    }

    /// Every spelling of the same values has one hash (ADR-0022 §1): the order of
    /// `variants` and `matrix`, and how a quantity is written.
    #[test]
    fn every_spelling_of_the_vocabulary_has_one_hash(
        flip_variants: bool,
        flip_matrix: bool,
        minutes: bool,
        decimal: bool,
    ) {
        let spelled = |flip_variants: bool, flip_matrix: bool, minutes: bool, decimal: bool| {
            let variants = if flip_variants {
                "{premium: 20%, basic: 80%}"
            } else {
                "{basic: 80%, premium: 20%}"
            };
            let first = "{from: basic, to: premium, time: 6min}";
            let second = "{from: premium, to: basic, time: 0s}";
            let matrix = if flip_matrix {
                format!("[{second}, {first}]")
            } else {
                format!("[{first}, {second}]")
            };
            let per_variant = if minutes { "4min" } else { "240s" };
            let capacity = if decimal { "20.0 parts" } else { "20 parts" };
            support::document(
                "8h",
                &format!(
                    "  - {{id: source, type: Source, variants: {variants}}}\n  \
                     - {{id: m, type: Machine, cycle_time: 42s, setup: {{per_variant: {per_variant}, matrix: {matrix}}}}}\n  \
                     - {{id: buf, type: Buffer, capacity: {capacity}}}\n  - {{id: sink, type: Sink}}"
                ),
                "  - [source, m, buf, sink]",
            )
        };
        let plain = load(&spelled(false, false, true, false));
        let other = load(&spelled(flip_variants, flip_matrix, minutes, decimal));
        prop_assert_eq!(plain.hash(), other.hash());
        prop_assert_eq!(plain.system_hash(), other.system_hash());
        prop_assert_eq!(plain.canonical_json(), other.canonical_json());
    }

    #[test]
    fn generated_chains_keep_their_invariants(shape in chain()) {
        check(&shape)?;
    }

    #[test]
    fn generated_acyclic_graphs_keep_their_invariants(shape in graph()) {
        check(&shape)?;
    }
}

/// What a holder's leaving edges are, for reading its trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leaving {
    /// One edge with a status rule: one standing offer.
    Standing,
    /// One edge, first free: an offer to every target.
    Everywhere,
    /// A condition or a split: an offer at least to one target.
    Somewhere,
}

/// A generated routed model: layers of stations and buffers, with every kind of edge,
/// rules, failures and possibly a loop back to the first layer's only machine.
#[derive(Debug, Clone)]
struct Routed {
    on_demand: bool,
    interval: u32,
    layers: Vec<Vec<Node>>,
    /// For each node (in order), its kind of leaving edge.
    kinds: Vec<u8>,
    /// For each machine (in order), its rule, if any.
    rules: Vec<u8>,
    /// With a loop: the share in percent that goes round.
    rework: Option<u32>,
    seed: u64,
}

fn routed() -> impl Strategy<Value = Routed> {
    (
        any::<bool>(),
        1_u32..30,
        proptest::collection::vec(proptest::collection::vec(node(), 1..4), 1..4),
        proptest::collection::vec(0_u8..6, 16),
        proptest::collection::vec(0_u8..8, 16),
        prop_oneof![Just(None), (5_u32..80).prop_map(Some)],
        any::<u64>(),
    )
        .prop_map(
            |(on_demand, interval, mut layers, kinds, rules, rework, seed)| {
                if rework.is_some() {
                    // New parts enter a loop only at its return target, a machine.
                    let first = Node::Machine {
                        cycle: 3,
                        capacity: 1,
                        failing: seed % 2 == 0,
                        variant_setup: seed % 3 == 0,
                    };
                    layers.insert(0, vec![first]);
                }
                Routed {
                    on_demand,
                    interval,
                    layers,
                    kinds,
                    rules,
                    rework,
                    seed,
                }
            },
        )
}

/// What the document and the reader of its trace need.
struct RoutedModel {
    yaml: String,
    leaving: BTreeMap<String, (Leaving, Vec<String>)>,
    capacity: BTreeMap<String, u64>,
    machines: BTreeSet<String>,
    buffers: BTreeSet<String>,
}

/// One holder's edges to `targets`, by `kind`.
fn write_edges(
    flow: &mut String,
    from: &str,
    targets: &[String],
    kind: u8,
    machines_only: bool,
) -> Leaving {
    let list = targets.join(", ");
    if targets.len() == 1 {
        let _ = writeln!(flow, "  - {{from: {from}, to: {list}}}");
        return Leaving::Everywhere;
    }
    match kind {
        1 => {
            let _ = writeln!(
                flow,
                "  - {{from: {from}, to: [{list}], choose: preferred}}"
            );
            Leaving::Standing
        }
        2 => {
            let _ = writeln!(
                flow,
                "  - {{from: {from}, to: [{list}], choose: shortest_queue}}"
            );
            Leaving::Standing
        }
        3 if machines_only => {
            let _ = writeln!(
                flow,
                "  - {{from: {from}, to: [{list}], choose: lowest_utilization}}"
            );
            Leaving::Standing
        }
        4 => {
            let _ = writeln!(
                flow,
                "  - {{from: {from}, to: {}, when: \"variant == 'red'\"}}",
                targets[0]
            );
            let _ = writeln!(
                flow,
                "  - {{from: {from}, to: [{}]}}",
                targets[1..].join(", ")
            );
            Leaving::Somewhere
        }
        5 => {
            let share = 100 / targets.len();
            let last = 100 - share * (targets.len() - 1);
            let shares: Vec<String> = targets
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let share = if i + 1 == targets.len() { last } else { share };
                    format!("{t}: {share}%")
                })
                .collect();
            let _ = writeln!(
                flow,
                "  - {{from: {from}, split: {{{}}}}}",
                shares.join(", ")
            );
            Leaving::Somewhere
        }
        _ => {
            let _ = writeln!(flow, "  - {{from: {from}, to: [{list}]}}");
            Leaving::Everywhere
        }
    }
}

/// The line of one generated component, stating what the reader of its trace needs.
fn component_line(
    name: &str,
    node: &Node,
    rule: u8,
    model: &mut RoutedModel,
    control: &mut Vec<String>,
) -> String {
    match node {
        Node::Machine {
            cycle,
            capacity,
            failing,
            variant_setup,
        } => {
            let failures = if *failing {
                ", mtbf: {dist: exponential, mean: 200s}, mttr: {dist: exponential, mean: 20s}"
            } else {
                ""
            };
            let setup = if *variant_setup {
                ", setup: {per_variant: 5s, matrix: [{from: red, to: blue, time: 0s}]}"
            } else {
                ""
            };
            model.capacity.insert(name.to_owned(), u64::from(*capacity));
            model.machines.insert(name.to_owned());
            let word = match rule {
                1 => "fifo",
                2 => "earliest_due_date",
                3 => "shortest_processing_time",
                4 => "minimum_setup",
                5 => "priority_class",
                _ => "",
            };
            if !word.is_empty() {
                control.push(format!("{name}: {word}"));
            }
            format!(
                "  - {{id: {name}, type: Machine, cycle_time: {{dist: exponential, mean: {cycle}s}}, \
                 cycle_time_by_variant: {{red: {{dist: uniform, min: 1s, max: {}s}}}}, capacity: {capacity} parts{failures}{setup}}}\n",
                cycle + 1
            )
        }
        Node::Buffer {
            capacity,
            discipline,
        } => {
            model.capacity.insert(name.to_owned(), u64::from(*capacity));
            model.buffers.insert(name.to_owned());
            format!(
                "  - {{id: {name}, type: Buffer, capacity: {capacity} parts, discipline: {discipline}}}\n"
            )
        }
    }
}

/// The edges of each layer to the next, and of the last to the sinks or back.
fn routed_flow(shape: &Routed, names: &[Vec<String>], model: &mut RoutedModel) -> String {
    let mut flow = String::new();
    flow.push_str("  - [source, guard]\n");
    let first = names[0].join(", ");
    let _ = writeln!(flow, "  - {{from: [guard, other], to: [{first}]}}");
    model.leaving.insert(
        "source".to_owned(),
        (Leaving::Everywhere, vec!["guard".to_owned()]),
    );
    for from in ["guard", "other"] {
        model
            .leaving
            .insert(from.to_owned(), (Leaving::Everywhere, names[0].clone()));
    }
    let mut kinds = shape.kinds.iter().copied().cycle();
    for pair in names.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        let machines_only = to.iter().all(|t| model.machines.contains(t));
        // The i-th feeds the i-th target, the last any left.
        let mut targets: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); from.len()];
        for t in 0..to.len() {
            targets[t.min(from.len() - 1)].insert(t);
        }
        for (s, set) in targets.iter_mut().enumerate() {
            if set.is_empty() {
                set.insert(s % to.len());
            }
        }
        for (s, set) in targets.iter().enumerate() {
            let list: Vec<String> = set.iter().map(|t| to[*t].clone()).collect();
            let kind = kinds.next().unwrap_or(0);
            let leaves = write_edges(&mut flow, &from[s], &list, kind, machines_only);
            model.leaving.insert(from[s].clone(), (leaves, list));
        }
    }
    let ends = names.last().cloned().unwrap_or_default();
    let sinks = ["sink".to_owned(), "sink2".to_owned()];
    for name in &ends {
        if let Some(back) = shape.rework {
            let target = &names[0][0];
            let _ = writeln!(
                flow,
                "  - {{from: {name}, split: {{{target}: {back}%, sink: {}%}}}}",
                100 - back
            );
            model
                .leaving
                .insert(name.clone(), (Leaving::Somewhere, vec!["sink".to_owned()]));
        } else {
            // A status rule on a Sink has nothing to wait for: it always has room.
            let kind = u8::from(kinds.next().unwrap_or(0) % 2 == 0);
            let leaves = write_edges(&mut flow, name, &sinks, kind, false);
            model.leaving.insert(name.clone(), (leaves, sinks.to_vec()));
        }
    }
    flow
}

fn routed_model(shape: &Routed) -> RoutedModel {
    let mut model = RoutedModel {
        yaml: String::new(),
        leaving: BTreeMap::new(),
        capacity: BTreeMap::from([("guard".to_owned(), 1)]),
        machines: BTreeSet::from(["guard".to_owned()]),
        buffers: BTreeSet::new(),
    };
    let mut components = String::new();
    let mut names: Vec<Vec<String>> = Vec::new();
    let mut rules = shape.rules.iter().copied().cycle();
    let mut control = Vec::new();
    let source = if shape.on_demand {
        "{id: source, type: Source, variants: {red: 70%, blue: 30%}}".to_owned()
    } else {
        format!(
            "{{id: source, type: Source, interarrival: {{dist: exponential, mean: {}s}}, batch: 2 parts, \
             variants: {{red: 70%, blue: 30%}}, due_in: {{dist: uniform, min: 1min, max: 1h}}, \
             priority_mix: [{{priority: 3, share: 40%}}, {{priority: 1, share: 60%}}]}}",
            shape.interval
        )
    };
    let _ = writeln!(components, "  - {source}");
    components
        .push_str("  - {id: other, type: Source, interarrival: 17s, variants: {red: 100%}}\n");
    components
        .push_str("  - {id: guard, type: Machine, cycle_time: {dist: exponential, mean: 5s}}\n");
    for (l, layer) in shape.layers.iter().enumerate() {
        let mut layer_names = Vec::new();
        for (i, node) in layer.iter().enumerate() {
            let name = format!("n{l}-{i}");
            let rule = rules.next().unwrap_or(0);
            components.push_str(&component_line(&name, node, rule, &mut model, &mut control));
            layer_names.push(name);
        }
        names.push(layer_names);
    }
    components.push_str("  - {id: sink, type: Sink}\n");
    if shape.rework.is_none() {
        components.push_str("  - {id: sink2, type: Sink}\n");
    }
    let flow = routed_flow(shape, &names, &mut model);
    let mut yaml = support::document("20min", components.trim_end(), flow.trim_end());
    if !control.is_empty() {
        let _ = writeln!(yaml, "control:\n  dispatch: {{{}}}", control.join(", "));
    }
    model.yaml = yaml;
    model
}

/// What the trace of a routed run says, as it is read.
#[derive(Default)]
struct RoutedBooks {
    /// Parts each holder can give.
    to_give: BTreeMap<String, i64>,
    /// Items at each station or buffer, by its places, the lane apart.
    fill: BTreeMap<String, i64>,
    down: BTreeSet<String>,
    /// (upstream, target) with an entry at the target.
    entries: BTreeSet<(String, String)>,
    /// (holder, target) -> the room the holder last heard of. It assumes room at first.
    room: BTreeMap<(String, String), bool>,
}

impl RoutedBooks {
    fn event(&mut self, from: &str, to: &str, message: &FlowMessage, on_demand: bool) {
        let part_left = |books: &mut Self, holder: &str| {
            if !(holder == "source" && on_demand) {
                *books.to_give.entry(holder.to_owned()).or_default() -= 1;
            }
            *books.fill.entry(holder.to_owned()).or_default() -= 1;
        };
        match message {
            FlowMessage::Arrive(_) => {
                part_left(self, from);
                *self.fill.entry(to.to_owned()).or_default() += 1;
            }
            FlowMessage::Return(_) => part_left(self, from),
            FlowMessage::CycleDone { .. } => {
                *self.to_give.entry(to.to_owned()).or_default() += 1;
            }
            FlowMessage::Offer | FlowMessage::OfferOf(_) => {
                self.entries.insert((from.to_owned(), to.to_owned()));
            }
            FlowMessage::Withdraw => {
                self.entries.remove(&(from.to_owned(), to.to_owned()));
            }
            FlowMessage::Accept => {
                self.entries.remove(&(to.to_owned(), from.to_owned()));
            }
            FlowMessage::Failure => {
                self.down.insert(to.to_owned());
            }
            FlowMessage::Repaired => {
                self.down.remove(to);
            }
            FlowMessage::Status(status) => {
                self.room
                    .insert((to.to_owned(), from.to_owned()), status.room);
            }
            FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::Decline
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }
            | FlowMessage::RouteDecision(_)
            | FlowMessage::DispatchDecision(_)
            | FlowMessage::EdgeCondition { .. } => {}
        }
    }

    /// Between instants: target entries match outstanding offers, and no part stands
    /// beside a free target.
    fn check_instant(
        &self,
        model: &RoutedModel,
        on_demand: bool,
        time: taktsim_units::SimTime,
    ) -> Result<(), TestCaseError> {
        for (holder, (kind, targets)) in &model.leaving {
            if self.down.contains(holder) {
                continue;
            }
            let has = (holder == "source" && on_demand)
                || self.to_give.get(holder).copied().unwrap_or(0) > 0;
            let standing: Vec<&String> = targets
                .iter()
                .filter(|t| self.entries.contains(&(holder.clone(), (*t).clone())))
                .collect();
            if !has {
                prop_assert_eq!(
                    standing.len(),
                    0,
                    "{} has nothing to give at {}, and offers {:?}",
                    holder,
                    time,
                    standing
                );
                continue;
            }
            match kind {
                Leaving::Standing => {
                    prop_assert_eq!(
                        standing.len(),
                        1,
                        "{} at {}: not one standing offer: {:?}",
                        holder,
                        time,
                        standing
                    );
                    let knows = |target: &String| {
                        self.room
                            .get(&(holder.clone(), target.clone()))
                            .copied()
                            .unwrap_or(true)
                    };
                    if !knows(standing[0]) {
                        prop_assert!(
                            !targets.iter().any(knows),
                            "{} at {}: stands at {} beside a free target",
                            holder,
                            time,
                            standing[0]
                        );
                    }
                }
                Leaving::Everywhere => {
                    prop_assert_eq!(
                        standing.len(),
                        targets.len(),
                        "{} at {}: an offer to every target: {:?}",
                        holder,
                        time,
                        standing
                    );
                }
                Leaving::Somewhere => {}
            }
        }
        for (name, places) in &model.capacity {
            let held = self.fill.get(name).copied().unwrap_or(0);
            // A machine can hold what came back on its lane, and that is no place.
            if model.buffers.contains(name) {
                prop_assert!(held <= i64::try_from(*places).unwrap_or(i64::MAX));
            }
        }
        Ok(())
    }
}

fn check_routed(shape: &Routed) -> Result<(), TestCaseError> {
    let model = routed_model(shape);
    let loaded = load(&model.yaml);
    // The engine checks conservation and every invariant at every event.
    let r = run_model(&loaded, shape.seed);
    let c = r.report.items;
    prop_assert_eq!(
        c.created.get(),
        c.held.get() + c.in_transit.get() + c.delivered.get() + c.scrapped.get() + c.consumed.get()
    );
    prop_assert_eq!(c.in_transit.get(), 0);
    // No deadlock: the run reaches the horizon with deliveries late in the run.
    prop_assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    let last = r.delivered().last().map(|d| d.0);
    prop_assert!(
        last.is_some_and(|time| time > support::t("10min")),
        "the last delivery is at {:?} in\n{}",
        last,
        model.yaml
    );
    support::check_records(&r);
    let mut books = RoutedBooks::default();
    let mut now = None;
    for record in &r.records {
        let time = record.time();
        if now != Some(time) {
            if let Some(previous) = now {
                books.check_instant(&model, shape.on_demand, previous)?;
            }
            now = Some(time);
        }
        match record {
            Record::Event {
                target,
                from,
                message,
                ..
            } => books.event(r.name(*from), r.name(*target), message, shape.on_demand),
            Record::ItemCreated { by, .. } => {
                let by = r.name(*by).to_owned();
                if !(by == "source" && shape.on_demand) {
                    *books.to_give.entry(by.clone()).or_default() += 1;
                    *books.fill.entry(by).or_default() += 1;
                }
            }
            Record::ItemDisposed { .. } | Record::Note { .. } | Record::Decision { .. } => {}
        }
        // A part entering a Buffer can give at once.
        if let Record::Event {
            target,
            message: FlowMessage::Arrive(_),
            ..
        } = record
            && model.buffers.contains(r.name(*target))
        {
            *books.to_give.entry(r.name(*target).to_owned()).or_default() += 1;
        }
    }
    if let Some(last) = now {
        books.check_instant(&model, shape.on_demand, last)?;
    }
    prop_assert!(model.machines.contains("guard"));
    // Determinism: the same run, byte for byte.
    let again = run_model(&loaded, shape.seed);
    prop_assert!(r.bytes == again.bytes);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        rng_seed: proptest::test_runner::RngSeed::Fixed(6),
        ..ProptestConfig::default()
    })]

    /// Generated graphs with conditions, splits, pools, dispatching, failures and loops.
    #[test]
    fn generated_routed_graphs_keep_their_invariants(shape in routed()) {
        check_routed(&shape)?;
    }

    /// A model that loads with the standard catalog always builds.
    #[test]
    fn a_loaded_routed_model_always_builds(shape in routed()) {
        let model = load(&routed_model(&shape).yaml);
        let built = support::registry().build(model.document(), EngineConfig::new(shape.seed));
        prop_assert!(built.is_ok());
    }
}

#[test]
fn a_fast_source_in_front_of_a_one_place_buffer_does_not_deadlock() {
    let yaml = support::document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1s, batch: 5 parts}\n  \
         - {id: buf, type: Buffer, capacity: 1 parts}\n  \
         - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - [source, buf, m, sink]",
    );
    let r = support::run(&yaml, 1);
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    assert_eq!(r.report.items.delivered.get(), 59);
}
