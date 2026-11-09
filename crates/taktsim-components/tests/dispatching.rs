//! Dispatching rules on hand-built queues (ADR-0023 §6, roadmap M6): each rule picks
//! the expected unit, at a station's inlet and in a Buffer upstream.

mod support;

use std::fmt::Write as _;

use support::{arrive_attrs, document, offered_unit, run, t};
use taktsim_components::{FlowMessage, ItemAttrs};
use taktsim_model::DispatchRule;

/// A line through one slow station `m`, with sources of a known unit each: `sources`
/// is a list of source entries, `m` is declared with `rule`.
fn at_station(sources: &[&str], m: &str, rule: &str, until: &str) -> String {
    let mut components = String::new();
    let mut names = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let _ = writeln!(components, "  - {{id: s{i}, type: Source, {source}}}");
        names.push(format!("s{i}"));
    }
    let _ = write!(
        components,
        "  - {{id: m, type: Machine, {m}}}\n  - {{id: sink, type: Sink}}"
    );
    let flow = format!("  - {{from: [{}], to: m}}\n  - [m, sink]", names.join(", "));
    format!(
        "{}control:\n  dispatch: {{m: {rule}}}\n",
        document(until, &components, &flow)
    )
}

fn from(r: &support::Run) -> Vec<String> {
    r.arrivals("m").iter().map(|e| e.from.clone()).collect()
}

#[allow(clippy::unwrap_used)] // Test helper: called on arrivals only.
fn attrs(e: &support::Delivered) -> ItemAttrs {
    arrive_attrs(e).unwrap()
}

#[test]
fn fifo_takes_the_oldest_of_three() {
    // Three sources, offering at 1 s, 2 s and 3 s, behind a machine busy until 10 s.
    let r = run(
        &at_station(
            &[
                "interarrival: 3s, batch: 1 parts",
                "interarrival: 2s",
                "interarrival: 1s",
            ],
            "cycle_time: 10s",
            "fifo",
            "30s",
        ),
        1,
    );
    // s2 at 1 s takes the machine until 11 s. Then s1's unit from 2 s, older than s0's
    // from 3 s and s2's next from 2 s (later in the inlet).
    assert_eq!(from(&r)[..3], ["s2", "s1", "s2"]);
    let (_, _, record) = &r.dispatches()[0];
    assert_eq!(record.rule, DispatchRule::Fifo);
    assert_eq!(record.total, 3);
    // Best first, by the time each became ready.
    let since: Vec<_> = record.candidates.iter().map(|c| c.since).collect();
    assert!(since.windows(2).all(|w| w[0] <= w[1]), "{since:?}");
}

#[test]
fn earliest_due_date_takes_the_nearest_and_no_date_goes_last() {
    let r = run(
        &at_station(
            &[
                "interarrival: 2s, due_in: 3h",
                "interarrival: 2s",
                "interarrival: 2s, due_in: 1h",
                "interarrival: 1s",
            ],
            "cycle_time: 100s",
            "earliest_due_date",
            "210s",
        ),
        1,
    );
    // s3 at 1 s. At 101 s, of 3 h, none, 1 h and none: 1 h, then 3 h; none come last.
    assert_eq!(from(&r)[..3], ["s3", "s2", "s2"]);
    let (_, _, first) = &r.dispatches()[0];
    assert_eq!(first.candidates.last().map(|c| c.key), Some(u64::MAX));
    assert_eq!(
        first.candidates[0].key,
        t("1h").as_nanos() + t("2s").as_nanos()
    );
}

#[test]
fn no_due_date_sorts_last_then_fifo() {
    let r = run(
        &at_station(
            &["interarrival: 2s", "interarrival: 3s", "interarrival: 1s"],
            "cycle_time: 10s",
            "earliest_due_date",
            "22s",
        ),
        1,
    );
    // None has a date: every key is the same, and the older goes first.
    assert_eq!(from(&r)[..2], ["s2", "s0"]);
    let (_, _, record) = &r.dispatches()[0];
    assert!(record.candidates.iter().all(|c| c.key == u64::MAX));
}

