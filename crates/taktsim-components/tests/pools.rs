//! Pools and choice rules (ADR-0023 §5): a part stands as one offer at the best-ranked
//! target, and moves when a target with room outranks where it stands.

mod support;

use support::{document, run, t};
use taktsim_components::{FlowMessage, RouteRule};
use taktsim_units::SimTime;

/// A source, two machines in a pool with `rule`, and a sink.
fn pool(rule: &str, interval: &str, a: &str, b: &str, until: &str) -> String {
    document(
        until,
        &format!(
            "  - {{id: source, type: Source, interarrival: {interval}}}\n  \
             - {{id: a, type: Machine, cycle_time: {a}}}\n  \
             - {{id: b, type: Machine, cycle_time: {b}}}\n  - {{id: sink, type: Sink}}"
        ),
        &format!(
            "  - {{from: source, to: [a, b], choose: {rule}}}\n  - {{from: [a, b], to: sink}}"
        ),
    )
}

fn arrivals_at(r: &support::Run, name: &str) -> Vec<SimTime> {
    r.arrivals(name).iter().map(|e| e.time).collect()
}

#[test]
fn each_choice_rule_breaks_ties_by_list_order() {
    for rule in ["preferred", "shortest_queue"] {
        // Everything equal and idle: the first listed gets every part.
        let r = run(&pool(rule, "10s", "5s", "5s", "2min"), 1);
        assert_eq!(r.arrivals("a").len(), 12, "{rule}");
        assert_eq!(r.arrivals("b").len(), 0, "{rule}");
        // The list is what decides.
        let yaml = pool(rule, "10s", "5s", "5s", "2min").replace("to: [a, b]", "to: [b, a]");
        let r = run(&yaml, 1);
        assert_eq!(r.arrivals("a").len(), 0, "{rule}");
        assert_eq!(r.arrivals("b").len(), 12, "{rule}");
    }
    // Work done separates the targets at once, but the first part sees a tie, and so
    // does every part that finds them equal: two targets taking turns.
    let r = run(&pool("lowest_utilization", "10s", "5s", "5s", "2min"), 1);
    assert_eq!(arrivals_at(&r, "a")[0], t("10s"));
    assert_eq!(arrivals_at(&r, "b")[0], t("20s"));
    assert_eq!(arrivals_at(&r, "a")[1], t("30s"));
    let yaml =
        pool("lowest_utilization", "10s", "5s", "5s", "2min").replace("to: [a, b]", "to: [b, a]");
    let r = run(&yaml, 1);
    assert_eq!(arrivals_at(&r, "b")[0], t("10s"));
    assert_eq!(arrivals_at(&r, "a")[0], t("20s"));
}

#[test]
fn preferred_is_not_first_free_when_room_opens_together() {
    // Parts at 4 s, then 4 s after the last left. `a` takes the first until 17 s, `b`
    // the second from 8 s until 30 s, `a` the third from 17 s until 30 s. The fourth is
    // held from 21 s. Room opens at both at 30 s, `b` first, since its timer was
    // scheduled first.
    let first_free = run(&pool("first_free", "4s", "13s", "22s", "33s"), 1);
    assert_eq!(arrivals_at(&first_free, "a"), [t("4s"), t("17s")]);
    assert_eq!(arrivals_at(&first_free, "b"), [t("8s"), t("30s")]);
    // `preferred` gives it to the first listed, whatever the order of events.
    let preferred = run(&pool("preferred", "4s", "13s", "22s", "33s"), 1);
    assert_eq!(arrivals_at(&preferred, "a"), [t("4s"), t("17s"), t("30s")]);
    assert_eq!(arrivals_at(&preferred, "b"), [t("8s")]);
    // With the list reversed the rule still follows it.
    let yaml = pool("preferred", "4s", "13s", "22s", "33s").replace("to: [a, b]", "to: [b, a]");
    let reversed = run(&yaml, 1);
    assert_eq!(arrivals_at(&reversed, "b")[0], t("4s"));
}

