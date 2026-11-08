//! The decision records (ADR-0023 §9): every routing choice has one, written when the
//! part leaves, and every record explains itself.

mod support;

use support::{check_records, document, run, t};
use taktsim_components::{FlowMessage, RouteRule};
use taktsim_model::DispatchRule;
use taktsim_trace::Record;

/// A model with every word.
fn rich(seed_free: bool) -> String {
    let rule = if seed_free {
        "preferred"
    } else {
        "shortest_queue"
    };
    format!(
        "{}control:\n  dispatch: {{weld-1: earliest_due_date, weld-2: priority_class, paint: minimum_setup, qc: fifo, pack: shortest_processing_time}}\n",
        document(
            "2h",
            "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 10s}, variants: {red: 60%, blue: 40%}, due_in: {dist: uniform, min: 1h, max: 2h}, priority_mix: [{priority: 2, share: 30%}, {priority: 0, share: 70%}]}\n  \
             - {id: more, type: Source, interarrival: 13s, variants: {red: 100%}}\n  \
             - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 4s}}\n  \
             - {id: weld-1, type: Machine, cycle_time: {dist: exponential, mean: 11s}, cycle_time_by_variant: {red: {dist: exponential, mean: 7s}}}\n  \
             - {id: weld-2, type: Machine, cycle_time: {dist: exponential, mean: 9s}, mtbf: {dist: exponential, mean: 15min}, mttr: {dist: exponential, mean: 1min}}\n  \
             - {id: buf, type: Buffer, capacity: 4 parts, discipline: lifo}\n  \
             - {id: qc, type: Machine, cycle_time: {dist: exponential, mean: 2s}}\n  \
             - {id: paint, type: Machine, cycle_time: {dist: exponential, mean: 8s}, setup: {per_variant: 20s}}\n  \
             - {id: pack, type: Machine, cycle_time: {dist: exponential, mean: 3s}, cycle_time_by_variant: {red: 2s}}\n  \
             - {id: sink-a, type: Sink}\n  - {id: sink-b, type: Sink}",
            &format!(
                "  - {{from: [source, more], to: cut}}\n  - {{from: cut, to: [weld-1, weld-2], choose: {rule}}}\n  \
                 - {{from: [weld-1, weld-2], to: buf}}\n  - [buf, qc]\n  \
                 - {{from: qc, to: paint, when: \"variant == 'red' and priority > 0\"}}\n  \
                 - {{from: qc, split: {{cut: 10%, pack: 90%}}}}\n  - [paint, pack]\n  \
                 - {{from: pack, to: [sink-a, sink-b]}}"
            ),
        )
    )
}

fn rule_counts(r: &support::Run) -> std::collections::BTreeMap<RouteRule, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for (_, _, record) in r.routes() {
        *counts.entry(record.rule).or_default() += 1;
    }
    counts
}

/// Parts leaving `name`: every part it sent on, by either way.
fn leaving(r: &support::Run, name: &str) -> usize {
    r.events()
        .iter()
        .filter(|e| {
            e.from == name && matches!(e.message, FlowMessage::Arrive(_) | FlowMessage::Return(_))
        })
        .count()
}

#[test]
fn every_routing_choice_has_a_record() {
    for shortest in [false, true] {
        let r = run(&rich(!shortest), 5);
        let id = |name| r.id(name);
        let of = |name, rule| {
            r.routes()
                .iter()
                .filter(|(_, by, record)| *by == id(name) && record.rule == rule)
                .count()
        };
        // Part by part, at each choice: `cut` chooses among the pool, `qc` writes its
        // `when` for every part and its `split` for those that took that edge, `pack`
        // chooses a sink.
        let pool = if shortest {
            RouteRule::ShortestQueue
        } else {
            RouteRule::Preferred
        };
        assert_eq!(of("cut", pool), leaving(&r, "cut"));
        assert_eq!(of("qc", RouteRule::When), leaving(&r, "qc"));
        assert_eq!(
            of("qc", RouteRule::Split),
            leaving(&r, "qc")
                - r.events()
                    .iter()
                    .filter(|e| e.from == "qc"
                        && e.to == "paint"
                        && matches!(e.message, FlowMessage::Arrive(_)))
                    .count()
        );
        assert_eq!(of("pack", RouteRule::FirstFree), leaving(&r, "pack"));
        // Nothing is written for a chain.
        for name in ["source", "weld-1", "weld-2", "buf", "paint", "more"] {
            assert!(
                r.routes().iter().all(|(_, by, _)| *by != id(name)),
                "{name} wrote a route record"
            );
        }
        assert!(leaving(&r, "pack") > 100);
        assert!(of("qc", RouteRule::Split) > 100);
    }
}