#[test]
fn a_due_date_past_the_clock_sorts_with_no_due_date() {
    // s0's due date, 213,503 days after 26 h, is past the clock's 213,503.98 days:
    // it has none. s2 arrives first and holds `m` until 34 h; s3's unit, due at 28 h,
    // goes next; then s1's from 25 h and s0's from 26 h tie at no due date, and the
    // older goes first, as without a date.
    let r = run(
        &at_station(
            &[
                "interarrival: 26h, due_in: 213503d",
                "interarrival: 25h",
                "interarrival: 24h",
                "interarrival: 27h, due_in: 1h",
            ],
            "cycle_time: 10h",
            "earliest_due_date",
            "60h",
        ),
        1,
    );
    let arrivals = r.arrivals("m");
    assert_eq!(from(&r)[..4], ["s2", "s3", "s1", "s0"]);
    assert_eq!(attrs(&arrivals[3]).due, None);
    let (_, _, third) = &r.dispatches()[1];
    assert_eq!(third.total, 2, "{third:?}");
    assert!(
        third.candidates.iter().all(|c| c.key == u64::MAX),
        "{third:?}"
    );
}

#[test]
fn shortest_processing_time_takes_the_shorter_mean() {
    // Machine `m` takes 100 s for `long` and 7 s for `short`. The first unit is `long`,
    // from `s0`; `long` from `s2` waited longer than `short` from `s1`, and still
    // loses.
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, variants: {long: 100%}",
                "interarrival: 3s, variants: {short: 100%}",
                "interarrival: 2s, variants: {long: 100%}",
            ],
            "cycle_time: 100s, cycle_time_by_variant: {short: 7s}",
            "shortest_processing_time",
            "120s",
        ),
        1,
    );
    assert_eq!(from(&r)[..2], ["s0", "s1"]);
    let (_, _, record) = &r.dispatches()[0];
    assert_eq!(record.candidates[0].key, t("7s").as_nanos(), "{record:?}");
    assert_eq!(record.candidates[1].key, t("100s").as_nanos());
}

#[test]
fn shortest_processing_time_is_fifo_without_times_per_variant() {
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, variants: {long: 100%}",
                "interarrival: 3s, variants: {short: 100%}",
                "interarrival: 2s, variants: {long: 100%}",
            ],
            "cycle_time: 100s",
            "shortest_processing_time",
            "120s",
        ),
        1,
    );
    assert_eq!(from(&r)[..2], ["s0", "s2"]);
}

#[test]
fn the_mean_of_a_truncated_normal_is_what_ranks_it() {
    // `mean: 10s, sd: 20s` draws about 20.18 s on average: longer than a constant 15 s.
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, variants: {first: 100%}",
                "interarrival: 2s, variants: {wide: 100%}",
                "interarrival: 3s, variants: {steady: 100%}",
            ],
            "cycle_time: 40s, cycle_time_by_variant: {wide: {dist: normal, mean: 10s, sd: 20s}, steady: 15s}",
            "shortest_processing_time",
            "60s",
        ),
        1,
    );
    assert_eq!(from(&r)[..2], ["s0", "s2"]);
}

#[test]
fn minimum_setup_takes_the_same_variant_then_the_cheapest_change() {
    // The first unit is `a`. Then, of `b`, `c` and `a` again: the same variant.
    let setup = "cycle_time: 10s, setup: {per_variant: 5min, matrix: [{from: a, to: c, time: 1s}]}";
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, variants: {a: 100%}",
                "interarrival: 2s, variants: {b: 100%}",
                "interarrival: 3s, variants: {c: 100%}",
            ],
            setup,
            "minimum_setup",
            "30s",
        ),
        1,
    );
    assert_eq!(from(&r)[..2], ["s0", "s0"]);
    // No `setup` at all: the same variant first, then the oldest.
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, variants: {a: 100%}",
                "interarrival: 2s, variants: {b: 100%}",
            ],
            "cycle_time: 10s",
            "minimum_setup",
            "30s",
        ),
        1,
    );
    assert_eq!(from(&r)[..2], ["s0", "s0"]);
}

