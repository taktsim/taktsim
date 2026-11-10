//! Loops (ADR-0023 §8): a part coming back goes straight into the return target's
//! lane, ahead of new parts, so no loop deadlocks, and the lane stays bounded.

mod support;

use support::{check_records, document, is_arrive, run, t};
use taktsim_components::{FlowMessage, MachineState};
use taktsim_core::RunOutcome;
use taktsim_trace::Record;

/// A rework loop: `weld` to `qc`, and from `qc` a share back to `weld`.
fn rework(back: u32, source: &str, weld: &str, qc: &str, until: &str) -> String {
    document(
        until,
        &format!(
            "  - {{id: source, type: Source{source}}}\n  \
             - {{id: weld, type: Machine, {weld}}}\n  \
             - {{id: qc, type: Machine, {qc}}}\n  - {{id: sink, type: Sink}}"
        ),
        &format!(
            "  - [source, weld, qc]\n  - {{from: qc, split: {{weld: {back}%, sink: {}%}}}}",
            100 - back
        ),
    )
}

/// Conservation, the horizon reached and deliveries late in the run.
#[allow(clippy::expect_used)] // Test helper: a run with no delivery fails the test.
fn alive(r: &support::Run, until: &str) {
    let c = r.report.items;
    assert_eq!(
        c.created.get(),
        c.held.get() + c.in_transit.get() + c.delivered.get() + c.scrapped.get() + c.consumed.get()
    );
    assert_eq!(c.in_transit.get(), 0);
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    let last = r
        .delivered()
        .last()
        .map(|d| d.0)
        .expect("something delivered");
    let until = t(until).as_nanos();
    assert!(last.as_nanos() > until / 10 * 9, "last delivery at {last}");
}

#[test]
fn rework_loop_neither_deadlocks_nor_loses_parts() {
    // A heavy loop, in every combination of busy and random, with failures.
    for (back, source, weld, qc, seed) in [
        (
            20,
            ", interarrival: 40s",
            "cycle_time: 30s",
            "cycle_time: 20s",
            1,
        ),
        (50, "", "cycle_time: 30s", "cycle_time: 20s", 2),
        (
            90,
            "",
            "cycle_time: {dist: exponential, mean: 10s}",
            "cycle_time: {dist: exponential, mean: 5s}, capacity: 3 parts",
            3,
        ),
        (
            60,
            ", interarrival: {dist: exponential, mean: 3s}",
            "cycle_time: {dist: exponential, mean: 7s}, mtbf: {dist: exponential, mean: 2min}, mttr: {dist: exponential, mean: 20s}, capacity: 2 parts",
            "cycle_time: {dist: exponential, mean: 5s}, mtbf: {dist: exponential, mean: 3min}, mttr: {dist: exponential, mean: 10s}",
            4,
        ),
    ] {
        let until = "2h";
        let r = run(&rework(back, source, weld, qc, until), seed);
        alive(&r, until);
        check_records(&r);
        // Some parts did come back.
        let returned = r
            .events()
            .iter()
            .filter(|e| matches!(e.message, FlowMessage::Return(_)))
            .count();
        assert!(returned > 10, "{returned} came back");
    }
}

#[test]
fn a_part_coming_back_goes_ahead_of_new_parts() {
    // `qc` sends everything back at first. A part comes back to `weld` while a new part
    // waits; a free place takes the part coming back first, though the other waited
    // longer.
    let yaml = document(
        "1min",
        "  - {id: source, type: Source, interarrival: 1s}\n  \
         - {id: weld, type: Machine, cycle_time: 5s}\n  \
         - {id: qc, type: Machine, cycle_time: 5s}\n  - {id: sink, type: Sink}",
        "  - [source, weld, qc]\n  - {from: qc, split: {weld: 99%, sink: 1%}}",
    );
    let r = run(&yaml, 1);
    let returns = r.delivered_to("weld", |m| matches!(m, FlowMessage::Return(_)));
    assert_ne!(returns.len(), 0);
    // A new part takes a place only once nothing waits on the lane: while the first part
    // circles, the place is taken again at each return.
    let new: Vec<_> = r.arrivals("weld").iter().map(|e| e.time).collect();
    let first_return = returns[0].time;
    let during: Vec<_> = new.iter().filter(|time| **time > first_return).collect();
    assert!(new.len() >= 2, "{new:?}");
    assert!(during.len() < new.len(), "{new:?}");
    // No offer stands for a way back: a gate sends without one.
    assert_eq!(
        r.events()
            .iter()
            .filter(|e| e.from == "qc"
                && e.to == "weld"
                && matches!(e.message, FlowMessage::Offer | FlowMessage::OfferOf(_)))
            .count(),
        0
    );
    assert!(returns.iter().all(|e| e.from == "qc"));
}

