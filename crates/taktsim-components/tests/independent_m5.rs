//! Independent tests of M5. Every expected value here was derived by hand or
//! from theory, from `docs/spec.md`, ADR-0022 and the concept pages, without
//! reading the implementation; the derivation is in the comment by each test.
//!
//! Conventions of the derivations:
//!
//! - Time is in whole seconds in the hand-counted cases, so a timeline is exact.
//! - A horizon is inclusive: an event due at the horizon runs (`RunOutcome`:
//!   "the next event is after the horizon").
//! - A *settled* state is the last state written at an instant. A zero-length
//!   state within an instant (`blocked` for zero time, ADR-0022 §5) is not in a
//!   settled timeline, and a timeline lists only the changes.

// Tests may unwrap and panic (CLAUDE.md); clippy's test exemption covers
// `#[test]` bodies, not the helpers of an integration test.
#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::atomic::AtomicBool;

use proptest::prelude::*;
use taktsim_components::{
    Flow, FlowMessage, ItemAttrs, MachineState, MachineStateNote, Registry, RunFailure, RunOptions,
    RunReport, SourceState, SourceStateNote, VariantId, standard,
};
use taktsim_core::{ComponentId, Disposal, EngineConfig, ItemId, Message, RunOutcome};
use taktsim_model::{Format, LoadErrors, Loader, Model};
use taktsim_trace::{Record, TraceMessage, TraceReader, encode_message, replay};
use taktsim_units::{SimDuration, SimTime};

const NS: u64 = 1_000_000_000;

/// A timeline: at each listed time, the state changes to the one named.
type Timeline = Vec<(u64, &'static str)>;

fn secs(t: SimTime) -> u64 {
    let nanos = t.as_nanos();
    assert_eq!(nanos % NS, 0, "{t} is not a whole second");
    nanos / NS
}

fn count_f64(n: u64) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

fn nanos_f64(nanos: u64) -> f64 {
    SimDuration::from_nanos(nanos).as_secs_f64()
}

fn model(horizon: &str, components: &[&str], flow: &[&str]) -> String {
    let mut text = format!(
        "format: \"1.0\"\nmodel: {{name: t, seed: 1}}\nexperiment: {{type: terminating, horizon: {horizon}}}\ncomponents:\n"
    );
    for component in components {
        text.push_str("  - ");
        text.push_str(component);
        text.push('\n');
    }
    text.push_str("flow:\n");
    for edge in flow {
        text.push_str("  - ");
        text.push_str(edge);
        text.push('\n');
    }
    text
}

fn registry() -> Registry<Flow> {
    standard().unwrap()
}

fn load(text: &str) -> Result<Model, LoadErrors> {
    Loader::new(registry().catalog()).load_str(text, Format::Yaml)
}

fn load_ok(text: &str) -> Model {
    load(text).unwrap_or_else(|errors| panic!("this model is valid: {errors}\n{text}"))
}

/// One finished run with its records read back from its own trace.
struct Run {
    names: Vec<String>,
    records: Vec<Record<FlowMessage>>,
    report: RunReport,
    bytes: Vec<u8>,
}

fn run_model(m: &Model, options: RunOptions<'_>) -> Run {
    let registry = registry();
    let (bytes, report) = registry
        .prepare(m, options)
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    let mut reader = TraceReader::<_, FlowMessage>::new(bytes.as_slice()).unwrap();
    let names = reader.header().components.clone();
    let mut records = Vec::new();
    while let Some(record) = reader.next_record().unwrap() {
        records.push(record);
    }
    Run {
        names,
        records,
        report,
        bytes,
    }
}

fn run(text: &str) -> Run {
    run_model(&load_ok(text), RunOptions::new())
}

fn run_seeded(text: &str, seed: u64) -> Run {
    run_model(&load_ok(text), RunOptions::new().seed(seed))
}

impl Run {
    fn index(&self, name: &str) -> usize {
        self.names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no component `{name}`"))
    }

    fn name(&self, id: ComponentId) -> &str {
        &self.names[id.index()]
    }

    /// When each part was delivered, in order.
    fn deliveries(&self) -> Vec<u64> {
        self.records
            .iter()
            .filter_map(|record| match record {
                Record::ItemDisposed { time, how, .. } => {
                    assert_eq!(*how, Disposal::Delivered);
                    Some(secs(*time))
                }
                Record::Event { .. } | Record::ItemCreated { .. } | Record::Note { .. } => None,
            })
            .collect()
    }

    /// When each part was created, in order.
    fn created(&self) -> Vec<u64> {
        self.records
            .iter()
            .filter_map(|record| match record {
                Record::ItemCreated { time, .. } => Some(secs(*time)),
                Record::Event { .. } | Record::ItemDisposed { .. } | Record::Note { .. } => None,
            })
            .collect()
    }

    /// When an event matching `pick` was delivered to `name`.
    fn events_at(&self, name: &str, pick: fn(&FlowMessage) -> bool) -> Vec<u64> {
        let at = self.index(name);
        self.records
            .iter()
            .filter_map(|record| match record {
                Record::Event {
                    time,
                    target,
                    message,
                    ..
                } if target.index() == at && pick(message) => Some(secs(*time)),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. } => None,
            })
            .collect()
    }

    /// When a part landed at `name`.
    fn arrivals_at(&self, name: &str) -> Vec<u64> {
        self.events_at(name, |m| matches!(m, FlowMessage::Arrive(_)))
    }

    /// When each changeover at `name` finished.
    fn setups_done(&self, name: &str) -> Vec<u64> {
        self.events_at(name, |m| matches!(m, FlowMessage::SetupDone))
    }

    fn event_count(&self) -> usize {
        self.records
            .iter()
            .filter(|r| matches!(r, Record::Event { .. }))
            .count()
    }

    fn note_count(&self) -> usize {
        self.records
            .iter()
            .filter(|r| matches!(r, Record::Note { .. }))
            .count()
    }

    fn machine_notes(&self, name: &str) -> Vec<(SimTime, MachineStateNote)> {
        let at = self.index(name);
        self.records
            .iter()
            .filter_map(|record| match record {
                Record::Note {
                    time,
                    by,
                    message: FlowMessage::MachineState(note),
                } if by.index() == at => Some((*time, *note)),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. } => None,
            })
            .collect()
    }

    fn source_notes(&self, name: &str) -> Vec<(SimTime, SourceStateNote)> {
        let at = self.index(name);
        self.records
            .iter()
            .filter_map(|record| match record {
                Record::Note {
                    time,
                    by,
                    message: FlowMessage::SourceState(note),
                } if by.index() == at => Some((*time, *note)),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. } => None,
            })
            .collect()
    }

    /// The settled state of a machine at each instant, changes only.
    fn timeline(&self, name: &str) -> Timeline {
        settled(
            self.machine_notes(name)
                .iter()
                .map(|(t, n)| (*t, n.state.as_str())),
        )
    }

    /// The settled count of units in a cycle, changes only.
    fn units(&self, name: &str) -> Vec<(u64, u32)> {
        settled(
            self.machine_notes(name)
                .iter()
                .map(|(t, n)| (*t, n.units_in_cycle)),
        )
    }

    fn source_timeline(&self, name: &str) -> Timeline {
        settled(
            self.source_notes(name)
                .iter()
                .map(|(t, n)| (*t, n.state.as_str())),
        )
    }

    fn source_held(&self, name: &str) -> Vec<(u64, u32)> {
        settled(self.source_notes(name).iter().map(|(t, n)| (*t, n.held)))
    }

    /// Seconds a machine spent in each state, from its notes to the run's end.
    fn shares(&self, name: &str) -> BTreeMap<&'static str, u64> {
        let notes = self.machine_notes(name);
        let end = self.report.summary.now;
        let mut totals = BTreeMap::new();
        for (index, (time, note)) in notes.iter().enumerate() {
            let until = notes.get(index + 1).map_or(end, |(next, _)| *next);
            let spent = until.as_nanos() - time.as_nanos();
            *totals.entry(note.state.as_str()).or_insert(0) += spent;
        }
        totals
            .into_iter()
            .filter(|(_, nanos)| *nanos > 0)
            .map(|(state, nanos)| {
                assert_eq!(nanos % NS, 0);
                (state, nanos / NS)
            })
            .collect()
    }
}

/// The last value at each instant, with repeats of the previous value dropped.
fn settled<V: PartialEq + Copy>(values: impl Iterator<Item = (SimTime, V)>) -> Vec<(u64, V)> {
    let mut by_instant: Vec<(u64, V)> = Vec::new();
    for (time, value) in values {
        let t = secs(time);
        match by_instant.last_mut() {
            Some((at, last)) if *at == t => *last = value,
            _ => by_instant.push((t, value)),
        }
    }
    let mut changes: Vec<(u64, V)> = Vec::new();
    for (t, value) in by_instant {
        if changes.last().is_none_or(|(_, last)| *last != value) {
            changes.push((t, value));
        }
    }
    changes
}

// ---------------------------------------------------------------------------
// Whole-trace checks: conservation and bounds, read from the records alone.
// ---------------------------------------------------------------------------

/// Walks the records and tracks where every item is. After every record:
/// an item moves only from where it is; `fill + reserved ≤ capacity` for each
/// listed component; attributes never change on the way. At every instant's
/// end: no reservation stands (ADR-0022 §4), a sink holds nothing, a Source's
/// `held` equals what it holds, and so `created = emitted + held`. At the end:
/// the engine's own counts equal the walk's.
fn assert_flow_invariants(run: &Run, capacities: &[(&str, u64)]) {
    let bounds: Vec<(usize, u64)> = capacities
        .iter()
        .map(|(name, cap)| (run.index(name), *cap))
        .collect();
    let mut walk = Walk::new(run.names.len());
    let mut last_time = SimTime::START;
    for record in &run.records {
        let time = record.time();
        assert!(time >= last_time, "time went backwards at {time}");
        if time > last_time {
            walk.end_of_instant(last_time);
            last_time = time;
        }
        walk.apply(record);
        for (at, cap) in &bounds {
            let used = walk.fill[*at] + walk.reserved_into(*at);
            assert!(
                used <= i64::try_from(*cap).unwrap(),
                "`{}` holds and reserves {used} at {time}, capacity {cap}",
                run.names[*at]
            );
        }
    }
    if run.report.summary.outcome == RunOutcome::Horizon {
        walk.end_of_instant(last_time);
    }
    assert_eq!(walk.created, run.report.items.created.get(), "created");
    assert_eq!(
        walk.delivered,
        run.report.items.delivered.get(),
        "delivered"
    );
    assert_eq!(
        u64::try_from(walk.place.len()).unwrap(),
        run.report.items.held.get() + run.report.items.in_transit.get(),
        "parts in the model"
    );
}

struct Walk {
    fill: Vec<i64>,
    place: BTreeMap<ItemId, usize>,
    attrs: BTreeMap<ItemId, ItemAttrs>,
    /// Accepts delivered and not yet answered, by (upstream, downstream).
    open: BTreeMap<(usize, usize), i64>,
    held_note: BTreeMap<usize, u32>,
    disposers: BTreeSet<usize>,
    created: u64,
    delivered: u64,
    /// A sequence number is unique, not rising: it is given when an event is
    /// scheduled, and a message's priority can deliver a later one first.
    seen_seq: BTreeSet<u64>,
}

impl Walk {
    fn new(components: usize) -> Self {
        Self {
            fill: vec![0; components],
            place: BTreeMap::new(),
            attrs: BTreeMap::new(),
            open: BTreeMap::new(),
            held_note: BTreeMap::new(),
            disposers: BTreeSet::new(),
            created: 0,
            delivered: 0,
            seen_seq: BTreeSet::new(),
        }
    }

    fn reserved_into(&self, downstream: usize) -> i64 {
        self.open
            .iter()
            .filter(|((_, to), _)| *to == downstream)
            .map(|(_, n)| *n)
            .sum()
    }

    fn answered(&mut self, upstream: usize, downstream: usize, time: SimTime) {
        let open = self.open.entry((upstream, downstream)).or_insert(0);
        *open -= 1;
        assert!(
            *open >= 0,
            "at {time}, {upstream} answered {downstream} without an Accept"
        );
    }

    fn apply(&mut self, record: &Record<FlowMessage>) {
        match record {
            Record::Event {
                seq,
                time,
                target,
                from,
                message,
                item,
            } => {
                assert!(self.seen_seq.insert(*seq), "seq {seq} delivered twice");
                self.event(*time, target.index(), from.index(), message, *item);
            }
            Record::ItemCreated { item, by, .. } => {
                assert!(self.place.insert(*item, by.index()).is_none(), "made twice");
                self.fill[by.index()] += 1;
                self.created += 1;
            }
            Record::ItemDisposed { item, by, how, .. } => {
                assert_eq!(
                    self.place.remove(item),
                    Some(by.index()),
                    "disposed elsewhere"
                );
                assert_eq!(*how, Disposal::Delivered);
                self.fill[by.index()] -= 1;
                self.disposers.insert(by.index());
                self.delivered += 1;
            }
            Record::Note { by, message, .. } => {
                if let FlowMessage::SourceState(note) = message {
                    self.held_note.insert(by.index(), note.held);
                }
            }
        }
    }

    fn event(
        &mut self,
        time: SimTime,
        target: usize,
        from: usize,
        message: &FlowMessage,
        item: Option<ItemId>,
    ) {
        if let FlowMessage::Arrive(attrs) = message {
            let item = item.unwrap_or_else(|| panic!("an Arrive without an item at {time}"));
            assert_eq!(self.place.get(&item), Some(&from), "moved from elsewhere");
            self.answered(from, target, time);
            self.place.insert(item, target);
            self.fill[from] -= 1;
            self.fill[target] += 1;
            let first = self.attrs.entry(item).or_insert(*attrs);
            assert_eq!(first, attrs, "attributes changed on the way");
            return;
        }
        assert_eq!(item, None, "only an Arrive carries an item, at {time}");
        if matches!(message, FlowMessage::Accept) {
            *self.open.entry((target, from)).or_insert(0) += 1;
        } else if matches!(message, FlowMessage::Decline) {
            self.answered(from, target, time);
        }
    }

    fn end_of_instant(&self, time: SimTime) {
        for (pair, open) in &self.open {
            assert_eq!(
                *open, 0,
                "a reservation {pair:?} outlived the instant {time}"
            );
        }
        for sink in &self.disposers {
            assert_eq!(self.fill[*sink], 0, "a sink holds a part at {time}");
        }
        for (source, held) in &self.held_note {
            assert_eq!(
                self.fill[*source],
                i64::from(*held),
                "created != emitted + held at {time}"
            );
        }
        assert!(self.fill.iter().all(|n| *n >= 0), "negative fill at {time}");
    }
}

/// ADR-0022 §5's table, copied by hand, so a change on either side is seen.
fn table(from: MachineState) -> Vec<MachineState> {
    use MachineState::{Blocked, Busy, Down, Idle, Setup, Starved};
    match from {
        Idle | Starved => vec![Busy, Setup, Down],
        Setup => vec![Busy, Down],
        Busy => vec![Blocked, Setup, Down],
        Blocked => vec![Busy, Setup, Idle, Starved, Down],
        Down => vec![Idle, Starved, Busy, Blocked, Setup],
    }
}

