//! The KPIs of a run (ADR-0025 §1, §2, §3). Traces are written record by record with no
//! engine, and every expected number is worked out in a comment from the times.

mod support;

use std::collections::BTreeMap;

use support::{TempDir, model as model_text, run_text, run_with, test_registry};
use taktsim_components::{
    BuildError, ComponentType, Creation, Domain, FlowMessage, ItemAttrs, Kpi, MachineState,
    MachineStateNote, MessageFact, Registry, Roles, Sink, Source, SourceState, SourceStateNote,
    VariantId, Wiring, standard,
};
use taktsim_core::{ComponentId, Disposal, ItemId, RunOutcome, RunSummary, TraceRecord, TraceSink};
use taktsim_model::expr::{Arity, Signature, Type};
use taktsim_model::{Document, Encode, Findings, Format, Json, Loader, Model};
use taktsim_run::{
    AccuracyKind, Failure, FailureClass, Metric, MetricValue, RunRequest, Scalar, Unreported,
    metrics_of_trace,
};
use taktsim_trace::{Header, ModelHash, SystemHash, TraceMessage, TraceWriter};
use taktsim_units::SimTime;

fn t(seconds: u64) -> SimTime {
    SimTime::from_nanos(seconds * 1_000_000_000)
}

fn id(position: usize) -> ComponentId {
    ComponentId::from_index(position)
}

fn item(number: u64) -> ItemId {
    ItemId::from_raw(number)
}

/// Writes a trace by hand and folds it.
struct Hand<M: TraceMessage> {
    writer: TraceWriter<Vec<u8>, M>,
    seq: u64,
}

impl<M: TraceMessage> Hand<M> {
    #[allow(clippy::unwrap_used)] // Test helper.
    fn new(components: &[&str]) -> Self {
        let header = Header::new::<M>(
            ModelHash::default(),
            SystemHash::default(),
            Some(1),
            components.iter().map(ToString::to_string).collect(),
        );
        Self {
            writer: TraceWriter::new(Vec::new(), &header).unwrap(),
            seq: 0,
        }
    }

    fn created(&mut self, at: u64, number: u64, by: usize) {
        self.writer.record(&TraceRecord::<M>::ItemCreated {
            time: t(at),
            item: item(number),
            by: id(by),
        });
    }

    /// The event that carries an item from one place to another.
    fn moved(&mut self, at: u64, number: u64, from: usize, to: usize, message: &M) {
        self.seq += 1;
        self.writer.record(&TraceRecord::Event {
            seq: self.seq,
            time: t(at),
            target: id(to),
            from: id(from),
            message,
            item: Some(item(number)),
        });
    }

    fn disposed(&mut self, at: u64, number: u64, by: usize, how: Disposal) {
        self.writer.record(&TraceRecord::<M>::ItemDisposed {
            time: t(at),
            item: item(number),
            by: id(by),
            how,
        });
    }

    fn note(&mut self, at: u64, by: usize, message: &M) {
        self.writer.record(&TraceRecord::Note {
            time: t(at),
            by: id(by),
            message,
        });
    }

    #[allow(clippy::unwrap_used)] // Test helper.
    fn finish(mut self, end: u64) -> Vec<u8> {
        self.writer.run_ended(&RunSummary {
            outcome: RunOutcome::Horizon,
            events: self.seq,
            now: t(end),
        });
        self.writer.finish().unwrap().0
    }
}

fn arrive() -> FlowMessage {
    FlowMessage::Arrive(ItemAttrs::new(VariantId::new(0), 0))
}

fn machine_note(state: MachineState, units: u32) -> FlowMessage {
    FlowMessage::MachineState(MachineStateNote {
        state,
        units_in_cycle: units,
        setup_variant: None,
    })
}

fn source_note(state: SourceState, held: u32) -> FlowMessage {
    FlowMessage::SourceState(SourceStateNote { state, held })
}

#[allow(clippy::unwrap_used)] // Test helper.
fn load<D: Domain>(registry: &Registry<D>, yaml: &str) -> Model {
    Loader::new(registry.catalog())
        .load_str(yaml, Format::Yaml)
        .unwrap()
}

/// The model of the hand-written traces: positions 0 to 3.
fn line_model(head: &str, capacity: u32) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: hand{head}}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n  - {{id: source, type: Source}}\n  \
         - {{id: buf, type: Buffer, capacity: 5 parts}}\n  \
         - {{id: cut, type: Machine, cycle_time: 1min, capacity: {capacity} parts}}\n  \
         - {{id: sink, type: Sink}}\nflow:\n  - [source, buf, cut, sink]\n"
    )
}

const SOURCE: usize = 0;
const BUF: usize = 1;
const CUT: usize = 2;
const SINK: usize = 3;

fn start_notes(hand: &mut Hand<FlowMessage>) {
    hand.note(0, SOURCE, &source_note(SourceState::Waiting, 0));
    hand.note(0, CUT, &machine_note(MachineState::Idle, 0));
}

#[allow(clippy::unwrap_used)] // Test helper.
fn fold(yaml: &str, trace: &[u8]) -> (Vec<Metric>, Vec<Unreported>) {
    let registry = standard().unwrap();
    metrics_of_trace(&registry, &load(&registry, yaml), trace).unwrap()
}

#[allow(clippy::panic)] // Test helper.
fn metric<'a>(metrics: &'a [Metric], name: &str) -> &'a Metric {
    metrics
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no metric {name}: {:?}", names(metrics)))
}

fn names(metrics: &[Metric]) -> Vec<&str> {
    metrics.iter().map(|m| m.name.as_str()).collect()
}

