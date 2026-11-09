//! The protocol between neighbours (ADR-0022 §3, §4).

mod support;

use std::collections::BTreeMap;

use support::{document, is_arrive, run, t};
use taktsim_components::{FlowMessage, MachineState};

#[test]
fn a_part_takes_three_events_a_hop_and_a_line_fifteen_events_per_part() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 2min}\n  - {id: cut, type: Machine, cycle_time: 42s}\n  \
         - {id: buf1, type: Buffer, capacity: 20 parts}\n  \
         - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: sink, type: Sink}",
        "  - [source, cut, buf1, weld, sink]",
    );
    let r = run(&yaml, 1);
    // Parts every 2 min, no blocking: 30 created; the last still inside? It
    // left: 87 s after 1 h is past the horizon, so 29 delivered.
    let delivered = r.report.items.delivered.get();
    assert_eq!(delivered, 29);
    let events = r.events();
    let first_hour_part = events.iter().filter(|e| e.time <= t("207s")).count();
    // Everything of the first part, and only that: arrival at 120 s, out at 207 s.
    assert_eq!(first_hour_part, 15);
    // 8 notes per part after the ones at the start: source 2, each machine 3.
    let notes = r
        .records
        .iter()
        .filter(|record| {
            matches!(
                record,
                taktsim_trace::Record::Note {
                    message: FlowMessage::MachineState(_) | FlowMessage::SourceState(_),
                    ..
                }
            )
        })
        .filter(|record| record.time() > t("0s") && record.time() <= t("207s"))
        .count();
    assert_eq!(notes, 8);
    // No `Withdraw` on a line without a branch.
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.message, FlowMessage::Withdraw))
    );
}

#[test]
fn reservation_never_lasts_past_its_instant() {
    // Branches, merges, failures, capacities: every `Accept` gets one reply at
    // its instant.
    let yaml = document(
        "4h",
        "  - {id: s1, type: Source, interarrival: {dist: exponential, mean: 11s}}\n  \
         - {id: s2, type: Source}\n  \
         - {id: a, type: Machine, cycle_time: {dist: exponential, mean: 15s}, mtbf: {dist: exponential, mean: 10min}, mttr: 1min, capacity: 2 parts}\n  \
         - {id: b, type: Machine, cycle_time: {dist: exponential, mean: 20s}}\n  \
         - {id: buf, type: Buffer, capacity: 2 parts}\n  \
         - {id: c, type: Machine, cycle_time: {dist: exponential, mean: 9s}, mtbf: {dist: exponential, mean: 7min}, mttr: 30s}\n  \
         - {id: sink, type: Sink}",
        "  - {from: s1, to: [a, b]}\n  - {from: s2, to: b}\n  - {from: [a, b], to: buf}\n  - [buf, c, sink]",
    );
    let r = run(&yaml, 7);
    let mut pending: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut now = t("0s");
    let mut declines = 0;
    let mut accepts = 0;
    for e in r.events() {
        if e.time != now {
            assert!(
                pending.values().all(|n| *n == 0),
                "reservation left at {now}: {pending:?}"
            );
            now = e.time;
        }
        if matches!(e.message, FlowMessage::Accept) {
            accepts += 1;
            *pending.entry((e.to.clone(), e.from.clone())).or_default() += 1;
        }
        if matches!(e.message, FlowMessage::Arrive(_) | FlowMessage::Decline) {
            if matches!(e.message, FlowMessage::Decline) {
                declines += 1;
            }
            let n = pending.get_mut(&(e.from.clone(), e.to.clone()));
            let n = n.unwrap_or_else(|| panic!("no accept before {e:?}"));
            assert!(*n > 0, "no accept before {e:?}");
            *n -= 1;
        }
    }
    assert!(accepts > 100);
    assert!(declines > 0, "the model should exercise crossings");
}

/// A source, two stations in parallel and a sink.
fn branch(interval: &str, a: &str, b: &str, until: &str) -> String {
    document(
        until,
        &format!(
            "  - {{id: source, type: Source, interarrival: {interval}}}\n  \
             - {{id: a, type: Machine, cycle_time: {a}}}\n  \
             - {{id: b, type: Machine, cycle_time: {b}}}\n  - {{id: sink, type: Sink}}"
        ),
        "  - {from: source, to: [a, b]}\n  - {from: [a, b], to: sink}",
    )
}

