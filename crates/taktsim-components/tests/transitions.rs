//! Machine states: every transition of ADR-0022 §5's table occurs in a model and
//! nothing else does, and a state always says what the machine holds.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use support::{Run, document, run};
use taktsim_components::{FlowMessage, MachineState};
use taktsim_units::SimTime;

use MachineState::{Blocked, Busy, Down, Idle, Setup, Starved};

/// The state changes of `name`, as (from, to).
fn changes(r: &Run, name: &str) -> Vec<(MachineState, MachineState)> {
    r.machine_notes(name)
        .windows(2)
        .filter(|pair| pair[0].1.state != pair[1].1.state)
        .map(|pair| (pair[0].1.state, pair[1].1.state))
        .collect()
}

/// A model where each machine goes through many states: variants, failures,
/// blocking, a buffer, a merge.
fn rich(seed: u64) -> Run {
    let yaml = document(
        "40h",
        "  - {id: s1, type: Source, interarrival: {dist: exponential, mean: 50s}, variants: {red: 50%, blue: 50%}}\n  \
         - {id: first, type: Machine, cycle_time: {dist: exponential, mean: 30s}, setup: {per_variant: 20s}, mtbf: {dist: exponential, mean: 7min}, mttr: {dist: exponential, mean: 50s}}\n  \
         - {id: buf, type: Buffer, capacity: 1 parts}\n  \
         - {id: second, type: Machine, cycle_time: {dist: exponential, mean: 45s}, setup: {per_variant: 15s}, mtbf: {dist: exponential, mean: 5min}, mttr: {dist: exponential, mean: 40s}, capacity: 2 parts}\n  \
         - {id: third, type: Machine, cycle_time: {dist: exponential, mean: 25s}, mtbf: {dist: exponential, mean: 11min}, mttr: 1min}\n  \
         - {id: sink, type: Sink}",
        "  - [s1, first, buf, second, third, sink]",
    );
    run(&yaml, seed)
}

#[test]
fn every_transition_in_the_table() {
    let r = rich(2026);
    let mut seen: BTreeSet<(MachineState, MachineState)> = BTreeSet::new();
    for name in ["first", "second", "third"] {
        seen.extend(changes(&r, name));
    }
    // Nothing outside the table.
    for (from, to) in &seen {
        assert!(
            from.successors().contains(to),
            "{} to {} is not in the table",
            from.as_str(),
            to.as_str()
        );
    }
    // Everything in the table that this mix of machines can show.
    let expected = [
        // `first` is fed by a Source: idle.
        (Idle, Busy),
        (Idle, Setup),
        (Idle, Down),
        // `second` and `third` are fed through a station: starved.
        (Starved, Busy),
        (Starved, Setup),
        (Starved, Down),
        (Setup, Busy),
        (Setup, Down),
        (Busy, Blocked),
        (Busy, Setup),
        (Busy, Down),
        (Blocked, Busy),
        (Blocked, Setup),
        (Blocked, Idle),
        (Blocked, Starved),
        (Blocked, Down),
        (Down, Idle),
        (Down, Starved),
        (Down, Busy),
        (Down, Blocked),
        (Down, Setup),
    ];
    for pair in expected {
        assert!(
            seen.contains(&pair),
            "{} to {} never happened: {seen:?}",
            pair.0.as_str(),
            pair.1.as_str()
        );
    }
    // Idle and starved never turn into each other; a machine starts in one.
    assert!(!seen.contains(&(Idle, Starved)) && !seen.contains(&(Starved, Idle)));
}