/// What ADR-0022 §5 and §9 say of every note: it starts at time zero in `idle`
/// or `starved`, follows the table, is written only on a change, and its fields
/// agree with its state.
fn assert_note_invariants(run: &Run) {
    for name in &run.names {
        let notes = run.machine_notes(name);
        if let Some((first_time, first)) = notes.first() {
            assert_eq!(*first_time, SimTime::START, "`{name}`'s first note");
            assert!(
                matches!(first.state, MachineState::Idle | MachineState::Starved),
                "`{name}` starts {:?}",
                first.state
            );
        }
        for (time, note) in &notes {
            let units = note.units_in_cycle;
            let fits = match note.state {
                MachineState::Busy => units >= 1,
                MachineState::Idle
                | MachineState::Starved
                | MachineState::Blocked
                | MachineState::Down
                | MachineState::Setup => units == 0,
            };
            assert!(fits, "`{name}` at {time}: {note:?}");
            assert_eq!(
                note.setup_variant.is_some(),
                note.state == MachineState::Setup,
                "`{name}` at {time}: {note:?}"
            );
        }
        for pair in notes.windows(2) {
            let (time, before, after) = (pair[1].0, &pair[0].1, &pair[1].1);
            assert!(
                before.state != after.state || before.units_in_cycle != after.units_in_cycle,
                "`{name}` at {time}: a note without a change: {after:?}"
            );
            assert!(
                before.state == after.state || table(before.state).contains(&after.state),
                "`{name}` at {time}: {:?} -> {:?} is not in the table",
                before.state,
                after.state
            );
        }
        for (time, note) in run.source_notes(name) {
            assert_eq!(
                note.state == SourceState::Blocked,
                note.held > 0,
                "`{name}` at {time}: {note:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Hand-counted deterministic lines. Each is a function, because the invariant
// test below runs them all again.
// ---------------------------------------------------------------------------

fn line_one_machine() -> String {
    model(
        "100s",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// One saturated machine, 10 s. Parts leave at 10, 20, ..., 100, inclusive of
/// the horizon, so 10 parts.
///
/// Events (ADR-0022 §4, §7). At 0: Offer, Accept, Arrive, and the on-demand
/// Source's next Offer, which waits: 4. At each of the 10 ends of a cycle:
/// `CycleDone`, Offer to the sink, Accept, Arrive; then the place is free, so
/// Accept to the Source, Arrive, and the Source's next Offer: 7. Total
/// 4 + 10 x 7 = 74.
#[test]
fn one_saturated_machine_counts_every_event() {
    let r = run(&line_one_machine());
    assert_eq!(r.deliveries(), [10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
    assert_eq!(r.event_count(), 74);
    assert_eq!(r.report.summary.events, 74);
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    assert_eq!(secs(r.report.summary.now), 100);
}

fn line_five_components() -> String {
    model(
        "1000s",
        &[
            "{id: source, type: Source, interarrival: 100s}",
            "{id: cut, type: Machine, cycle_time: 10s}",
            "{id: buf1, type: Buffer, capacity: 5 parts}",
            "{id: weld, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, cut, buf1, weld, sink]"],
    )
}

/// ADR-0022 §4 and §9: 15 events and 8 notes per part on this line, with no
/// blocking. A part arrives every 100 s and is gone 20 s later.
///
/// Events: `NextArrival`, 3 for each of 4 hops, 2 `CycleDone` = 15. The tenth part
/// is created at the horizon, 1000 s, and reaches cut: `NextArrival`, Offer,
/// Accept, Arrive = 4. So 9 x 15 + 4 = 139.
///
/// Notes: start notes for source, cut and weld, and one `VariantName`, `default`
/// = 4. Per part: Source blocked and waiting (2); each machine busy, blocked for
/// zero time, then back to waiting (3 x 2) = 8. The tenth part: Source blocked
/// and waiting, cut busy = 3. So 4 + 9 x 8 + 3 = 79.
#[test]
fn five_component_line_costs_fifteen_events_and_eight_notes_per_part() {
    let r = run(&line_five_components());
    assert_eq!(
        r.deliveries(),
        [120, 220, 320, 420, 520, 620, 720, 820, 920]
    );
    assert_eq!(r.event_count(), 139);
    assert_eq!(r.note_count(), 79);
    assert_eq!(
        r.arrivals_at("buf1"),
        [110, 210, 310, 410, 510, 610, 710, 810, 910]
    );
    assert_eq!(r.arrivals_at("weld"), r.arrivals_at("buf1"));
}

fn line_a_page_1() -> String {
    model(
        "8h",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 42s}",
            "{id: buf1, type: Buffer, capacity: 20 parts}",
            "{id: weld, type: Machine, cycle_time: 45s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, cut, buf1, weld, sink]"],
    )
}

/// Page 1's line A, an on-demand Source. The first part takes 42 + 45 = 87 s,
/// then one leaves every 45 s: the last k with 87 + 45k <= 28800 is 638, so 639
/// parts.
///
/// The buffer gains one part per 630 s (the rates differ by 1/42 - 1/45), so it
/// is full long before 8 h, and then at the horizon the line holds cut's 1, the
/// buffer's 20 and weld's 1: 22.
///
/// A part entering the full line spends 45 s in cut (42 s plus 3 s blocked), 20
/// x 45 s waiting through the buffer, and 45 s in weld: 990 s, 16.5 min.
#[test]
fn line_a_gives_page_1s_numbers() {
    let r = run(&line_a_page_1());
    let deliveries = r.deliveries();
    assert_eq!(deliveries.len(), 639);
    assert_eq!(deliveries[0], 87);
    assert_eq!(deliveries[638], 87 + 45 * 638);
    assert!(deliveries.windows(2).all(|p| p[1] - p[0] == 45));
    assert_eq!(r.report.items.held.get(), 22);
    assert_eq!(r.report.items.in_transit.get(), 0);
    assert_eq!(r.report.items.created.get(), 639 + 22);
    let mut released: BTreeMap<ItemId, SimTime> = BTreeMap::new();
    let mut last_lead = 0;
    for record in &r.records {
        match record {
            Record::Event {
                time,
                item: Some(item),
                message: FlowMessage::Arrive(_),
                ..
            } => {
                released.entry(*item).or_insert(*time);
            }
            Record::ItemDisposed { time, item, .. } => {
                last_lead = secs(*time) - secs(released[item]);
            }
            Record::Event { .. } | Record::ItemCreated { .. } | Record::Note { .. } => {}
        }
    }
    assert_eq!(last_lead, 990);
}

fn line_blocking() -> String {
    model(
        "100s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 10s}",
            "{id: b, type: Machine, cycle_time: 30s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, b, sink]"],
    )
}

/// A fast machine before a slow one, no buffer. Worked by hand:
///
/// - p1: a 0-10, b 10-40, delivered 40.
/// - p2: a 10-20, blocked 20-40, b 40-70, delivered 70.
/// - p3: a 40-50, blocked 50-70, b 70-100, delivered 100.
/// - p4: a 70-80, blocked 80-100, moves to b at 100.
/// - p5: a from 100.
///
/// So a is busy 20 + 10 + 10 = 40 s, blocked 20 + 20 + 20 = 60 s; b is starved
/// 10 s, then busy to the end, 90 s.
#[test]
fn blocking_timeline_by_hand() {
    let r = run(&line_blocking());
    assert_eq!(r.deliveries(), [40, 70, 100]);
    assert_eq!(
        r.timeline("a"),
        [
            (0, "busy"),
            (20, "blocked"),
            (40, "busy"),
            (50, "blocked"),
            (70, "busy"),
            (80, "blocked"),
            (100, "busy")
        ]
    );
    assert_eq!(r.timeline("b"), [(0, "starved"), (10, "busy")]);
    assert_eq!(
        r.shares("a"),
        BTreeMap::from([("busy", 40), ("blocked", 60)])
    );
    assert_eq!(
        r.shares("b"),
        BTreeMap::from([("starved", 10), ("busy", 90)])
    );
}

fn line_starving() -> String {
    model(
        "100s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 30s}",
            "{id: b, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, b, sink]"],
    )
}

/// A slow machine before a fast one. a is never settled in anything but busy; b
/// runs 30-40, 60-70, 90-100 and waits otherwise: starved, since a station is
/// upstream. Deliveries at 40, 70, 100. b: busy 30 s, starved 70 s.
#[test]
fn starving_timeline_by_hand() {
    let r = run(&line_starving());
    assert_eq!(r.deliveries(), [40, 70, 100]);
    assert_eq!(r.timeline("a"), [(0, "busy")]);
    assert_eq!(
        r.timeline("b"),
        [
            (0, "starved"),
            (30, "busy"),
            (40, "starved"),
            (60, "busy"),
            (70, "starved"),
            (90, "busy"),
            (100, "starved")
        ]
    );
    assert_eq!(
        r.shares("b"),
        BTreeMap::from([("starved", 70), ("busy", 30)])
    );
}

fn line_idle_and_starved() -> String {
    model(
        "130s",
        &[
            "{id: source, type: Source, interarrival: 50s}",
            "{id: a, type: Machine, cycle_time: 10s}",
            "{id: b, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, b, sink]"],
    )
}

/// ADR-0022 §5: the first station waits idle, every later one starved. Parts at
/// 50 and 100: a runs 50-60 and 100-110, b 60-70 and 110-120. The third part
/// comes at 150, after the horizon.
#[test]
fn first_station_is_idle_and_a_later_one_starved() {
    let r = run(&line_idle_and_starved());
    assert_eq!(
        r.timeline("a"),
        [
            (0, "idle"),
            (50, "busy"),
            (60, "idle"),
            (100, "busy"),
            (110, "idle")
        ]
    );
    assert_eq!(
        r.timeline("b"),
        [
            (0, "starved"),
            (60, "busy"),
            (70, "starved"),
            (110, "busy"),
            (120, "starved")
        ]
    );
    assert_eq!(r.deliveries(), [70, 120]);
}

/// ADR-0022 §5, what decides is whether a Source reaches the station through
/// non-stations only: a buffer does not make its station starved, and a station
/// fed both by a Source and by a machine is idle. `behind` has a station in
/// front of its buffer, so it is starved.
#[test]
fn idle_or_starved_follows_the_stations_on_the_way() {
    let r = run(&model(
        "400s",
        &[
            "{id: s1, type: Source, interarrival: 50s}",
            "{id: s2, type: Source, interarrival: 70s}",
            "{id: q0, type: Buffer, capacity: 3 parts}",
            "{id: first, type: Machine, cycle_time: 5s}",
            "{id: q1, type: Buffer, capacity: 3 parts}",
            "{id: behind, type: Machine, cycle_time: 5s}",
            "{id: mixed, type: Machine, cycle_time: 5s}",
            "{id: sink, type: Sink}",
        ],
        &["[s1, q0, first, q1, behind, mixed, sink]", "[s2, mixed]"],
    ));
    let states = |name: &str| -> BTreeSet<&str> {
        r.machine_notes(name)
            .iter()
            .map(|(_, n)| n.state.as_str())
            .collect()
    };
    assert_eq!(states("first"), BTreeSet::from(["idle", "busy", "blocked"]));
    assert_eq!(
        states("behind"),
        BTreeSet::from(["starved", "busy", "blocked"])
    );
    assert_eq!(states("mixed"), BTreeSet::from(["idle", "busy", "blocked"]));
}

fn line_blocked_source() -> String {
    model(
        "100s",
        &[
            "{id: source, type: Source, interarrival: 10s}",
            "{id: m, type: Machine, cycle_time: 25s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// ADR-0022 §7, S2: a held part waits, and the next interval starts when it has
/// left.
///
/// p1 made at 10, leaves at once; m 10-35. p2 made at 20, held to 35; m 35-60.
/// p3 made at 35 + 10 = 45, held to 60; m 60-85. p4 made at 70, held to 85; m
/// from 85. p5 made at 95, still held at the horizon.
///
/// So created 5 = delivered 3 + one in m + one at the Source.
#[test]
fn a_blocked_source_counts_its_interval_from_departure() {
    let r = run(&line_blocked_source());
    assert_eq!(r.created(), [10, 20, 45, 70, 95]);
    assert_eq!(r.arrivals_at("m"), [10, 35, 60, 85]);
    assert_eq!(r.deliveries(), [35, 60, 85]);
    assert_eq!(r.report.items.held.get(), 2);
    assert_eq!(
        r.source_timeline("source"),
        [
            (0, "waiting"),
            (20, "blocked"),
            (35, "waiting"),
            (45, "blocked"),
            (60, "waiting"),
            (70, "blocked"),
            (85, "waiting"),
            (95, "blocked")
        ]
    );
    assert_eq!(
        r.source_held("source"),
        [
            (0, 0),
            (20, 1),
            (35, 0),
            (45, 1),
            (60, 0),
            (70, 1),
            (85, 0),
            (95, 1)
        ]
    );
}

fn line_batch() -> String {
    model(
        "500s",
        &[
            "{id: source, type: Source, interarrival: 100s, batch: 3 parts}",
            "{id: m, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// ADR-0022 §7: a batch is made at one instant and offered one part at a time;
/// the next interval starts when the batch has left. A batch at 100 leaves at
/// 100, 110, 120, so the next comes at 220, then 340, then 460 (and 580, after
/// the horizon).
#[test]
fn a_batch_leaves_one_at_a_time_and_the_interval_starts_after_the_last() {
    let r = run(&line_batch());
    assert_eq!(
        r.created(),
        [100, 100, 100, 220, 220, 220, 340, 340, 340, 460, 460, 460]
    );
    assert_eq!(
        r.deliveries(),
        [110, 120, 130, 230, 240, 250, 350, 360, 370, 470, 480, 490]
    );
    assert_eq!(
        r.source_held("source"),
        [
            (0, 0),
            (100, 2),
            (110, 1),
            (120, 0),
            (220, 2),
            (230, 1),
            (240, 0),
            (340, 2),
            (350, 1),
            (360, 0),
            (460, 2),
            (470, 1),
            (480, 0)
        ]
    );
}

fn line_full_buffer() -> String {
    model(
        "120s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 10s}",
            "{id: buf, type: Buffer, capacity: 1 parts}",
            "{id: b, type: Machine, cycle_time: 35s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, buf, b, sink]"],
    )
}

/// A one-place buffer before a slow station.
///
/// - p1: a 0-10, through the empty buffer, b 10-45.
/// - p2: a 10-20, in the buffer from 20, b 45-80.
/// - p3: a 20-30, blocked 30-45 because the buffer is full, in it from 45, b
///   80-115.
/// - p4: a 45-55, blocked 55-80, in the buffer from 80.
/// - p5: a 80-90, blocked 90-115, in the buffer from 115.
/// - p6: a from 115.
#[test]
fn a_full_buffer_blocks_the_machine_before_it() {
    let r = run(&line_full_buffer());
    assert_eq!(r.arrivals_at("buf"), [10, 20, 45, 80, 115]);
    assert_eq!(r.arrivals_at("b"), [10, 45, 80, 115]);
    assert_eq!(r.deliveries(), [45, 80, 115]);
    assert_eq!(
        r.timeline("a"),
        [
            (0, "busy"),
            (30, "blocked"),
            (45, "busy"),
            (55, "blocked"),
            (80, "busy"),
            (90, "blocked"),
            (115, "busy")
        ]
    );
    assert_eq!(r.timeline("b"), [(0, "starved"), (10, "busy")]);
}

fn line_capacity_two() -> String {
    model(
        "100s",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 10s, capacity: 2 parts}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// Two places, saturated: two parts start together at every 10 s, so two leave
/// at each of 10, ..., 100 and two cycles run all the time.
#[test]
fn two_places_run_two_cycles_at_once() {
    let r = run(&line_capacity_two());
    let expected: Vec<u64> = (1..=10).flat_map(|k| [10 * k, 10 * k]).collect();
    assert_eq!(r.deliveries(), expected);
    assert_eq!(r.timeline("m"), [(0, "busy")]);
    assert_eq!(r.units("m"), [(0, 2)]);
}

/// Page 6's line A with a buffer of one: it runs at weld's pace, and weld never
/// waits after its first part, so 639 parts as on page 1. Cut gains 3 s per part
/// on weld until the one place is full when it finishes, which takes about 630 s
/// (one place at the 630 s per place of page 1's derivation); from then on it is
/// blocked 3 of every 45 s. So its blocked share over 8 h is 3/45 x (1 - 630 /
/// 28800) = 6.52 %, within the 0.2 points allowed here for the exact end of that
/// transient. ADR-0022's table has 6.5 %.
#[test]
fn page_6s_line_with_one_place() {
    let r = run(&model(
        "8h",
        &[
            "{id: source, type: Source}",
            "{id: cut, type: Machine, cycle_time: 42s}",
            "{id: buf1, type: Buffer, capacity: 1 parts}",
            "{id: weld, type: Machine, cycle_time: 45s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, cut, buf1, weld, sink]"],
    ));
    assert_eq!(r.deliveries().len(), 639);
    let cut = r.shares("cut");
    let blocked = count_f64(cut["blocked"]) / 28800.0;
    assert!(
        (blocked - 0.0652).abs() < 0.002,
        "cut blocked share {blocked}"
    );
    assert!(!r.shares("weld").contains_key("blocked"));
}

// ---------------------------------------------------------------------------
// Failures with fixed durations (ADR-0022 §6).
// ---------------------------------------------------------------------------

fn line_failing() -> String {
    model(
        "1000s",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 100s, mtbf: 250s, mttr: 30s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// A saturated machine: the up clock counts from time zero and each repair's
/// end, so failures at 250, 530, 810 (and 1090, after the horizon), repairs at
/// 280, 560, 840. Down 3 x 30 = 90 s, busy the other 910 s.
///
/// - p1 0-100; p2 100-200.
/// - p3 from 200, stopped at 250 with 50 s left, back at 280, ends 330.
/// - p4 330-430.
/// - p5 from 430 ends exactly at the failure at 530. A failure at the same
///   instant comes first and the cycle resumes with zero left, ending at the
///   repair, 560 (ADR-0022 §6). No `CycleDone` at 530.
/// - p6 560-660; p7 660-760.
/// - p8 from 760, stopped at 810 with 50 s left, back at 840, ends 890.
/// - p9 890-990. p10 still runs at the horizon.
#[test]
fn failure_timeline_resumes_each_cycle_with_the_time_left() {
    let r = run(&line_failing());
    assert_eq!(
        r.deliveries(),
        [100, 200, 330, 430, 560, 660, 760, 890, 990]
    );
    assert_eq!(
        r.events_at("m", |m| matches!(m, FlowMessage::CycleDone { .. })),
        [100, 200, 330, 430, 560, 660, 760, 890, 990]
    );
    assert_eq!(
        r.events_at("m", |m| matches!(m, FlowMessage::Failure)),
        [250, 530, 810]
    );
    assert_eq!(
        r.events_at("m", |m| matches!(m, FlowMessage::Repaired)),
        [280, 560, 840]
    );
    assert_eq!(
        r.timeline("m"),
        [
            (0, "busy"),
            (250, "down"),
            (280, "busy"),
            (530, "down"),
            (560, "busy"),
            (810, "down"),
            (840, "busy")
        ]
    );
    assert_eq!(r.shares("m"), BTreeMap::from([("busy", 910), ("down", 90)]));
    assert_eq!(
        r.units("m"),
        [
            (0, 1),
            (250, 0),
            (280, 1),
            (530, 0),
            (560, 1),
            (810, 0),
            (840, 1)
        ]
    );
}

fn line_failing_while_idle() -> String {
    model(
        "430s",
        &[
            "{id: source, type: Source, interarrival: 100s}",
            "{id: m, type: Machine, cycle_time: 10s, mtbf: 95s, mttr: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// The clock is calendar time, so a waiting machine fails too, and a down
/// machine queues an offer without accepting (ADR-0022 §4, §6).
///
/// Failures at 95, 200, 305, 410; repairs at 105, 210, 315, 420.
///
/// - Part made at 100, while m is down; held to the repair at 105; m 105-115.
/// - The Source's next interval counts from 105: part at 205, while down again;
///   held to 210; m 210-220.
/// - Next at 310, held to 315, m 315-325. Next at 415, held to 420, m 420-430.
#[test]
fn a_waiting_machine_fails_and_queues_an_offer_until_repaired() {
    let r = run(&line_failing_while_idle());
    assert_eq!(r.created(), [100, 205, 310, 415]);
    assert_eq!(r.arrivals_at("m"), [105, 210, 315, 420]);
    assert_eq!(r.deliveries(), [115, 220, 325, 430]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (95, "down"),
            (105, "busy"),
            (115, "idle"),
            (200, "down"),
            (210, "busy"),
            (220, "idle"),
            (305, "down"),
            (315, "busy"),
            (325, "idle"),
            (410, "down"),
            (420, "busy"),
            (430, "idle")
        ]
    );
}

fn line_failing_while_blocked() -> String {
    model(
        "365s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 10s, mtbf: 50s, mttr: 100s}",
            "{id: b, type: Machine, cycle_time: 60s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, b, sink]"],
    )
}

/// A blocked machine fails, and room opens while it is down: its finished part
/// does not move (ADR-0022 §5: nothing moves in or out). Failures of a at 50,
/// 200, 350; repairs at 150, 300, 450.
///
/// - p1: a 0-10, b 10-70, delivered 70.
/// - p2: a 10-20, blocked 20-50, a down 50-150. At 70 b has room and answers a's
///   standing offer; a is down, so it declines. b starved 70-150. At 150 p2
///   moves; b 150-210, delivered 210.
/// - p3: a 150-160, blocked 160-200, down 200-300. Same at 210: a decline, b
///   starved 210-300. At 300 p3 moves; b 300-360, delivered 360.
/// - p4: a 300-310, blocked 310-350, down from 350. At 360 a decline.
#[test]
fn a_blocked_machine_that_fails_moves_nothing_until_repaired() {
    let r = run(&line_failing_while_blocked());
    assert_eq!(r.deliveries(), [70, 210, 360]);
    assert_eq!(r.arrivals_at("b"), [10, 150, 300]);
    assert_eq!(
        r.events_at("b", |m| matches!(m, FlowMessage::Decline)),
        [70, 210, 360]
    );
    assert_eq!(
        r.timeline("a"),
        [
            (0, "busy"),
            (20, "blocked"),
            (50, "down"),
            (150, "busy"),
            (160, "blocked"),
            (200, "down"),
            (300, "busy"),
            (310, "blocked"),
            (350, "down")
        ]
    );
    assert_eq!(
        r.timeline("b"),
        [
            (0, "starved"),
            (10, "busy"),
            (70, "starved"),
            (150, "busy"),
            (210, "starved"),
            (300, "busy"),
            (360, "starved")
        ]
    );
}

fn line_failing_and_still_blocked() -> String {
    model(
        "150s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 10s, mtbf: 30s, mttr: 5s}",
            "{id: b, type: Machine, cycle_time: 100s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, b, sink]"],
    )
}

/// A blocked machine fails and is repaired while there is still no room: it is
/// blocked again, and its part stays first in line, without duplicate answers
/// (ADR-0022 §4: "at most one outstanding per neighbour"). Failures of a at 30,
/// 65, 100, 135, repairs at 35, 70, 105, 140.
///
/// - p1: a 0-10, b 10-110, delivered 110.
/// - p2: a 10-20, then blocked, except while down; moves at 110.
/// - p3: a 110-120, blocked 120-135, down 135-140, blocked 140-150.
///
/// b answers a exactly twice, at 10 and 110, and never gets a decline.
#[test]
fn a_repair_with_no_room_goes_back_to_blocked() {
    let r = run(&line_failing_and_still_blocked());
    assert_eq!(r.deliveries(), [110]);
    assert_eq!(r.arrivals_at("b"), [10, 110]);
    assert_eq!(
        r.timeline("a"),
        [
            (0, "busy"),
            (20, "blocked"),
            (30, "down"),
            (35, "blocked"),
            (65, "down"),
            (70, "blocked"),
            (100, "down"),
            (105, "blocked"),
            (110, "busy"),
            (120, "blocked"),
            (135, "down"),
            (140, "blocked")
        ]
    );
    assert_eq!(
        r.events_at("a", |m| matches!(m, FlowMessage::Accept)),
        [10, 110]
    );
    assert_eq!(r.events_at("b", |m| matches!(m, FlowMessage::Decline)), []);
}

fn line_capacity_two_failing() -> String {
    model(
        "399s",
        &[
            "{id: source, type: Source, interarrival: 30s}",
            "{id: m, type: Machine, cycle_time: 100s, capacity: 2 parts, mtbf: 100s, mttr: 50s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    )
}

/// ADR-0022 §5, §6: all units stop together, each resumes with its own time
/// left, and units in cycle read 0 while down. Failures at 100, 250 (and 400,
/// after this horizon); repairs at 150, 300.
///
/// - p1 from 30, 70 s done at 100, ends 150 + 30 = 180.
/// - p2 from 60, 40 s done at 100, ends 150 + 60 = 210.
/// - p3 made at 90, full, held to 180; from 180, 70 s done at 250, ends 300 + 30
///   = 330.
/// - p4 made at 180 + 30 = 210, accepted at once as p2 has left; 40 s done at
///   250, ends 300 + 60 = 360.
/// - p5 made at 240, held to 330; runs from 330.
/// - p6 made at 360, accepted at once. p7 made at 390, held.
///
/// Delivered 4, created 7, so 3 are still in the model.
#[test]
fn every_unit_pauses_together_and_keeps_its_own_time_left() {
    let r = run(&line_capacity_two_failing());
    assert_eq!(r.created(), [30, 60, 90, 210, 240, 360, 390]);
    assert_eq!(r.deliveries(), [180, 210, 330, 360]);
    assert_eq!(r.arrivals_at("m"), [30, 60, 180, 210, 330, 360]);
    assert_eq!(
        r.units("m"),
        [
            (0, 0),
            (30, 1),
            (60, 2),
            (100, 0),
            (150, 2),
            (250, 0),
            (300, 2)
        ]
    );
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (30, "busy"),
            (100, "down"),
            (150, "busy"),
            (250, "down"),
            (300, "busy")
        ]
    );
    assert_eq!(r.report.items.held.get(), 3);
}

/// Page 8's arithmetic with constant failures: 42 s, up 1 h, repair 3 min, 8 h.
/// Repairs end at 3780, 7560, 11340, 15120, 18900, 22680, 26460, and the next
/// failure is at 30060, after the horizon. Down 7 x 180 = 1260 s, so up
/// 28800 - 1260 = 27540 s, and 27540 / 42 = 655.7: 655 parts. The first failure
/// at 3600 is not a multiple of 42, so no tie.
#[test]
fn a_shift_with_constant_failures_gives_the_counted_parts() {
    let r = run(&model(
        "8h",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 42s, mtbf: 1h, mttr: 3min}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    ));
    assert_eq!(r.deliveries().len(), 655);
    assert_eq!(r.shares("m")["down"], 1260);
    assert_eq!(
        r.events_at("m", |m| matches!(m, FlowMessage::Failure)),
        [3600, 7380, 11160, 14940, 18720, 22500, 26280]
    );
}

/// ADR-0022 §6: `mtbf: 6h` is a constant: exactly one failure in 8 h, at 6:00:00,
/// and none in 5 h.
#[test]
fn a_constant_mtbf_fails_on_schedule_or_not_at_all() {
    let line = |horizon: &str| {
        run(&model(
            horizon,
            &[
                "{id: source, type: Source}",
                "{id: m, type: Machine, cycle_time: 42s, mtbf: 6h, mttr: 12min}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ))
    };
    let long = line("8h");
    assert_eq!(
        long.events_at("m", |m| matches!(m, FlowMessage::Failure)),
        [21600]
    );
    assert_eq!(
        long.events_at("m", |m| matches!(m, FlowMessage::Repaired)),
        [22320]
    );
    assert_eq!(
        line("5h").events_at("m", |m| matches!(m, FlowMessage::Failure)),
        []
    );
}

/// ADR-0022 §6, the case M10 pins: two machines with the same constant failures
/// and no buffer stop together for ever. Up-time U is the same on both clocks;
/// a part leaves b at U = 20, 30, ..., and the two clocks never disagree, so no
/// blocking and no starving beyond the start. Failures at 1000 and 2050, repairs
/// at 1050 and 2100. A cycle ending exactly at a failure resumes with zero left,
/// so at the horizon, 2100, U = 2000: (2000 - 20) / 10 + 1 = 199 parts.
#[test]
fn two_machines_with_the_same_constant_failures_stop_together() {
    let r = run(&model(
        "2100s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 10s, mtbf: 1000s, mttr: 50s}",
            "{id: b, type: Machine, cycle_time: 10s, mtbf: 1000s, mttr: 50s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, a, b, sink]"],
    ));
    assert_eq!(r.deliveries().len(), 199);
    assert_eq!(*r.deliveries().last().unwrap(), 2100);
    for name in ["a", "b"] {
        assert_eq!(
            r.events_at(name, |m| matches!(m, FlowMessage::Failure)),
            [1000, 2050]
        );
        assert!(!r.shares(name).contains_key("blocked"), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Setups, counted by hand (ADR-0022 §7). Two one-variant Sources make the order
// of variants exact: no random mix.
// ---------------------------------------------------------------------------

fn two_sources(sa: &str, sb: &str, machine: &str, horizon: &str) -> String {
    let a = format!("{{id: sa, type: Source, interarrival: {sa}, variants: {{a: 100%}}}}");
    let b = format!("{{id: sb, type: Source, interarrival: {sb}, variants: {{b: 100%}}}}");
    let m = format!("{{id: m, type: Machine, {machine}}}");
    model(
        horizon,
        &[&a, &b, &m, "{id: sink, type: Sink}"],
        &["[sa, m, sink]", "[sb, m]"],
    )
}

fn line_per_variant() -> String {
    two_sources(
        "100s",
        "160s",
        "cycle_time: 10s, setup: {per_variant: 20s}",
        "470s",
    )
}

/// Cycle 10 s, any change 20 s; variant a at 100, 200, 300, 400; b at 160, 320
/// (480 is after the horizon).
///
/// - a at 100: the first unit costs no setup. 100-110.
/// - b at 160: setup 160-180, busy 180-190.
/// - a at 200: setup 200-220, busy 220-230.
/// - a at 300: same variant, no setup. 300-310.
/// - b at 320: setup 320-340, busy 340-350.
/// - a at 400: setup 400-420, busy 420-430.
///
/// Four changeovers, 80 s; six cycles, 60 s; the rest, 330 s, idle.
#[test]
fn a_change_of_variant_costs_a_setup_and_the_first_unit_none() {
    let r = run(&line_per_variant());
    assert_eq!(r.deliveries(), [110, 190, 230, 310, 350, 430]);
    assert_eq!(r.setups_done("m"), [180, 220, 340, 420]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (100, "busy"),
            (110, "idle"),
            (160, "setup"),
            (180, "busy"),
            (190, "idle"),
            (200, "setup"),
            (220, "busy"),
            (230, "idle"),
            (300, "busy"),
            (310, "idle"),
            (320, "setup"),
            (340, "busy"),
            (350, "idle"),
            (400, "setup"),
            (420, "busy"),
            (430, "idle")
        ]
    );
    assert_eq!(
        r.shares("m"),
        BTreeMap::from([("setup", 80), ("busy", 60), ("idle", 330)])
    );
    // Variant ids index the sorted names: a is 0, b is 1. Each setup note names
    // the variant being set up for.
    let for_variant: Vec<u16> = r
        .machine_notes("m")
        .iter()
        .filter_map(|(_, n)| n.setup_variant.map(VariantId::get))
        .collect();
    assert_eq!(for_variant, [1, 0, 1, 0]);
}

fn line_matrix() -> String {
    two_sources(
        "100s",
        "165s",
        "cycle_time: 10s, setup: {matrix: [{from: a, to: b, time: 30s}, {from: b, to: a, time: 0s}]}",
        "420s",
    )
}

/// `matrix` with a free way back: a to b costs 30 s, b to a is `0s`: no timer and
/// no `setup` state (ADR-0022 §7).
///
/// - a at 100: 100-110.
/// - b at 165: setup 165-195, busy 195-205.
/// - a at 200: m is full, the part waits at the Source to 205. Then free: busy
///   205-215. The Source's next interval counts from 205: a at 305.
/// - a at 305: 305-315. Next a at 405.
/// - b at 165 + 165 = 330: setup 330-360, busy 360-370.
/// - a at 405: free. 405-415.
#[test]
fn a_zero_matrix_entry_is_no_changeover() {
    let r = run(&line_matrix());
    assert_eq!(r.deliveries(), [110, 205, 215, 315, 370, 415]);
    assert_eq!(r.setups_done("m"), [195, 360]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (100, "busy"),
            (110, "idle"),
            (165, "setup"),
            (195, "busy"),
            (215, "idle"),
            (305, "busy"),
            (315, "idle"),
            (330, "setup"),
            (360, "busy"),
            (370, "idle"),
            (405, "busy"),
            (415, "idle")
        ]
    );
}

fn line_free_change_is_remembered() -> String {
    two_sources(
        "100s",
        "150s",
        "cycle_time: 10s, setup: {matrix: [{from: a, to: b, time: 0s}, {from: b, to: a, time: 20s}]}",
        "260s",
    )
}

/// A free change still changes the variant the machine is set up for. a at 100:
/// 100-110. b at 150, free from a: 150-160. a at 200: now from b, so 20 s: setup
/// 200-220, busy 220-230. A machine that forgot the free change would have
/// delivered at 210.
#[test]
fn a_free_change_still_changes_what_the_machine_is_set_up_for() {
    let r = run(&line_free_change_is_remembered());
    assert_eq!(r.deliveries(), [110, 160, 230]);
    assert_eq!(r.setups_done("m"), [220]);
}

fn line_matrix_and_per_variant() -> String {
    two_sources(
        "100s",
        "165s",
        "cycle_time: 10s, setup: {per_variant: 7s, matrix: [{from: a, to: b, time: 30s}]}",
        "400s",
    )
}

/// `matrix` if it has the pair, otherwise `per_variant` (ADR-0022 §7).
///
/// - a at 100: 100-110.
/// - b at 165: from the matrix, setup 165-195; busy 195-205.
/// - a at 200 waits at the Source to 205; b to a is not in the matrix: setup
///   205-212, busy 212-222. Next a at 305.
/// - a at 305: 305-315.
/// - b at 330: setup 330-360, busy 360-370.
#[test]
fn a_pair_outside_the_matrix_takes_per_variant() {
    let r = run(&line_matrix_and_per_variant());
    assert_eq!(r.deliveries(), [110, 205, 222, 315, 370]);
    assert_eq!(r.setups_done("m"), [195, 212, 360]);
}

fn line_setup_failing() -> String {
    two_sources(
        "105s",
        "240s",
        "cycle_time: 10s, setup: {per_variant: 40s}, mtbf: 120s, mttr: 20s",
        "399s",
    )
}

/// A failure pauses a changeover like a cycle (ADR-0022 §6, §7), and one
/// `SetupDone` is one finished changeover, however it was interrupted (§9).
/// Failures at 120, 260 (and 400, after this horizon); repairs at 140, 280.
///
/// - a at 105: free, 105-115. Idle, then down 120-140 while waiting.
/// - a at 210: same variant, 210-220.
/// - b at 240: setup from 240; 20 s done at 260; 20 s left, back at 280, ends 300.
///   Busy 300-310.
/// - a at 315: setup 315-355, busy 355-365.
#[test]
fn a_failure_pauses_a_changeover() {
    let r = run(&line_setup_failing());
    assert_eq!(r.deliveries(), [115, 220, 310, 365]);
    assert_eq!(r.setups_done("m"), [300, 355]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (105, "busy"),
            (115, "idle"),
            (120, "down"),
            (140, "idle"),
            (210, "busy"),
            (220, "idle"),
            (240, "setup"),
            (260, "down"),
            (280, "setup"),
            (300, "busy"),
            (310, "idle"),
            (315, "setup"),
            (355, "busy"),
            (365, "idle")
        ]
    );
    let for_variant: Vec<u16> = r
        .machine_notes("m")
        .iter()
        .filter_map(|(_, n)| n.setup_variant.map(VariantId::get))
        .collect();
    assert_eq!(for_variant, [1, 1, 0]);
}

fn line_change_waits_for_cycles() -> String {
    two_sources(
        "100s",
        "115s",
        "cycle_time: 50s, capacity: 2 parts, setup: {per_variant: 20s}",
        "299s",
    )
}

/// ADR-0022 §7: a unit whose change costs time waits at the front until no unit
/// is in a cycle; then the changeover runs.
///
/// - a1 at 100: 100-150. Next a at 200.
/// - b1 at 115: a place is free, it enters and waits. At 150 a1 ends: setup
///   150-170, busy 170-220. Next b at 230.
/// - a2 at 200: enters, waits. At 220 b1 ends: setup 220-240, busy 240-290.
/// - b2 at 230: enters, waits. At 290 a2 ends: setup from 290.
///
/// Never two units in a cycle.
#[test]
fn a_changeover_waits_for_the_units_in_their_cycle() {
    let r = run(&line_change_waits_for_cycles());
    assert_eq!(r.arrivals_at("m"), [100, 115, 200, 230]);
    assert_eq!(r.deliveries(), [150, 220, 290]);
    assert_eq!(r.setups_done("m"), [170, 240]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (100, "busy"),
            (150, "setup"),
            (170, "busy"),
            (220, "setup"),
            (240, "busy"),
            (290, "setup")
        ]
    );
    assert!(
        r.machine_notes("m")
            .iter()
            .all(|(_, n)| n.units_in_cycle <= 1)
    );
}

fn line_units_keep_their_order() -> String {
    two_sources(
        "60s",
        "95s",
        "cycle_time: 100s, capacity: 3 parts, setup: {per_variant: 10s}",
        "379s",
    )
}

/// ADR-0022 §7: units start in arrival order, so a unit that needs no change does
/// not pass one waiting for a change.
///
/// - a1 at 60: 60-160.
/// - b1 at 95 enters and waits for a1.
/// - a2 at 120 enters. It is set up for a, but b1 is first, so it waits.
/// - At 160 a1 leaves, setup 160-170, b1 170-270.
/// - a3 at 180 enters: three in the machine, full.
/// - b2 at 190 is held at its Source; a4 at 240 is held too.
/// - At 270 b1 leaves. Oldest offer first: b2's from 190. a2 is at the front with
///   a change: setup 270-280. At 280 a2 and a3 both start, a3 needing no change,
///   and run to 380, after the horizon.
#[test]
fn a_unit_with_no_change_does_not_pass_one_waiting_for_a_change() {
    let r = run(&line_units_keep_their_order());
    assert_eq!(r.arrivals_at("m"), [60, 95, 120, 180, 270]);
    assert_eq!(r.deliveries(), [160, 270]);
    assert_eq!(r.setups_done("m"), [170, 280]);
    assert_eq!(
        r.units("m"),
        [(0, 0), (60, 1), (160, 0), (170, 1), (270, 0), (280, 2)]
    );
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (60, "busy"),
            (160, "setup"),
            (170, "busy"),
            (270, "setup"),
            (280, "busy")
        ]
    );
}

fn line_free_change_does_not_wait() -> String {
    two_sources(
        "100s",
        "110s",
        "cycle_time: 50s, capacity: 2 parts, setup: {matrix: [{from: a, to: b, time: 0s}]}",
        "165s",
    )
}

/// ADR-0022 §7: "a change that costs nothing does not wait". a at 100: 100-150. b
/// at 110, free: 110-160, while a still runs.
#[test]
fn a_free_change_does_not_wait_for_the_units_in_their_cycle() {
    let r = run(&line_free_change_does_not_wait());
    assert_eq!(r.deliveries(), [150, 160]);
    assert_eq!(r.setups_done("m"), []);
    assert_eq!(
        r.units("m"),
        [(0, 0), (100, 1), (110, 2), (150, 1), (160, 0)]
    );
}

fn line_change_does_not_wait_for_finished() -> String {
    let a = "{id: sa, type: Source, interarrival: 100s, variants: {a: 100%}}";
    let b = "{id: sb, type: Source, interarrival: 120s, variants: {b: 100%}}";
    model(
        "320s",
        &[
            a,
            b,
            "{id: m, type: Machine, cycle_time: 10s, capacity: 2 parts, setup: {per_variant: 5s}}",
            "{id: slow, type: Machine, cycle_time: 100s}",
            "{id: sink, type: Sink}",
        ],
        &["[sa, m, slow, sink]", "[sb, m]"],
    )
}

/// ADR-0022 §7: a changeover runs "without waiting for finished units to leave",
/// and finished units leave in the order they finished.
///
/// - a1 at 100: 100-110; slow 110-210.
/// - b1 at 120: setup 120-125, busy 125-135, then blocked: slow is busy.
/// - a2 at 200: a place is free. No unit in a cycle, so setup 200-205 beside the
///   finished b1; busy 205-215.
/// - At 210 slow delivers a1 and takes b1, 210-310.
/// - At 215 a2 is finished: blocked.
/// - b2 at 240: setup 240-245, busy 245-255, then two finished units: blocked.
/// - a3 at 300: m is full, held at its Source.
/// - At 310 slow delivers b1 and takes a2, which finished first. A place is free:
///   a3 enters, setup 310-315, busy from 315.
#[test]
fn a_changeover_does_not_wait_for_finished_units() {
    let r = run(&line_change_does_not_wait_for_finished());
    assert_eq!(r.deliveries(), [210, 310]);
    assert_eq!(r.setups_done("m"), [125, 205, 245, 315]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (100, "busy"),
            (110, "idle"),
            (120, "setup"),
            (125, "busy"),
            (135, "blocked"),
            (200, "setup"),
            (205, "busy"),
            (215, "blocked"),
            (240, "setup"),
            (245, "busy"),
            (255, "blocked"),
            (310, "setup"),
            (315, "busy")
        ]
    );
    let slow = r.index("slow");
    let variants: Vec<(u64, u16)> = r
        .records
        .iter()
        .filter_map(|record| match record {
            Record::Event {
                time,
                target,
                message: FlowMessage::Arrive(attrs),
                ..
            } if target.index() == slow => Some((secs(*time), attrs.variant.get())),
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. } => None,
        })
        .collect();
    assert_eq!(variants, [(110, 0), (210, 1), (310, 0)]);
}

// ---------------------------------------------------------------------------
// Disciplines, merge and branch.
// ---------------------------------------------------------------------------

fn line_discipline(discipline: &str) -> String {
    let buffer = format!("{{id: q, type: Buffer, capacity: 100 parts, discipline: {discipline}}}");
    model(
        "450s",
        &[
            "{id: low, type: Source, interarrival: 20s}",
            "{id: high, type: Source, interarrival: 37s, priority: 5}",
            &buffer,
            "{id: m, type: Machine, cycle_time: 107s}",
            "{id: sink, type: Sink}",
        ],
        &["[low, q, m, sink]", "[high, q]"],
    )
}

/// Which parts a machine takes from a buffer, by when each part was made: a part
/// here is always released when made, as the buffer has room.
///
/// `low` makes one every 20 s, `high` every 37 s with priority 5, and neither
/// time ever ties with the machine's ends, which are 127, 234, 341, 448: the
/// first part (20) goes straight through the empty buffer to m.
///
/// In the buffer at 127, entry order: 37, 40, 60, 74, 80, 100, 111, 120.
///
/// - fifo: first in each time: 37, 40, 60, 74.
/// - lifo: last in each time: 120; at 234, the later of 220 and 222 is 222; at
///   341, the later of 340 and 333 is 340; at 448, the later of 440 and 444 is 444.
/// - priority: `high` first, earliest of them: 37, 74, 111, 148.
#[test]
fn each_discipline_picks_the_part_worked_out_by_hand() {
    for (discipline, expected) in [
        ("fifo", [20, 37, 40, 60, 74]),
        ("lifo", [20, 120, 222, 340, 444]),
        ("priority", [20, 37, 74, 111, 148]),
    ] {
        let r = run(&line_discipline(discipline));
        let mut made: BTreeMap<ItemId, u64> = BTreeMap::new();
        let mut taken = Vec::new();
        let m = r.index("m");
        for record in &r.records {
            match record {
                Record::ItemCreated { time, item, .. } => {
                    made.insert(*item, secs(*time));
                }
                Record::Event {
                    target,
                    item: Some(item),
                    message: FlowMessage::Arrive(_),
                    ..
                } if target.index() == m => taken.push(made[item]),
                Record::Event { .. } | Record::ItemDisposed { .. } | Record::Note { .. } => {}
            }
        }
        assert_eq!(taken, expected, "{discipline}");
    }
}

fn line_merge() -> String {
    model(
        "320s",
        &[
            "{id: s1, type: Source, interarrival: 10s}",
            "{id: s2, type: Source, interarrival: 15s}",
            "{id: m, type: Machine, cycle_time: 100s}",
            "{id: sink, type: Sink}",
        ],
        &["[s1, m, sink]", "[s2, m]"],
    )
}

/// ADR-0022 §3: waiting offers are served in the order they arrived.
///
/// - s1's part at 10: m 10-110. Its next is at 20, and it offers then.
/// - s2's part at 15 offers then.
/// - At 110 the oldest is s2's (15). Its next is at 125.
/// - At 210 the oldest is s1's (20). Its next is at 220.
/// - At 310 the oldest is s2's (125).
#[test]
fn a_merge_serves_the_oldest_offer_first() {
    let r = run(&line_merge());
    let m = r.index("m");
    let order: Vec<(u64, &str)> = r
        .records
        .iter()
        .filter_map(|record| match record {
            Record::Event {
                time,
                target,
                from,
                message: FlowMessage::Arrive(_),
                ..
            } if target.index() == m => Some((secs(*time), r.name(*from))),
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. } => None,
        })
        .collect();
    assert_eq!(order, [(10, "s1"), (110, "s2"), (210, "s1"), (310, "s2")]);
    assert_eq!(r.deliveries(), [110, 210, 310]);
}

fn line_branch() -> String {
    model(
        "40s",
        &[
            "{id: source, type: Source}",
            "{id: first, type: Machine, cycle_time: 1s}",
            "{id: a, type: Machine, cycle_time: 10s}",
            "{id: b, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &[
            "[source, first]",
            "{from: first, to: [a, b]}",
            "[a, sink]",
            "[b, sink]",
        ],
    )
}

/// ADR-0022 §3: the part goes to the first target with room, in list order.
///
/// - p1: first 0-1, a free: a 1-11.
/// - p2: first 1-2, a busy: b 2-12.
/// - p3: first 2-3, waits to 11: a 11-21.
/// - p4: first 11-12, b ends at 12: b 12-22.
/// - p5: first 12-13, waits to 21: a 21-31.
/// - p6: first 21-22: b 22-32.
/// - p7: first 22-23, waits to 31: a from 31.
/// - p8: first 31-32: b from 32.
#[test]
fn a_branch_gives_each_part_to_the_first_target_with_room() {
    let r = run(&line_branch());
    assert_eq!(r.arrivals_at("a"), [1, 11, 21, 31]);
    assert_eq!(r.arrivals_at("b"), [2, 12, 22, 32]);
    assert_eq!(r.deliveries(), [11, 12, 21, 22, 31, 32]);
    assert_eq!(
        r.timeline("first"),
        [
            (0, "busy"),
            (3, "blocked"),
            (11, "busy"),
            (13, "blocked"),
            (21, "busy"),
            (23, "blocked"),
            (31, "busy"),
            (33, "blocked")
        ]
    );
}

fn line_branch_target_down() -> String {
    model(
        "25s",
        &[
            "{id: source, type: Source}",
            "{id: first, type: Machine, cycle_time: 1s}",
            "{id: a, type: Machine, cycle_time: 2s, mtbf: 5s, mttr: 20s}",
            "{id: b, type: Machine, cycle_time: 2s}",
            "{id: sink, type: Sink}",
        ],
        &[
            "[source, first]",
            "{from: first, to: [a, b]}",
            "[a, sink]",
            "[b, sink]",
        ],
    )
}

/// A target that fails takes no more parts, and the others go on; an offer it
/// queued is withdrawn when the part goes elsewhere (ADR-0022 §4).
///
/// - p1: first 0-1, a 1-3, delivered 3.
/// - p2: first 1-2, a busy, b 2-4, delivered 4.
/// - p3: first 2-3, a 3-5; the failure at 5 comes first and a holds it with zero
///   left until the repair at 25, delivered 25.
/// - p4: first 3-4, b 4-6, delivered 6.
/// - From then on b takes one every 2 s: delivered 8, 10, ..., 24.
/// - At 25 first has just finished one and a has room: a takes it.
#[test]
fn a_branch_goes_round_a_target_that_is_down() {
    let r = run(&line_branch_target_down());
    let mut expected = vec![3, 4];
    expected.extend((3..=12).map(|k| 2 * k));
    expected.push(25);
    assert_eq!(r.deliveries(), expected);
    assert_eq!(r.arrivals_at("a"), [1, 3, 25]);
    let b: Vec<u64> = (1..=12).map(|k| 2 * k).collect();
    assert_eq!(r.arrivals_at("b"), b);
}

// ---------------------------------------------------------------------------
// Conservation and bounds on every hand model, and at every event.
// ---------------------------------------------------------------------------

/// What a hand model is called, its document, and its capacities.
type HandModel = (&'static str, String, Vec<(&'static str, u64)>);

fn hand_models() -> Vec<HandModel> {
    vec![
        ("one machine", line_one_machine(), vec![("m", 1)]),
        (
            "five components",
            line_five_components(),
            vec![("cut", 1), ("buf1", 5), ("weld", 1)],
        ),
        (
            "line a",
            line_a_page_1(),
            vec![("cut", 1), ("buf1", 20), ("weld", 1)],
        ),
        ("blocking", line_blocking(), vec![("a", 1), ("b", 1)]),
        ("starving", line_starving(), vec![("a", 1), ("b", 1)]),
        ("idle", line_idle_and_starved(), vec![("a", 1), ("b", 1)]),
        ("blocked source", line_blocked_source(), vec![("m", 1)]),
        ("batch", line_batch(), vec![("m", 1)]),
        (
            "full buffer",
            line_full_buffer(),
            vec![("a", 1), ("buf", 1), ("b", 1)],
        ),
        ("capacity two", line_capacity_two(), vec![("m", 2)]),
        ("failing", line_failing(), vec![("m", 1)]),
        ("failing idle", line_failing_while_idle(), vec![("m", 1)]),
        (
            "failing blocked",
            line_failing_while_blocked(),
            vec![("a", 1), ("b", 1)],
        ),
        (
            "failing, still blocked",
            line_failing_and_still_blocked(),
            vec![("a", 1), ("b", 1)],
        ),
        (
            "capacity two failing",
            line_capacity_two_failing(),
            vec![("m", 2)],
        ),
        ("per variant", line_per_variant(), vec![("m", 1)]),
        ("matrix", line_matrix(), vec![("m", 1)]),
        (
            "free change",
            line_free_change_is_remembered(),
            vec![("m", 1)],
        ),
        ("both", line_matrix_and_per_variant(), vec![("m", 1)]),
        ("setup failing", line_setup_failing(), vec![("m", 1)]),
        ("waits", line_change_waits_for_cycles(), vec![("m", 2)]),
        ("order", line_units_keep_their_order(), vec![("m", 3)]),
        (
            "free waits",
            line_free_change_does_not_wait(),
            vec![("m", 2)],
        ),
        (
            "finished",
            line_change_does_not_wait_for_finished(),
            vec![("m", 2), ("slow", 1)],
        ),
        ("fifo", line_discipline("fifo"), vec![("q", 100), ("m", 1)]),
        ("lifo", line_discipline("lifo"), vec![("q", 100), ("m", 1)]),
        (
            "priority",
            line_discipline("priority"),
            vec![("q", 100), ("m", 1)],
        ),
        ("merge", line_merge(), vec![("m", 1)]),
        (
            "branch",
            line_branch(),
            vec![("first", 1), ("a", 1), ("b", 1)],
        ),
        (
            "branch down",
            line_branch_target_down(),
            vec![("first", 1), ("a", 1), ("b", 1)],
        ),
    ]
}

/// Spec §4, ADR-0022 §10: in every model above, at every record, a part is in
/// exactly one place, and that place has room for it.
#[test]
fn conservation_and_bounds_hold_at_every_record_of_every_hand_model() {
    for (name, text, capacities) in hand_models() {
        println!("{name}");
        assert_flow_invariants(&run(&text), &capacities);
    }
}

/// ADR-0022 §5, §9: what each machine and Source note states, in every model.
#[test]
fn notes_agree_with_their_states_in_every_hand_model() {
    for (name, text, _) in hand_models() {
        println!("{name}");
        assert_note_invariants(&run(&text));
    }
}

/// Spec §4: conservation "at every event". Cutting a run after k events, for
/// every k, the engine's counts add up: created = held + in transit + delivered.
#[test]
fn conservation_holds_after_every_single_event() {
    let chosen = [
        "failing, still blocked",
        "capacity two failing",
        "branch down",
    ];
    for (name, text, _) in hand_models().into_iter().filter(|m| chosen.contains(&m.0)) {
        let m = load_ok(&text);
        for k in 1..=200 {
            let r = run_model(&m, RunOptions::new().max_events(k));
            let items = r.report.items;
            assert_eq!(
                items.created.get(),
                items.held.get()
                    + items.in_transit.get()
                    + items.delivered.get()
                    + items.scrapped.get()
                    + items.consumed.get(),
                "{name}, after {k} events"
            );
        }
    }
}

/// ADR-0022 §5's table, copied by hand, against the public `successors`.
#[test]
fn successors_are_the_table_of_the_adr() {
    use MachineState::{Blocked, Busy, Down, Idle, Setup, Starved};
    for state in [Idle, Busy, Blocked, Starved, Down, Setup] {
        let got: BTreeSet<&str> = state.successors().iter().map(|s| s.as_str()).collect();
        let want: BTreeSet<&str> = table(state).iter().map(|s| s.as_str()).collect();
        assert_eq!(got, want, "{state:?}");
    }
}

/// ADR-0022 §8 and §9, copied by hand: priorities, tags, sizes and the format
/// name. Each is part of the trace contract.
#[test]
fn priorities_tags_and_sizes_are_the_adrs() {
    let attrs = ItemAttrs::new(VariantId::new(1), 2);
    let item = ItemId::from_raw(9);
    let note = MachineStateNote {
        state: MachineState::Busy,
        units_in_cycle: 1,
        setup_variant: None,
    };
    let source = SourceStateNote {
        state: SourceState::Waiting,
        held: 0,
    };
    // (message, priority, tag, encoded bytes): a tag and then its fixed fields.
    let cases = [
        (FlowMessage::Failure, 0, 0, 1),
        (FlowMessage::Repaired, 1, 1, 1),
        (FlowMessage::CycleDone { item }, 2, 2, 1 + 8),
        (FlowMessage::SetupDone, 2, 3, 1),
        (FlowMessage::NextArrival, 3, 4, 1),
        (FlowMessage::Arrive(attrs), 4, 5, 1 + 2 + 1 + 1 + 8),
        (FlowMessage::Accept, 5, 6, 1),
        (FlowMessage::Decline, 6, 7, 1),
        (FlowMessage::Offer, 7, 8, 1),
        (FlowMessage::MachineState(note), 8, 9, 1 + 1 + 4 + 1 + 2),
        (FlowMessage::SourceState(source), 8, 10, 1 + 1 + 4),
        (FlowMessage::Withdraw, 7, 12, 1),
    ];
    for (message, priority, tag, size) in cases {
        assert_eq!(message.priority(), priority, "{message:?}");
        let bytes = encode_message(&message).unwrap();
        assert_eq!(bytes[0], tag, "{message:?}");
        assert_eq!(bytes.len(), size, "{message:?}");
    }
    let name = FlowMessage::VariantName {
        id: VariantId::new(0),
        name: "a".into(),
    };
    assert_eq!(name.priority(), 8);
    assert_eq!(encode_message(&name).unwrap()[0], 11);
    assert_eq!(FlowMessage::FORMAT, "taktsim-components/1");
}

/// ADR-0022 §7: names sort, `default` is one of them when a Source declares none,
/// and an `Arrive` carries the id and the Source's priority; due is absent in M5.
/// `{zeta, alpha}` and a Source with none give alpha 0, default 1, zeta 2.
#[test]
fn variant_ids_index_the_sorted_names_and_attributes_travel() {
    let r = run(&model(
        "100s",
        &[
            "{id: s1, type: Source, interarrival: 10s, variants: {zeta: 100%}, priority: 255}",
            "{id: s2, type: Source, interarrival: 15s, variants: {alpha: 100%}}",
            "{id: s3, type: Source, interarrival: 20s}",
            "{id: sink, type: Sink}",
        ],
        &["[s1, sink]", "[s2, sink]", "[s3, sink]"],
    ));
    let mut names = BTreeMap::new();
    for record in &r.records {
        if let Record::Note {
            message: FlowMessage::VariantName { id, name },
            time,
            ..
        } = record
        {
            assert_eq!(secs(*time), 0, "a Source names its variants at start");
            names.insert(id.get(), name.clone());
        }
    }
    assert_eq!(
        names,
        BTreeMap::from([
            (0, "alpha".to_string()),
            (1, "default".to_string()),
            (2, "zeta".to_string())
        ])
    );
    for record in &r.records {
        if let Record::Event {
            from,
            message: FlowMessage::Arrive(attrs),
            ..
        } = record
        {
            let expected = match r.name(*from) {
                "s1" => (2, 255),
                "s2" => (0, 0),
                "s3" => (1, 0),
                other => panic!("an Arrive from {other}"),
            };
            assert_eq!((attrs.variant.get(), attrs.priority), expected);
            assert_eq!(attrs.due, None);
        }
    }
}

/// ADR-0022 §7: on demand, `ItemCreated` and the first `Arrive` have the same
/// time, `held` is always 0, and only one part exists at the Source at a time.
#[test]
fn an_on_demand_source_makes_a_part_when_it_is_taken() {
    let r = run(&line_blocking());
    let mut made: BTreeMap<ItemId, SimTime> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for record in &r.records {
        match record {
            Record::ItemCreated { time, item, .. } => {
                made.insert(*item, *time);
            }
            Record::Event {
                time,
                item: Some(item),
                message: FlowMessage::Arrive(_),
                ..
            } if seen.insert(*item) => assert_eq!(*time, made[item]),
            Record::Event { .. } | Record::ItemDisposed { .. } | Record::Note { .. } => {}
        }
    }
    assert!(r.source_notes("source").iter().all(|(_, n)| n.held == 0));
    assert!(
        r.source_notes("source")
            .iter()
            .all(|(_, n)| n.state == SourceState::Waiting)
    );
}

// ---------------------------------------------------------------------------
// A second oracle: the departure recursion of a line with blocking after
// service. It is arithmetic on paper, not a simulator of the protocol.
// ---------------------------------------------------------------------------

/// A line: a Source with a buffer behind it, then stations each with a buffer
/// behind it (the last one's is unused). A buffer of 0 is no buffer.
#[derive(Debug, Clone)]
struct Chain {
    interarrival: Option<u64>,
    first_buffer: u64,
    stations: Vec<(u64, u64)>,
}

impl Chain {
    fn yaml(&self, horizon: u64) -> String {
        let mut components = vec![match self.interarrival {
            Some(t) => format!("{{id: s, type: Source, interarrival: {t}s}}"),
            None => "{id: s, type: Source}".to_string(),
        }];
        let mut path = vec!["s".to_string()];
        let buffer =
            |id: String, cap: u64, components: &mut Vec<String>, path: &mut Vec<String>| {
                if cap > 0 {
                    components.push(format!("{{id: {id}, type: Buffer, capacity: {cap} parts}}"));
                    path.push(id);
                }
            };
        buffer(
            "q".to_string(),
            self.first_buffer,
            &mut components,
            &mut path,
        );
        let last = self.stations.len() - 1;
        for (index, (cycle, after)) in self.stations.iter().enumerate() {
            let id = format!("m{index}");
            components.push(format!("{{id: {id}, type: Machine, cycle_time: {cycle}s}}"));
            path.push(id);
            if index < last {
                buffer(format!("q{index}"), *after, &mut components, &mut path);
            }
        }
        components.push("{id: sink, type: Sink}".to_string());
        path.push("sink".to_string());
        let refs: Vec<&str> = components.iter().map(String::as_str).collect();
        let flow = format!("[{}]", path.join(", "));
        model(&format!("{horizon}s"), &refs, &[&flow])
    }

    /// Job by job, station by station:
    ///
    /// - `leave(j, i) = max(finish(j, i), leave(j - B_i - 1, i + 1))`: a part leaves
    ///   a station when its cycle is over and a place is free ahead, where the
    ///   `B_i + 1` places ahead are the buffer's and the next station's.
    /// - `start(j, i) = max(leave(j, i - 1), leave(j - 1, i))`.
    /// - A Source makes the first part after one interval and the next one
    ///   interval after the last left; on demand the interval is zero.
    fn schedule(&self, horizon: u64) -> Schedule {
        let n = self.stations.len();
        let gap = self.interarrival.unwrap_or(0);
        let mut schedule = Schedule::default();
        let mut made = gap;
        loop {
            let j = schedule.leave.len();
            let ahead = usize::try_from(self.first_buffer).unwrap() + 1;
            let room = if j >= ahead {
                schedule.leave[j - ahead][0]
            } else {
                0
            };
            let released = made.max(room);
            schedule.created.push(if self.interarrival.is_some() {
                made
            } else {
                released
            });
            let (mut start, mut finish, mut leave) = (vec![0; n], vec![0; n], vec![0; n]);
            let mut before = released;
            for i in 0..n {
                let (cycle, after) = self.stations[i];
                let free = if j == 0 { 0 } else { schedule.leave[j - 1][i] };
                start[i] = before.max(free);
                finish[i] = start[i] + cycle;
                let ahead = usize::try_from(after).unwrap() + 1;
                let room = if i + 1 < n && j >= ahead {
                    schedule.leave[j - ahead][i + 1]
                } else {
                    0
                };
                leave[i] = finish[i].max(room);
                before = leave[i];
            }
            schedule.start.push(start);
            schedule.finish.push(finish);
            schedule.leave.push(leave);
            made = released + gap;
            if released > horizon {
                return schedule;
            }
        }
    }
}

#[derive(Debug, Default)]
struct Schedule {
    created: Vec<u64>,
    start: Vec<Vec<u64>>,
    finish: Vec<Vec<u64>>,
    leave: Vec<Vec<u64>>,
}

impl Schedule {
    /// Seconds station `i` spends in each state up to the horizon: waiting
    /// before each start, busy in the cycle, blocked between finish and leave.
    fn shares(&self, i: usize, horizon: u64) -> BTreeMap<&'static str, u64> {
        let waiting = if i == 0 { "idle" } else { "starved" };
        let mut totals = BTreeMap::new();
        let mut free = 0;
        for j in 0..self.leave.len() {
            let clip = |t: u64| t.min(horizon);
            let (start, finish, leave) = (self.start[j][i], self.finish[j][i], self.leave[j][i]);
            *totals.entry(waiting).or_insert(0) += clip(start) - clip(free);
            *totals.entry("busy").or_insert(0) += clip(finish) - clip(start);
            *totals.entry("blocked").or_insert(0) += clip(leave) - clip(finish);
            free = leave;
        }
        totals.into_iter().filter(|(_, spent)| *spent > 0).collect()
    }
}

fn assert_chain_matches_the_recursion(chain: &Chain, horizon: u64) {
    let r = run(&chain.yaml(horizon));
    let plan = chain.schedule(horizon);
    let upto =
        |times: Vec<u64>| -> Vec<u64> { times.into_iter().filter(|t| *t <= horizon).collect() };
    assert_eq!(r.created(), upto(plan.created.clone()), "created {chain:?}");
    let n = chain.stations.len();
    for i in 0..n {
        let name = format!("m{i}");
        let starts = plan.start.iter().map(|s| s[i]).collect();
        assert_eq!(
            r.arrivals_at(&name),
            upto(starts),
            "starts of {name} {chain:?}"
        );
        assert_eq!(
            r.shares(&name),
            plan.shares(i, horizon),
            "shares of {name} {chain:?}"
        );
    }
    let leaves = plan.leave.iter().map(|l| l[n - 1]).collect();
    assert_eq!(r.deliveries(), upto(leaves), "deliveries {chain:?}");
}

/// The recursion's own hand check: line A with 20 places gives page 1's count.
#[test]
fn the_recursion_reproduces_the_hand_counted_lines() {
    let line_a = Chain {
        interarrival: None,
        first_buffer: 0,
        stations: vec![(42, 20), (45, 0)],
    };
    let plan = line_a.schedule(28800);
    assert_eq!(plan.leave[0][1], 87);
    assert_eq!(plan.leave.iter().filter(|l| l[1] <= 28800).count(), 639);
    assert_chain_matches_the_recursion(&line_a, 28800);
}

fn fixed_cases(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20_261_108),
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(fixed_cases(150))]

    /// Any deterministic line, with or without buffers and a blocked Source:
    /// every creation, start and delivery time and every state share equals the
    /// recursion's. A lost wake-up in the protocol would show as a time too late.
    #[test]
    fn a_deterministic_chain_follows_the_departure_recursion(
        interarrival in proptest::option::of(1u64..=60),
        first_buffer in 0u64..=3,
        stations in proptest::collection::vec((1u64..=50, 0u64..=3), 1..=4),
    ) {
        let chain = Chain { interarrival, first_buffer, stations };
        assert_chain_matches_the_recursion(&chain, 400);
    }
}

#[derive(Debug, Clone)]
struct RandomStation {
    cycle: u64,
    capacity: u64,
    failing: Option<(u64, u64)>,
    setup: bool,
    buffer: u64,
    discipline: usize,
}

fn random_station() -> impl Strategy<Value = RandomStation> {
    (
        1u64..=40,
        1u64..=3,
        proptest::option::of((20u64..=200, 1u64..=30)),
        any::<bool>(),
        0u64..=3,
        0usize..3,
    )
        .prop_map(
            |(cycle, capacity, failing, setup, buffer, discipline)| RandomStation {
                cycle,
                capacity,
                failing,
                setup,
                buffer,
                discipline,
            },
        )
}

proptest! {
    #![proptest_config(fixed_cases(60))]

    /// M5's property: random chains, with everything on. Conservation and bounds
    /// at every record, the state table on every note, and the trace replays.
    #[test]
    fn random_chains_keep_every_invariant_and_replay(
        seed in any::<u64>(),
        gap in proptest::option::of(1u64..=30),
        batch in 1u64..=3,
        stations in proptest::collection::vec(random_station(), 1..=4),
    ) {
        let mut components = vec![match gap {
            Some(gap) => format!(
                "{{id: s, type: Source, interarrival: {{dist: exponential, mean: {gap}s}}, batch: {batch} parts, variants: {{a: 50%, b: 50%}}}}"
            ),
            None => "{id: s, type: Source, variants: {a: 50%, b: 50%}}".to_string(),
        }];
        let mut path = vec!["s".to_string()];
        let mut capacities = Vec::new();
        for (index, s) in stations.iter().enumerate() {
            let mut machine = format!(
                "{{id: m{index}, type: Machine, cycle_time: {{dist: exponential, mean: {}s}}, capacity: {} parts",
                s.cycle, s.capacity
            );
            if let Some((mtbf, mttr)) = s.failing {
                write!(
                    machine,
                    ", mtbf: {{dist: exponential, mean: {mtbf}s}}, mttr: {{dist: exponential, mean: {mttr}s}}"
                ).unwrap();
            }
            if s.setup {
                machine.push_str(", setup: {per_variant: {dist: exponential, mean: 3s}}");
            }
            machine.push('}');
            components.push(machine);
            path.push(format!("m{index}"));
            capacities.push((format!("m{index}"), s.capacity));
            if s.buffer > 0 {
                let discipline = ["fifo", "lifo", "priority"][s.discipline];
                components.push(format!(
                    "{{id: q{index}, type: Buffer, capacity: {} parts, discipline: {discipline}}}",
                    s.buffer
                ));
                path.push(format!("q{index}"));
                capacities.push((format!("q{index}"), s.buffer));
            }
        }
        components.push("{id: sink, type: Sink}".to_string());
        path.push("sink".to_string());
        let refs: Vec<&str> = components.iter().map(String::as_str).collect();
        let flow = format!("[{}]", path.join(", "));
        let text = model("1000s", &refs, &[&flow]);

        let r = run_seeded(&text, seed);
        let bounds: Vec<(&str, u64)> = capacities.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        assert_flow_invariants(&r, &bounds);
        assert_note_invariants(&r);

        let m = load_ok(&text);
        let registry = registry();
        let mut engine = registry.build(m.document(), EngineConfig::new(seed)).unwrap();
        replay(TraceReader::new(r.bytes.as_slice()).unwrap(), &mut engine).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Long-run rates against closed form, fixed seeds. Each tolerance is about four
// standard errors, worked out in the comment by the test.
// ---------------------------------------------------------------------------

/// What a long run comes to, read from its trace as a stream.
struct Stats {
    end: f64,
    delivered: u64,
    /// Sum of lead times, from release (first `Arrive`) to delivery.
    lead: f64,
    /// The integral of parts in the model from release to delivery.
    wip_area: f64,
    spent: BTreeMap<(String, &'static str), f64>,
    /// The integral of units in a cycle.
    units_area: BTreeMap<String, f64>,
    setups: u64,
}

fn stats(text: &str, seed: u64) -> Stats {
    let m = load_ok(text);
    let registry = registry();
    let (bytes, report) = registry
        .prepare(&m, RunOptions::new().seed(seed))
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    let mut reader = TraceReader::<_, FlowMessage>::new(bytes.as_slice()).unwrap();
    let names = reader.header().components.clone();
    let end = report.summary.now.as_nanos();
    let mut out = Stats {
        end: nanos_f64(end),
        delivered: 0,
        lead: 0.0,
        wip_area: 0.0,
        spent: BTreeMap::new(),
        units_area: BTreeMap::new(),
        setups: 0,
    };
    let mut released: BTreeMap<ItemId, u64> = BTreeMap::new();
    let mut last: BTreeMap<usize, (u64, MachineStateNote)> = BTreeMap::new();
    while let Some(record) = reader.next_record().unwrap() {
        match record {
            Record::Event {
                time,
                item: Some(item),
                message: FlowMessage::Arrive(_),
                ..
            } => {
                released.entry(item).or_insert(time.as_nanos());
            }
            Record::Event {
                message: FlowMessage::SetupDone,
                ..
            } => out.setups += 1,
            Record::ItemDisposed { time, item, .. } => {
                let lead = nanos_f64(time.as_nanos() - released.remove(&item).unwrap());
                out.lead += lead;
                out.wip_area += lead;
                out.delivered += 1;
            }
            Record::Note {
                time,
                by,
                message: FlowMessage::MachineState(note),
            } => {
                let now = time.as_nanos();
                if let Some((since, before)) = last.insert(by.index(), (now, note)) {
                    out.close(&names[by.index()], &before, nanos_f64(now - since));
                }
            }
            Record::Event { .. } | Record::ItemCreated { .. } | Record::Note { .. } => {}
        }
    }
    for (at, (since, note)) in &last {
        out.close(&names[*at], note, nanos_f64(end - since));
    }
    for start in released.values() {
        out.wip_area += nanos_f64(end - start);
    }
    out
}

impl Stats {
    fn close(&mut self, name: &str, note: &MachineStateNote, spent: f64) {
        *self
            .spent
            .entry((name.to_string(), note.state.as_str()))
            .or_insert(0.0) += spent;
        *self.units_area.entry(name.to_string()).or_insert(0.0) +=
            spent * f64::from(note.units_in_cycle);
    }

    fn share(&self, name: &str, state: &'static str) -> f64 {
        self.spent
            .get(&(name.to_string(), state))
            .copied()
            .unwrap_or(0.0)
            / self.end
    }

    fn mean_units(&self, name: &str) -> f64 {
        self.units_area[name] / self.end
    }

    fn rate(&self) -> f64 {
        count_f64(self.delivered) / self.end
    }

    fn mean_lead(&self) -> f64 {
        self.lead / count_f64(self.delivered)
    }
}

fn assert_near(what: &str, got: f64, want: f64, tolerance: f64) {
    assert!(
        (got - want).abs() <= tolerance,
        "{what}: got {got}, want {want} +- {tolerance}"
    );
}

fn queue_line(arrival_mean: &str, machine: &str, horizon: &str) -> String {
    let source = format!(
        "{{id: source, type: Source, interarrival: {{dist: exponential, mean: {arrival_mean}}}}}"
    );
    let m = format!("{{id: m, type: Machine, {machine}}}");
    model(
        horizon,
        &[
            &source,
            // Far above anything these loads reach, so the Source is never
            // blocked and the arrivals are the declared Poisson process.
            "{id: q, type: Buffer, capacity: 60000 parts}",
            &m,
            "{id: sink, type: Sink}",
        ],
        &["[source, q, m, sink]"],
    )
}

/// M/M/1 at rho = 0.5: lambda = 1/20, mu = 1/10. W = 1 / (mu - lambda) = 20 s, L =
/// rho / (1 - rho) = 1, the server idle 1 - rho = 0.5 and never starved.
///
/// Error: with relaxation time 1 / (mu (1 - sqrt(rho))^2) = 116 s and Var(L) = 2,
/// the time average over T = 2e6 s has variance about 2 x 2 x 116 / T = 2.3e-4,
/// sd 0.015. So 5 % on W and L is above three sd. The busy share has sd below 0.006:
/// tolerance 0.02. Throughput, 1e5 Poisson arrivals, sd 0.32 %: tolerance 1.5 %.
#[test]
fn m_m_1_at_half_load_matches_closed_form() {
    let s = stats(
        &queue_line(
            "20s",
            "cycle_time: {dist: exponential, mean: 10s}",
            "2000000s",
        ),
        11,
    );
    assert_near("throughput", s.rate(), 0.05, 0.05 * 0.015);
    assert_near("W", s.mean_lead(), 20.0, 1.0);
    assert_near("L", s.wip_area / s.end, 1.0, 0.05);
    assert_near("idle", s.share("m", "idle"), 0.5, 0.02);
    assert_near("busy", s.share("m", "busy"), 0.5, 0.02);
    assert_near("starved", s.share("m", "starved"), 0.0, 0.0);
}

/// M/M/2 at rho = 0.5: lambda = 1/20, S = 20 s, a = 1. P0 = 1 / (1 + a + a^2 / (2 (1 -
/// rho))) = 1/3; W = S / (1 - rho^2) = 26.67 s. The machine is `idle` only with an
/// empty system: 1/3. `busy` is the rest, as any unit in a cycle makes it so (§5).
/// Mean units in a cycle a = 1, so utilization 0.5 (§5, time-weighted).
///
/// Tolerance as for M/M/1: 5 % on W, 0.02 on shares, 0.015 on mean units per place.
#[test]
fn m_m_2_at_half_load_matches_closed_form() {
    let s = stats(
        &queue_line(
            "20s",
            "cycle_time: {dist: exponential, mean: 20s}, capacity: 2 parts",
            "2000000s",
        ),
        12,
    );
    assert_near("throughput", s.rate(), 0.05, 0.05 * 0.015);
    assert_near("W", s.mean_lead(), 80.0 / 3.0, 80.0 / 3.0 * 0.05);
    assert_near("idle", s.share("m", "idle"), 1.0 / 3.0, 0.02);
    assert_near("busy", s.share("m", "busy"), 2.0 / 3.0, 0.02);
    assert_near("mean units", s.mean_units("m"), 1.0, 0.03);
}

/// Two exponential stations with no buffer, saturated, blocking after service.
/// With equal rates mu the chain has three states: (working, empty), (working,
/// working), (blocked, working). Balance gives 1/3 each. So throughput is mu x
/// (1/3 + 1/3) = 2 mu / 3: 1/15 at mu = 1/10. The first station is busy 2/3 and
/// blocked 1/3; the second busy 2/3 and starved 1/3.
///
/// Over 1e6 s, about 66,667 parts; the relative error of the count is below 0.5 %
/// at sd: tolerance 2 %, and 0.015 on shares.
#[test]
fn two_exponential_stations_without_a_buffer_give_two_thirds() {
    let s = stats(
        &model(
            "1000000s",
            &[
                "{id: source, type: Source}",
                "{id: a, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: b, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, a, b, sink]"],
        ),
        13,
    );
    assert_near("throughput", s.rate(), 1.0 / 15.0, 1.0 / 15.0 * 0.02);
    assert_near("a busy", s.share("a", "busy"), 2.0 / 3.0, 0.015);
    assert_near("a blocked", s.share("a", "blocked"), 1.0 / 3.0, 0.015);
    assert_near("b busy", s.share("b", "busy"), 2.0 / 3.0, 0.015);
    assert_near("b starved", s.share("b", "starved"), 1.0 / 3.0, 0.015);
}

/// Availability A = MTBF / (MTBF + MTTR) = 1000 / 1100 = 10/11. Resume means a
/// saturated machine of 10 s makes A / 10 = 1/11 per second (§6), and is down 1/11
/// of the time.
///
/// Alternating renewal: the down share over T has variance 2 A (1 - A) / ((1/1000 +
/// 1/100) T) = 7.5e-6 at T = 2e6, sd 0.0027: tolerance 0.011, and so 0.0011 on the
/// rate.
#[test]
fn a_saturated_machine_runs_at_availability_over_cycle() {
    let s = stats(
        &model(
            "2000000s",
            &[
                "{id: source, type: Source}",
                "{id: m, type: Machine, cycle_time: 10s, mtbf: {dist: exponential, mean: 1000s}, mttr: {dist: exponential, mean: 100s}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ),
        14,
    );
    assert_near("down", s.share("m", "down"), 1.0 / 11.0, 0.011);
    assert_near("busy", s.share("m", "busy"), 10.0 / 11.0, 0.011);
    assert_near("rate", s.rate(), 1.0 / 11.0, 0.011 / 10.0);
}

/// The clock is calendar time, so the down share is the same at half load (§6):
/// 1/11, tolerance as above. With a rate of 1/20 and a cycle of 10 s, busy is 0.5
/// whatever the failures do, as repair time is not work. Idle takes the rest:
/// 1 - 0.5 - 1/11 = 0.409. Stable: 0.5 / (10/11) = 0.55.
#[test]
fn the_down_share_does_not_depend_on_load() {
    let s = stats(
        &queue_line(
            "20s",
            "cycle_time: 10s, mtbf: {dist: exponential, mean: 1000s}, mttr: {dist: exponential, mean: 100s}",
            "2000000s",
        ),
        15,
    );
    assert_near("down", s.share("m", "down"), 1.0 / 11.0, 0.011);
    assert_near("busy", s.share("m", "busy"), 0.5, 0.015);
    assert_near("idle", s.share("m", "idle"), 0.5 - 1.0 / 11.0, 0.02);
}

/// A Source alone: exponential gaps of mean 40 s into a sink. Over 1e6 s, n = 25,000:
/// rate sd 0.63 %, tolerance 2.5 %; a squared coefficient of variation of 1, with
/// variance 8/n, sd 0.018, tolerance 0.072. The first part after the first interval.
#[test]
fn an_exponential_source_has_its_declared_rate_and_variability() {
    let r = run_seeded(
        &model(
            "1000000s",
            &[
                "{id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, sink]"],
        ),
        16,
    );
    let mut times: Vec<u64> = vec![0];
    for record in &r.records {
        if let Record::ItemCreated { time, .. } = record {
            times.push(time.as_nanos());
        }
    }
    let gaps: Vec<f64> = times.windows(2).map(|w| nanos_f64(w[1] - w[0])).collect();
    let n = count_f64(u64::try_from(gaps.len()).unwrap());
    let mean = gaps.iter().sum::<f64>() / n;
    let square = gaps.iter().map(|g| g * g).sum::<f64>() / n;
    assert_near("mean gap", mean, 40.0, 1.0);
    assert_near("scv", (square - mean * mean) / (mean * mean), 1.0, 0.072);
}

/// `{dist: lognormal, mean, sigma}`: `mean` is the distribution's mean, so a
/// saturated machine makes one per 42 s and its cycle's coefficient of variation is
/// sqrt(exp(0.08^2) - 1) = 0.0801. Over 420,000 s, n = 10,000: the mean's sd is 42 x
/// 0.08 / 100 = 0.034, tolerance 0.14; the CV's sd is 0.0801 / sqrt(2n) = 0.0006,
/// tolerance 0.0025.
#[test]
fn a_lognormal_cycle_has_its_declared_mean_and_spread() {
    let r = run_seeded(
        &model(
            "420000s",
            &[
                "{id: source, type: Source}",
                "{id: m, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ),
        17,
    );
    let mut times: Vec<u64> = vec![0];
    for record in &r.records {
        if let Record::ItemDisposed { time, .. } = record {
            times.push(time.as_nanos());
        }
    }
    let gaps: Vec<f64> = times.windows(2).map(|w| nanos_f64(w[1] - w[0])).collect();
    let n = count_f64(u64::try_from(gaps.len()).unwrap());
    let mean = gaps.iter().sum::<f64>() / n;
    let square = gaps.iter().map(|g| g * g).sum::<f64>() / n;
    let cv = ((square - mean * mean).sqrt()) / mean;
    assert_near("mean cycle", mean, 42.0, 0.14);
    assert_near("cv", cv, 0.0801, 0.0025);
}

/// The variant mix: a batch of 4 draws once and all four share a variant (§7, §8).
/// 10,000 batches at 80/20: the share of `a` has sd sqrt(0.16 / 10000) = 0.004:
/// tolerance 0.016.
#[test]
fn the_variant_mix_is_drawn_per_batch_at_its_declared_share() {
    let r = run_seeded(
        &model(
            "10000s",
            &[
                "{id: source, type: Source, interarrival: 1s, batch: 4 parts, variants: {a: 80%, b: 20%}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, sink]"],
        ),
        18,
    );
    let source = r.index("source");
    let mut by_batch: BTreeMap<u64, BTreeSet<u16>> = BTreeMap::new();
    let (mut a, mut parts) = (0u64, 0u64);
    for record in &r.records {
        if let Record::Event {
            time,
            from,
            message: FlowMessage::Arrive(attrs),
            ..
        } = record
            && from.index() == source
        {
            by_batch
                .entry(secs(*time))
                .or_default()
                .insert(attrs.variant.get());
            parts += 1;
            if attrs.variant.get() == 0 {
                a += 1;
            }
        }
    }
    assert!(by_batch.values().all(|variants| variants.len() == 1));
    assert_near("share of a", count_f64(a) / count_f64(parts), 0.8, 0.016);
}

/// A 50/50 on-demand mix: the variant is drawn per part, so a part differs from the
/// one before with probability 1/2. Setups per part 1/2, and a part takes 10 + 2/2 =
/// 11 s, setup share 1/11 = 9.09 % (ADR-0022's check table).
///
/// 100,000 parts: "differs from the one before" is a fair coin each time, so the
/// setups-per-part sd is 0.0016: tolerance 0.0064. With p setups per part, the share
/// is 2p / (10 + 2p) and the rate 1 / (10 + 2p), whose slopes at p = 1/2 are 20/121
/// and 2/121: tolerances 0.0012 and 2 x 0.0064 / 121.
#[test]
fn setups_per_part_and_their_share_at_an_even_mix() {
    let s = stats(
        &model(
            "1100000s",
            &[
                "{id: source, type: Source, variants: {a: 50%, b: 50%}}",
                "{id: m, type: Machine, cycle_time: 10s, setup: {per_variant: 2s}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ),
        19,
    );
    let per_part = count_f64(s.setups) / count_f64(s.delivered);
    assert_near("setups per part", per_part, 0.5, 0.0064);
    assert_near("setup share", s.share("m", "setup"), 1.0 / 11.0, 0.0012);
    assert_near("rate", s.rate(), 1.0 / 11.0, 2.0 * 0.0064 / 121.0);
}

// ---------------------------------------------------------------------------
// Determinism, replay and streams.
// ---------------------------------------------------------------------------

/// Everything random at once, and no two times can tie.
fn stochastic_components() -> Vec<&'static str> {
    vec![
        "{id: source, type: Source, interarrival: {dist: exponential, mean: 40s}, batch: 2 parts, variants: {a: 60%, b: 40%}, priority: 3}",
        "{id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 30s, sigma: 0.2}, mtbf: {dist: exponential, mean: 30min}, mttr: {dist: exponential, mean: 2min}, setup: {per_variant: {dist: exponential, mean: 20s}}}",
        "{id: q, type: Buffer, capacity: 4 parts, discipline: lifo}",
        "{id: weld, type: Machine, cycle_time: {dist: erlang, mean: 25s, k: 3}, capacity: 2 parts}",
        "{id: sink, type: Sink}",
    ]
}

const STOCHASTIC_FLOW: &str = "[source, cut, q, weld, sink]";

fn stochastic(horizon: &str) -> String {
    model(horizon, &stochastic_components(), &[STOCHASTIC_FLOW])
}

/// What each component saw, in order, without engine-wide numbers: item ids and
/// event sequence numbers follow what else is in the model.
fn trails(run: &Run) -> BTreeMap<String, Vec<(u64, String)>> {
    let mut trails: BTreeMap<String, Vec<(u64, String)>> = BTreeMap::new();
    for record in &run.records {
        let (at, what) = match record {
            Record::Event {
                target,
                from,
                message,
                ..
            } => {
                // Everything but the item id, which follows the rest of the model.
                let what = if matches!(message, FlowMessage::CycleDone { .. }) {
                    "CycleDone".to_string()
                } else {
                    format!("{message:?}")
                };
                (*target, format!("{} {what}", run.name(*from)))
            }
            Record::ItemCreated { by, .. } => (*by, "created".to_string()),
            Record::ItemDisposed { by, .. } => (*by, "disposed".to_string()),
            Record::Note { by, message, .. } => (*by, format!("{message:?}")),
        };
        trails
            .entry(run.name(at).to_string())
            .or_default()
            .push((record.time().as_nanos(), what));
    }
    trails
}

/// Principle 3: same model and seed, the same bytes, whichever way it is run: again,
/// on another thread, or with the checks off. A check never changes a result.
#[test]
fn reruns_are_byte_identical() {
    let text = stochastic("8h");
    let m = load_ok(&text);
    let first = run_model(&m, RunOptions::new().seed(7));
    assert!(first.report.items.delivered.get() > 100, "a real run");
    assert_eq!(first.bytes, run_model(&m, RunOptions::new().seed(7)).bytes);
    assert_eq!(
        first.bytes,
        run_model(&m, RunOptions::new().seed(7).without_checks()).bytes
    );
    let threaded = std::thread::spawn({
        let text = text.clone();
        move || run_seeded(&text, 7).bytes
    })
    .join()
    .unwrap();
    assert_eq!(first.bytes, threaded);
}

/// ADR-0022 §2: `build` "can be called once per seed, from several threads",
/// sharing one registry. Each thread's run equals the sequential one.
#[test]
fn one_registry_builds_on_several_threads() {
    use taktsim_core::{NoTrace, RunLimits};
    let text = stochastic("2h");
    let m = load_ok(&text);
    let registry = registry();
    let horizon: SimTime = "2h".parse().unwrap();
    let delivered = |seed| {
        let mut engine = registry
            .build(m.document(), EngineConfig::new(seed))
            .unwrap();
        engine.run(RunLimits::until(horizon), &mut NoTrace).unwrap();
        engine.item_counts().delivered.get()
    };
    let sequential: Vec<u64> = (1..=4).map(delivered).collect();
    let threaded: Vec<u64> = std::thread::scope(|scope| {
        let handles: Vec<_> = (1..=4)
            .map(|seed| scope.spawn(move || delivered(seed)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(sequential, threaded);
}

/// A seed drives the draws: another seed gives other times, and the header names
/// the seed that ran (§2, "a header cannot name a seed that did not run").
#[test]
fn the_seed_changes_the_draws_and_the_header_states_it() {
    let text = stochastic("2h");
    let model_seed = run(&text);
    let other = run_seeded(&text, 2);
    assert_ne!(model_seed.bytes, other.bytes);
    assert_ne!(trails(&model_seed), trails(&other));
    let same = run_seeded(&text, 1);
    assert_eq!(
        model_seed.bytes, same.bytes,
        "option 1 equals the model's seed: 1"
    );
    let reader = TraceReader::<_, FlowMessage>::new(other.bytes.as_slice()).unwrap();
    assert_eq!(reader.header().seed, Some(2));
}

/// ADR-0022 §2: neither options nor model give a seed: `RunFailure::NoSeed`, with
/// the path `model.seed`, and no run.
#[test]
fn no_seed_is_an_error_with_its_path() {
    let m = load_ok(
        "format: \"1.0\"\nmodel: {name: t}\nexperiment: {type: terminating, horizon: 1h}\n\
         components: [{id: source, type: Source}, {id: m, type: Machine, cycle_time: 1s}, {id: sink, type: Sink}]\n\
         flow: [[source, m, sink]]\n",
    );
    let registry = registry();
    match registry.prepare(&m, RunOptions::new()) {
        Err(RunFailure::NoSeed { path }) => assert_eq!(path.to_string(), "model.seed"),
        other => panic!("not NoSeed: {other:?}"),
    }
    assert!(registry.prepare(&m, RunOptions::new().seed(3)).is_ok());
}

/// A trace is the run (ADR-0018): it replays on an engine of the same model and
/// seed, and not on one with another seed or another cycle time.
#[test]
fn a_trace_replays_and_a_different_engine_is_found_out() {
    let text = stochastic("2h");
    let m = load_ok(&text);
    let r = run_model(&m, RunOptions::new().seed(9));
    let registry = registry();

    let mut same = registry.build(m.document(), EngineConfig::new(9)).unwrap();
    replay(TraceReader::new(r.bytes.as_slice()).unwrap(), &mut same).unwrap();

    let mut other_seed = registry.build(m.document(), EngineConfig::new(10)).unwrap();
    assert!(
        replay(
            TraceReader::new(r.bytes.as_slice()).unwrap(),
            &mut other_seed
        )
        .is_err()
    );

    let changed = text.replace("mean: 25s", "mean: 26s");
    assert_ne!(changed, text);
    let changed = load_ok(&changed);
    let mut other_model = registry
        .build(changed.document(), EngineConfig::new(9))
        .unwrap();
    assert!(
        replay(
            TraceReader::new(r.bytes.as_slice()).unwrap(),
            &mut other_model
        )
        .is_err()
    );
}

/// Horizon and draws are independent: a shorter run is the longer run cut short,
/// record for record, since a horizon stops the clock and changes no decision.
#[test]
fn a_shorter_horizon_is_a_prefix_of_a_longer_one() {
    let m = load_ok(&stochastic("8h"));
    let short_horizon: SimDuration = "3h".parse().unwrap();
    let short = m.with_horizon(short_horizon).unwrap();
    let long = run_model(&m, RunOptions::new());
    let cut = run_model(&short, RunOptions::new());
    let limit = SimTime::from_nanos(3 * 3600 * NS);
    let prefix: Vec<_> = long
        .records
        .iter()
        .filter(|r| r.time() <= limit)
        .cloned()
        .collect();
    assert_eq!(cut.records, prefix);
}

/// Spec §9, ADR-0022 §8: streams are keyed by component id and purpose. So a line
/// with nothing to do with this one, placed first to shift every component's
/// position, changes nothing that this line's components see.
#[test]
fn adding_an_unrelated_line_changes_nothing_here() {
    let alone = run_seeded(&stochastic("4h"), 21);
    let mut components = vec![
        "{id: x_source, type: Source, interarrival: {dist: exponential, mean: 7s}, variants: {a: 100%}}",
        "{id: x_m, type: Machine, cycle_time: {dist: exponential, mean: 5s}, mtbf: {dist: exponential, mean: 90s}, mttr: {dist: exponential, mean: 9s}}",
        "{id: x_sink, type: Sink}",
    ];
    components.extend(stochastic_components());
    let together = run_seeded(
        &model(
            "4h",
            &components,
            &["[x_source, x_m, x_sink]", STOCHASTIC_FLOW],
        ),
        21,
    );
    let (alone, mut together) = (trails(&alone), trails(&together));
    together.retain(|name, _| !name.starts_with("x_"));
    assert_eq!(alone, together);
}

/// Streams are keyed by id, not position: the same document with its components in
/// another order gives every component the same story, as nothing here ties.
#[test]
fn the_order_of_components_changes_no_draw() {
    let forward = run_seeded(&stochastic("4h"), 22);
    let mut reversed = stochastic_components();
    reversed.reverse();
    let backward = run_seeded(&model("4h", &reversed, &[STOCHASTIC_FLOW]), 22);
    assert_eq!(trails(&forward), trails(&backward));
}

fn delivery_nanos(r: &Run) -> Vec<u64> {
    r.records
        .iter()
        .filter_map(|record| match record {
            Record::ItemDisposed { time, .. } => Some(time.as_nanos()),
            Record::Event { .. } | Record::ItemCreated { .. } | Record::Note { .. } => None,
        })
        .collect()
}

fn creation_nanos(r: &Run) -> Vec<u64> {
    r.records
        .iter()
        .filter_map(|record| match record {
            Record::ItemCreated { time, .. } => Some(time.as_nanos()),
            Record::Event { .. } | Record::ItemDisposed { .. } | Record::Note { .. } => None,
        })
        .collect()
}

/// §8: a Source's `interarrival` stream is its own. Behind a buffer that never
/// fills, the Source is never blocked, so a different machine leaves every creation
/// time as it was, to the nanosecond.
#[test]
fn changing_a_machine_keeps_the_sources_arrivals() {
    let line = |cycle: &str| {
        run_seeded(
            &queue_line(
                "20s",
                &format!("cycle_time: {{dist: exponential, mean: {cycle}}}"),
                "200000s",
            ),
            23,
        )
    };
    let (fast, slow) = (line("5s"), line("15s"));
    assert_ne!(delivery_nanos(&fast), delivery_nanos(&slow));
    assert_eq!(creation_nanos(&fast), creation_nanos(&slow));
}

/// §8: adding failures takes two new streams and shifts no other. Failures only
/// stretch the cycles: with constant up and repair times, removing the downtime
/// from each delivery time gives exactly the delivery times of the same machine
/// with no failures at all. Exact in integers, because a cycle resumes with its
/// remaining nanoseconds.
#[test]
fn failures_stretch_the_cycles_but_do_not_change_a_draw() {
    let plain = run_seeded(
        &model(
            "100000s",
            &[
                "{id: source, type: Source}",
                "{id: m, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ),
        24,
    );
    let failing = run_seeded(
        &model(
            "100000s",
            &[
                "{id: source, type: Source}",
                "{id: m, type: Machine, cycle_time: {dist: exponential, mean: 10s}, mtbf: 997s, mttr: 31s}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ),
        24,
    );
    let repairs = failing.events_at("m", |m| matches!(m, FlowMessage::Repaired));
    assert!(repairs.len() > 50);
    let up_time: Vec<u64> = delivery_nanos(&failing)
        .into_iter()
        .map(|t| {
            let finished = u64::try_from(repairs.iter().filter(|r| **r * NS <= t).count()).unwrap();
            t - finished * 31 * NS
        })
        .collect();
    let plain = delivery_nanos(&plain);
    assert_eq!(up_time.as_slice(), &plain[..up_time.len()]);
}

/// §8: a repeat of the same parameters under another id is another stream. Two
/// lines with the same machine do not make the same parts at the same times.
#[test]
fn two_components_with_the_same_parameters_draw_differently() {
    let r = run_seeded(
        &model(
            "2000s",
            &[
                "{id: s1, type: Source}",
                "{id: left, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: s2, type: Source}",
                "{id: right, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: sink, type: Sink}",
            ],
            &["[s1, left, sink]", "[s2, right, sink]"],
        ),
        25,
    );
    let (left, right) = (r.index("left"), r.index("right"));
    let times = |at: usize| -> Vec<u64> {
        r.records
            .iter()
            .filter_map(|record| match record {
                Record::Event {
                    time,
                    target,
                    message: FlowMessage::CycleDone { .. },
                    ..
                } if target.index() == at => Some(time.as_nanos()),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. } => None,
            })
            .collect()
    };
    assert_ne!(times(left), times(right));
}

// ---------------------------------------------------------------------------
// How a run ends (ADR-0022 §9) and its hashes (§1, §2).
// ---------------------------------------------------------------------------

/// The end marker states the end time and the outcome: at the horizon, for a run
/// that got there, also after reading it back.
#[test]
fn a_finished_trace_states_its_end() {
    let r = run(&line_one_machine());
    let end = r.report.trace.unwrap().end.unwrap();
    assert_eq!(end.outcome, RunOutcome::Horizon);
    assert_eq!(secs(end.time), 100);
    let mut reader = TraceReader::<_, FlowMessage>::new(r.bytes.as_slice()).unwrap();
    while reader.next_record().unwrap().is_some() {}
    let read = reader.summary().unwrap().end.unwrap();
    assert_eq!(read, end);
}

/// A cancelled run ends where it stopped, says so, and has processed nothing if
/// cancelled before the first event. An event limit stops exactly at the limit.
#[test]
fn a_cut_run_states_how_it_was_cut() {
    let m = load_ok(&line_one_machine());
    let flag = AtomicBool::new(true);
    let cancelled = run_model(&m, RunOptions::new().cancel_on(&flag));
    assert_eq!(cancelled.report.summary.outcome, RunOutcome::Cancelled);
    assert_eq!(cancelled.report.summary.events, 0);
    let end = cancelled.report.trace.unwrap().end.unwrap();
    assert_eq!(
        (end.outcome, end.time),
        (RunOutcome::Cancelled, SimTime::START)
    );

    let limited = run_model(&m, RunOptions::new().max_events(10));
    assert_eq!(limited.report.summary.outcome, RunOutcome::EventLimit);
    assert_eq!(limited.report.summary.events, 10);
    assert_eq!(limited.event_count(), 10);
    assert_eq!(
        limited.report.trace.unwrap().end.unwrap().outcome,
        RunOutcome::EventLimit
    );
}

fn hash_of(components: &[&str], flow: &[&str]) -> String {
    format!("{:?}", load_ok(&model("1h", components, flow)).hash())
}

/// ADR-0022 §1: `variants` and `matrix` are sets, so their written order is no part
/// of the hash; a written default stays written, so it is (N1). A different value
/// is a different hash, or the rest proves nothing.
#[test]
fn the_hash_ignores_spelling_and_keeps_written_defaults() {
    let m = |rest: &str| format!("{{id: m, type: Machine, cycle_time: 10s{rest}}}");
    let source = |mix: &str| format!("{{id: s, type: Source, interarrival: 1s, variants: {mix}}}");
    let sink = "{id: sink, type: Sink}";
    let flow = ["[s, m, sink]"];
    let of = |mix: &str, rest: &str| hash_of(&[&source(mix), &m(rest), sink], &flow);

    let base = of("{a: 30%, b: 70%}", "");
    assert_eq!(base, of("{b: 70%, a: 30%}", ""));
    assert_ne!(base, of("{a: 31%, b: 69%}", ""));

    let matrix = |pairs: &str| format!(", setup: {{matrix: [{pairs}]}}");
    let (ab, ba) = ("{from: a, to: b, time: 1s}", "{from: b, to: a, time: 2s}");
    assert_eq!(
        of("{a: 30%, b: 70%}", &matrix(&format!("{ab}, {ba}"))),
        of("{a: 30%, b: 70%}", &matrix(&format!("{ba}, {ab}")))
    );

    assert_ne!(base, of("{a: 30%, b: 70%}", ", capacity: 1 parts"));
    // 60s and 1min are one duration.
    assert_eq!(
        of("{a: 30%, b: 70%}", ", mtbf: 60s, mttr: 1s"),
        of("{a: 30%, b: 70%}", ", mtbf: 1min, mttr: 1s")
    );
}

/// ADR-0022 §2: another horizon is a new version: another model hash, the same
/// system hash, and the original untouched. Zero is refused. The trace's header
/// carries the version that ran.
#[test]
fn another_horizon_is_a_new_version_of_the_model() {
    let m = load_ok(&line_one_machine());
    let half: SimDuration = "50s".parse().unwrap();
    let shorter = m.with_horizon(half).unwrap();
    assert_ne!(shorter.hash(), m.hash());
    assert_eq!(shorter.system_hash(), m.system_hash());
    assert_eq!(m.document().experiment().horizon().to_string(), "100s");
    assert!(m.with_horizon(SimDuration::from_nanos(0)).is_err());

    let registry = registry();
    let prepared = registry.prepare(&shorter, RunOptions::new()).unwrap();
    assert_eq!(
        prepared.header().model_hash.to_string(),
        shorter.hash().to_string()
    );
    assert_eq!(
        prepared.header().system_hash.to_string(),
        m.system_hash().to_string()
    );
    assert_eq!(
        run_model(&shorter, RunOptions::new()).deliveries(),
        [10, 20, 30, 40, 50]
    );
}

// ---------------------------------------------------------------------------
// Malformed input (principle 2): rejected at load, with the field path.
// ---------------------------------------------------------------------------

const SRC: &str = "{id: s, type: Source}";
const MCH: &str = "{id: m, type: Machine, cycle_time: 10s}";
const BUF: &str = "{id: q, type: Buffer, capacity: 2 parts}";
const SNK: &str = "{id: sink, type: Sink}";
const LINE: &str = "[s, m, sink]";

/// A model whose first component is `s`, a Source as written.
fn with_source(source: &str) -> String {
    model("1h", &[source, MCH, SNK], &[LINE])
}

/// A model whose second component is `m`, a Machine as written, after a Source
/// with variants `a` and `b`.
fn with_machine(machine: &str) -> String {
    model(
        "1h",
        &[
            "{id: s, type: Source, variants: {a: 50%, b: 50%}}",
            machine,
            SNK,
        ],
        &[LINE],
    )
}

/// `(what, document, every field path it may be reported at)`. Each is what ADR-0022's
/// rules say, or a type error against a unit. Where the rule spans keys, the path is
/// the entry.
type Malformed = (&'static str, String, Vec<&'static str>);

fn malformed() -> Vec<Malformed> {
    let mut all = malformed_sources();
    all.extend(malformed_machines());
    all.extend(malformed_buffers_and_sinks());
    all.extend(malformed_flows());
    all
}

fn malformed_sources() -> Vec<Malformed> {
    let long = "x".repeat(256);
    let long_variant = format!("{{id: s, type: Source, variants: {{{long}: 100%}}}}");
    let src = |rest: &str| with_source(&format!("{{id: s, type: Source, {rest}}}"));
    let at0 = |key: &'static str| vec![key];
    vec![
        (
            "batch 0",
            src("interarrival: 1s, batch: 0 parts"),
            at0("/components/0/batch"),
        ),
        (
            "batch 65536",
            src("interarrival: 1s, batch: 65536 parts"),
            at0("/components/0/batch"),
        ),
        (
            "batch in seconds",
            src("interarrival: 1s, batch: 2s"),
            at0("/components/0/batch"),
        ),
        (
            "batch on demand",
            src("batch: 2 parts"),
            at0("/components/0"),
        ),
        (
            "interarrival 0",
            src("interarrival: 0s"),
            at0("/components/0/interarrival"),
        ),
        (
            "interarrival in parts",
            src("interarrival: 3 parts"),
            at0("/components/0/interarrival"),
        ),
        (
            "variants empty",
            src("variants: {}"),
            at0("/components/0/variants"),
        ),
        (
            "variants 90",
            src("variants: {a: 40%, b: 50%}"),
            at0("/components/0/variants"),
        ),
        (
            "variants 110",
            src("variants: {a: 60%, b: 50%}"),
            at0("/components/0/variants"),
        ),
        (
            "variants 1e-8 short",
            src("variants: {a: 33.33333333%, b: 33.33333333%, c: 33.33333333%}"),
            at0("/components/0/variants"),
        ),
        (
            "variant share 0",
            src("variants: {a: 0%, b: 100%}"),
            at0("/components/0/variants"),
        ),
        (
            "variant name",
            src("variants: {\"a b\": 100%}"),
            at0("/components/0/variants"),
        ),
        (
            "variant name too long",
            with_source(&long_variant),
            at0("/components/0/variants"),
        ),
        (
            "priority 256",
            src("priority: 256"),
            at0("/components/0/priority"),
        ),
        (
            "priority -1",
            src("priority: -1"),
            at0("/components/0/priority"),
        ),
        (
            "priority 1.5",
            src("priority: 1.5"),
            at0("/components/0/priority"),
        ),
        (
            "priority text",
            src("priority: high"),
            at0("/components/0/priority"),
        ),
        (
            "unknown Source key",
            src("arrival: 1s"),
            at0("/components/0"),
        ),
    ]
}

fn malformed_machines() -> Vec<Malformed> {
    let mch = |rest: &str| with_machine(&format!("{{id: m, type: Machine, {rest}}}"));
    let at0 = |key: &'static str| vec![key];
    vec![
        (
            "no cycle time",
            with_machine("{id: m, type: Machine}"),
            at0("/components/1"),
        ),
        (
            "cycle 0",
            mch("cycle_time: 0s"),
            at0("/components/1/cycle_time"),
        ),
        (
            "cycle in parts",
            mch("cycle_time: 5 parts"),
            at0("/components/1/cycle_time"),
        ),
        (
            "mtbf alone",
            mch("cycle_time: 1s, mtbf: 1h"),
            at0("/components/1"),
        ),
        (
            "mttr alone",
            mch("cycle_time: 1s, mttr: 1h"),
            at0("/components/1"),
        ),
        (
            "mtbf 0",
            mch("cycle_time: 1s, mtbf: 0s, mttr: 1s"),
            at0("/components/1/mtbf"),
        ),
        (
            "mttr 0",
            mch("cycle_time: 1s, mtbf: 1s, mttr: 0s"),
            at0("/components/1/mttr"),
        ),
        (
            "capacity 0",
            mch("cycle_time: 1s, capacity: 0 parts"),
            at0("/components/1/capacity"),
        ),
        (
            "capacity 65536",
            mch("cycle_time: 1s, capacity: 65536 parts"),
            at0("/components/1/capacity"),
        ),
        (
            "setup empty",
            mch("cycle_time: 1s, setup: {}"),
            at0("/components/1/setup"),
        ),
        (
            "per_variant 0",
            mch("cycle_time: 1s, setup: {per_variant: 0s}"),
            at0("/components/1/setup/per_variant"),
        ),
        (
            "from equals to",
            mch("cycle_time: 1s, setup: {matrix: [{from: a, to: a, time: 1s}]}"),
            at0("/components/1/setup/matrix"),
        ),
        (
            "pair twice",
            mch(
                "cycle_time: 1s, setup: {matrix: [{from: a, to: b, time: 1s}, {from: a, to: b, time: 2s}]}",
            ),
            at0("/components/1/setup/matrix"),
        ),
        (
            "unknown variant",
            mch("cycle_time: 1s, setup: {matrix: [{from: a, to: zebra, time: 1s}]}"),
            at0("/components/1/setup/matrix"),
        ),
        (
            "`default` where every Source names its variants",
            mch("cycle_time: 1s, setup: {matrix: [{from: default, to: a, time: 1s}]}"),
            at0("/components/1/setup/matrix"),
        ),
        (
            "unknown setup key",
            mch("cycle_time: 1s, setup: {each: 1s}"),
            at0("/components/1/setup"),
        ),
    ]
}

fn malformed_buffers_and_sinks() -> Vec<Malformed> {
    let at0 = |key: &'static str| vec![key];
    vec![
        (
            "buffer no capacity",
            model(
                "1h",
                &[SRC, "{id: q, type: Buffer}", MCH, SNK],
                &["[s, q, m, sink]"],
            ),
            at0("/components/1"),
        ),
        (
            "buffer capacity 0",
            model(
                "1h",
                &[SRC, "{id: q, type: Buffer, capacity: 0 parts}", MCH, SNK],
                &["[s, q, m, sink]"],
            ),
            at0("/components/1/capacity"),
        ),
        (
            "unknown discipline",
            model(
                "1h",
                &[
                    SRC,
                    "{id: q, type: Buffer, capacity: 1 parts, discipline: FIFO}",
                    MCH,
                    SNK,
                ],
                &["[s, q, m, sink]"],
            ),
            at0("/components/1/discipline"),
        ),
        (
            "sink with a parameter",
            model(
                "1h",
                &[SRC, MCH, "{id: sink, type: Sink, capacity: 1 parts}"],
                &[LINE],
            ),
            at0("/components/2"),
        ),
    ]
}

fn malformed_flows() -> Vec<Malformed> {
    vec![
        (
            "two edges from one",
            model(
                "1h",
                &[SRC, MCH, "{id: other, type: Machine, cycle_time: 1s}", SNK],
                &["[s, m, sink]", "[m, other, sink]"],
            ),
            vec!["/flow"],
        ),
        (
            "a loop",
            model(
                "1h",
                &[SRC, MCH, "{id: other, type: Machine, cycle_time: 1s}", SNK],
                &["[s, m, other]", "[other, m]", "{from: other, to: sink}"],
            ),
            vec!["/flow"],
        ),
        (
            "edge into a Source",
            model("1h", &[SRC, MCH, SNK], &[LINE, "[m, s]"]),
            vec!["/flow"],
        ),
        (
            "edge out of a Sink",
            model("1h", &[SRC, MCH, SNK], &[LINE, "[sink, m]"]),
            vec!["/flow"],
        ),
        (
            "Source not left",
            model("1h", &[SRC, "{id: t, type: Source}", MCH, SNK], &[LINE]),
            vec!["/flow", "/components/1"],
        ),
        (
            "Machine not entered",
            model(
                "1h",
                &[SRC, MCH, "{id: t, type: Machine, cycle_time: 1s}", SNK],
                &[LINE, "[t, sink]"],
            ),
            vec!["/components/2"],
        ),
        (
            "Machine not left",
            model(
                "1h",
                &[SRC, MCH, "{id: t, type: Machine, cycle_time: 1s}", SNK],
                &["{from: s, to: [m, t]}", "[m, sink]"],
            ),
            vec!["/components/2"],
        ),
        (
            "Sink not entered",
            model("1h", &[SRC, MCH, SNK, "{id: other, type: Sink}"], &[LINE]),
            vec!["/flow", "/components/3"],
        ),
        (
            "on demand into a sink",
            model("1h", &[SRC, SNK], &["[s, sink]"]),
            vec!["/flow", "/components/0"],
        ),
        (
            "on demand through a buffer into a sink",
            model("1h", &[SRC, BUF, SNK], &["[s, q, sink]"]),
            vec!["/flow", "/components/0"],
        ),
        (
            "edge to no component",
            model("1h", &[SRC, MCH, SNK], &[LINE, "[m, nowhere]"]),
            vec!["/flow"],
        ),
    ]
}

/// Each one is refused at load, and some error names the expected place.
#[test]
fn every_malformed_document_is_rejected_with_its_field_path() {
    let mut wrong = Vec::new();
    for (what, text, places) in malformed() {
        match load(&text) {
            Ok(_) => wrong.push(format!("{what}: accepted")),
            Err(errors) => {
                let paths: Vec<String> = errors.iter().map(|e| e.path().pointer()).collect();
                let found = paths
                    .iter()
                    .any(|path| places.iter().any(|place| path.starts_with(place)));
                if !found {
                    wrong.push(format!(
                        "{what}: reported at {paths:?}, expected under {places:?}"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// CLAUDE.md, Rust rules: "Each variant can produce a field path, what was found,
/// what is allowed and one valid example." A key that is absent has nothing found,
/// so this asks for what is allowed and the example on every rejection.
#[test]
fn every_rejection_says_what_is_allowed_and_gives_an_example() {
    let mut wrong = Vec::new();
    for (what, text, _) in malformed() {
        if let Err(errors) = load(&text) {
            for error in errors.iter() {
                let kind = error.kind();
                if kind.allowed().is_none() {
                    wrong.push(format!("{what}: no `allowed` in `{error}`"));
                }
                if kind.example().is_none() {
                    wrong.push(format!("{what}: no `example` in `{error}`"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// ADR-0022: `priority` is "0 to 255", so what a rejection calls allowed must say so.
#[test]
fn a_negative_priority_is_told_the_real_range() {
    let errors = load(&with_source("{id: s, type: Source, priority: -1}"))
        .expect_err("a negative priority is refused");
    let allowed = errors.first().kind().allowed().unwrap();
    assert!(allowed.contains("255"), "allowed: {allowed}");
}

/// A document with several faults reports them all, so one load fixes them all.
#[test]
fn several_faults_are_reported_together() {
    let text = model(
        "1h",
        &[
            "{id: s, type: Source, interarrival: 1s, batch: 0 parts}",
            "{id: m, type: Machine, cycle_time: 1s, capacity: 0 parts}",
            SNK,
        ],
        &[LINE],
    );
    let errors = load(&text).expect_err("two faults");
    let paths: BTreeSet<String> = errors.iter().map(|e| e.path().pointer()).collect();
    assert!(paths.contains("/components/0/batch"), "{paths:?}");
    assert!(paths.contains("/components/1/capacity"), "{paths:?}");
}

/// The other side of each limit: exactly at it, a document loads and runs (§ADR-0022
/// Consequences: "A model that loads with the standard catalog always builds").
#[test]
fn every_limit_is_accepted_exactly_at_the_limit() {
    let name = "x".repeat(255);
    let long = format!("{{id: s, type: Source, variants: {{{name}: 100%}}}}");
    let mch =
        |rest: &str| with_machine(&format!("{{id: m, type: Machine, cycle_time: 1s, {rest}}}"));
    let src = |rest: &str| with_source(&format!("{{id: s, type: Source, {rest}}}"));
    let cases = [
        ("priority 0", src("priority: 0")),
        ("priority 255", src("priority: 255")),
        (
            "variants within 1e-9",
            src("variants: {a: 33.3333333333%, b: 33.3333333333%, c: 33.3333333334%}"),
        ),
        ("one variant", src("variants: {a: 100%}")),
        ("a long name", with_source(&long)),
        ("capacity 65535", mch("capacity: 65535 parts")),
        (
            "a 0s matrix entry",
            mch("setup: {matrix: [{from: a, to: b, time: 0s}]}"),
        ),
        (
            "per_variant and matrix",
            mch("setup: {per_variant: 1s, matrix: [{from: a, to: b, time: 0s}]}"),
        ),
        (
            "a distribution in setup",
            mch("setup: {per_variant: {dist: exponential, mean: 1s}}"),
        ),
        (
            "disciplines",
            model(
                "1h",
                &[
                    SRC,
                    "{id: q, type: Buffer, capacity: 1 parts, discipline: lifo}",
                    "{id: r, type: Buffer, capacity: 1 parts, discipline: priority}",
                    MCH,
                    SNK,
                ],
                &["[s, q, r, m, sink]"],
            ),
        ),
        (
            "on demand behind a buffer",
            model("1h", &[SRC, BUF, MCH, SNK], &["[s, q, m, sink]"]),
        ),
    ];
    for (what, text) in cases {
        let m = load(&text).unwrap_or_else(|e| panic!("{what}: {e}"));
        let registry = registry();
        registry
            .prepare(&m, RunOptions::new().max_events(1000))
            .unwrap_or_else(|e| panic!("{what}: {e}"))
            .run_traced(Vec::new())
            .unwrap_or_else(|e| panic!("{what}: {e}"));
    }
}

/// ADR-0022: "a repeating zero-length timer never lets time advance", which is why
/// a constant `0s` is refused. A distribution that can only give zero is the same
/// thing in other words. Either the loader refuses it, or a run must still get to
/// its horizon. Given 100,000 events, a clock that is still at zero is the hang the
/// rule exists to prevent.
#[test]
fn a_distribution_that_is_always_zero_cannot_stall_a_run() {
    let zeros = [
        "{dist: uniform, min: 0s, max: 0s}",
        "{dist: normal, mean: 0s, sd: 0s}",
        "{dist: triangular, min: 0s, mode: 0s, max: 0s}",
        "{dist: exponential, mean: 0s}",
        "{dist: lognormal, mean: 0s, sigma: 0.1}",
        "{dist: gamma, mean: 0s, cv: 1}",
        "{dist: erlang, mean: 0s, k: 2}",
        "{dist: weibull, scale: 0s, shape: 1}",
    ];
    let mut stalled = Vec::new();
    for zero in zeros {
        let keys = [
            (
                "cycle_time",
                model(
                    "1h",
                    &[
                        SRC,
                        &format!("{{id: m, type: Machine, cycle_time: {zero}}}"),
                        SNK,
                    ],
                    &[LINE],
                ),
            ),
            (
                "interarrival",
                model(
                    "1h",
                    &[
                        &format!("{{id: s, type: Source, interarrival: {zero}}}"),
                        SNK,
                    ],
                    &["[s, sink]"],
                ),
            ),
            (
                "mtbf",
                model(
                    "1h",
                    &[
                        SRC,
                        &format!(
                            "{{id: m, type: Machine, cycle_time: 1s, mtbf: {zero}, mttr: 1s}}"
                        ),
                        SNK,
                    ],
                    &[LINE],
                ),
            ),
        ];
        for (key, text) in keys {
            if let Ok(m) = load(&text) {
                let registry = registry();
                let report = registry
                    .prepare(&m, RunOptions::new().max_events(100_000))
                    .unwrap()
                    .run_traced(Vec::new())
                    .unwrap()
                    .1;
                if report.summary.now == SimTime::START {
                    stalled.push(format!("{key}: {zero}"));
                }
            }
        }
    }
    assert!(
        stalled.is_empty(),
        "loaded, and the clock never moved:\n{}",
        stalled.join("\n")
    );
}

// ---------------------------------------------------------------------------
// More hand counts: ties between a failure and what was due at that instant.
// ---------------------------------------------------------------------------

/// A failure at the instant a unit's cycle ends, with a unit waiting for a change.
/// The failure comes first (§6), so the cycle resumes with zero left at the repair,
/// and only then the changeover starts.
///
/// - a at 100: 100-150; the failure at 150 comes first; the repair at 160 ends it.
/// - b at 110 enters the second place and waits: setup 160-180, busy 180-230.
/// - Next failure 160 + 150 = 310, after the horizon.
#[test]
fn a_failure_at_the_end_of_a_cycle_delays_the_waiting_changeover() {
    let r = run(&two_sources(
        "100s",
        "110s",
        "cycle_time: 50s, capacity: 2 parts, setup: {per_variant: 20s}, mtbf: 150s, mttr: 10s",
        "235s",
    ));
    assert_eq!(r.deliveries(), [160, 230]);
    assert_eq!(r.setups_done("m"), [180]);
    assert_eq!(
        r.timeline("m"),
        [
            (0, "idle"),
            (100, "busy"),
            (150, "down"),
            (160, "setup"),
            (180, "busy"),
            (230, "setup")
        ]
    );
}

/// A failure at the instant a changeover ends: it resumes with zero left, so one
/// `SetupDone`, at the repair, and no cycle starts before it.
///
/// a at 100: 100-110. b at 160: setup 160-180; failure at 180 comes first; repair
/// at 185; `SetupDone` at 185; busy 185-195.
#[test]
fn a_failure_at_the_end_of_a_changeover_holds_it_to_the_repair() {
    let r = run(&two_sources(
        "100s",
        "160s",
        "cycle_time: 10s, setup: {per_variant: 20s}, mtbf: 180s, mttr: 5s",
        "199s",
    ));
    assert_eq!(r.setups_done("m"), [185]);
    assert_eq!(r.deliveries(), [110, 195]);
}

/// §8: failure before a new part at one instant, repair before a new part at one
/// instant.
///
/// Failure tie, interval 100: failure at 100 comes first, the part made at 100
/// waits to the repair at 110; m 110-120. The next interval counts from 110: part at
/// 210, and the next failure is at 110 + 100 = 210, the same tie again: m 220-230.
///
/// Repair tie, interval 110: failure at 100, repair at 110, then the part made at
/// 110 is taken at once: m 110-120. Next part at 220; next failure 210, repair 220:
/// m 220-230.
///
/// Both give the same deliveries; only when the part waited differs.
#[test]
fn a_new_part_comes_after_a_failure_or_a_repair_at_one_instant() {
    let line = |interval: &str| {
        let source = format!("{{id: source, type: Source, interarrival: {interval}}}");
        run(&model(
            "235s",
            &[
                &source,
                "{id: m, type: Machine, cycle_time: 10s, mtbf: 100s, mttr: 10s}",
                "{id: sink, type: Sink}",
            ],
            &["[source, m, sink]"],
        ))
    };
    let failure_tie = line("100s");
    assert_eq!(failure_tie.created(), [100, 210]);
    assert_eq!(failure_tie.arrivals_at("m"), [110, 220]);
    assert_eq!(failure_tie.deliveries(), [120, 230]);
    assert_eq!(
        failure_tie.source_timeline("source"),
        [
            (0, "waiting"),
            (100, "blocked"),
            (110, "waiting"),
            (210, "blocked"),
            (220, "waiting")
        ]
    );
    let repair_tie = line("110s");
    assert_eq!(repair_tie.created(), [110, 220]);
    assert_eq!(repair_tie.arrivals_at("m"), [110, 220]);
    assert_eq!(repair_tie.deliveries(), [120, 230]);
    assert_eq!(repair_tie.source_timeline("source"), [(0, "waiting")]);
}

/// A batch larger than the room ahead. Batch of 5 at 100, a buffer of 3 before m:
/// the first part goes through to m, three stay in the buffer, the fifth is held.
///
/// At 110 m's part leaves, the buffer's oldest moves to m, and the fifth enters the
/// buffer, so the batch has left at 110 and the next interval counts from there: 210.
/// m runs 100-110, ..., 140-150, then 210 onward.
#[test]
fn a_batch_larger_than_the_room_ahead_is_held_in_part() {
    let r = run(&model(
        "215s",
        &[
            "{id: source, type: Source, interarrival: 100s, batch: 5 parts}",
            "{id: q, type: Buffer, capacity: 3 parts}",
            "{id: m, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, q, m, sink]"],
    ));
    assert_eq!(
        r.created(),
        [100, 100, 100, 100, 100, 210, 210, 210, 210, 210]
    );
    assert_eq!(
        r.arrivals_at("q"),
        [100, 100, 100, 100, 110, 210, 210, 210, 210]
    );
    assert_eq!(r.deliveries(), [110, 120, 130, 140, 150]);
    assert_eq!(
        r.source_held("source"),
        [(0, 0), (100, 1), (110, 0), (210, 1)]
    );
}

/// ADR-0022: `default` is a name like any other when some Source declares none. From
/// a to `default`: 30 s. Back: `per_variant`, 5 s.
///
/// a at 100: 100-110. `default` at 160: setup 160-190, busy 190-200. a at 200: its
/// Offer comes after m's parts have moved, setup 200-205, busy 205-215. Next a at
/// 300: same variant, 300-310.
#[test]
fn the_default_variant_takes_part_in_the_matrix() {
    let r = run(&model(
        "315s",
        &[
            "{id: sa, type: Source, interarrival: 100s, variants: {a: 100%}}",
            "{id: sb, type: Source, interarrival: 160s}",
            "{id: m, type: Machine, cycle_time: 10s, setup: {per_variant: 5s, matrix: [{from: a, to: default, time: 30s}]}}",
            "{id: sink, type: Sink}",
        ],
        &["[sa, m, sink]", "[sb, m]"],
    ));
    assert_eq!(r.deliveries(), [110, 200, 215, 310]);
    assert_eq!(r.setups_done("m"), [190, 205]);
}

/// A machine with a `setup` and one variant never changes over.
#[test]
fn one_variant_never_costs_a_setup() {
    let r = run(&model(
        "100s",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 10s, setup: {per_variant: 5s}}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    ));
    assert_eq!(r.deliveries().len(), 10);
    assert_eq!(r.setups_done("m"), []);
}

/// §8: shares are cumulative over *sorted* names, so a mix written the other way
/// round, 80 % on the later name, still gives the later name 80 %. Sorted names:
/// aa 0, zz 1. Error as in the mix test above: 0.016.
#[test]
fn a_mix_gives_each_name_its_share_whatever_the_order() {
    let r = run_seeded(
        &model(
            "10000s",
            &[
                "{id: source, type: Source, interarrival: 1s, variants: {zz: 80%, aa: 20%}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, sink]"],
        ),
        26,
    );
    let (mut zz, mut parts) = (0u64, 0u64);
    for record in &r.records {
        if let Record::Event {
            message: FlowMessage::Arrive(attrs),
            ..
        } = record
        {
            parts += 1;
            if attrs.variant.get() == 1 {
                zz += 1;
            }
        }
    }
    assert_near("zz", count_f64(zz) / count_f64(parts), 0.8, 0.016);
}

/// §1: spelling is no part of the model. The same model, written with `variants` in
/// another order, or as JSON, or as the canonical form the run store keeps (§2), has
/// the same hash, and so gives the same trace byte for byte.
#[test]
fn spelling_changes_no_byte_of_a_trace() {
    let yaml = |mix: &str| {
        model(
            "2h",
            &[
                &format!(
                    "{{id: s, type: Source, interarrival: {{dist: exponential, mean: 40s}}, variants: {mix}}}"
                ),
                "{id: m, type: Machine, cycle_time: {dist: exponential, mean: 30s}, setup: {per_variant: 5s}}",
                "{id: sink, type: Sink}",
            ],
            &["[s, m, sink]"],
        )
    };
    let m = load_ok(&yaml("{a: 30%, b: 70%}"));
    let bytes = run_model(&m, RunOptions::new()).bytes;
    let other_order = load_ok(&yaml("{b: 70%, a: 30%}"));
    assert_eq!(bytes, run_model(&other_order, RunOptions::new()).bytes);

    let reloaded = Loader::new(registry().catalog())
        .load_str(m.canonical_json(), Format::Json)
        .unwrap();
    assert_eq!(reloaded.hash(), m.hash());
    assert_eq!(bytes, run_model(&reloaded, RunOptions::new()).bytes);

    let json = r#"{"format": "1.0", "model": {"name": "t", "seed": 1},
        "experiment": {"type": "terminating", "horizon": "2h"},
        "components": [
          {"id": "s", "type": "Source", "interarrival": {"dist": "exponential", "mean": "40s"},
           "variants": {"b": "70%", "a": "30%"}},
          {"id": "m", "type": "Machine", "cycle_time": {"dist": "exponential", "mean": "30s"},
           "setup": {"per_variant": "5s"}},
          {"id": "sink", "type": "Sink"}],
        "flow": [["s", "m", "sink"]]}"#;
    let from_json = Loader::new(registry().catalog())
        .load_str(json, Format::Json)
        .unwrap();
    assert_eq!(from_json.hash(), m.hash());
}

/// ADR-0022 §2: the canonical form of a version made by `with_horizon` loads back as
/// that version.
#[test]
fn the_canonical_form_of_another_horizon_loads_back_as_that_version() {
    let m = load_ok(&line_one_machine());
    let shorter = m.with_horizon("50s".parse().unwrap()).unwrap();
    let back = Loader::new(registry().catalog())
        .load_str(shorter.canonical_json(), Format::Json)
        .unwrap();
    assert_eq!(back.hash(), shorter.hash());
    assert_eq!(back.system_hash(), m.system_hash());
}

/// `RunOutcome::EventLimit` is "`max_events` events were processed and another one
/// is due". This line has exactly 74 events by its horizon and the next is after it,
/// so a limit of exactly 74 ends at the horizon; a limit of 73 does not.
#[test]
fn a_limit_equal_to_the_run_is_not_a_cut() {
    let m = load_ok(&line_one_machine());
    let exact = run_model(&m, RunOptions::new().max_events(74));
    assert_eq!(exact.report.summary.outcome, RunOutcome::Horizon);
    assert_eq!(secs(exact.report.summary.now), 100);
    let short = run_model(&m, RunOptions::new().max_events(73));
    assert_eq!(short.report.summary.outcome, RunOutcome::EventLimit);
}

/// A trace that was cut is still the run so far (§9): it replays, at every length,
/// including mid-instant, and its end marker says how it ended.
#[test]
fn a_cut_trace_replays_at_every_length() {
    let m = load_ok(&line_failing_while_blocked());
    let registry = registry();
    for k in 1..=60 {
        let r = run_model(&m, RunOptions::new().max_events(k));
        let mut engine = registry.build(m.document(), EngineConfig::new(1)).unwrap();
        let replayed = replay(TraceReader::new(r.bytes.as_slice()).unwrap(), &mut engine);
        assert!(replayed.is_ok(), "after {k} events: {:?}", replayed.err());
    }
}

/// Nothing happens before the horizon: a Source whose first interval ends after it.
/// The first part comes after one interval (§7). Only start notes, no events, the
/// clock at the horizon, and the end stated.
#[test]
fn a_run_where_nothing_is_due_still_ends_at_its_horizon() {
    let r = run(&model(
        "1h",
        &[
            "{id: source, type: Source, interarrival: 2h}",
            "{id: m, type: Machine, cycle_time: 1s}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    ));
    assert_eq!(r.event_count(), 0);
    assert_eq!(secs(r.report.summary.now), 3600);
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    assert_eq!(
        r.report.trace.unwrap().end.unwrap().outcome,
        RunOutcome::Horizon
    );
    assert_eq!(r.timeline("m"), [(0, "idle")]);
}

/// Principle 2: "Invalid input is rejected at load time ... never during a run", and
/// ADR-0022: "a model that loads must run the same everywhere". The clock holds 584
/// years (a `u64` of nanoseconds, 213,503 days). A duration that loads, drawn or
/// constant, whose end falls beyond that is either refused at load or just never
/// happens, as anything after the horizon does not. It must not fail a run.
///
/// - Exponential cycles of mean 100,000 days: a draw above the limit has probability
///   exp(-2.135) = 12 %, so 64 seeds meet it almost surely (1 - 0.88^64 = 0.9997). The
///   horizon is 1 h, so every run is a machine that never finishes.
/// - A constant cycle of 150,000 days and a horizon of 213,000 days: the first part
///   ends at 150,000; the next would end at 300,000, after both horizon and clock.
///   One part delivered.
/// - A constant `mtbf` of 150,000 days, the same horizon: one failure at 150,000; the
///   next would be at 300,000. Cycles of 1000 days, so 212 parts or so.
#[test]
fn a_duration_past_the_end_of_the_clock_does_not_fail_a_run() {
    let long_draws = model(
        "1h",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: {dist: exponential, mean: 100000d}}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    );
    let constant = model(
        "213000d",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 150000d}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    );
    let failing = model(
        "213000d",
        &[
            "{id: source, type: Source}",
            "{id: m, type: Machine, cycle_time: 1000d, mtbf: 150000d, mttr: 1d}",
            "{id: sink, type: Sink}",
        ],
        &["[source, m, sink]"],
    );
    let registry = registry();
    let mut wrong = Vec::new();
    for (what, text, seeds) in [
        ("long draws", &long_draws, 1..=64u64),
        ("constant cycle", &constant, 1..=1),
        ("constant mtbf", &failing, 1..=1),
    ] {
        let Ok(m) = load(text) else {
            continue;
        };
        for seed in seeds {
            let outcome = registry
                .prepare(&m, RunOptions::new().seed(seed))
                .unwrap()
                .run_traced(Vec::new());
            if let Err(failure) = outcome {
                wrong.push(format!("{what}, seed {seed}: {failure}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

// ---------------------------------------------------------------------------
// Random graphs, read against ADR-0022 §4 and §7 from the trace alone.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Source { on_demand: bool },
    Machine { capacity: u64 },
    Buffer { capacity: u64, discipline: usize },
    Sink,
}

/// A graph: each node with its kind and the targets of its one edge, in order.
#[derive(Debug, Clone)]
struct Graph {
    nodes: Vec<(String, Kind, Vec<String>)>,
}

/// Walks a trace with what ADR-0022 §4 and §7 promise of the protocol:
///
/// - an offer is outstanding at most once per neighbour;
/// - an `Accept` goes to the oldest waiting offer (§3, merge);
/// - a machine gives up a part only after its cycle, oldest finished first;
/// - a buffer gives its part by its discipline, chosen at the `Accept`, in entry
///   order;
/// - at the end of every instant, no part could still have moved: nothing that holds
///   a part and is up has a target with room that is up (§4: "a wake-up cannot be
///   lost").
struct ProtocolWalk<'a> {
    graph: &'a Graph,
    index: BTreeMap<&'a str, usize>,
    fill: Vec<i64>,
    /// Accepts delivered and not yet answered, by target.
    reserved: Vec<i64>,
    down: Vec<bool>,
    waiting: Vec<Vec<usize>>,
    finished: Vec<Vec<ItemId>>,
    /// A buffer's parts in entry order, with their priority.
    parts: Vec<Vec<(ItemId, u8)>>,
    chosen: BTreeMap<(usize, usize), Option<ItemId>>,
}

impl<'a> ProtocolWalk<'a> {
    fn new(graph: &'a Graph, names: &'a [String]) -> Self {
        let index: BTreeMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(at, name)| (name.as_str(), at))
            .collect();
        let n = names.len();
        Self {
            graph,
            index,
            fill: vec![0; n],
            reserved: vec![0; n],
            down: vec![false; n],
            waiting: vec![Vec::new(); n],
            finished: vec![Vec::new(); n],
            parts: vec![Vec::new(); n],
            chosen: BTreeMap::new(),
        }
    }

    fn discipline_of(&self, at: usize) -> Option<usize> {
        self.graph
            .nodes
            .iter()
            .find_map(|(name, kind, _)| match kind {
                Kind::Buffer { discipline, .. } if self.index[name.as_str()] == at => {
                    Some(*discipline)
                }
                Kind::Source { .. } | Kind::Machine { .. } | Kind::Buffer { .. } | Kind::Sink => {
                    None
                }
            })
    }

    fn machine_at(&self, at: usize) -> bool {
        self.graph.nodes.iter().any(|(name, kind, _)| {
            matches!(kind, Kind::Machine { .. }) && self.index[name.as_str()] == at
        })
    }

    /// The part a buffer must choose now.
    fn choose(&mut self, buffer: usize, discipline: usize) -> Option<ItemId> {
        let parts = &mut self.parts[buffer];
        if parts.is_empty() {
            return None;
        }
        let at = match discipline {
            0 => 0,
            1 => parts.len() - 1,
            _ => {
                let top = parts.iter().map(|(_, p)| *p).max()?;
                parts.iter().position(|(_, p)| *p == top)?
            }
        };
        Some(parts.remove(at).0)
    }

    fn event(&mut self, record: &Record<FlowMessage>) {
        let Record::Event {
            time,
            target,
            from,
            message,
            item,
            ..
        } = record
        else {
            return;
        };
        let (target, from) = (target.index(), from.index());
        match message {
            FlowMessage::Offer => {
                assert!(
                    !self.waiting[target].contains(&from),
                    "at {time}: a second offer from {from} to {target}"
                );
                self.waiting[target].push(from);
            }
            FlowMessage::Withdraw => self.waiting[target].retain(|w| *w != from),
            FlowMessage::Accept => {
                assert_eq!(
                    self.waiting[from].first(),
                    Some(&target),
                    "at {time}: {from} answered {target} before an older offer: {:?}",
                    self.waiting[from]
                );
                self.waiting[from].remove(0);
                self.reserved[from] += 1;
                if let Some(discipline) = self.discipline_of(target) {
                    let pick = self.choose(target, discipline);
                    self.chosen.insert((target, from), pick);
                }
            }
            FlowMessage::Decline => {
                self.reserved[target] -= 1;
                if let Some(pick) = self.chosen.remove(&(from, target)) {
                    assert_eq!(pick, None, "at {time}: {from} declined with a part to give");
                }
            }
            FlowMessage::Arrive(attrs) => {
                let item = item.unwrap();
                self.reserved[target] -= 1;
                self.fill[from] -= 1;
                self.fill[target] += 1;
                if let Some(pick) = self.chosen.remove(&(from, target)) {
                    assert_eq!(pick, Some(item), "at {time}: {from} gave the wrong part");
                }
                if self.machine_at(from) {
                    assert_eq!(
                        self.finished[from].first(),
                        Some(&item),
                        "at {time}: {from} gave up a part out of finishing order"
                    );
                    self.finished[from].remove(0);
                }
                if self.discipline_of(target).is_some() {
                    self.parts[target].push((item, attrs.priority));
                }
            }
            FlowMessage::CycleDone { item } => self.finished[target].push(*item),
            FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. } => {}
        }
    }

    fn apply(&mut self, record: &Record<FlowMessage>) {
        match record {
            Record::Event { .. } => self.event(record),
            Record::ItemCreated { by, .. } => self.fill[by.index()] += 1,
            Record::ItemDisposed { by, .. } => self.fill[by.index()] -= 1,
            Record::Note {
                by,
                message: FlowMessage::MachineState(note),
                ..
            } => self.down[by.index()] = note.state == MachineState::Down,
            Record::Note { .. } => {}
        }
    }

    fn room(&self, at: usize, kind: &Kind) -> bool {
        let used = self.fill[at] + self.reserved[at];
        match kind {
            Kind::Sink => true,
            Kind::Machine { capacity } | Kind::Buffer { capacity, .. } => {
                !self.down[at] && used < i64::try_from(*capacity).unwrap()
            }
            Kind::Source { .. } => false,
        }
    }

    fn end_of_instant(&self, time: SimTime) {
        for (name, kind, targets) in &self.graph.nodes {
            let at = self.index[name.as_str()];
            let has_part = match kind {
                Kind::Source { on_demand } => *on_demand || self.fill[at] > 0,
                Kind::Buffer { .. } => self.fill[at] > 0,
                Kind::Machine { .. } => !self.finished[at].is_empty(),
                Kind::Sink => false,
            };
            if !has_part || self.down[at] {
                continue;
            }
            for target in targets {
                let to = self.index[target.as_str()];
                let (_, target_kind, _) = self
                    .graph
                    .nodes
                    .iter()
                    .find(|(n, _, _)| n == target)
                    .unwrap();
                assert!(
                    !self.room(to, target_kind),
                    "at {time}: `{name}` has a part and `{target}` has room"
                );
            }
        }
    }
}

fn assert_protocol(run: &Run, graph: &Graph) {
    let mut walk = ProtocolWalk::new(graph, &run.names);
    let mut last = SimTime::START;
    for record in &run.records {
        if record.time() > last {
            walk.end_of_instant(last);
            last = record.time();
        }
        walk.apply(record);
    }
    walk.end_of_instant(last);
}

#[derive(Debug, Clone)]
struct GraphPlan {
    nodes: Vec<(bool, u64, usize)>,
    extra_target: Vec<Option<usize>>,
    second_source: Option<(usize, u8)>,
    on_demand: bool,
}

fn graph_plan() -> impl Strategy<Value = GraphPlan> {
    (2usize..=6)
        .prop_flat_map(|n| {
            (
                proptest::collection::vec((any::<bool>(), 1u64..=3, 0usize..3), n),
                proptest::collection::vec(proptest::option::of(0usize..8), n),
                proptest::option::of((0usize..n, 1u8..=9)),
                any::<bool>(),
            )
        })
        .prop_map(
            |(nodes, extra_target, second_source, on_demand)| GraphPlan {
                nodes,
                extra_target,
                second_source,
                on_demand,
            },
        )
}

/// Builds the plan's document: a backbone `s -> n0 -> n1 -> ... -> sink`, with a second
/// target on some nodes (any later node, or the sink) and perhaps a second Source. A
/// node that a part cannot avoid is a machine at the head, so most plans load.
fn build_graph(plan: &GraphPlan, seed_failures: bool) -> (Graph, String) {
    let n = plan.nodes.len();
    let name = |i: usize| format!("n{i}");
    let mut components = Vec::new();
    let mut nodes = Vec::new();
    let on_demand = plan.on_demand;
    components.push(if on_demand {
        "{id: s, type: Source, variants: {a: 50%, b: 50%}}".to_string()
    } else {
        "{id: s, type: Source, interarrival: {dist: exponential, mean: 4s}, variants: {a: 50%, b: 50%}}".to_string()
    });
    nodes.push(("s".to_string(), Kind::Source { on_demand }, vec![name(0)]));
    if let Some((into, priority)) = plan.second_source {
        components.push(format!(
            "{{id: s2, type: Source, interarrival: {{dist: exponential, mean: 6s}}, priority: {priority}, variants: {{a: 100%}}}}"
        ));
        nodes.push((
            "s2".to_string(),
            Kind::Source { on_demand: false },
            vec![name(into)],
        ));
    }
    for (i, (is_machine, capacity, discipline)) in plan.nodes.iter().enumerate() {
        let next = if i + 1 < n {
            name(i + 1)
        } else {
            "sink".to_string()
        };
        let mut targets = vec![next];
        if let Some(extra) = plan.extra_target[i] {
            let extra = if i + 2 + extra < n {
                name(i + 2 + extra)
            } else {
                "sink".to_string()
            };
            if !targets.contains(&extra) {
                targets.push(extra);
            }
        }
        // The first node is a machine, so an on-demand Source always passes one.
        if *is_machine || i == 0 {
            let failing = if seed_failures && i % 2 == 0 {
                ", mtbf: {dist: exponential, mean: 60s}, mttr: {dist: exponential, mean: 9s}"
            } else {
                ""
            };
            components.push(format!(
                "{{id: {}, type: Machine, cycle_time: {{dist: exponential, mean: 3s}}, capacity: {capacity} parts, setup: {{per_variant: 1s}}{failing}}}",
                name(i)
            ));
            nodes.push((
                name(i),
                Kind::Machine {
                    capacity: *capacity,
                },
                targets,
            ));
        } else {
            let word = ["fifo", "lifo", "priority"][*discipline];
            components.push(format!(
                "{{id: {}, type: Buffer, capacity: {capacity} parts, discipline: {word}}}",
                name(i)
            ));
            nodes.push((
                name(i),
                Kind::Buffer {
                    capacity: *capacity,
                    discipline: *discipline,
                },
                targets,
            ));
        }
    }
    components.push("{id: sink, type: Sink}".to_string());
    nodes.push(("sink".to_string(), Kind::Sink, vec![]));
    let flow: Vec<String> = nodes
        .iter()
        .filter(|(_, _, targets)| !targets.is_empty())
        .map(|(id, _, targets)| format!("{{from: {id}, to: [{}]}}", targets.join(", ")))
        .collect();
    let components: Vec<&str> = components.iter().map(String::as_str).collect();
    let flow: Vec<&str> = flow.iter().map(String::as_str).collect();
    (Graph { nodes }, model("600s", &components, &flow))
}

proptest! {
    #![proptest_config(fixed_cases(80))]

    /// Random acyclic graphs with branches, merges, capacities, disciplines and
    /// failures: the protocol's promises hold in every trace, no part is ever stuck
    /// with room ahead, and the model is still delivering in its last half: no
    /// deadlock (§4).
    #[test]
    fn random_graphs_keep_the_protocols_promises(
        seed in any::<u64>(),
        failing in any::<bool>(),
        plan in graph_plan(),
    ) {
        let (graph, text) = build_graph(&plan, failing);
        let m = load(&text);
        prop_assume!(m.is_ok());
        let r = run_model(&m.unwrap(), RunOptions::new().seed(seed));
        assert_protocol(&r, &graph);
        assert_note_invariants(&r);
        let capacities: Vec<(&str, u64)> = graph
            .nodes
            .iter()
            .filter_map(|(name, kind, _)| match kind {
                Kind::Machine { capacity } | Kind::Buffer { capacity, .. } => {
                    Some((name.as_str(), *capacity))
                }
                Kind::Source { .. } | Kind::Sink => None,
            })
            .collect();
        assert_flow_invariants(&r, &capacities);
        let late = r.records.iter().any(|record| {
            matches!(record, Record::ItemDisposed { .. })
                && record.time().as_nanos() > 300 * NS
        });
        prop_assert!(late, "nothing delivered after 300 s");
    }
}

// ---------------------------------------------------------------------------
// The checkers above must be able to fail: each is shown a damaged trace.
// ---------------------------------------------------------------------------

fn without_first_arrive(mut r: Run) -> Run {
    let at = r
        .records
        .iter()
        .position(|record| {
            matches!(
                record,
                Record::Event {
                    message: FlowMessage::Arrive(_),
                    ..
                }
            )
        })
        .unwrap();
    r.records.remove(at);
    r
}

#[test]
#[should_panic(expected = "outlived the instant")]
fn the_flow_walk_notices_a_reservation_with_no_answer() {
    let r = without_first_arrive(run(&line_blocking()));
    assert_flow_invariants(&r, &[("a", 1), ("b", 1)]);
}

#[test]
#[should_panic(expected = "capacity 0")]
fn the_flow_walk_notices_too_many_parts_in_one_place() {
    assert_flow_invariants(&run(&line_full_buffer()), &[("buf", 0)]);
}

#[test]
#[should_panic(expected = "has room")]
fn the_protocol_walk_notices_a_part_that_could_have_moved() {
    // The graph claims b has five places where it has one: then a's finished part
    // and b's four free places look like a lost wake-up.
    let r = run(&line_blocking());
    let graph = Graph {
        nodes: vec![
            (
                "source".to_string(),
                Kind::Source { on_demand: true },
                vec!["a".to_string()],
            ),
            (
                "a".to_string(),
                Kind::Machine { capacity: 1 },
                vec!["b".to_string()],
            ),
            (
                "b".to_string(),
                Kind::Machine { capacity: 5 },
                vec!["sink".to_string()],
            ),
            ("sink".to_string(), Kind::Sink, vec![]),
        ],
    };
    assert_protocol(&r, &graph);
}

// ---------------------------------------------------------------------------
// More of the protocol and the streams.
// ---------------------------------------------------------------------------

/// ADR-0022's check table: "`Withdraw` is 0 on a line without a branch", and a
/// `Decline` answers an `Accept` only when the outlet has nothing, which on a chain
/// without failures never happens: each offer stands for a part that stays.
#[test]
fn a_chain_without_failures_needs_no_withdraw_and_no_decline() {
    for text in [
        line_blocking(),
        line_full_buffer(),
        line_batch(),
        line_a_page_1(),
    ] {
        let r = run(&text);
        for record in &r.records {
            if let Record::Event { message, time, .. } = record {
                assert!(
                    !matches!(message, FlowMessage::Withdraw | FlowMessage::Decline),
                    "{message:?} at {time}"
                );
            }
        }
    }
}

/// An on-demand Source in front of a branch: "always has a part on offer", so each
/// target takes one whenever it has room. a runs 10 s and b 30 s, both from 0.
///
/// a takes at 0, 10, ..., 100; b at 0, 30, 60, 90. 15 parts, made as they are taken.
/// Delivered: a's at 10, ..., 100, and b's at 30, 60, 90.
#[test]
fn an_on_demand_source_feeds_each_target_of_a_branch() {
    let r = run(&model(
        "100s",
        &[
            "{id: source, type: Source}",
            "{id: a, type: Machine, cycle_time: 10s}",
            "{id: b, type: Machine, cycle_time: 30s}",
            "{id: sink, type: Sink}",
        ],
        &["{from: source, to: [a, b]}", "[a, sink]", "[b, sink]"],
    ));
    let a: Vec<u64> = (0..=10).map(|k| 10 * k).collect();
    assert_eq!(r.arrivals_at("a"), a);
    assert_eq!(r.arrivals_at("b"), [0, 30, 60, 90]);
    assert_eq!(
        r.deliveries(),
        [10, 20, 30, 30, 40, 50, 60, 60, 70, 80, 90, 90, 100]
    );
    assert_eq!(r.report.items.created.get(), 15);
    assert_eq!(r.report.items.held.get(), 2);
}

/// Two on-demand Sources into one machine take turns, oldest offer first. Both offer
/// at time zero, s1 first in document order, so m takes s1's. s1's next offer comes
/// after s2's, which is now the oldest, and so on: s1, s2, s1, s2, ...
#[test]
fn two_on_demand_sources_take_turns() {
    let r = run(&model(
        "50s",
        &[
            "{id: s1, type: Source}",
            "{id: s2, type: Source}",
            "{id: m, type: Machine, cycle_time: 10s}",
            "{id: sink, type: Sink}",
        ],
        &["[s1, m, sink]", "[s2, m]"],
    ));
    let m = r.index("m");
    let order: Vec<(u64, &str)> = r
        .records
        .iter()
        .filter_map(|record| match record {
            Record::Event {
                time,
                target,
                from,
                message: FlowMessage::Arrive(_),
                ..
            } if target.index() == m => Some((secs(*time), r.name(*from))),
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. } => None,
        })
        .collect();
    assert_eq!(
        order,
        [
            (0, "s1"),
            (10, "s2"),
            (20, "s1"),
            (30, "s2"),
            (40, "s1"),
            (50, "s2")
        ]
    );
}

/// The trace's own account of itself and its header (ADR-0021, §2): every record
/// counted, the components in document order, the formats named.
#[test]
fn a_trace_accounts_for_itself() {
    let r = run(&line_five_components());
    let summary = r.report.trace.unwrap();
    assert_eq!(summary.records, u64::try_from(r.records.len()).unwrap());
    assert_eq!(summary.events, r.report.summary.events);
    assert_eq!(summary.bytes, u64::try_from(r.bytes.len()).unwrap());
    assert_eq!(r.names, ["source", "cut", "buf1", "weld", "sink"]);
    let reader = TraceReader::<_, FlowMessage>::new(r.bytes.as_slice()).unwrap();
    assert_eq!(reader.header().message_format, "taktsim-components/1");
    assert_eq!(reader.header().seed, Some(1));
}

fn event_nanos(r: &Run, name: &str, pick: fn(&FlowMessage) -> bool) -> Vec<u64> {
    let at = r.index(name);
    r.records
        .iter()
        .filter_map(|record| match record {
            Record::Event {
                time,
                target,
                message,
                ..
            } if target.index() == at && pick(message) => Some(time.as_nanos()),
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. } => None,
        })
        .collect()
}

/// §8: `failures` and `repairs` are separate streams, drawn in turn. So a different
/// repair mean leaves every up time as it was, to the nanosecond, and a different
/// up-time mean leaves every repair time as it was.
#[test]
fn up_times_and_repair_times_are_separate_streams() {
    let line = |mtbf: &str, mttr: &str| {
        let machine = format!(
            "{{id: m, type: Machine, cycle_time: 5s, mtbf: {{dist: exponential, mean: {mtbf}}}, mttr: {{dist: exponential, mean: {mttr}}}}}"
        );
        run_seeded(
            &model(
                "100000s",
                &[
                    "{id: source, type: Source}",
                    &machine,
                    "{id: sink, type: Sink}",
                ],
                &["[source, m, sink]"],
            ),
            27,
        )
    };
    let times = |r: &Run| {
        (
            event_nanos(r, "m", |m| matches!(m, FlowMessage::Failure)),
            event_nanos(r, "m", |m| matches!(m, FlowMessage::Repaired)),
        )
    };
    let up = |(failures, repairs): &(Vec<u64>, Vec<u64>)| -> Vec<u64> {
        let mut ups = vec![failures[0]];
        ups.extend(failures[1..].iter().zip(repairs).map(|(f, r)| f - r));
        ups
    };
    let repair = |(failures, repairs): &(Vec<u64>, Vec<u64>)| -> Vec<u64> {
        repairs.iter().zip(failures).map(|(r, f)| r - f).collect()
    };
    let base = times(&line("200s", "5s"));
    let other_repair = times(&line("200s", "50s"));
    let other_up = times(&line("100s", "5s"));
    let (a, b) = (up(&base), up(&other_repair));
    let n = a.len().min(b.len());
    assert!(n > 50);
    assert_eq!(a[..n], b[..n]);
    assert_ne!(repair(&base)[0], repair(&other_repair)[0]);
    let (a, b) = (repair(&base), repair(&other_up));
    let n = a.len().min(b.len());
    assert!(n > 50);
    assert_eq!(a[..n], b[..n]);
}

/// §8: the `variant` stream is apart from the `interarrival` stream, so a different
/// interval gives the same sequence of variants.
#[test]
fn the_variant_stream_is_apart_from_the_interval() {
    let line = |gap: &str| {
        let source =
            format!("{{id: s, type: Source, interarrival: {gap}, variants: {{a: 50%, b: 50%}}}}");
        let r = run_seeded(
            &model(
                "3000s",
                &[&source, "{id: sink, type: Sink}"],
                &["[s, sink]"],
            ),
            28,
        );
        r.records
            .iter()
            .filter_map(|record| match record {
                Record::Event {
                    message: FlowMessage::Arrive(attrs),
                    ..
                } => Some(attrs.variant.get()),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. } => None,
            })
            .collect::<Vec<u16>>()
    };
    let (slow, fast) = (line("3s"), line("1s"));
    assert_eq!(slow.len(), 1000);
    assert_eq!(fast.len(), 3000);
    assert_eq!(slow[..], fast[..1000]);
    assert!(slow.contains(&0) && slow.contains(&1));
}

/// M/D/1 at rho = 0.5: Pollaczek-Khinchine with S = 10 s constant: W = S + rho S / (2
/// (1 - rho)) = 15 s. A constant cycle takes a path of its own through the machine.
/// Its error is below M/M/1's, so 5 % on W stays above three sd.
#[test]
fn m_d_1_at_half_load_matches_pollaczek_khinchine() {
    let s = stats(&queue_line("20s", "cycle_time: 10s", "2000000s"), 29);
    assert_near("W", s.mean_lead(), 15.0, 0.75);
    assert_near("idle", s.share("m", "idle"), 0.5, 0.02);
}

/// Two M/M/1 stations in a series, with buffers that never fill: by Burke's theorem
/// the second sees Poisson arrivals, so each has rho = 0.5 and W = 20 s: 40 s in all.
/// The second waits starved, not idle (§5): a share of 0.5. Little's law ties W and
/// L: L = lambda W = 2, exactly on the estimates but for the parts still inside.
/// Tolerances: two independent stations at 1 sd = 1.5 % each, so 8 % is above 3 sd.
#[test]
fn a_tandem_of_two_queues_matches_burke() {
    let s = stats(
        &model(
            "2000000s",
            &[
                "{id: source, type: Source, interarrival: {dist: exponential, mean: 20s}}",
                "{id: q1, type: Buffer, capacity: 60000 parts}",
                "{id: a, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: q2, type: Buffer, capacity: 60000 parts}",
                "{id: b, type: Machine, cycle_time: {dist: exponential, mean: 10s}}",
                "{id: sink, type: Sink}",
            ],
            &["[source, q1, a, q2, b, sink]"],
        ),
        30,
    );
    assert_near("W", s.mean_lead(), 40.0, 3.2);
    assert_near("a idle", s.share("a", "idle"), 0.5, 0.02);
    assert_near("b starved", s.share("b", "starved"), 0.5, 0.02);
    assert_near("b idle", s.share("b", "idle"), 0.0, 0.0);
    assert_near("L", s.wip_area / s.end, s.rate() * s.mean_lead(), 0.01);
}

// ---------------------------------------------------------------------------
// More of what a document may not say.
// ---------------------------------------------------------------------------

fn observing(names: &[&str]) -> String {
    let mut text = model(
        "1h",
        &[
            "{id: s, type: Source}",
            "{id: cut, type: Machine, cycle_time: 42s}",
            "{id: buf1, type: Buffer, capacity: 1 parts}",
            "{id: weld, type: Machine, cycle_time: 45s}",
            "{id: sink, type: Sink}",
        ],
        &["[s, cut, buf1, weld, sink]"],
    );
    text.push_str("observe:\n");
    for name in names {
        text.push_str("  - ");
        text.push_str(name);
        text.push('\n');
    }
    text
}

/// ADR-0022, Observables: `throughput(Sink)`, `lead_time(Sink)`, `wip(Buffer)`,
/// `state_shares(Machine, ...)`, as spec §4's example and page 6 write them. Each
/// wrong type of component is refused at `observe`'s path, every argument checked.
#[test]
fn observables_take_only_the_components_they_are_about() {
    load_ok(&observing(&[
        "throughput(sink)",
        "lead_time(sink)",
        "wip(buf1)",
        "state_shares(cut)",
        "state_shares(cut, weld)",
    ]));
    let mut wrong = Vec::new();
    for name in [
        "throughput(cut)",
        "lead_time(buf1)",
        "wip(weld)",
        "state_shares(buf1)",
        "state_shares(cut, buf1)",
        "state_shares(cut, sink)",
        "wip(nothing)",
        "nothing(sink)",
    ] {
        match load(&observing(&[name])) {
            Ok(_) => wrong.push(format!("{name}: accepted")),
            Err(errors) => {
                if !errors
                    .iter()
                    .any(|e| e.path().pointer().starts_with("/observe/0"))
                {
                    wrong.push(format!("{name}: {errors}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// ADR-0022 §3: "at most one edge leaves a component in M5, in any spelling", and a
/// target is one neighbour with one outstanding offer (§4). So the same edge written
/// twice, and one target listed twice, are refused. Ids and types too.
#[test]
fn an_edge_and_an_id_are_said_only_once() {
    let cases = [
        (
            "the same edge twice",
            model("1h", &[SRC, MCH, SNK], &[LINE, "[s, m]"]),
        ),
        (
            "one target listed twice",
            model(
                "1h",
                &[SRC, MCH, SNK],
                &["{from: s, to: [m, m]}", "[m, sink]"],
            ),
        ),
        (
            "an edge to itself",
            model("1h", &[SRC, MCH, SNK], &["[s, m, m, sink]"]),
        ),
        ("an id twice", model("1h", &[SRC, MCH, MCH, SNK], &[LINE])),
        (
            "an unknown type",
            model(
                "1h",
                &[SRC, "{id: m, type: Robot, cycle_time: 1s}", SNK],
                &[LINE],
            ),
        ),
        (
            "a branch around the only machine",
            model(
                "1h",
                &[SRC, MCH, BUF, SNK],
                &["{from: s, to: [m, q]}", "[m, sink]", "[q, sink]"],
            ),
        ),
    ];
    let mut accepted = Vec::new();
    for (what, text) in cases {
        if load(&text).is_ok() {
            accepted.push(what);
        }
    }
    assert!(accepted.is_empty(), "accepted: {accepted:?}");
}

/// ADR-0020: `description` and `x-*` are reserved in a component entry, and the
/// loader accepts only `id` and `type` today, so each is refused at its own path. A
/// component type may not read them either (ADR-0022 calls them "not parameters").
#[test]
fn reserved_keys_are_refused_for_now() {
    let text = model(
        "100s",
        &[
            "{id: source, type: Source, description: raw material}",
            "{id: a, type: Machine, cycle_time: 10s, x-note: first}",
            "{id: b, type: Machine, cycle_time: 30s}",
            "{id: sink, type: Sink, description: the end}",
        ],
        &["[source, a, b, sink]"],
    );
    let errors = load(&text).expect_err("reserved keys are refused today");
    let paths: BTreeSet<String> = errors.iter().map(|e| e.path().pointer()).collect();
    assert_eq!(
        paths,
        BTreeSet::from([
            "/components/0/description".to_string(),
            "/components/1/x-note".to_string(),
            "/components/3/description".to_string()
        ])
    );
}