#[allow(clippy::panic)] // Test helper.
fn number(value: &MetricValue) -> f64 {
    value
        .value
        .as_f64()
        .unwrap_or_else(|| panic!("no number: {value:?}"))
}

fn scalar(metrics: &[Metric], name: &str) -> f64 {
    number(&metric(metrics, name).values[0])
}

#[allow(clippy::panic)] // Test helper.
fn row(metrics: &[Metric], name: &str, key: &str, word: &str) -> f64 {
    let found = metric(metrics, name).values.iter().find(|value| {
        value
            .labels
            .as_ref()
            .is_some_and(|labels| labels.get(key).is_some_and(|v| v == word))
    });
    number(found.unwrap_or_else(|| panic!("{name} has no {key}={word}")))
}

/// Part 1: created at 0 s, held to 10 s, 20 s in the buffer, in the machine 30 to 60 s,
/// delivered at 60 s. Part 2: created at 40 s, held to 50 s, still in the buffer at the
/// end, 100 s.
fn worked_trace() -> Vec<u8> {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(0, 1, SOURCE);
    hand.note(0, SOURCE, &source_note(SourceState::Blocked, 1));
    hand.moved(10, 1, SOURCE, BUF, &arrive());
    hand.note(10, SOURCE, &source_note(SourceState::Waiting, 0));
    hand.moved(30, 1, BUF, CUT, &arrive());
    hand.note(30, CUT, &machine_note(MachineState::Busy, 1));
    hand.created(40, 2, SOURCE);
    hand.note(40, SOURCE, &source_note(SourceState::Blocked, 1));
    hand.moved(50, 2, SOURCE, BUF, &arrive());
    hand.note(50, SOURCE, &source_note(SourceState::Waiting, 0));
    hand.moved(60, 1, CUT, SINK, &arrive());
    hand.note(60, CUT, &machine_note(MachineState::Idle, 0));
    hand.disposed(60, 1, SINK, Disposal::Delivered);
    hand.finish(100)
}

#[test]
fn every_kpi_by_hand() {
    let (metrics, unreported) = fold(&line_model("", 1), &worked_trace());
    assert_eq!(unreported, []);
    // 1 delivered in 100 s.
    assert_eq!(scalar(&metrics, "throughput(sink)"), 0.01);
    // Held: 10 s and 10 s over 100 s.
    assert_eq!(scalar(&metrics, "held(source)"), 0.2);
    // Buffer: part 1 for 10..30, part 2 for 50..100, so 70 s over 100 s.
    assert_eq!(scalar(&metrics, "wip(buf)"), 0.7);
    // Machine: part 1 for 30..60.
    assert_eq!(scalar(&metrics, "wip(cut)"), 0.3);
    // Released and not disposed of: part 1 for 10..60, part 2 for 50..100.
    assert_eq!(scalar(&metrics, "wip()"), 1.0);
    // One unit in a cycle for 30 s.
    assert_eq!(scalar(&metrics, "utilization(cut)"), 0.3);
    assert_eq!(row(&metrics, "state_shares(cut)", "state", "busy"), 0.3);
    assert_eq!(row(&metrics, "state_shares(cut)", "state", "idle"), 0.7);
    assert_eq!(row(&metrics, "state_shares(cut)", "state", "down"), 0.0);
    assert_eq!(
        row(&metrics, "state_shares(source)", "state", "blocked"),
        0.2
    );
    // Release at 10 s to delivery at 60 s.
    for word in ["mean", "min", "p50", "p90", "p95", "p99", "max"] {
        assert_eq!(
            row(&metrics, "lead_time(sink)", "statistic", word),
            50.0,
            "{word}"
        );
    }
    assert_eq!(metric(&metrics, "lead_time(sink)").unit, "s");
    assert_eq!(metric(&metrics, "throughput(sink)").unit, "items/s");
    assert_eq!(metric(&metrics, "state_shares(cut)").unit, "share");
    assert_eq!(metric(&metrics, "utilization(cut)").unit, "share");
    assert_eq!(metric(&metrics, "wip(buf)").unit, "items");
    assert_eq!(metric(&metrics, "held(source)").unit, "items");
}

/// The new rows follow `items_alive`, sorted by KPI name and then in model order.
#[test]
fn rows_are_in_their_order() {
    let (metrics, _) = fold(&line_model("", 1), &worked_trace());
    assert_eq!(
        names(&metrics),
        [
            "created(source)",
            "delivered(sink)",
            "items_alive",
            "held(source)",
            "lead_time(sink)",
            "state_shares(source)",
            "state_shares(cut)",
            "throughput(sink)",
            "utilization(cut)",
            "wip()",
            "wip(buf)",
            "wip(cut)",
        ]
    );
}

/// A KPI's rows tie to M7's counters: at the end, items alive is WIP and held.
#[test]
fn units_inside_at_the_end_are_stated_on_lead_time() {
    let (metrics, _) = fold(&line_model("", 1), &worked_trace());
    assert_eq!(scalar(&metrics, "items_alive"), 1.0);
    let values = &metric(&metrics, "lead_time(sink)").values;
    for value in values {
        assert_eq!(value.accuracy.len(), 2);
        let AccuracyKind::Censored { units_inside, .. } = &value.accuracy[1].kind else {
            panic!("{value:?}");
        };
        assert_eq!(*units_inside, 1.0);
    }
    // Only lead time has that entry.
    assert_eq!(metric(&metrics, "wip()").values[0].accuracy.len(), 1);
}