#[test]
fn a_free_change_comes_after_the_same_variant_and_before_a_paid_one() {
    // `m` takes `a` from `s0` at 1 s. By 11 s, `b` (a paid change), `d` (free) and `e`
    // (free) wait, and no more `a`: `s0` makes one part only, by its long interval.
    let setup = "cycle_time: 10s, setup: {per_variant: 5min, matrix: [{from: a, to: d, time: 0s}, {from: a, to: e, time: 1s}]}";
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, variants: {a: 100%}",
                "interarrival: 2s, variants: {b: 100%}",
                "interarrival: 3s, variants: {e: 100%}",
                "interarrival: 4s, variants: {d: 100%}",
            ],
            setup,
            "minimum_setup",
            "12s",
        )
        .replace(
            "interarrival: 1s, variants: {a: 100%}",
            "interarrival: 1s, batch: 1 parts, variants: {a: 100%}",
        ),
        1,
    );
    // The first offer, then `a` again: nothing else ranks with the same variant.
    assert_eq!(from(&r)[..2], ["s0", "s0"]);
    let (_, _, record) = &r.dispatches()[0];
    let keys: Vec<u64> = record.candidates.iter().map(|c| c.key).collect();
    // Same: 0. Free: 1. One second: 2 s + 1. Five minutes: 10 min + 1.
    assert_eq!(
        keys,
        [
            0,
            1,
            2 * t("1s").as_nanos() + 1,
            2 * t("5min").as_nanos() + 1
        ]
    );
}

#[test]
fn minimum_setup_follows_the_last_committed_unit() {
    // Two places. `red` is in its cycle from 1 s until 11 s, and `blue` waits behind it
    // from 2 s. Offers of `red` (from 2 s) and `blue` (from 4 s) queue. At 11 s the `red`
    // leaves, a changeover to `blue` starts, and a place is free: the `blue` wins although
    // the machine is set up for `red` until the change ends, and although the `red` waited
    // longer. Taking it spares a change to `red` and back.
    let text = format!(
        "{}control:\n  dispatch: {{m: minimum_setup}}\n",
        document(
            "12s",
            "  - {id: sx, type: Source, interarrival: 1s, variants: {red: 100%}}\n  \
             - {id: sy, type: Source, interarrival: 2s, variants: {blue: 100%}}\n  \
             - {id: m, type: Machine, cycle_time: 10s, capacity: 2 parts, setup: {per_variant: 20s}}\n  \
             - {id: sink, type: Sink}",
            "  - {from: [sx, sy], to: m}\n  - [m, sink]",
        )
    );
    let r = run(&text, 1);
    let order: Vec<_> = r
        .arrivals("m")
        .iter()
        .map(|e| (r.variant_name(attrs(e).variant).unwrap(), e.time))
        .collect();
    assert_eq!(
        order,
        [
            ("red".to_owned(), t("1s")),
            ("blue".to_owned(), t("2s")),
            ("blue".to_owned(), t("11s")),
        ]
    );
    let dispatches = r.dispatches();
    let (_, by, record) = dispatches.last().unwrap();
    assert_eq!(r.name(*by), "m");
    let keys: Vec<u64> = record.candidates.iter().map(|c| c.key).collect();
    assert_eq!(keys, [0, 2 * t("20s").as_nanos() + 1]);
    assert_eq!(record.candidates[0].from, r.id("sy"));
    assert_eq!(record.candidates[1].from, r.id("sx"));
}

/// As above, but through a Buffer: what it knows from `Status` ranks the same way.
#[test]
fn a_buffer_upstream_of_minimum_setup_ranks_by_what_the_station_follows() {
    // The Buffer holds units of two variants in the order they came, and gives the one
    // `m` is on as long as it has one, whatever came first.
    let text = format!(
        "{}control:\n  dispatch: {{m: minimum_setup}}\n",
        document(
            "2min",
            "  - {id: source, type: Source, interarrival: 1s, variants: {red: 50%, blue: 50%}}\n  \
             - {id: buf, type: Buffer, capacity: 30 parts}\n  \
             - {id: m, type: Machine, cycle_time: 5s, setup: {per_variant: 1min}}\n  \
             - {id: sink, type: Sink}",
            "  - [source, buf, m, sink]",
        )
    );
    let r = run(&text, 4);
    let variants: Vec<String> = r
        .arrivals("m")
        .iter()
        .map(|e| r.variant_name(attrs(e).variant).unwrap())
        .collect();
    let changes = variants.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(variants.len() > 10, "{variants:?}");
    assert!(changes <= 2, "{variants:?}");
    // It heard from the station, through `Status`, and only `m` to `buf` talk.
    assert!(
        r.statuses()
            .iter()
            .any(|(_, _, to, s)| to == "buf" && s.follows.is_some())
    );
}

