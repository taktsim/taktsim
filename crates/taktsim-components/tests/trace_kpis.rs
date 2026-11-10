//! What M8 needs, read from a trace alone (ADR-0022 §9): hand-computed models
//! whose KPIs come out of the records and match by hand. M8's own fold will
//! replace this one; its answers must not change.

mod support;

use std::collections::BTreeMap;

use support::{document, run, t};
use taktsim_components::{FlowMessage, MachineState};
use taktsim_trace::Record;
use taktsim_units::SimTime;

/// Seconds as an exact count, for comparing.
fn s(time: SimTime) -> f64 {
    time.as_secs_f64()
}

/// A constant-time line: two stations and a buffer, parts every 50 s.
///
/// source: parts at 50, 100, 150... (they leave at once, cut has room).
/// cut, 30 s: busy 50-80, 100-130... weld, 40 s: 80-120, 130-170...
fn simple() -> support::Run {
    let yaml = document(
        "500s",
        "  - {id: source, type: Source, interarrival: 50s}\n  \
         - {id: cut, type: Machine, cycle_time: 30s}\n  \
         - {id: buf, type: Buffer, capacity: 2 parts}\n  \
         - {id: weld, type: Machine, cycle_time: 40s}\n  - {id: sink, type: Sink}",
        "  - [source, cut, buf, weld, sink]",
    );
    run(&yaml, 1)
}

#[test]
fn throughput_comes_from_item_disposed() {
    let r = simple();
    // Parts at 50 s ... 500 s; each leaves 70 s later: out at 120 s ... 470 s.
    let delivered = r.delivered();
    assert_eq!(delivered.len(), 8);
    assert_eq!(delivered[0].0, t("120s"));
    assert_eq!(delivered[7].0, t("470s"));
}

#[test]
fn state_shares_come_from_the_notes() {
    let r = simple();
    let shares = r.state_shares("cut");
    // Busy 30 s per part for 10 parts (the last at 500 s has not run: 9 cycles
    // up to 480 s, and the tenth starts at 500 s). 9 x 30 = 270 of 500 s.
    assert_eq!(
        shares[usize::from(MachineState::Busy.code())],
        270.0 / 500.0
    );
    assert_eq!(
        shares[usize::from(MachineState::Idle.code())],
        230.0 / 500.0
    );
    assert_eq!(shares[usize::from(MachineState::Blocked.code())], 0.0);
    // Weld: 40 s per part, 80-120, 130-170, ..., eight done by 470 s; the ninth,
    // 480-520 s, cut at the horizon: 8 x 40 + 20 = 340 of 500 s.
    let shares = r.state_shares("weld");
    assert_eq!(
        shares[usize::from(MachineState::Busy.code())],
        340.0 / 500.0
    );
    assert_eq!(
        shares[usize::from(MachineState::Starved.code())],
        160.0 / 500.0
    );
    // The end is stated, not guessed from the last event.
    assert_eq!(r.end, t("500s"));
}

#[test]
fn wip_of_a_buffer_comes_from_events_with_an_item() {
    let r = simple();
    // A part passes the buffer at one instant here: 0 for the whole run.
    let mut area = 0.0;
    let (mut fill, mut last) = (0_i32, 0.0);
    for e in r.events() {
        if support::is_arrive(&e.message) && (e.to == "buf" || e.from == "buf") {
            area += f64::from(fill) * (s(e.time) - last);
            last = s(e.time);
            fill += if e.to == "buf" { 1 } else { -1 };
        }
    }
    area += f64::from(fill) * (s(r.end) - last);
    assert_eq!(area, 0.0);
    assert_eq!(fill, 0);
}

#[test]
fn time_weighted_wip_of_a_filling_buffer_matches_by_hand() {
    // cut 10 s, weld 25 s, on demand, 2 places, 100 s:
    // weld takes parts at 10, 35, 60, 85. The buffer fills as cut outruns it.
    let yaml = document(
        "100s",
        "  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 10s}\n  \
         - {id: buf, type: Buffer, capacity: 2 parts}\n  \
         - {id: weld, type: Machine, cycle_time: 25s}\n  - {id: sink, type: Sink}",
        "  - [source, cut, buf, weld, sink]",
    );
    let r = run(&yaml, 1);
    let (mut area, mut fill, mut last) = (0.0, 0_i32, 0.0);
    let mut most = 0;
    for e in r.events() {
        if support::is_arrive(&e.message) && (e.to == "buf" || e.from == "buf") {
            area += f64::from(fill) * (s(e.time) - last);
            last = s(e.time);
            fill += if e.to == "buf" { 1 } else { -1 };
            most = most.max(fill);
        }
    }
    area += f64::from(fill) * (s(r.end) - last);
    // Cut finishes parts at 10, 20, 30, 40, then blocks. The first goes straight
    // to weld. Weld takes the next at 35 and 60 and 85. So the fill is 0 to 20 s,
    // 1 to 30 s, 2 to 35 s, 1 to 40 s, and 2 from then on, since the part cut
    // holds enters as one leaves: 10 + 10 + 5 + 2 x 60 = 145 part-seconds.
    assert_eq!(most, 2);
    assert_eq!(area, 145.0);
}