/// The events `source` sent to `to` at `time`, as message names.
fn offers_and_withdrawals(r: &support::Run, from: &str, to: &str, time: &str) -> Vec<&'static str> {
    r.events()
        .iter()
        .filter(|e| e.from == from && e.to == to && e.time == t(time))
        .filter_map(|e| {
            if matches!(e.message, FlowMessage::Offer | FlowMessage::OfferOf(_)) {
                Some("offer")
            } else if matches!(e.message, FlowMessage::Withdraw) {
                Some("withdraw")
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn no_part_stands_beside_a_free_target() {
    // Two holders offer `a` at one instant, on the same old `room`. The second's offer
    // must not stay there beside an empty `b`.
    let yaml = document(
        "30s",
        "  - {id: p, type: Source, interarrival: 10s}\n  \
         - {id: q, type: Source, interarrival: 10s}\n  \
         - {id: a, type: Machine, cycle_time: 100s}\n  \
         - {id: b, type: Machine, cycle_time: 100s}\n  - {id: sink, type: Sink}",
        "  - {from: [p, q], to: [a, b], choose: preferred}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let at_a: Vec<_> = r
        .arrivals("a")
        .iter()
        .map(|e| (e.from.clone(), e.time))
        .collect();
    let at_b: Vec<_> = r
        .arrivals("b")
        .iter()
        .map(|e| (e.from.clone(), e.time))
        .collect();
    assert_eq!(at_a, [("p".to_owned(), t("10s"))]);
    assert_eq!(at_b, [("q".to_owned(), t("10s"))]);

    // A holder down when a gain arrives catches up at its repair.
    let r = run(&repaired_holder(), 1);
    assert_eq!(arrivals_at(&r, "b"), [t("2s"), t("15s")]);
}

#[test]
fn offer_moves_when_its_target_loses_room() {
    // `a` fails at 10 s, as the part appears: `a`'s `Status` lands after the offer went
    // out on the old `room`, and the offer moves to `b` at once.
    let yaml = document(
        "12s",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: a, type: Machine, cycle_time: 1s, mtbf: 10s, mttr: 5s}\n  \
         - {id: b, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: preferred}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    assert_eq!(arrivals_at(&r, "b"), [t("10s")]);
    assert_eq!(arrivals_at(&r, "a").len(), 0);
    assert_eq!(
        offers_and_withdrawals(&r, "source", "a", "10s"),
        ["offer", "withdraw"]
    );
    assert_eq!(offers_and_withdrawals(&r, "source", "b", "10s"), ["offer"]);
    // The record shows the ranking that placed the offer where it was taken.
    let routes = r.routes();
    assert_eq!(routes.len(), 1);
    let record = &routes[0].2;
    assert_eq!(record.rule, RouteRule::Preferred);
    assert_eq!(record.chosen, 1);
    assert_eq!(record.detail, t("10s").as_nanos());
    assert_eq!(record.candidates[0].room, Some(false));
    assert_eq!(record.candidates[1].room, Some(true));
}

/// A machine with a finished part standing at `a`, down while `b` frees.
fn repaired_holder() -> String {
    // `h`: first part until 1 s, to `a` (until 101 s). Second until 2 s, to `b` (until
    // 14 s). Third until 3 s, standing at `a`: no room anywhere. `h` fails at 5 s until
    // 15 s, and `b` frees at 14 s, while it is down.
    document(
        "20s",
        "  - {id: source, type: Source}\n  \
         - {id: h, type: Machine, cycle_time: 1s, mtbf: 5s, mttr: 10s}\n  \
         - {id: a, type: Machine, cycle_time: 100s}\n  \
         - {id: b, type: Machine, cycle_time: 12s}\n  - {id: sink, type: Sink}",
        "  - [source, h]\n  - {from: h, to: [a, b], choose: preferred}\n  - {from: [a, b], to: sink}",
    )
}

#[test]
fn repaired_holder_re_ranks() {
    let r = run(&repaired_holder(), 1);
    // Down from 5 s, so it moves nothing at 14 s; at 15 s it moves the offer to `b`.
    assert_eq!(arrivals_at(&r, "b"), [t("2s"), t("15s")]);
    assert_eq!(
        offers_and_withdrawals(&r, "h", "b", "14s"),
        Vec::<&str>::new()
    );
    assert_eq!(offers_and_withdrawals(&r, "h", "a", "15s"), ["withdraw"]);
    assert_eq!(offers_and_withdrawals(&r, "h", "b", "15s"), ["offer"]);
    // It heard of the gain while down: the `Status` was there.
    assert!(
        r.statuses()
            .iter()
            .any(|(time, from, to, s)| *time == t("14s") && from == "b" && to == "h" && s.room)
    );
}

#[test]
fn queue_changes_alone_move_nothing() {
    // `source`: parts at 1 s to `a`, 2 s to `b`, and from 3 s one standing at `a`, the
    // first listed, with no room anywhere. `other` offers `a` at 4 s, so `a`'s queue is
    // longer than `b`'s from then on, and the offer stays where it is.
    let yaml = document(
        "50s",
        "  - {id: source, type: Source, interarrival: 1s}\n  \
         - {id: other, type: Source, interarrival: 4s}\n  \
         - {id: a, type: Machine, cycle_time: 100s}\n  \
         - {id: b, type: Machine, cycle_time: 100s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: shortest_queue}\n  - {from: other, to: a}\n  \
         - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let heard: Vec<_> = r
        .statuses()
        .into_iter()
        .filter(|(_, from, _, _)| from == "a")
        .map(|(time, _, _, status)| (time, status.queue))
        .collect();
    assert!(heard.contains(&(t("4s"), 2)), "{heard:?}");
    // No withdrawal of what stands at `a`: no move.
    assert!(
        r.events()
            .iter()
            .all(|e| !(e.from == "source" && matches!(e.message, FlowMessage::Withdraw))),
        "the offer moved"
    );
}

#[test]
fn queue_counts_what_it_says() {
    let yaml = document(
        "50s",
        "  - {id: source, type: Source, interarrival: 1s}\n  \
         - {id: other, type: Source, interarrival: 4s}\n  \
         - {id: a, type: Machine, cycle_time: 100s}\n  \
         - {id: b, type: Machine, cycle_time: 100s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: shortest_queue}\n  - {from: other, to: a}\n  \
         - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let statuses = r.statuses();
    let from_a: Vec<_> = statuses
        .iter()
        .filter(|(_, from, _, _)| from == "a")
        .collect();
    // 1 s: the place reserved for this very holder counts, and its own offer before did
    // not. The `Arrive` turns the reservation into a unit, which starts: only `running`
    // changes.
    assert_eq!(from_a[0].0, t("1s"));
    assert_eq!(from_a[0].3.queue, 1);
    assert!(!from_a[0].3.room);
    assert!(from_a[0].3.accept_follows);
    assert_eq!(from_a[1].0, t("1s"));
    assert_eq!((from_a[1].3.queue, from_a[1].3.running), (1, 1));
    assert!(!from_a[1].3.accept_follows);
    // 3 s: its own offer waits at `a`, and is left out: nothing changes for it. 4 s:
    // `other`'s offer waits there, and counts.
    assert_eq!(from_a[2].0, t("4s"));
    assert_eq!(from_a[2].3.queue, 2);
    assert!(!from_a[2].3.accept_follows);
    assert_eq!(from_a.len(), 3, "{from_a:?}");
    // A Sink is always room and queue 0, and sends nothing.
    let yaml = document(
        "1min",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [m, sink], choose: shortest_queue}\n  - {from: m, to: sink}",
    );
    let r = run(&yaml, 1);
    assert!(r.statuses().iter().all(|(_, from, _, _)| from != "sink"));
    assert_eq!(arrivals_at(&r, "m"), [t("10s")]);
}

#[test]
fn the_shortest_queue_ranks_after_room() {
    // Both busy: the second has the shorter queue, but the first gains room.
    let yaml = pool("shortest_queue", "1s", "3s", "100s", "10s");
    let r = run(&yaml, 1);
    assert_eq!(arrivals_at(&r, "a")[..2], [t("1s"), t("4s")]);
    assert_eq!(arrivals_at(&r, "b")[0], t("2s"));
}

#[test]
fn utilization_rank_is_exact_integers() {
    // `a` has two places, `b` one, both idle at first. The times are odd, so a float
    // would round and an integer is exact, rounded down.
    let yaml = document(
        "10s",
        "  - {id: source, type: Source, interarrival: 1.000000001s}\n  \
         - {id: a, type: Machine, cycle_time: 5s, capacity: 2 parts}\n  \
         - {id: b, type: Machine, cycle_time: 5s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: lowest_utilization}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let routes = r.routes();
    let keys: Vec<(u64, u64)> = routes
        .iter()
        .map(|(_, _, record)| (record.candidates[0].key, record.candidates[1].key))
        .collect();
    // 1.000000001 s: both 0. 2.000000002 s: `a` worked 1.000000001 s on 2 places, 500000000
    // ns per place, rounded down.
    assert_eq!(keys[0], (0, 0));
    assert_eq!(keys[1], (500_000_000, 0));
    assert_eq!(routes[0].2.chosen, 0);
    assert_eq!(routes[1].2.chosen, 1);
    // 3.000000003 s: `a` worked 2.000000002 s on 2 places, `b` 1.000000001 s on 1.
    assert_eq!(keys[2], (1_000_000_001, 1_000_000_001));
    // A tie, and `b` is full: `a` by room, as the record shows.
    assert_eq!(routes[2].2.chosen, 0);
    assert_eq!(routes[2].2.candidates[1].room, Some(false));
}

/// Nanoseconds worked per place by `name` up to `until`, from its notes alone.
#[allow(clippy::unwrap_used)] // Test helper: a count of work per place fits.
fn from_notes(r: &support::Run, name: &str, places: u128, until: SimTime) -> u64 {
    let notes = r.machine_notes(name);
    let mut worked: u128 = 0;
    for pair in notes.windows(2) {
        let (start, end) = (
            pair[0].0.as_nanos(),
            pair[1].0.as_nanos().min(until.as_nanos()),
        );
        if end > start {
            worked += u128::from(pair[0].1.units_in_cycle) * u128::from(end - start);
        }
    }
    if let Some((time, note)) = notes.last()
        && until.as_nanos() > time.as_nanos()
    {
        worked += u128::from(note.units_in_cycle) * u128::from(until.as_nanos() - time.as_nanos());
    }
    u64::try_from(worked / places).unwrap()
}

#[test]
fn work_per_place_matches_trace_utilization() {
    // With failures, setups and random times: every key a record states is what the
    // machine's own notes add up to at the time of the ranking.
    let yaml = document(
        "2h",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 20s}, variants: {red: 50%, blue: 50%}}\n  \
         - {id: a, type: Machine, cycle_time: {dist: exponential, mean: 30s}, capacity: 2 parts, mtbf: {dist: exponential, mean: 10min}, mttr: {dist: exponential, mean: 1min}}\n  \
         - {id: b, type: Machine, cycle_time: {dist: exponential, mean: 25s}, setup: {per_variant: 10s}}\n  \
         - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: lowest_utilization}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 21);
    let routes = r.routes();
    assert!(routes.len() > 100);
    for (_, _, record) in &routes {
        let at = SimTime::from_nanos(record.detail);
        assert_eq!(
            record.candidates[0].key,
            from_notes(&r, "a", 2, at),
            "{record:?}"
        );
        assert_eq!(
            record.candidates[1].key,
            from_notes(&r, "b", 1, at),
            "{record:?}"
        );
    }
}

#[test]
fn a_model_without_a_status_rule_sends_no_status() {
    let r = run(&pool("first_free", "10s", "5s", "5s", "2min"), 1);
    assert_eq!(r.statuses().len(), 0);
    let routes = r.routes();
    assert_eq!(routes.len(), 12);
    for (_, _, record) in routes {
        assert_eq!(record.rule, RouteRule::FirstFree);
        assert!(
            record
                .candidates
                .iter()
                .all(|c| c.key == 0 && c.room.is_none())
        );
    }
}

#[test]
fn a_part_behind_busy_machines_leaves_for_the_first_to_free() {
    // Spec §4's pool without buffers: the part behind busy `a` and `b` moves when `b`
    // frees, and does not wait behind `a` while `b` stands empty.
    let r = run(&pool("shortest_queue", "1s", "100s", "10s", "30s"), 1);
    assert_eq!(arrivals_at(&r, "a"), [t("1s")]);
    assert_eq!(arrivals_at(&r, "b"), [t("2s"), t("12s"), t("22s")]);
}

#[test]
fn an_accept_gives_a_part_its_offer_stood_for_when_edges_share_a_target() {
    // `b` is on two edges of `h`: the first for `xx` alone, the second a pool behind
    // `c`. yy item 2 stands at `c` from 21 s, with no room anywhere. xx item 3 is
    // offered to `b` at 31 s. When `b` frees at 116 s, its `Accept` answers the offer
    // made for item 3: item 2's offer stands at `c`, so item 2 is not for `b`.
    let yaml = document(
        "400s",
        "  - {id: sy, type: Source, interarrival: 10s, variants: {yy: 100%}}\n  \
         - {id: sx, type: Source, interarrival: 15s, variants: {xx: 100%}}\n  \
         - {id: h, type: Machine, cycle_time: 1s, capacity: 20 parts}\n  \
         - {id: b, type: Machine, cycle_time: 100s}\n  \
         - {id: c, type: Machine, cycle_time: 200s}\n  - {id: sink, type: Sink}",
        "  - {from: [sy, sx], to: h}\n  - {from: h, to: b, when: \"variant == 'xx'\"}\n  \
         - {from: h, to: [c, b], choose: preferred}\n  - {from: [b, c], to: sink}",
    );
    let r = run(&yaml, 1);
    support::check_records(&r);
    let at_b: Vec<_> = r
        .arrivals("b")
        .iter()
        .map(|e| {
            (
                e.time,
                support::arrive_attrs(e).and_then(|a| r.variant_name(a.variant)),
            )
        })
        .collect();
    assert_eq!(at_b[1], (t("116s"), Some("xx".to_owned())), "{at_b:?}");
    // Nothing crossed: the offer never moved at 116 s.
    assert!(
        r.routes().iter().all(|(_, _, record)| !record.crossed),
        "{:?}",
        r.routes()
    );
}

#[test]
fn an_offer_moves_when_an_accept_lands_and_takes_another_part() {
    // Two parts reach `buf` at 10 s: the first for `a` alone, the second on a pool
    // whose offer stands at `a`. `a` accepts: its `Status` (no room, `Accept` follows)
    // comes while its `Accept` is on its way, so nothing moves then. The `Accept` takes
    // the first part, which comes first, and the pool's part is still here: its offer
    // moves to `b`, which has room, and does not wait at `a`.
    let yaml = document(
        "30s",
        "  - {id: s1, type: Source, interarrival: 10s, priority: 1}\n  \
         - {id: s2, type: Source, interarrival: 10s}\n  \
         - {id: buf, type: Buffer, capacity: 10 parts}\n  \
         - {id: a, type: Machine, cycle_time: 5s}\n  \
         - {id: b, type: Machine, cycle_time: 5s}\n  - {id: sink, type: Sink}",
        "  - [s1, buf]\n  - [s2, buf]\n  - {from: buf, to: a, when: \"priority > 0\"}\n  \
         - {from: buf, to: [a, b], choose: preferred}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    support::check_records(&r);
    assert_eq!(arrivals_at(&r, "a")[0], t("10s"));
    assert_eq!(arrivals_at(&r, "b")[0], t("10s"));
}