#[test]
fn priority_class_takes_the_highest() {
    let r = run(
        &at_station(
            &[
                "interarrival: 1s, priority: 0",
                "interarrival: 2s, priority: 0",
                "interarrival: 3s, priority: 2",
                "interarrival: 4s, priority: 1",
            ],
            "cycle_time: 10s",
            "priority_class",
            "22s",
        ),
        1,
    );
    assert_eq!(from(&r)[..3], ["s0", "s2", "s2"]);
    let (_, _, record) = &r.dispatches()[0];
    assert_eq!(record.candidates[0].key, 253);
    // Ties go to the older.
    let r = run(
        &at_station(
            &["interarrival: 2s", "interarrival: 3s", "interarrival: 1s"],
            "cycle_time: 10s",
            "priority_class",
            "12s",
        ),
        1,
    );
    assert_eq!(from(&r)[..2], ["s2", "s0"]);
}

/// Two buffers into one station with `fifo` or none.
fn two_buffers(rule: Option<&str>) -> String {
    let control = rule
        .map(|rule| format!("control:\n  dispatch: {{m: {rule}}}\n"))
        .unwrap_or_default();
    let yaml = document(
        "60s",
        "  - {id: early, type: Source, interarrival: 1s, batch: 1 parts}\n  \
         - {id: late, type: Source, interarrival: 5s}\n  \
         - {id: p, type: Buffer, capacity: 10 parts}\n  - {id: q, type: Buffer, capacity: 10 parts}\n  \
         - {id: m, type: Machine, cycle_time: 7s}\n  - {id: sink, type: Sink}",
        "  - [early, p]\n  - [late, q]\n  - {from: [p, q], to: m}\n  - [m, sink]",
    );
    format!("{yaml}{control}")
}

#[test]
fn fifo_is_oldest_ready_across_two_buffers() {
    // A declared `fifo` takes the oldest unit of either buffer: `p` fills every second,
    // `q` every five, and `p`'s units are the older most of the time.
    let r = run(&two_buffers(Some("fifo")), 1);
    let mut last = None;
    for (time, _, record) in r.dispatches() {
        // What it took is the oldest of what it chose from.
        let chosen = record.candidates[0].since;
        assert!(
            record.candidates.iter().all(|c| chosen <= c.since),
            "{time}"
        );
        if let Some(last) = last {
            assert!(chosen >= last, "{time}: {chosen} after {last}");
        }
        last = Some(chosen);
    }
    assert_ne!(r.dispatches().len(), 0);
    // Without a rule, a merge keeps its oldest offer: the two take turns.
    let plain = run(&two_buffers(None), 1);
    let from: Vec<String> = plain.arrivals("m").iter().map(|e| e.from.clone()).collect();
    assert!(
        from.windows(2).filter(|w| w[0] != w[1]).count() >= from.len() - 2,
        "{from:?}"
    );
    assert_eq!(plain.dispatches().len(), 0);
    // And the rule changes the order.
    let ruled: Vec<String> = r.arrivals("m").iter().map(|e| e.from.clone()).collect();
    assert_ne!(ruled, from);
}

/// A Buffer holding known units, and a station with a rule behind it.
fn behind_a_buffer(discipline: &str, rule: &str) -> String {
    format!(
        "{}control:\n  dispatch: {{m: {rule}}}\n",
        document(
            "90s",
            &format!(
                "  - {{id: source, type: Source, interarrival: 1s, priority_mix: [{{priority: 1, share: 50%}}, {{priority: 5, share: 50%}}], variants: {{red: 50%, blue: 50%}}}}\n  \
                 - {{id: buf, type: Buffer, capacity: 20 parts, discipline: {discipline}}}\n  \
                 - {{id: m, type: Machine, cycle_time: 10s}}\n  - {{id: sink, type: Sink}}"
            ),
            "  - [source, buf, m, sink]",
        )
    )
}