/// The lead time of each delivered part, from its first `Arrive` from a Source.
fn lead_times(r: &support::Run, source: &str) -> BTreeMap<u64, f64> {
    let id = r.id(source);
    let mut released = BTreeMap::new();
    let mut lead = BTreeMap::new();
    for record in &r.records {
        match record {
            Record::Event {
                time,
                from,
                message: FlowMessage::Arrive(_),
                item: Some(item),
                ..
            } if *from == id => {
                released.insert(item.as_u64(), s(*time));
            }
            Record::ItemDisposed { time, item, .. } => {
                lead.insert(item.as_u64(), s(*time) - released[&item.as_u64()]);
            }
            Record::Event { .. } | Record::ItemCreated { .. } | Record::Note { .. } => {}
        }
    }
    lead
}

#[test]
fn lead_time_starts_at_release() {
    // A part held at the Source is in neither the lead time nor the WIP: parts
    // every 10 s into a 30 s machine. A part is created at 20 s and leaves the
    // Source at 40 s: its lead time is the machine's 30 s, not 50 s.
    let yaml = document(
        "3min",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: m, type: Machine, cycle_time: 30s}\n  - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    let lead = lead_times(&r, "source");
    assert!(lead.len() > 3);
    assert!(lead.values().all(|x| (*x - 30.0).abs() < 1e-9), "{lead:?}");
    // The hold is on the Source's notes instead.
    let notes = r.source_notes("source");
    assert!(notes.iter().any(|(_, n)| n.held == 1));
    let created: Vec<SimTime> = r
        .records
        .iter()
        .filter_map(|record| match record {
            Record::ItemCreated { time, .. } => Some(*time),
            Record::Event { .. } | Record::ItemDisposed { .. } | Record::Note { .. } => None,
        })
        .collect();
    assert_eq!(created[1], t("20s"));
    assert_eq!(r.arrivals("m")[1].time, t("40s"));
}

#[test]
fn a_failing_machine_of_capacity_two_counts_units_with_zero_while_down() {
    // Two units at a time, 100 s each, up 150 s, down 50 s, saturated, to 600 s.
    // 0-100: 2 units; 100-150: 2 again (two new ones at 100 s, 50 s of work
    // left); 150-200: down, 0 on the note; 200-250: 2 finish the remaining 50 s;
    // 250-350: 2; 350-400: down (up 150 s from 200 s); 400-500: 2 ...
    let yaml = document(
        "600s",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: 100s, capacity: 2 parts, mtbf: 150s, mttr: 50s}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    let notes = r.machine_notes("m");
    // Units in a cycle, time-weighted, over capacity: utilization.
    let mut area = 0.0;
    for pair in notes.windows(2) {
        area += f64::from(pair[0].1.units_in_cycle) * (s(pair[1].0) - s(pair[0].0));
    }
    let (last_time, last) = notes.last().unwrap();
    area += f64::from(last.units_in_cycle) * (s(r.end) - s(*last_time));
    let utilization = area / (2.0 * s(r.end));
    let down = r.state_shares("m")[usize::from(MachineState::Down.code())];
    // Failures at 150, 350, 550; each for 50 s, the last cut at 600 s: 150 s of 600.
    assert_eq!(down, 150.0 / 600.0);
    // No blocking and no waiting here: what is not down is work, for both units.
    assert_eq!(utilization + down, 1.0);
    // Every down note says 0.
    assert!(
        notes
            .iter()
            .filter(|(_, n)| n.state == MachineState::Down)
            .all(|(_, n)| n.units_in_cycle == 0)
    );
    assert_eq!(r.report.items.delivered.get(), 8);
}

#[test]
fn an_interrupted_changeover_counts_once() {
    // Do not count notes: an interrupted changeover writes setup, down, setup.
    // One `SetupDone` for it, whose variant is on the setup note before it.
    let yaml = document(
        "240s",
        "  - {id: a, type: Source, interarrival: 10s, variants: {red: 100%}}\n  \
         - {id: b, type: Source, interarrival: 15s, variants: {blue: 100%}}\n  \
         - {id: m, type: Machine, cycle_time: 100s, capacity: 2 parts, setup: {per_variant: 7s}, mtbf: 115s, mttr: 10s}\n  \
         - {id: sink, type: Sink}",
        "  - {from: [a, b], to: m}\n  - [m, sink]",
    );
    let r = run(&yaml, 1);
    let id = r.id("m");
    let mut last_setup = None;
    let mut finished = Vec::new();
    let mut setup_notes = 0;
    for record in &r.records {
        match record {
            Record::Note {
                by,
                time,
                message: FlowMessage::MachineState(note),
            } if *by == id && *time <= t("127s") && note.state == MachineState::Setup => {
                setup_notes += 1;
                last_setup = note.setup_variant;
            }
            Record::Event {
                target,
                message: FlowMessage::SetupDone,
                time,
                ..
            } if *target == id && *time <= t("127s") => {
                finished.push(r.variant_name(last_setup.unwrap()).unwrap());
            }
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. } => {}
        }
    }
    assert_eq!(setup_notes, 2);
    assert_eq!(finished, ["blue"]);
}

#[test]
fn every_note_comes_after_the_start() {
    // A state history begins at time zero, so shares need no assumption.
    let r = simple();
    for name in ["cut", "weld"] {
        assert_eq!(r.machine_notes(name)[0].0, SimTime::START);
    }
    assert_eq!(r.source_notes("source")[0].0, SimTime::START);
}