#[test]
fn branch_ties_go_to_the_first_declared() {
    // Room at both: the first listed gets it, every time.
    let r = run(&branch("10s", "5s", "5s", "2min"), 1);
    assert_eq!(r.arrivals("a").len(), 12);
    assert_eq!(r.arrivals("b").len(), 0);
    // The other way round, with the list reversed.
    let yaml = branch("10s", "5s", "5s", "2min").replace("to: [a, b]", "to: [b, a]");
    let r = run(&yaml, 1);
    assert_eq!(r.arrivals("a").len(), 0);
    assert_eq!(r.arrivals("b").len(), 12);
    // When the first is busy, the second gets the part.
    let r = run(&branch("3s", "5s", "5s", "20s"), 1);
    let a: Vec<_> = r.arrivals("a").iter().map(|e| e.time).collect();
    let b: Vec<_> = r.arrivals("b").iter().map(|e| e.time).collect();
    assert_eq!(a, [t("3s"), t("9s"), t("15s")]);
    assert_eq!(b, [t("6s"), t("12s"), t("18s")]);
}

#[test]
fn branch_room_opening_together_follows_event_order() {
    // Parts at 4 s, then 4 s after the last left. `a` takes the first until
    // 17 s. `b` takes the second at 8 s until 30 s: its timer is scheduled now.
    // The third is held from 12 s, and `a` takes it at 17 s until 30 s. The
    // fourth is held from 21 s. Room opens at both at 30 s, and `b`'s timer was
    // scheduled first, so `b`, the second listed, gets the part.
    let r = run(&branch("4s", "13s", "22s", "33s"), 1);
    let a: Vec<_> = r.arrivals("a").iter().map(|e| e.time).collect();
    let b: Vec<_> = r.arrivals("b").iter().map(|e| e.time).collect();
    assert_eq!(a, [t("4s"), t("17s")]);
    assert_eq!(b, [t("8s"), t("30s")]);
}

#[test]
fn merge_serves_oldest_offer() {
    // Two sources into one slow machine; offers queue in the order they arrive.
    let yaml = document(
        "400s",
        "  - {id: p, type: Source, interarrival: 11s}\n  \
         - {id: q, type: Source, interarrival: 13s}\n  \
         - {id: m, type: Machine, cycle_time: 100s}\n  - {id: sink, type: Sink}",
        "  - {from: [p, q], to: m}\n  - [m, sink]",
    );
    let r = run(&yaml, 1);
    let from: Vec<(String, _)> = r
        .arrivals("m")
        .iter()
        .map(|e| (e.from.clone(), e.time))
        .collect();
    // p at 11 s. q offers at 13 s, p again at 22 s. So q first at 111 s, then p
    // at 211 s; q offers again at 124 s, p at 222 s.
    assert_eq!(
        from,
        [
            ("p".to_owned(), t("11s")),
            ("q".to_owned(), t("111s")),
            ("p".to_owned(), t("211s")),
            ("q".to_owned(), t("311s")),
        ]
    );
}

#[test]
fn withdrawn_offer_loses_its_place() {
    // `split` feeds `m` first, then `x`. A part at 10 s goes to `m` (until 110 s),
    // one at 20 s to `x` (until 55 s). The next, at 30 s, offers to both, and
    // queues at `m`. `other` offers to `m` at 40 s, behind it. At 55 s the part
    // goes to `x`, and its offer at `m` is withdrawn. A new part of `split`
    // offers again at 65 s, behind `other`. So at 110 s `m` serves `other`,
    // whose part has waited longest, not the old place of a part gone elsewhere.
    let yaml = document(
        "120s",
        "  - {id: split, type: Source, interarrival: 10s}\n  \
         - {id: other, type: Source, interarrival: 40s}\n  \
         - {id: m, type: Machine, cycle_time: 100s}\n  \
         - {id: x, type: Machine, cycle_time: 35s}\n  - {id: sink, type: Sink}",
        "  - {from: split, to: [m, x]}\n  - {from: other, to: m}\n  - {from: [m, x], to: sink}",
    );
    let r = run(&yaml, 1);
    let at_m: Vec<(String, _)> = r
        .arrivals("m")
        .iter()
        .map(|e| (e.from.clone(), e.time))
        .collect();
    assert_eq!(
        at_m,
        [
            ("split".to_owned(), t("10s")),
            ("other".to_owned(), t("110s"))
        ]
    );
    let withdrawn = r.times("m", |m| matches!(m, FlowMessage::Withdraw));
    assert!(withdrawn.contains(&t("55s")), "{withdrawn:?}");
}