#[test]
fn censored_lead_time_says_so() {
    // Everything delivered: no such entry.
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(0, 1, SOURCE);
    hand.moved(5, 1, SOURCE, SINK, &arrive());
    hand.disposed(5, 1, SINK, Disposal::Delivered);
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(10));
    assert_eq!(
        metric(&metrics, "lead_time(sink)").values[0].accuracy.len(),
        1
    );
    assert_eq!(scalar(&metrics, "wip()"), 0.0);
    // A part still held is no unit a lead time has missed.
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(0, 1, SOURCE);
    hand.moved(5, 1, SOURCE, SINK, &arrive());
    hand.disposed(5, 1, SINK, Disposal::Delivered);
    hand.created(6, 2, SOURCE);
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(10));
    assert_eq!(
        metric(&metrics, "lead_time(sink)").values[0].accuracy.len(),
        1
    );
    assert_eq!(scalar(&metrics, "items_alive"), 1.0);
    assert_eq!(scalar(&metrics, "held(source)"), 0.9);
}

/// A blocked Source's held parts are in `held`, and not in `wip()`.
#[test]
fn a_blocked_sources_part_is_held_not_wip() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(0, 1, SOURCE);
    hand.note(0, SOURCE, &source_note(SourceState::Blocked, 1));
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(10));
    assert_eq!(scalar(&metrics, "held(source)"), 1.0);
    assert_eq!(scalar(&metrics, "wip()"), 0.0);
    assert_eq!(scalar(&metrics, "items_alive"), 1.0);
    assert_eq!(
        row(&metrics, "state_shares(source)", "state", "blocked"),
        1.0
    );
}

/// `units_in_cycle` is 0 while down, so a repair counts as no work.
#[test]
fn a_repair_is_no_work() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(0, 1, SOURCE);
    hand.moved(0, 1, SOURCE, CUT, &arrive());
    hand.note(0, CUT, &machine_note(MachineState::Busy, 1));
    hand.note(4, CUT, &machine_note(MachineState::Down, 0));
    hand.note(6, CUT, &machine_note(MachineState::Busy, 1));
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(10));
    // Busy 8 s of 10; down 2 s; the unit stays in the machine all the time.
    assert_eq!(scalar(&metrics, "utilization(cut)"), 0.8);
    assert_eq!(row(&metrics, "state_shares(cut)", "state", "down"), 0.2);
    assert_eq!(scalar(&metrics, "wip(cut)"), 1.0);
}

/// Moves at one instant change levels and integrate over no time.
#[test]
fn simultaneous_moves_take_no_time() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(3, 1, SOURCE);
    hand.moved(3, 1, SOURCE, BUF, &arrive());
    hand.moved(3, 1, BUF, CUT, &arrive());
    hand.moved(3, 1, CUT, SINK, &arrive());
    hand.disposed(3, 1, SINK, Disposal::Delivered);
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(10));
    assert_eq!(scalar(&metrics, "wip()"), 0.0);
    assert_eq!(scalar(&metrics, "wip(buf)"), 0.0);
    assert_eq!(scalar(&metrics, "held(source)"), 0.0);
    assert_eq!(row(&metrics, "lead_time(sink)", "statistic", "max"), 0.0);
    assert_eq!(scalar(&metrics, "throughput(sink)"), 0.1);
}

#[test]
fn nearest_rank_quantiles_are_stated_values() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    // Lead times of 1, 2, 3, 4 s.
    for number in 1..=4_u64 {
        hand.created(number, number, SOURCE);
    }
    for number in 1..=4_u64 {
        let start = 10 * number;
        hand.moved(start, number, SOURCE, SINK, &arrive());
        hand.disposed(start + number, number, SINK, Disposal::Delivered);
    }
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(100));
    let lead = |word| row(&metrics, "lead_time(sink)", "statistic", word);
    assert_eq!(lead("min"), 1.0);
    assert_eq!(lead("mean"), 2.5);
    // ⌈0.5·4⌉ = 2, ⌈0.9·4⌉ = 4.
    assert_eq!(lead("p50"), 2.0);
    assert_eq!(lead("p90"), 4.0);
    assert_eq!(lead("p95"), 4.0);
    assert_eq!(lead("p99"), 4.0);
    assert_eq!(lead("max"), 4.0);
}

/// M7's metrics keep their names, order and bytes; the numbers are the ones its CLI
/// snapshots pin for this model and seed.
#[test]
fn m7_rows_are_unchanged() {
    let yaml = "format: \"1.0\"\nmodel: {name: line-a, seed: 42}\nexperiment: {type: terminating, horizon: 8h}\n\
        components:\n  - {id: source, type: Source, interarrival: {dist: exponential, mean: 45s}}\n  \
        - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}}\n  \
        - {id: buf1, type: Buffer, capacity: 20 parts}\n  \
        - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: sink, type: Sink}\n\
        flow:\n  - [source, cut, buf1, weld, sink]\n";
    let dir = TempDir::new();
    let result = run_text(yaml, &RunRequest::new(), &dir.store())
        .unwrap()
        .result;
    let window = "\"over\":{\"from\":\"0s\",\"to\":\"8h\"}";
    let accuracy = "\"accuracy\":[{\"detail\":\"one replication has no interval; run several seeds\",\"kind\":\"none\",\"reason\":\"single_replication\"}]";
    let created = result.metrics[0].values[0].value;
    let delivered = result.metrics[1].values[0].value;
    let alive = result.metrics[2].values[0].value;
    let (Scalar::Integer(created), Scalar::Integer(delivered), Scalar::Integer(alive)) =
        (created, delivered, alive)
    else {
        panic!("counts are integers");
    };
    assert_eq!(created, delivered + alive);
    assert_eq!(
        result.metrics[0].encode().to_string(),
        format!(
            "{{\"name\":\"created(source)\",{window},\"unit\":\"items\",\"values\":[{{{accuracy},\"value\":{created}}}]}}"
        )
    );
    assert_eq!(
        result.metrics[1].encode().to_string(),
        format!(
            "{{\"name\":\"delivered(sink)\",{window},\"unit\":\"items\",\"values\":[{{{accuracy},\"value\":{delivered}}}]}}"
        )
    );
    assert_eq!(
        result.metrics[2].encode().to_string(),
        format!(
            "{{\"name\":\"items_alive\",\"over\":{{\"at\":\"8h\"}},\"unit\":\"items\",\"values\":[{{{accuracy},\"value\":{alive}}}]}}"
        )
    );
    assert_eq!(result.metrics[3].name, "held(source)");
}

