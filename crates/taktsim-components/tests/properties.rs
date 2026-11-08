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