#[test]
fn a_part_promised_before_a_failure_arrives_and_waits() {
    // `a` is offered a part at the same instant it fails, in the same instant
    // the source's interval ends: the failure comes first (priority 0), so it
    // queues the offer, then starts after the repair.
    let yaml = document(
        "40s",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: a, type: Machine, cycle_time: 1s, mtbf: 10s, mttr: 5s}\n  - {id: sink, type: Sink}",
        "  - [source, a, sink]",
    );
    let r = run(&yaml, 1);
    let arrivals: Vec<_> = r.arrivals("a").iter().map(|e| e.time).collect();
    // The part held from 10 s is taken at the repair, 15 s.
    assert_eq!(arrivals[0], t("15s"));
    assert_eq!(r.delivered()[0].0, t("16s"));
    let notes = r.machine_notes("a");
    assert_eq!(notes[1].1.state, MachineState::Down);
}

#[test]
fn a_down_machine_declines_an_accept_and_offers_again_on_repair() {
    // `a` finishes at 1 s and offers `b`, which is busy. `b` opens at 9 s, while
    // `a` is down from 5 s to 10 s: its offer stands, `b` accepts, `a` declines,
    // and offers again at 10 s.
    let yaml = document(
        "30s",
        "  - {id: source, type: Source}\n  \
         - {id: a, type: Machine, cycle_time: 1s, mtbf: 5s, mttr: 5s}\n  \
         - {id: b, type: Machine, cycle_time: 8s}\n  - {id: sink, type: Sink}",
        "  - [source, a, b, sink]",
    );
    let r = run(&yaml, 1);
    let events = r.events();
    let decline = events
        .iter()
        .find(|e| e.from == "a" && e.to == "b" && matches!(e.message, FlowMessage::Decline))
        .unwrap();
    assert_eq!(decline.time, t("9s"));
    let arrivals: Vec<_> = r.arrivals("b").iter().map(|e| e.time).collect();
    assert_eq!(arrivals[..2], [t("1s"), t("10s")]);
    assert!(events.iter().any(|e| is_arrive(&e.message)));
}

#[test]
fn an_accept_crossing_a_withdraw_keeps_both_sides_in_step() {
    // Up to 39 s: `a` holds part 1 from 9 s, `b` part 2 from 18 s, both until 39 s,
    // `a`'s timer scheduled first. The buffer holds part 3 from 27 s, and `up` is
    // blocked with part 4 from 36 s. At 39 s, `a` gives its part and takes part 3:
    // the buffer has nothing left and withdraws its offer to `b`, then takes part 4
    // from `up` and offers again, all while `b`, which has not seen the withdrawal,
    // gives its part and accepts. Its `Accept` answers the old offer. Part 4 goes to
    // `b`, and a second `Withdraw` cancels the new offer on its way.
    let yaml = document(
        "40s",
        "  - {id: source, type: Source}\n  - {id: up, type: Machine, cycle_time: 9s}\n  \
         - {id: buf, type: Buffer, capacity: 1 parts}\n  \
         - {id: a, type: Machine, cycle_time: 30s}\n  \
         - {id: b, type: Machine, cycle_time: 21s}\n  - {id: sink, type: Sink}",
        "  - [source, up, buf]\n  - {from: buf, to: [a, b]}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let from_buffer = |to: &str| -> Vec<_> {
        r.arrivals(to)
            .iter()
            .filter(|e| e.from == "buf")
            .map(|e| e.time)
            .collect()
    };
    assert_eq!(from_buffer("a"), [t("9s"), t("39s")]);
    assert_eq!(from_buffer("b"), [t("18s"), t("39s")]);
    let withdrawals = r.times("b", |m| matches!(m, FlowMessage::Withdraw));
    assert_eq!(
        withdrawals.iter().filter(|time| **time == t("39s")).count(),
        2,
        "{withdrawals:?}"
    );
    // After the instant, nothing stands: no part left to give, and no entry.
    let offers = r.times("b", |m| matches!(m, FlowMessage::Offer));
    assert_eq!(offers.iter().filter(|time| **time == t("39s")).count(), 1);
    assert!(
        r.machine_notes("b")
            .iter()
            .any(|(time, n)| *time == t("39s") && n.state == MachineState::Busy)
    );
}