/// Page 1's table, through the whole pipeline, from an on-demand Source.
#[test]
fn page_1_table() {
    let yaml = "format: \"1.0\"\nmodel: {name: line-a}\nexperiment: {type: terminating, horizon: 8h}\n\
        components:\n  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 42s}\n  \
        - {id: buf1, type: Buffer, capacity: 20 parts}\n  - {id: weld, type: Machine, cycle_time: 45s}\n  \
        - {id: sink, type: Sink}\nflow:\n  - [source, cut, buf1, weld, sink]\n";
    let dir = TempDir::new();
    let metrics = run_text(yaml, &RunRequest::new(), &dir.store())
        .unwrap()
        .result
        .metrics;
    assert_eq!(scalar(&metrics, "delivered(sink)"), 639.0);
    // 639 parts in 8 h.
    assert_eq!(scalar(&metrics, "throughput(sink)"), 639.0 / 28_800.0);
    // The first part leaves after 42 s + 45 s; one that enters the full line needs
    // 16.5 minutes.
    let lead = |word| row(&metrics, "lead_time(sink)", "statistic", word);
    assert_eq!(lead("min"), 87.0);
    assert_eq!(lead("max"), 990.0);
    // At the end the line holds 22 parts: 20 in the buffer and one in each station.
    let values = &metric(&metrics, "lead_time(sink)").values;
    let AccuracyKind::Censored { units_inside, .. } = &values[0].accuracy[1].kind else {
        panic!("{values:?}");
    };
    assert_eq!(*units_inside, 22.0);
    // Welding works without a pause from the first part on, and its busy share has no
    // room for anything but the first 42 s.
    assert_eq!(
        row(&metrics, "state_shares(weld)", "state", "busy"),
        (28_800.0 - 42.0) / 28_800.0
    );
    assert_eq!(
        row(&metrics, "state_shares(weld)", "state", "busy"),
        scalar(&metrics, "utilization(weld)")
    );
}

/// ADR-0022's check at capacity 2: utilization + down = 1. The numbers here are worked
/// by hand; ADR-0022 reports 0.956 + 0.044 for one seed of a model it does not list.
#[test]
fn capacity_two_utilization_and_down_sum_to_one() {
    // Two units at a time, 100 s each, up 150 s, down 50 s, saturated, to 600 s.
    // Failures at 150, 350, 550; each for 50 s, the last cut at 600 s: 150 s of 600.
    let yaml = model_text_with(
        "600s",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: 100s, capacity: 2 parts, mtbf: 150s, mttr: 50s}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let dir = TempDir::new();
    let metrics = run_text(&yaml, &RunRequest::new(), &dir.store())
        .unwrap()
        .result
        .metrics;
    assert_eq!(row(&metrics, "state_shares(m)", "state", "down"), 0.25);
    assert_eq!(scalar(&metrics, "utilization(m)"), 0.75);

    // Random failures and cycles, 1 h 3 min, saturated, one seed: still 1.
    let yaml = model_text_with(
        "63min",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: {dist: exponential, mean: 1min}, capacity: 2 parts, \
         mtbf: {dist: exponential, mean: 40min}, mttr: {dist: exponential, mean: 2min}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let request = RunRequest {
        seeds: &[9],
        ..RunRequest::new()
    };
    let metrics = run_text(&yaml, &request, &dir.store())
        .unwrap()
        .result
        .metrics;
    let sum =
        row(&metrics, "state_shares(m)", "state", "down") + scalar(&metrics, "utilization(m)");
    assert!((sum - 1.0).abs() < 1e-12, "{sum}");
    assert!(row(&metrics, "state_shares(m)", "state", "down") > 0.0);
}

fn model_text_with(horizon: &str, components: &str, flow: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: test}}\nexperiment: {{type: terminating, horizon: {horizon}}}\n\
         components:\n{components}\nflow:\n{flow}\n"
    )
}