#[test]
fn a_when_record_and_its_split_come_in_that_order_for_the_same_part() {
    let r = run(&rich(true), 5);
    let routes = r.routes();
    let by_qc: Vec<_> = routes
        .iter()
        .filter(|(_, by, _)| *by == r.id("qc"))
        .collect();
    let mut seen = 0;
    for (i, (time, _, record)) in by_qc.iter().enumerate() {
        if record.rule == RouteRule::Split {
            let (before_time, _, before) = by_qc[i - 1];
            assert_eq!(before.rule, RouteRule::When);
            assert_eq!(before.item, record.item);
            assert_eq!(before_time, time);
            assert_eq!(before.edge, record.edge);
            seen += 1;
        }
    }
    assert!(seen > 0);
}

#[test]
fn the_condition_text_is_in_the_trace() {
    let r = run(&rich(true), 5);
    let conditions: Vec<_> = r
        .records
        .iter()
        .filter_map(|record| {
            if let Record::Note {
                time,
                by,
                message: FlowMessage::EdgeCondition { edge, text },
            } = record
            {
                Some((*time, r.name(*by).to_owned(), *edge, text.clone()))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        conditions,
        [(
            t("0s"),
            "qc".to_owned(),
            0,
            "variant == 'red' and priority > 0".to_owned()
        )]
    );
}

#[test]
fn every_record_recomputes_its_own_choice() {
    for (seed, preferred) in [(1, true), (2, false), (3, true), (4, false)] {
        let r = run(&rich(preferred), seed);
        assert!(!r.routes().is_empty() && !r.dispatches().is_empty());
        check_records(&r);
        let counts = rule_counts(&r);
        assert!(counts.len() >= 4, "{counts:?}");
    }
}

#[test]
fn status_record_shows_the_ranking_that_placed_the_offer() {
    // Both free: the part stands at `a`. By the time it leaves, `a`'s own `Status` has
    // landed ahead of its `Accept`, and says no room, one more in its queue. The record
    // states what placed it there.
    let yaml = document(
        "12s",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: a, type: Machine, cycle_time: 100s}\n  \
         - {id: b, type: Machine, cycle_time: 100s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: shortest_queue}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let statuses = r.statuses();
    assert_eq!(statuses[0].0, t("10s"));
    assert_eq!(statuses[0].1, "a");
    assert!(!statuses[0].3.room);
    assert_eq!(statuses[0].3.queue, 1);
    let routes = r.routes();
    assert_eq!(routes.len(), 1);
    let (time, _, record) = &routes[0];
    assert_eq!(*time, t("10s"));
    assert_eq!(record.chosen, 0);
    assert_eq!(record.detail, t("10s").as_nanos());
    assert!(!record.crossed);
    assert_eq!(record.candidates[0].room, Some(true));
    assert_eq!(record.candidates[0].key, 0);
    assert_eq!(record.candidates[1].room, Some(true));
    // It stayed where it was placed: no phantom move to `b`.
    assert!(
        r.events()
            .iter()
            .all(|e| !(e.from == "source" && matches!(e.message, FlowMessage::Withdraw))),
        "a loss of room caused by the holder's own offer moved it"
    );
    assert_eq!(
        r.delivered_to("b", |m| matches!(m, FlowMessage::Offer))
            .len(),
        0
    );
}

#[test]
fn crossed_only_at_the_instant_of_a_move() {
    // Room opens at `b` and then at `a` at one instant: the offer moves to `b`, and `a`'s
    // `Accept` crosses the move. The part goes to `a`, and says so.
    let yaml = document(
        "33s",
        "  - {id: source, type: Source, interarrival: 4s}\n  \
         - {id: a, type: Machine, cycle_time: 13s}\n  \
         - {id: b, type: Machine, cycle_time: 22s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: preferred}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 1);
    let routes = r.routes();
    let crossed: Vec<_> = routes
        .iter()
        .filter(|(_, _, record)| record.crossed)
        .collect();
    assert_eq!(crossed.len(), 1, "{routes:?}");
    let (time, _, record) = crossed[0];
    assert_eq!(*time, t("30s"));
    assert_eq!(record.detail, t("30s").as_nanos());
    assert_eq!(record.chosen, 0);
    // The ranking favoured the target it had just moved to: `a` had no room yet.
    assert_eq!(record.candidates[0].room, Some(false));
    assert_eq!(record.candidates[1].room, Some(true));
    // And not one other in a long run with ties, except at the instant of a move.
    let r = run(&rich(true), 11);
    for (time, _, record) in r.routes() {
        if record.crossed {
            assert_eq!(record.detail, time.as_nanos());
        }
    }
}

#[test]
fn declined_pick_writes_no_record_and_the_re_pick_writes_one() {
    // `holder` offers `u` and `t`. Part 1 goes to `u` at 3 s until 13 s, part 2 to `t` at 6
    // s until 13 s, and part 3 waits from 9 s. `other`'s part waits at `t` from 10 s. At
    // 13 s room opens at `u` and then at `t`. `u` takes part 3. `t`, not knowing, picks
    // `holder`, the oldest, and gets a `Decline`. Its pick had no effect and writes
    // nothing; it picks again, with one candidate, and that goes on record, since a pick
    // that failed came first.
    let yaml = format!(
        "{}control:\n  dispatch: {{t: fifo}}\n",
        document(
            "14s",
            "  - {id: holder, type: Source, interarrival: 3s}\n  \
             - {id: other, type: Source, interarrival: 10s}\n  \
             - {id: u, type: Machine, cycle_time: 10s}\n  \
             - {id: t, type: Machine, cycle_time: 7s}\n  - {id: sink, type: Sink}",
            "  - {from: holder, to: [u, t]}\n  - {from: other, to: t}\n  - {from: [u, t], to: sink}",
        )
    );
    let r = run(&yaml, 1);
    // The `Decline` happened.
    let declined = r
        .events()
        .iter()
        .filter(|e| e.from == "holder" && e.to == "t" && matches!(e.message, FlowMessage::Decline))
        .map(|e| e.time)
        .collect::<Vec<_>>();
    // At 3 s `t` accepted part 1 too, with nothing to pick from after its `Decline`.
    assert_eq!(declined, [t("3s"), t("13s")]);
    // One record, of the second pick, with the unit that came.
    let dispatches = r.dispatches();
    assert_eq!(dispatches.len(), 1, "{dispatches:?}");
    let (time, by, record) = &dispatches[0];
    assert_eq!((*time, r.name(*by)), (t("13s"), "t"));
    assert_eq!(record.rule, DispatchRule::Fifo);
    assert_eq!(record.total, 1);
    assert_eq!(record.candidates[0].from, r.id("other"));
    let arrivals = r.arrivals("t");
    let came = arrivals.iter().find(|e| e.from == "other").unwrap();
    assert_eq!(came.time, t("13s"));
    assert_eq!(record.candidates[0].item, came.item);
    // And `first_free` states which target took part 3, and no more.
    let last_route = r.routes().last().cloned().unwrap();
    assert_eq!(last_route.2.rule, RouteRule::FirstFree);
    assert_eq!(last_route.2.chosen, 0);
}

#[test]
fn on_demand_route_record_names_its_item() {
    let yaml = document(
        "1min",
        "  - {id: source, type: Source, variants: {red: 50%, blue: 50%}}\n  \
         - {id: a, type: Machine, cycle_time: 7s}\n  - {id: b, type: Machine, cycle_time: 9s}\n  \
         - {id: sink, type: Sink}",
        "  - {from: source, to: a, when: \"variant == 'red'\"}\n  \
         - {from: source, split: {a: 50%, b: 50%}}\n  - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 6);
    let routes = r.routes();
    assert!(routes.len() > 4);
    for (time, by, record) in &routes {
        if r.name(*by) != "source" {
            continue;
        }
        // The item exists, created at this very instant, just before the record.
        let created = r.records.iter().find_map(|rec| {
            if let Record::ItemCreated { time, item, .. } = rec
                && *item == record.item
            {
                Some(*time)
            } else {
                None
            }
        });
        assert_eq!(created, Some(*time));
        // The very part: it arrives with this item.
        assert!(
            r.events().iter().any(|e| e.item == Some(record.item)
                && e.from == "source"
                && matches!(e.message, FlowMessage::Arrive(_))),
            "{record:?}"
        );
    }
    check_records(&r);
}

#[test]
fn long_queue_lists_best_1024_and_total() {
    // 2,000 units in a Buffer, a declared rule, and one taken at a time.
    let yaml = format!(
        "{}control:\n  dispatch: {{m: fifo}}\n",
        document(
            "1h",
            "  - {id: source, type: Source, interarrival: 1s, batch: 2000 parts}\n  \
             - {id: buf, type: Buffer, capacity: 2000 parts, discipline: lifo}\n  \
             - {id: m, type: Machine, cycle_time: 10min}\n  - {id: sink, type: Sink}",
            "  - [source, buf, m, sink]",
        )
    );
    let r = run(&yaml, 1);
    let dispatches = r.dispatches();
    let (_, by, long) = dispatches
        .iter()
        .find(|(_, _, record)| record.total > 1024)
        .unwrap();
    assert_eq!(r.name(*by), "buf");
    assert_eq!(long.candidates.len(), 1024);
    // Best first: the oldest, the first into a `lifo` Buffer, and the list is what comes
    // next in that order.
    let items: Vec<u64> = long
        .candidates
        .iter()
        .map(|c| c.item.unwrap().as_u64())
        .collect();
    assert!(items.windows(2).all(|w| w[1] == w[0] + 1), "{items:?}");
    // Every unit left out ranks after the last listed: the next record says who was next.
    let position = dispatches
        .iter()
        .position(|(_, _, record)| std::ptr::eq(record, long))
        .unwrap();
    let (_, _, next) = &dispatches[position + 1];
    assert_eq!(next.candidates[0].item, long.candidates[1].item);
    // A smaller queue lists everything.
    check_records(&r);
}

#[test]
fn a_chain_and_one_candidate_write_nothing() {
    let yaml = format!(
        "{}control:\n  dispatch: {{m: fifo}}\n",
        document(
            "1h",
            "  - {id: source, type: Source}\n  - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
            "  - [source, m, sink]",
        )
    );
    let r = run(&yaml, 1);
    assert_eq!(r.routes().len(), 0);
    assert_eq!(r.dispatches().len(), 0);
}

#[test]
fn an_undeclared_order_writes_nothing() {
    // A Buffer's own discipline and the oldest offer at a merge with no rule follow from
    // records already in the trace.
    let yaml = document(
        "1h",
        "  - {id: p, type: Source, interarrival: 1s}\n  - {id: q, type: Source, interarrival: 1s}\n  \
         - {id: buf, type: Buffer, capacity: 5 parts, discipline: priority}\n  \
         - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - {from: [p, q], to: buf}\n  - [buf, m, sink]",
    );
    let r = run(&yaml, 1);
    assert_eq!(r.routes().len() + r.dispatches().len(), 0);
}