#[test]
fn a_buffer_counts_its_promised_places_when_offered() {
    // Two machines merge into a one-place buffer that feeds two failing
    // machines. At 13 s, `d1` takes the buffer's part and the buffer promises
    // its place to `up2`. Before `up2`'s part lands, `d2`'s accept comes in:
    // the buffer has no part and declines, and the promised place is not free,
    // so `up1`, waiting since 12 s, is accepted only once `up2`'s part is in
    // and gone (fill + reserved <= capacity).
    let yaml = document(
        "60s",
        "  - {id: s1, type: Source}\n  - {id: s2, type: Source}\n  \
         - {id: up1, type: Machine, cycle_time: 3s}\n  - {id: up2, type: Machine, cycle_time: 3s}\n  \
         - {id: buf, type: Buffer, capacity: 1 parts}\n  \
         - {id: d1, type: Machine, cycle_time: 500ms, mtbf: 7s, mttr: 6s}\n  \
         - {id: d2, type: Machine, cycle_time: 500ms, mtbf: 7s, mttr: 6s}\n  \
         - {id: sink, type: Sink}",
        "  - [s1, up1]\n  - [s2, up2]\n  - {from: [up1, up2], to: buf}\n  \
         - {from: buf, to: [d1, d2]}\n  - {from: [d1, d2], to: sink}",
    );
    let r = run(&yaml, 1);
    let at_13: Vec<_> = r
        .events()
        .into_iter()
        .filter(|e| e.time == t("13s"))
        .map(|e| (e.from, e.to, e.message))
        .collect();
    let index = |from: &str, to: &str, pick: fn(&FlowMessage) -> bool| {
        at_13
            .iter()
            .position(|(f, t, m)| f == from && t == to && pick(m))
            .unwrap()
    };
    let accept = |m: &FlowMessage| matches!(m, FlowMessage::Accept);
    let promised = index("buf", "up2", accept);
    let landed = index("up2", "buf", is_arrive);
    let declined = index("buf", "d2", |m| matches!(m, FlowMessage::Decline));
    // `d2`'s accept is handled while the place is promised to `up2`.
    let d2_accept = index("d2", "buf", accept);
    assert!(d2_accept < landed && promised < declined, "{at_13:?}");
    // `up1` is accepted only after `up2`'s part has landed.
    assert!(index("buf", "up1", accept) > landed, "{at_13:?}");
}

#[test]
fn a_part_promised_before_a_failure_can_land_while_the_machine_is_down() {
    // A failure drawn within a nanosecond can fall between the Accept and the
    // Arrive of one instant. The part still arrives, waits in its place, and
    // starts after the repair (ADR-0022 §4).
    let yaml = document(
        "1min",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: 1s, mtbf: {dist: uniform, min: 0s, max: 0.000000001s}, mttr: 1s}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let mut landed = 0;
    for seed in 1..=8 {
        let r = run(&yaml, seed);
        let mut down = false;
        for e in r.events().iter().filter(|e| e.to == "m") {
            if matches!(e.message, FlowMessage::Failure) {
                down = true;
            } else if matches!(e.message, FlowMessage::Repaired) {
                down = false;
            } else if down && is_arrive(&e.message) {
                landed += 1;
            }
        }
        // Every part that arrived is delivered or still held at the end.
        assert!(
            r.delivered().len() + 1 >= r.arrivals("m").len(),
            "seed {seed}"
        );
    }
    assert!(landed > 0, "no part landed while down");
}