/// Any unit in a cycle makes a machine `busy`, so at capacity 2 they differ, and at
/// capacity 1 they are equal.
#[test]
fn utilization_is_not_the_busy_share() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.note(0, CUT, &machine_note(MachineState::Busy, 2));
    hand.note(10, CUT, &machine_note(MachineState::Busy, 1));
    hand.note(20, CUT, &machine_note(MachineState::Idle, 0));
    let (metrics, _) = fold(&line_model("", 2), &hand.finish(40));
    // Busy 20 s of 40 s. Unit-seconds 2·10 + 1·10 = 30, of 2·40 = 80.
    assert_eq!(row(&metrics, "state_shares(cut)", "state", "busy"), 0.5);
    assert_eq!(scalar(&metrics, "utilization(cut)"), 0.375);

    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.note(0, CUT, &machine_note(MachineState::Busy, 1));
    hand.note(20, CUT, &machine_note(MachineState::Idle, 0));
    let (metrics, _) = fold(&line_model("", 1), &hand.finish(40));
    assert_eq!(
        row(&metrics, "state_shares(cut)", "state", "busy"),
        scalar(&metrics, "utilization(cut)")
    );

    // M/M/2 at ρ = 0.8: P(N ≥ 1) = 8/9, and utilization 0.8.
    let yaml = model_text_with(
        "20000s",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1s}}\n  \
         - {id: buf, type: Buffer, capacity: 1000 parts}\n  \
         - {id: m, type: Machine, cycle_time: {dist: exponential, mean: 1.6s}, capacity: 2 parts}\n  \
         - {id: sink, type: Sink}",
        "  - [source, buf, m, sink]",
    );
    let dir = TempDir::new();
    let request = RunRequest {
        seeds: &[2],
        ..RunRequest::new()
    };
    let metrics = run_text(&yaml, &request, &dir.store())
        .unwrap()
        .result
        .metrics;
    let busy = row(&metrics, "state_shares(m)", "state", "busy");
    let utilization = scalar(&metrics, "utilization(m)");
    // One run of this length spreads by about 0.015 around each (four seeds gave 0.78 to
    // 0.82), so 0.05 is more than three of that, and the gap is what the test is about.
    assert!((busy - 8.0 / 9.0).abs() < 0.05, "{busy}");
    assert!((utilization - 0.8).abs() < 0.05, "{utilization}");
    assert!(busy - utilization > 0.05, "{busy} {utilization}");
}

/// Each `state_shares` row list is its type's, in its order, and always all of them.
#[test]
fn state_rows_follow_the_type() {
    let (metrics, _) = fold(&line_model("", 1), &worked_trace());
    let labels = |name: &str| -> Vec<(String, String)> {
        metric(&metrics, name)
            .values
            .iter()
            .map(|v| {
                let labels = v.labels.as_ref().unwrap();
                (labels["resource"].clone(), labels["state"].clone())
            })
            .collect()
    };
    let of = |resource: &str, states: &[&str]| -> Vec<(String, String)> {
        states
            .iter()
            .map(|s| (resource.to_owned(), (*s).to_owned()))
            .collect()
    };
    assert_eq!(
        labels("state_shares(cut)"),
        of(
            "cut",
            &["idle", "busy", "blocked", "starved", "down", "setup"]
        )
    );
    assert_eq!(
        labels("state_shares(source)"),
        of("source", &["waiting", "blocked"])
    );
}

/// A note with a state its type did not declare is exit 2.
#[test]
fn undeclared_state_is_refused() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    // A Buffer declares no state, and this says it is blocked.
    hand.note(1, BUF, &source_note(SourceState::Blocked, 0));
    let registry = standard().unwrap();
    let failure = metrics_of_trace(
        &registry,
        &load(&registry, &line_model("", 1)),
        hand.finish(10).as_slice(),
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let line = &failure.diagnostics[0];
    assert_eq!(line.type_name, "run.kpi_undeclared_state");
    assert!(line.message.contains("`blocked`"), "{}", line.message);
    assert!(line.message.contains("`Buffer`"), "{}", line.message);
    assert_eq!(line.recent.len(), 3);
}

/// A Machine creates nothing; a trace that says so is exit 2, whatever made it.
#[test]
fn what_breaks_conservation_is_refused_with_its_last_records() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(1, 1, CUT);
    let registry = standard().unwrap();
    let model = load(&registry, &line_model("", 1));
    let refused = |trace: Vec<u8>| -> Failure {
        metrics_of_trace(&registry, &model, trace.as_slice()).unwrap_err()
    };
    let failure = refused(hand.finish(10));
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.kpi_conservation");
    assert_eq!(failure.diagnostics[0].recent.len(), 3);

    // An item moved from where it is not.
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(1, 1, SOURCE);
    hand.moved(2, 1, BUF, CUT, &arrive());
    assert_eq!(
        refused(hand.finish(10)).diagnostics[0].type_name,
        "run.kpi_conservation"
    );

    // Disposed of twice.
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(1, 1, SOURCE);
    hand.moved(2, 1, SOURCE, SINK, &arrive());
    hand.disposed(3, 1, SINK, Disposal::Delivered);
    hand.disposed(4, 1, SINK, Disposal::Delivered);
    assert_eq!(
        refused(hand.finish(10)).diagnostics[0].type_name,
        "run.kpi_conservation"
    );
}