#[test]
fn busy_never_goes_straight_to_waiting() {
    let r = rich(7);
    for name in ["first", "second", "third"] {
        for (from, to) in changes(&r, name) {
            assert!(!(from == Busy && (to == Idle || to == Starved)));
        }
    }
    // Page 6's line: passing through blocked takes no time if there is room.
    let yaml = document(
        "10min",
        "  - {id: source, type: Source, interarrival: 2min}\n  \
         - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    let notes = r.machine_notes("m");
    let states: Vec<_> = notes.iter().map(|(_, n)| n.state).collect();
    assert_eq!(states[..5], [Idle, Busy, Blocked, Idle, Busy]);
    assert_eq!(notes[2].0, notes[3].0);
    assert_eq!(r.state_shares("m")[usize::from(Blocked.code())], 0.0);
}

/// What a machine holds, rebuilt from the trace at each of its notes.
#[derive(Default)]
struct Holds {
    /// In a cycle or waiting to start.
    arrived: BTreeSet<u64>,
    finished: BTreeSet<u64>,
    down: bool,
}

#[test]
fn state_equals_what_the_machine_holds() {
    let r = rich(11);
    for name in ["first", "second", "third"] {
        let id = r.id(name);
        let mut holds = Holds::default();
        let mut setups_open = false;
        for record in &r.records {
            match record {
                taktsim_trace::Record::Event {
                    target,
                    from,
                    message,
                    item,
                    ..
                } => {
                    if *target == id {
                        if matches!(message, FlowMessage::Arrive(_)) {
                            holds.arrived.insert(item.unwrap().as_u64());
                        }
                        if let FlowMessage::CycleDone { item } = message {
                            holds.arrived.remove(&item.as_u64());
                            holds.finished.insert(item.as_u64());
                        }
                        holds.down = match message {
                            FlowMessage::Failure => true,
                            FlowMessage::Repaired => false,
                            FlowMessage::CycleDone { .. }
                            | FlowMessage::SetupDone
                            | FlowMessage::NextArrival
                            | FlowMessage::Arrive(_)
                            | FlowMessage::Accept
                            | FlowMessage::Decline
                            | FlowMessage::Offer
                            | FlowMessage::Withdraw
                            | FlowMessage::MachineState(_)
                            | FlowMessage::SourceState(_)
                            | FlowMessage::VariantName { .. } => holds.down,
                        };
                        if matches!(message, FlowMessage::SetupDone) {
                            setups_open = false;
                        }
                    }
                    // A unit leaves when the event carrying it is sent, which
                    // is when `Accept` is handled; it counts as gone once it
                    // arrives elsewhere, at the same instant.
                    if *from == id && matches!(message, FlowMessage::Arrive(_)) {
                        holds.finished.remove(&item.unwrap().as_u64());
                    }
                }
                taktsim_trace::Record::Note {
                    by,
                    message: FlowMessage::MachineState(note),
                    ..
                } if *by == id => {
                    if holds.down {
                        assert_eq!(note.state, Down);
                        assert_eq!(note.units_in_cycle, 0);
                    }
                    match note.state {
                        Down => assert!(holds.down),
                        Setup => {
                            setups_open = true;
                            assert_eq!(note.units_in_cycle, 0);
                            assert!(note.setup_variant.is_some());
                        }
                        Busy => {
                            assert!(!holds.down && !setups_open);
                            assert!(note.units_in_cycle > 0);
                            assert!(u64::from(note.units_in_cycle) <= 2);
                        }
                        Blocked => {
                            assert_eq!(note.units_in_cycle, 0);
                            assert!(!holds.finished.is_empty(), "{name} blocked with nothing");
                        }
                        Idle | Starved => {
                            assert_eq!(note.units_in_cycle, 0);
                            assert!(holds.arrived.is_empty(), "{name} waits while holding");
                        }
                    }
                }
                taktsim_trace::Record::ItemCreated { .. }
                | taktsim_trace::Record::ItemDisposed { .. }
                | taktsim_trace::Record::Note { .. } => {}
            }
        }
    }
    // Shares sum to one.
    for name in ["first", "second", "third"] {
        let total: f64 = r.state_shares(name).iter().sum();
        assert!((total - 1.0).abs() < 1e-9, "{name}: {total}");
    }
}

/// Which of a model's stations ever were `idle` and which `starved`.
fn waiting_states(r: &Run, names: &[&str]) -> BTreeMap<String, BTreeSet<MachineState>> {
    names
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                r.machine_notes(name)
                    .iter()
                    .map(|(_, n)| n.state)
                    .filter(|s| matches!(s, Idle | Starved))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn idle_when_a_source_reaches_it_through_non_stations() {
    let yaml = document(
        "1h",
        // cut: fed by a Source. weld: through a station. packed: through a
        // buffer from a Source, so idle. after: behind a station and a buffer.
        // both: fed directly by a Source and by a machine, so idle.
        "  - {id: source, type: Source, interarrival: 5min}\n  \
         - {id: cut, type: Machine, cycle_time: 1min}\n  \
         - {id: weld, type: Machine, cycle_time: 1min}\n  \
         - {id: other, type: Source, interarrival: 7min}\n  \
         - {id: buf, type: Buffer, capacity: 3 parts}\n  \
         - {id: packed, type: Machine, cycle_time: 1min}\n  \
         - {id: buf2, type: Buffer, capacity: 3 parts}\n  \
         - {id: after, type: Machine, cycle_time: 1min}\n  \
         - {id: third, type: Source, interarrival: 11min}\n  \
         - {id: fourth, type: Source, interarrival: 13min}\n  \
         - {id: feeder, type: Machine, cycle_time: 1min}\n  \
         - {id: both, type: Machine, cycle_time: 1min}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, weld, sink]\n  - [other, buf, packed, buf2, after, sink]\n  \
         - [third, feeder]\n  - {from: [feeder, fourth], to: both}\n  - [both, sink]",
    );
    let r = run(&yaml, 1);
    let states = waiting_states(&r, &["cut", "weld", "packed", "after", "feeder", "both"]);
    let only = |state: MachineState| BTreeSet::from([state]);
    assert_eq!(states["cut"], only(Idle));
    assert_eq!(states["weld"], only(Starved));
    assert_eq!(states["packed"], only(Idle));
    assert_eq!(states["after"], only(Starved));
    assert_eq!(states["feeder"], only(Idle));
    assert_eq!(states["both"], only(Idle));
    // At time zero already.
    assert_eq!(r.machine_notes("weld")[0].0, SimTime::START);
    assert_eq!(r.machine_notes("weld")[0].1.state, Starved);
}