#[test]
fn taker_rule_replaces_discipline() {
    // A `lifo` Buffer would give the newest; a declared `fifo` gives the oldest.
    let r = run(&behind_a_buffer("lifo", "fifo"), 3);
    let items: Vec<u64> = r
        .arrivals("m")
        .iter()
        .map(|e| e.item.unwrap().as_u64())
        .collect();
    assert!(items.windows(2).all(|w| w[1] == w[0] + 1), "{items:?}");
    // The Buffer states it, since it chooses when the part leaves.
    let dispatches = r.dispatches();
    assert_ne!(dispatches.len(), 0);
    let (_, by, record) = &dispatches[0];
    assert_eq!(r.name(*by), "buf");
    assert_eq!(r.name(record.taker), "m");
    // Without a rule, the discipline holds, and writes no record.
    let plain = document(
        "90s",
        "  - {id: source, type: Source, interarrival: 1s}\n  \
         - {id: buf, type: Buffer, capacity: 20 parts, discipline: lifo}\n  \
         - {id: m, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}",
        "  - [source, buf, m, sink]",
    );
    let r = run(&plain, 3);
    assert_eq!(r.dispatches().len(), 0);
    let items: Vec<u64> = r
        .arrivals("m")
        .iter()
        .map(|e| e.item.unwrap().as_u64())
        .collect();
    assert!(items.windows(2).any(|w| w[1] > w[0] + 1), "{items:?}");
    // By priority, whatever the discipline.
    let r = run(&behind_a_buffer("fifo", "priority_class"), 3);
    let priorities: Vec<u8> = r.arrivals("m").iter().map(|e| attrs(e).priority).collect();
    assert!(priorities[1..4].iter().all(|p| *p == 5), "{priorities:?}");
}

#[test]
fn a_buffer_describes_the_unit_that_would_win() {
    // A `fifo` Buffer whose best unit by a rule is not its first: what it offers says so.
    let r = run(&behind_a_buffer("fifo", "priority_class"), 3);
    let described: Vec<_> = r
        .delivered_to("m", |m| matches!(m, FlowMessage::OfferOf(_)))
        .iter()
        .filter_map(offered_unit)
        .collect();
    assert!(described.len() > 1);
    assert!(described.iter().any(|unit| unit.priority == 5));
    // A bare `Offer` is left to a station with no rule.
    assert_eq!(
        r.delivered_to("m", |m| matches!(m, FlowMessage::Offer))
            .len(),
        0
    );
}

#[test]
fn a_buffer_says_again_what_changed_and_its_offer_stays() {
    // A unit of higher priority enters while the Buffer waits: it says so with `OfferOf`
    // again, and sends no `Withdraw`.
    let r = run(&behind_a_buffer("fifo", "priority_class"), 3);
    let times = r.times("m", |m| matches!(m, FlowMessage::OfferOf(_)));
    assert!(times.len() > r.arrivals("m").len(), "{times:?}");
    assert!(
        r.times("m", |m| matches!(m, FlowMessage::Withdraw))
            .is_empty(),
        "an entry kept its place"
    );
}

#[test]
fn a_machine_upstream_gives_its_first_finished_unit_meant_for_the_station() {
    let r = run(
        &format!(
            "{}control:\n  dispatch: {{t: fifo}}\n",
            document(
                "40s",
                "  - {id: source, type: Source}\n  \
                 - {id: m, type: Machine, cycle_time: 1s, capacity: 3 parts}\n  \
                 - {id: t, type: Machine, cycle_time: 5s}\n  - {id: sink, type: Sink}",
                "  - [source, m, t, sink]",
            )
        ),
        1,
    );
    let items: Vec<u64> = r
        .arrivals("t")
        .iter()
        .map(|e| e.item.unwrap().as_u64())
        .collect();
    assert!(items.windows(2).all(|w| w[0] < w[1]), "{items:?}");
    // Only one upstream: one candidate each time, so nothing to choose and no record.
    assert_eq!(r.dispatches().len(), 0);
    assert!(r.arrivals("t").len() > 2);
}