/// A trace with no end marker has no window: refused, as ADR-0022 §9 says.
#[test]
fn a_trace_that_states_no_end_is_refused() {
    let hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    let (bytes, _) = hand.writer.finish().unwrap();
    let registry = standard().unwrap();
    let failure = metrics_of_trace(
        &registry,
        &load(&registry, &line_model("", 1)),
        bytes.as_slice(),
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.kpi_no_end");
}

/// A trace that cannot be read is an I/O failure, not a broken run.
#[test]
fn a_trace_that_does_not_read_is_an_io_failure() {
    let registry = standard().unwrap();
    let model = load(&registry, &line_model("", 1));
    let failure = metrics_of_trace(&registry, &model, &b"not a trace"[..]).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    let bytes = worked_trace();
    let failure = metrics_of_trace(&registry, &model, &bytes[..bytes.len() - 3]).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
}

/// A window with no time in it has no time-weighted row, and goes through the fold. A
/// declared one is in `unreported`.
#[test]
fn a_run_cut_at_zero_has_no_time_weighted_rows() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    hand.created(0, 1, SOURCE);
    let yaml = line_model("", 1).replace(
        "flow:\n  - [source, buf, cut, sink]\n",
        "flow:\n  - [source, buf, cut, sink]\nobserve:\n  - wip(buf)\n  - held(source)\n",
    );
    let (metrics, unreported) = fold(&yaml, &hand.finish(0));
    assert_eq!(
        names(&metrics),
        ["created(source)", "delivered(sink)", "items_alive"]
    );
    assert_eq!(scalar(&metrics, "items_alive"), 1.0);
    let missing: Vec<(&str, &str)> = unreported
        .iter()
        .map(|u| (u.observable.as_str(), u.reason.as_str()))
        .collect();
    assert_eq!(
        missing,
        [("held(source)", "undefined"), ("wip(buf)", "undefined")]
    );
}

/// With nothing delivered there is no lead time: only that row is left out. Everything
/// else has a true value: a Sink that received nothing has a throughput of 0.
#[test]
fn only_a_lead_time_with_no_delivery_is_undefined() {
    let mut hand = Hand::<FlowMessage>::new(&["source", "buf", "cut", "sink"]);
    start_notes(&mut hand);
    let yaml = line_model("", 1).replace(
        "flow:\n  - [source, buf, cut, sink]\n",
        "flow:\n  - [source, buf, cut, sink]\nobserve:\n  - lead_time(sink)\n  - throughput(sink)\n  - held(source)\n",
    );
    let (metrics, unreported) = fold(&yaml, &hand.finish(10));
    assert!(!names(&metrics).contains(&"lead_time(sink)"));
    assert_eq!(scalar(&metrics, "throughput(sink)"), 0.0);
    assert_eq!(scalar(&metrics, "held(source)"), 0.0);
    assert_eq!(unreported.len(), 1);
    assert_eq!(unreported[0].observable, "lead_time(sink)");
    assert_eq!(unreported[0].reason, "undefined");
}

/// Each declared text is on each metric that answers it, in canonical form, sorted, and
/// a metric nothing asks for says nothing.
#[test]
fn declared_observables_tag_their_metrics() {
    let yaml = line_model("", 1).replace(
        "flow:\n  - [source, buf, cut, sink]\n",
        "flow:\n  - [source, buf, cut, sink]\nobserve:\n  - state_shares( cut )\n  - state_shares(source, cut)\n  - wip(buf, cut)\n  - wip()\n",
    );
    let (metrics, unreported) = fold(&yaml, &worked_trace());
    assert_eq!(unreported, []);
    let tags = |name: &str| metric(&metrics, name).observables.clone();
    assert_eq!(
        tags("state_shares(cut)"),
        ["state_shares(cut)", "state_shares(source, cut)"]
    );
    assert_eq!(tags("state_shares(source)"), ["state_shares(source, cut)"]);
    assert_eq!(tags("wip(buf)"), ["wip(buf, cut)"]);
    assert_eq!(tags("wip(cut)"), ["wip(buf, cut)"]);
    assert_eq!(tags("wip()"), ["wip()"]);
    assert_eq!(tags("throughput(sink)"), [] as [&str; 0]);
    let text = metric(&metrics, "throughput(sink)").encode().to_string();
    assert!(!text.contains("observables"), "{text}");
}

/// A whole-line target loads, and so does `held`: every row can be asked for
/// (ADR-0025 §2).
#[test]
fn system_wip_and_held_can_be_targeted() {
    let registry = standard().unwrap();
    let with = |precision: &str| {
        line_model(", seed: 1", 1).replace(
            "experiment: {type: terminating, horizon: 1h}",
            &format!("experiment: {{type: terminating, horizon: 1h, precision: {{{precision}}}}}"),
        )
    };
    let model = load(&registry, &with("\"wip()\": 5%, \"held(source)\": 10%"));
    assert_eq!(model.document().experiment().precision().len(), 2);
    // `wip` of a Sink stays refused, and so is `held` of a Machine, and `state_shares()`.
    let loader = Loader::new(registry.catalog());
    for refused in [
        "\"wip(sink)\": 5%",
        "\"held(cut)\": 5%",
        "\"state_shares()\": 5%",
    ] {
        assert!(
            loader.load_str(&with(refused), Format::Yaml).is_err(),
            "{refused}"
        );
    }
}

/// Every KPI's metric name is the canonical text of an observable that answers it.
#[test]
fn every_kpi_name_is_an_observable_text() {
    let registry = standard().unwrap();
    let (metrics, _) = fold(&line_model("", 1), &worked_trace());
    let kpis: Vec<&str> = names(&metrics)
        .into_iter()
        .skip_while(|name| *name != "items_alive")
        .skip(1)
        .collect();
    assert_eq!(kpis.len(), 9);
    for name in kpis {
        let yaml = line_model("", 1).replace(
            "flow:\n  - [source, buf, cut, sink]\n",
            &format!("flow:\n  - [source, buf, cut, sink]\nobserve:\n  - {name}\n"),
        );
        let model = load(&registry, &yaml);
        assert_eq!(model.document().observe()[0].text(), name);
        let (metrics, unreported) =
            metrics_of_trace(&registry, &model, worked_trace().as_slice()).unwrap();
        assert_eq!(unreported, []);
        assert_eq!(metric(&metrics, name).observables, [name]);
    }
}