#[test]
fn a_gate_passes_through_blocked_for_zero_time() {
    // `qc`'s states follow the table of ADR-0022 §5 even when a part comes back at once.
    let r = run(
        &rework(
            100 - 1,
            ", interarrival: 40s",
            "cycle_time: 3s",
            "cycle_time: 2s",
            "10min",
        ),
        1,
    );
    let notes = r.machine_notes("qc");
    for pair in notes.windows(2) {
        if pair[0].1.state == MachineState::Busy && pair[1].1.state != MachineState::Busy {
            assert_eq!(pair[1].1.state, MachineState::Blocked, "{pair:?}");
        }
    }
    // The engine's checks hold every event: the run itself is the proof of the table.
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
}

#[test]
fn lane_never_exceeds_c() {
    // A narrow return target, a wide gate and nearly everything coming back: the lane is
    // what fills. C = 1 + 1 + 40 = 42 parts in the loop, and the engine checks the lane
    // against it after every event.
    let yaml = document(
        "4h",
        "  - {id: source, type: Source}\n  \
         - {id: weld, type: Machine, cycle_time: {dist: exponential, mean: 10s}}\n  \
         - {id: conveyor, type: Buffer, capacity: 1 parts}\n  \
         - {id: qc, type: Machine, cycle_time: {dist: exponential, mean: 1s}, capacity: 40 parts}\n  \
         - {id: sink, type: Sink}",
        "  - [source, weld, conveyor, qc]\n  - {from: qc, split: {weld: 95%, sink: 5%}}",
    );
    let r = run(&yaml, 5);
    alive(&r, "4h");
    let (mut inside, mut most) = (0_i64, 0_i64);
    for e in r.events() {
        if e.from == "source" && is_arrive(&e.message) {
            inside += 1;
        }
        if e.to == "sink" && is_arrive(&e.message) {
            inside -= 1;
        }
        most = most.max(inside);
    }
    assert!(most <= 42, "{most} parts in the loop");
    assert!(most >= 2, "{most} parts in the loop");
}

#[test]
fn the_lane_fills_while_the_target_is_down_and_nothing_moves() {
    // `weld` is down from 10 s to 30 s. Parts come back meanwhile, and only move at the
    // repair.
    let yaml = document(
        "40s",
        "  - {id: source, type: Source}\n  \
         - {id: weld, type: Machine, cycle_time: 1s, mtbf: 10s, mttr: 20s}\n  \
         - {id: qc, type: Machine, cycle_time: 15s, capacity: 3 parts}\n  - {id: sink, type: Sink}",
        "  - [source, weld, qc]\n  - {from: qc, split: {weld: 99%, sink: 1%}}",
    );
    let r = run(&yaml, 1);
    let came = r.times("weld", |m| matches!(m, FlowMessage::Return(_)));
    assert!(
        came.iter().any(|time| *time > t("10s") && *time < t("30s")),
        "{came:?}"
    );
    // Down, it starts nothing, with the lane non-empty and places free: the invariant
    // allows that only while down, and the engine checks it after every event.
    let starts = r.machine_notes("weld");
    assert!(starts.iter().any(|(_, n)| n.state == MachineState::Down));
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    let busy_while_down = starts
        .iter()
        .any(|(time, n)| *time > t("10s") && *time < t("30s") && n.state == MachineState::Busy);
    assert!(!busy_while_down);
}