#[test]
fn an_on_demand_source_with_a_rule_downstream_offers_what_it_drew() {
    let text = format!(
        "{}control:\n  dispatch: {{m: priority_class}}\n",
        document(
            "1min",
            "  - {id: source, type: Source, priority_mix: [{priority: 3, share: 50%}, {priority: 1, share: 50%}]}\n  \
             - {id: m, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}",
            "  - [source, m, sink]",
        )
    );
    let r = run(&text, 4);
    let described: Vec<_> = r
        .delivered_to("m", |m| matches!(m, FlowMessage::OfferOf(_)))
        .iter()
        .filter_map(offered_unit)
        .collect();
    // No item yet, and what it says is what comes.
    assert!(
        described
            .iter()
            .all(|unit| unit.item.is_none() && unit.due.is_none())
    );
    let came: Vec<u8> = r.arrivals("m").iter().map(|e| attrs(e).priority).collect();
    let said: Vec<u8> = described.iter().map(|unit| unit.priority).collect();
    assert_eq!(came[..], said[..came.len()]);
    assert!(came.contains(&3) && came.contains(&1), "{came:?}");
}

/// Two stations with rules behind one Buffer, free at one instant: the second was told
/// of the unit the first takes.
#[test]
fn a_station_told_of_a_unit_that_has_gone_picks_again() {
    let text = format!(
        "{}control:\n  dispatch: {{t1: fifo, t2: fifo}}\n",
        document(
            "60s",
            "  - {id: source, type: Source, interarrival: 1s, batch: 2 parts}\n  \
             - {id: buf, type: Buffer, capacity: 30 parts}\n  \
             - {id: t1, type: Machine, cycle_time: 10s}\n  \
             - {id: t2, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}",
            "  - [source, buf]\n  - {from: buf, to: [t1, t2]}\n  - {from: [t1, t2], to: sink}",
        )
    );
    let model = support::load(&text);
    let registry = support::registry();
    // A run that cannot end an instant would go on for ever: a limit tells it apart.
    let prepared = registry
        .prepare(
            &model,
            taktsim_components::RunOptions::new()
                .seed(1)
                .max_events(100_000),
        )
        .unwrap();
    let (bytes, report) = prepared.run_traced(Vec::new()).unwrap();
    assert_eq!(report.summary.outcome, taktsim_core::RunOutcome::Horizon);
    let r = support::read(bytes, report);
    // Both take a unit at each instant they free, and neither takes the same.
    let times = |name: &str| -> Vec<_> { r.arrivals(name).iter().map(|e| e.time).collect() };
    assert_eq!(times("t1")[..3], [t("1s"), t("11s"), t("21s")]);
    assert_eq!(times("t2")[..3], [t("1s"), t("11s"), t("21s")]);
    // The stale answer was declined, and the pick after it went on record.
    assert!(
        r.events()
            .iter()
            .any(|e| e.from == "buf" && matches!(e.message, FlowMessage::Decline))
    );
    support::check_records(&r);
}

/// A repair with two places free and offers waiting: the second pick follows the first.
#[test]
fn minimum_setup_takes_one_place_at_a_time_and_a_buffer_ranks_by_the_right_unit() {
    let text = format!(
        "{}control:\n  dispatch: {{m: minimum_setup}}\n",
        document(
            "2min",
            "  - {id: s1, type: Source, interarrival: 1s, variants: {red: 50%, blue: 50%}}\n  \
             - {id: s2, type: Source, interarrival: 3s, variants: {red: 50%, blue: 50%}}\n  \
             - {id: p, type: Buffer, capacity: 5 parts}\n  - {id: q, type: Buffer, capacity: 5 parts}\n  \
             - {id: m, type: Machine, cycle_time: 1s, capacity: 2 parts, mtbf: 10s, mttr: 12s}\n  \
             - {id: sink, type: Sink}",
            "  - [s1, p]\n  - [s2, q]\n  - {from: [p, q], to: m}\n  - [m, sink]",
        )
    );
    for seed in 1..20 {
        let r = run(&text, seed);
        support::check_records(&r);
        // No second `Accept` from `m` goes out before the first one's reply has landed.
        let mut open = 0;
        for e in r.events() {
            if e.from == "m" && matches!(e.message, FlowMessage::Accept) {
                open += 1;
            }
            if e.to == "m" && matches!(e.message, FlowMessage::Arrive(_) | FlowMessage::Decline) {
                open -= 1;
            }
            assert!(open <= 1, "{e:?}");
        }
    }
}