/// A test domain: a package with its own KPI words, types with their own states, and a
/// message that states two classes.
struct Ward;

impl Domain for Ward {
    type Message = FlowMessage;
    type Shared = ();
    const PACKAGE: &'static str = "ward";
    const PACKAGE_VERSION: &'static str = "1.0.0";

    fn check(_: &Document, _: &Roles, _: &mut Findings) {}

    fn prepare(_: &Document, _: &Roles) -> Result<(), BuildError> {
        Ok(())
    }

    fn draws_random(_: &Document) -> bool {
        false
    }

    /// Two classes on each arrival, named as the Flow domain names one.
    fn read_message(message: &FlowMessage, facts: &mut dyn FnMut(MessageFact<'_>)) {
        match message {
            FlowMessage::Arrive(attrs) => {
                facts(MessageFact::Class {
                    dimension: "product",
                    value: attrs.variant.get(),
                });
                facts(MessageFact::Class {
                    dimension: "material",
                    value: u16::from(attrs.priority),
                });
            }
            FlowMessage::VariantName { id, name } => facts(MessageFact::ClassName {
                dimension: "product",
                value: id.get(),
                name,
            }),
            FlowMessage::MachineState(note) => facts(MessageFact::State {
                name: note.state.as_str(),
                in_progress: note.units_in_cycle,
            }),
            FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::CycleDone { .. }
            | FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::Accept
            | FlowMessage::Decline
            | FlowMessage::Offer
            | FlowMessage::Withdraw
            | FlowMessage::SourceState(_) => {}
        }
    }
}

/// `type: Bed` in the ward: a place with its own states.
#[derive(Debug)]
struct Bed;

impl Encode for Bed {
    fn encode(&self) -> Json {
        Json::object([])
    }
}

impl taktsim_model::ComponentParams for Bed {
    const TYPE: &'static str = "Bed";
    const EXAMPLE: &'static str = "{id: b, type: Bed}";
    fn decode(_: &mut taktsim_model::Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

impl ComponentType<Ward> for Bed {
    const STATES: &'static [&'static str] = &["idle", "busy"];
    const CREATION: Creation = Creation::Never;

    fn places(&self) -> u32 {
        4
    }

    fn build(
        &self,
        _: &Wiring<'_, ()>,
    ) -> Result<Box<dyn taktsim_core::Component<FlowMessage>>, BuildError> {
        Err(never_built())
    }
}

fn never_built() -> BuildError {
    BuildError::UnknownVariant {
        path: taktsim_model::FieldPath::default(),
        name: "never".to_owned(),
        declared: Vec::new(),
    }
}

impl ComponentType<Ward> for Source {
    const PORTS: taktsim_components::Ports = taktsim_components::Ports::Outlet;
    const STATES: &'static [&'static str] = &[];
    const CREATION: Creation = Creation::InSystem;

    fn places(&self) -> u32 {
        1
    }

    fn build(
        &self,
        _: &Wiring<'_, ()>,
    ) -> Result<Box<dyn taktsim_core::Component<FlowMessage>>, BuildError> {
        Err(never_built())
    }
}

impl ComponentType<Ward> for Sink {
    const PORTS: taktsim_components::Ports = taktsim_components::Ports::Inlet;
    const STATES: &'static [&'static str] = &[];
    const CREATION: Creation = Creation::Never;

    fn places(&self) -> u32 {
        1
    }

    fn build(
        &self,
        _: &Wiring<'_, ()>,
    ) -> Result<Box<dyn taktsim_core::Component<FlowMessage>>, BuildError> {
        Err(never_built())
    }
}

#[allow(clippy::unwrap_used)] // Test helper.
fn ward() -> Registry<Ward> {
    let census = Signature::new(Arity::Exactly(1), Type::Number).accepting(["Bed"]);
    let discharges = Signature::new(Arity::Exactly(1), Type::Number).accepting(["Sink"]);
    let load = Signature::new(Arity::Exactly(1), Type::Number).accepting(["Bed"]);
    Registry::<Ward>::new()
        .with::<Source>()
        .unwrap()
        .with::<Bed>()
        .unwrap()
        .with::<Sink>()
        .unwrap()
        .with_kpi("census", census, Kpi::Occupancy)
        .unwrap()
        .with_kpi("discharges", discharges, Kpi::Deliveries)
        .unwrap()
        .with_kpi("load", load, Kpi::InProgress)
        .unwrap()
}

const WARD: &str = "format: \"1.0\"\nmodel: {name: ward}\nexperiment: {type: terminating, horizon: 1h}\n\
     components:\n  - {id: door, type: Source}\n  - {id: bed, type: Bed}\n  - {id: home, type: Sink}\n\
     flow:\n  - [door, bed, home]\n";

fn ward_arrive(variant: u16, priority: u8) -> FlowMessage {
    FlowMessage::Arrive(ItemAttrs::new(VariantId::new(variant), priority))
}