#[test]
fn a_full_branch_and_a_full_pool_do_not_deadlock() {
    // Rework through a pool, with a condition that sends some parts down a slow branch
    // that is nearly always full, and buffers that fill.
    let yaml = document(
        "4h",
        "  - {id: source, type: Source, variants: {red: 50%, blue: 50%}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 3s}}\n  \
         - {id: weld-1, type: Machine, cycle_time: {dist: exponential, mean: 8s}}\n  \
         - {id: weld-2, type: Machine, cycle_time: {dist: exponential, mean: 8s}, mtbf: {dist: exponential, mean: 10min}, mttr: {dist: exponential, mean: 1min}}\n  \
         - {id: buf, type: Buffer, capacity: 1 parts}\n  \
         - {id: qc, type: Machine, cycle_time: {dist: exponential, mean: 2s}}\n  \
         - {id: paint, type: Machine, cycle_time: {dist: exponential, mean: 30s}}\n  \
         - {id: pack, type: Machine, cycle_time: {dist: exponential, mean: 4s}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut]\n  - {from: cut, to: [weld-1, weld-2], choose: shortest_queue}\n  \
         - {from: [weld-1, weld-2], to: buf}\n  - [buf, qc]\n  \
         - {from: qc, to: paint, when: \"variant == 'red'\"}\n  \
         - {from: qc, split: {cut: 40%, pack: 60%}}\n  - [paint, pack, sink]",
    );
    for seed in [1, 2, 3] {
        let r = run(&yaml, seed);
        alive(&r, "4h");
        check_records(&r);
        assert!(r.arrivals("paint").len() > 10);
        assert!(
            r.times("cut", |m| matches!(m, FlowMessage::Return(_)))
                .len()
                > 10
        );
    }
}

#[test]
fn a_buffer_as_a_gate_sends_straight_back() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: m, type: Machine, cycle_time: 5s}\n  \
         - {id: gate, type: Buffer, capacity: 2 parts}\n  - {id: sink, type: Sink}",
        "  - [source, m, gate]\n  - {from: gate, split: {m: 30%, sink: 70%}}",
    );
    let r = run(&yaml, 3);
    alive(&r, "1h");
    assert!(r.times("m", |m| matches!(m, FlowMessage::Return(_))).len() > 10);
    // The records say so: one per part leaving `gate`, whichever way it went.
    let leaving = r
        .events()
        .iter()
        .filter(|e| {
            e.from == "gate" && matches!(e.message, FlowMessage::Arrive(_) | FlowMessage::Return(_))
        })
        .count();
    assert_eq!(r.routes().len(), leaving);
    check_records(&r);
}

#[test]
fn every_item_stays_accounted_for_at_every_event() {
    // Items held by the lane are counted: the engine compares what each component says
    // it holds with its own book at every event, and the run ends.
    let r = run(
        &rework(
            70,
            "",
            "cycle_time: 2s, capacity: 2 parts",
            "cycle_time: 1s",
            "20min",
        ),
        9,
    );
    let created = r
        .records
        .iter()
        .filter(|rec| matches!(rec, Record::ItemCreated { .. }))
        .count() as u64;
    assert_eq!(created, r.report.items.created.get());
    alive(&r, "20min");
}

/// A model with no `Source` declares no variant names. Without a `when` it says nothing
/// about conditions, so nothing about them is checked; a condition on `priority` alone
/// still loads, and one on `variant` is an error at that condition.
#[test]
fn a_model_without_a_source_loads_unless_a_condition_names_a_variant() {
    let components = "  - {id: a, type: Machine, cycle_time: 1s}\n  \
                      - {id: b, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}";
    let plain = document(
        "1h",
        components,
        "  - [a, b]\n  - {from: b, split: {a: 50%, sink: 50%}}",
    );
    let r = run(&plain, 1);
    assert_eq!(r.events().len(), 0);
    let by_priority = document(
        "1h",
        components,
        "  - [a, b]\n  - {from: b, to: sink, when: \"priority > 0\"}\n  \
         - {from: b, split: {a: 50%, sink: 50%}}",
    );
    let r = run(&by_priority, 1);
    assert_eq!(r.events().len(), 0);
    let by_variant = document(
        "1h",
        components,
        "  - [a, b]\n  - {from: b, to: sink, when: \"variant == 'red'\"}\n  \
         - {from: b, split: {a: 50%, sink: 50%}}",
    );
    let errors = support::try_load(&by_variant).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    assert_eq!(
        errors.first().path().to_string(),
        "flow[1].when",
        "{errors}"
    );
}