/// A package gets its rows under its own words, and none it did not register.
#[test]
fn a_package_gets_only_the_kpis_it_registers() {
    let registry = ward();
    let model = load(&registry, WARD);
    let mut hand = Hand::<FlowMessage>::new(&["door", "bed", "home"]);
    hand.note(0, 1, &machine_note(MachineState::Busy, 2));
    hand.created(0, 1, 0);
    hand.moved(0, 1, 0, 1, &ward_arrive(0, 0));
    hand.moved(5, 1, 1, 2, &ward_arrive(0, 0));
    hand.disposed(5, 1, 2, Disposal::Delivered);
    let (metrics, _) = metrics_of_trace(&registry, &model, hand.finish(10).as_slice()).unwrap();
    assert_eq!(
        names(&metrics),
        [
            "created(door)",
            "delivered(home)",
            "items_alive",
            "census(bed)",
            "discharges(home)",
            "load(bed)",
        ]
    );
    // InSystem at creation, then one in the bed for 5 s of 10 s.
    assert_eq!(scalar(&metrics, "census(bed)"), 0.5);
    assert_eq!(scalar(&metrics, "discharges(home)"), 0.1);
    // 2 units all the time, of 4 places.
    assert_eq!(scalar(&metrics, "load(bed)"), 0.5);
}

/// A message can state several classes, so per-product and per-material rows need no
/// second label: one row set per dimension with two or more classes.
#[test]
fn two_dimensions_two_row_sets() {
    let registry = ward();
    let model = load(&registry, WARD);
    let mut hand = Hand::<FlowMessage>::new(&["door", "bed", "home"]);
    hand.note(
        0,
        0,
        &FlowMessage::VariantName {
            id: VariantId::new(0),
            name: "alpha".to_owned(),
        },
    );
    hand.note(
        0,
        0,
        &FlowMessage::VariantName {
            id: VariantId::new(1),
            name: "beta".to_owned(),
        },
    );
    hand.note(0, 1, &machine_note(MachineState::Idle, 0));
    // Product alpha twice, beta once; material 7 twice and 8 once.
    for number in 1..=3 {
        hand.created(0, number, 0);
    }
    for (number, variant, material) in [(1, 0, 7), (2, 1, 7), (3, 0, 8)] {
        hand.moved(1, number, 0, 2, &ward_arrive(variant, material));
        hand.disposed(1, number, 2, Disposal::Delivered);
    }
    let (metrics, _) = metrics_of_trace(&registry, &model, hand.finish(10).as_slice()).unwrap();
    let rows: BTreeMap<String, f64> = metric(&metrics, "discharges(home)")
        .values
        .iter()
        .map(|v| {
            let label = v.labels.as_ref().map_or_else(String::new, |labels| {
                labels
                    .iter()
                    .map(|(k, w)| format!("{k}={w}"))
                    .collect::<Vec<_>>()
                    .join(",")
            });
            (label, number(v))
        })
        .collect();
    assert_eq!(
        rows,
        BTreeMap::from([
            (String::new(), 0.3),
            ("material=7".to_owned(), 0.2),
            ("material=8".to_owned(), 0.1),
            ("product=alpha".to_owned(), 0.2),
            ("product=beta".to_owned(), 0.1),
        ])
    );
    // The total comes first, then by dimension and name.
    let first = &metric(&metrics, "discharges(home)").values[0];
    assert_eq!(first.labels, None);
}

/// In Flow, a Source names every variant at the start, so a class not delivered still
/// has its row, and all runs of a model have the same rows.
#[test]
fn a_named_class_has_a_row_with_zero() {
    let yaml = model_text_with(
        "10s",
        "  - {id: source, type: Source, interarrival: 1h, variants: {a: 50%, b: 50%}}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let dir = TempDir::new();
    let request = RunRequest {
        seeds: &[1],
        ..RunRequest::new()
    };
    let metrics = run_text(&yaml, &request, &dir.store())
        .unwrap()
        .result
        .metrics;
    let labels: Vec<_> = metric(&metrics, "throughput(sink)")
        .values
        .iter()
        .map(|v| v.labels.clone())
        .collect();
    let of = |name: &str| Some(BTreeMap::from([("variant".to_owned(), name.to_owned())]));
    assert_eq!(labels, [None, of("a"), of("b")]);
    // One variant is no dimension to split.
    let one = model_text_with(
        "10s",
        "  - {id: source, type: Source, interarrival: 1s}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let metrics = run_text(&one, &request, &dir.store())
        .unwrap()
        .result
        .metrics;
    assert_eq!(metric(&metrics, "throughput(sink)").values.len(), 1);
}

/// Types the test registry adds have the rows their signatures give them, and what a
/// type declares about creating decides `held` and `wip`.
#[test]
fn creation_is_what_the_creator_declares() {
    let yaml = model_text(
        "",
        "  - {id: burst, type: Burst, count: 2}\n  - {id: relay, type: Relay}\n  \
         - {id: keep, type: Keep, how: delivered}\n  - {id: lonely, type: Lonely}",
        "  - [burst, relay, keep]",
    );
    let dir = TempDir::new();
    let result = run_with(&test_registry(), &yaml, &RunRequest::new(), &dir.store())
        .unwrap()
        .result;
    let names = names(&result.metrics);
    assert!(names.contains(&"wip()"), "{names:?}");
    assert!(!names.contains(&"wip(keep)"), "{names:?}");
    assert_eq!(scalar(&result.metrics, "items_alive"), 0.0);
}

/// A document with every kind of KPI reads back as itself: whole numbers come back as
/// integers, which [`Scalar::measured`] states up front.
#[test]
fn a_document_with_kpis_reads_back_as_itself() {
    let yaml = line_model(", seed: 3", 2).replace(
        "- {id: source, type: Source}",
        "- {id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}",
    );
    let dir = TempDir::new();
    let result = run_text(&yaml, &RunRequest::new(), &dir.store())
        .unwrap()
        .result;
    let text = result.to_text();
    let read: taktsim_run::ResultDocument = taktsim_model::decode_str(&text, Format::Json).unwrap();
    assert_eq!(read, result);
    assert_eq!(read.to_text(), text);
}
