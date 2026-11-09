//! From a part to a route (ADR-0023 §3, §4): which edge, then which target; a full
//! branch waits; a part for a free target passes a part for a full one.

mod support;

use std::collections::BTreeMap;

use proptest::prelude::*;
use support::{arrive_attrs, d, document, load, registry, run, run_model, t};
use taktsim_components::{FlowMessage, RouteRule};
use taktsim_core::{EngineConfig, NoTrace};

/// A source of two variants in equal shares, on a fixed draw per seed.
const RED_BLUE: &str =
    "{id: source, type: Source, interarrival: 10s, variants: {red: 50%, blue: 50%}}";

#[test]
fn first_match_in_written_order() {
    // A part takes the first edge whose condition holds, as written, even if a later
    // one holds too.
    let yaml = |first: &str, second: &str| {
        document(
            "10min",
            &format!(
                "  - {RED_BLUE}\n  - {{id: a, type: Sink}}\n  - {{id: b, type: Sink}}\n  - {{id: c, type: Sink}}"
            ),
            &format!(
                "  - {{from: source, to: a, when: \"{first}\"}}\n  \
                 - {{from: source, to: b, when: \"{second}\"}}\n  - {{from: source, to: c}}"
            ),
        )
    };
    let r = run(&yaml("variant == 'red'", "variant in ['red', 'blue']"), 5);
    let reds = r.arrival_variants("a");
    assert!(reds.iter().all(|v| v == "red"), "{reds:?}");
    let blues = r.arrival_variants("b");
    assert!(blues.iter().all(|v| v == "blue"), "{blues:?}");
    assert_eq!(
        reds.len() + blues.len(),
        r.events()
            .iter()
            .filter(|e| e.from == "source" && matches!(e.message, FlowMessage::Arrive(_)))
            .count()
    );
    assert!(!reds.is_empty() && !blues.is_empty());
    assert_eq!(r.arrivals("c").len(), 0);
    // The other way round, everything takes the first.
    let r = run(&yaml("variant in ['red', 'blue']", "variant == 'red'"), 5);
    assert!(r.arrivals("b").is_empty() && r.arrivals("c").is_empty());
    assert_eq!(r.arrivals("a").len(), 60);
    // Nothing matches: the last edge has no condition.
    let r = run(&yaml("priority > 0", "priority > 1"), 5);
    assert_eq!(r.arrivals("c").len(), 60);
    // The priority is read too.
    let yaml = document(
        "10min",
        "  - {id: source, type: Source, interarrival: 10s, priority_mix: [{priority: 3, share: 50%}, {priority: 0, share: 50%}]}\n  \
         - {id: a, type: Sink}\n  - {id: b, type: Sink}",
        "  - {from: source, to: a, when: \"priority > 0\"}\n  - {from: source, to: b}",
    );
    let r = run(&yaml, 5);
    let priorities = |name: &str| {
        r.arrivals(name)
            .iter()
            .filter_map(arrive_attrs)
            .map(|attrs| attrs.priority)
            .collect::<Vec<_>>()
    };
    assert!(priorities("a").iter().all(|p| *p == 3));
    assert!(priorities("b").iter().all(|p| *p == 0));
    assert!(!priorities("a").is_empty() && !priorities("b").is_empty());
}

/// Two branches by variant: `blue` is slow to free, `red` always free.
fn two_branches(discipline: &str) -> String {
    document(
        "1h",
        &format!(
            "  - {{id: source, type: Source, interarrival: 10s, variants: {{red: 50%, blue: 50%}}}}\n  \
             - {{id: buf, type: Buffer, capacity: 100 parts{discipline}}}\n  \
             - {{id: fast, type: Machine, cycle_time: 1s}}\n  \
             - {{id: slow, type: Machine, cycle_time: 5min}}\n  - {{id: sink, type: Sink}}"
        ),
        "  - [source, buf]\n  - {from: buf, to: fast, when: \"variant == 'red'\"}\n  - {from: buf, to: slow}\n  \
         - {from: [fast, slow], to: sink}",
    )
}

#[test]
fn full_branch_waits_and_never_takes_another() {
    let r = run(&two_branches(""), 3);
    // Every part goes down its own branch: the slow one is full nearly always, and a
    // blue part waits for it however free the other is.
    let of = |name: &str| r.arrival_variants(name);
    assert!(of("fast").iter().all(|v| v == "red"), "{:?}", of("fast"));
    assert!(of("slow").iter().all(|v| v == "blue"), "{:?}", of("slow"));
    assert_eq!(of("slow").len(), 12);
    // A part for the free branch passes parts for the full one (ADR-0023 §3, H2).
    assert!(of("fast").len() > 100, "{}", of("fast").len());
}

#[test]
fn a_split_part_waits_for_its_branch_and_offers_only_there() {
    // A split with one target always full: its parts never go to the other.
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 10s}\n  \
         - {id: slow, type: Machine, cycle_time: 10min}\n  \
         - {id: fast, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}",
        "  - {from: source, split: {slow: 50%, fast: 50%}}\n  - {from: [slow, fast], to: sink}",
    );
    let r = run(&yaml, 8);
    let routes = r.routes();
    assert!(
        routes
            .iter()
            .all(|(_, _, record)| record.rule == RouteRule::Split)
    );
    // The source holds one part at a time, so a part for `slow` holds up the rest.
    assert_eq!(r.arrivals("slow").len(), 6);
    // An offer goes only where the part is meant.
    let offers_slow = r.times("slow", |m| matches!(m, FlowMessage::Offer));
    let offers_fast = r.times("fast", |m| matches!(m, FlowMessage::Offer));
    assert_eq!(offers_slow.len(), 7);
    assert_eq!(offers_fast.len(), r.arrivals("fast").len());
    for time in &offers_slow {
        assert!(!offers_fast.contains(time), "{time}");
    }
}

#[test]
fn no_other_stream_shifts_when_a_split_is_added() {
    // The source's times do not depend on which way its parts go.
    let plain = document(
        "10min",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 10s}, variants: {red: 50%, blue: 50%}}\n  \
         - {id: a, type: Sink}",
        "  - [source, a]",
    );
    let split = document(
        "10min",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 10s}, variants: {red: 50%, blue: 50%}}\n  \
         - {id: a, type: Sink}\n  - {id: b, type: Sink}",
        "  - {from: source, split: {a: 30%, b: 70%}}",
    );
    let a = run(&plain, 9);
    let b = run(&split, 9);
    let made = |r: &support::Run| -> Vec<_> {
        r.events()
            .iter()
            .filter(|e| matches!(e.message, FlowMessage::NextArrival))
            .map(|e| e.time)
            .collect()
    };
    assert_eq!(made(&a), made(&b));
    let sent = |r: &support::Run| -> Vec<_> {
        r.events()
            .iter()
            .filter_map(arrive_attrs)
            .map(|attrs| attrs.variant)
            .collect()
    };
    assert_eq!(sent(&a), sent(&b));
}

/// Parts by a split, from a source that never waits: counts per target.
fn split_counts(yaml: &str, seed: u64) -> BTreeMap<String, u64> {
    let r = run(yaml, seed);
    let mut counts = BTreeMap::new();
    for name in &r.names {
        let n = r.arrivals(name).len() as u64;
        if n > 0 {
            counts.insert(name.clone(), n);
        }
    }
    counts
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 16,
        rng_seed: proptest::test_runner::RngSeed::Fixed(23),
        ..ProptestConfig::default()
    })]

    /// With fixed seeds, each share is within four standard deviations of its binomial.
    #[test]
    fn split_converges_to_declared_shares(
        seed in 0_u64..1000,
        a in 1_u32..98,
        b in 1_u32..98,
    ) {
        prop_assume!(a + b < 100);
        let c = 100 - a - b;
        let yaml = document(
            "2000s",
            "  - {id: source, type: Source, interarrival: 1s}\n  \
             - {id: s1, type: Sink}\n  - {id: s2, type: Sink}\n  - {id: s3, type: Sink}",
            &format!("  - {{from: source, split: {{s1: {a}%, s2: {b}%, s3: {c}%}}}}"),
        );
        let counts = split_counts(&yaml, seed);
        let n: u64 = counts.values().sum();
        prop_assert_eq!(n, 2000);
        for (name, percent) in [("s1", a), ("s2", b), ("s3", c)] {
            let share = f64::from(percent) / 100.0;
            // At most 2,000 parts, which a float holds exactly.
            let count = f64::from(u32::try_from(counts.get(name).copied().unwrap_or(0)).unwrap());
            let total = f64::from(u32::try_from(n).unwrap());
            let expected = share * total;
            let sd = (total * share * (1.0 - share)).sqrt();
            prop_assert!(
                (count - expected).abs() <= 4.0 * sd,
                "{}: {} parts, expected {} with sd {}",
                name, count, expected, sd
            );
        }
    }
}

#[test]
fn reordering_split_keys_changes_nothing() {
    let yaml = |split: &str| {
        document(
            "10min",
            "  - {id: source, type: Source, interarrival: 5s}\n  - {id: a, type: Sink}\n  - {id: b, type: Sink}",
            &format!("  - {{from: source, split: {split}}}"),
        )
    };
    let first = load(&yaml("{a: 20%, b: 80%}"));
    let second = load(&yaml("{b: 80%, a: 20%}"));
    assert_eq!(first.hash(), second.hash());
    assert_eq!(first.system_hash(), second.system_hash());
    let (r1, r2) = (run_model(&first, 4), run_model(&second, 4));
    assert_eq!(r1.bytes, r2.bytes);
}

#[test]
fn one_draw_per_part_when_it_becomes_ready() {
    // A batch of two: two draws at creation, whichever way each goes.
    let yaml = document(
        "1min",
        "  - {id: source, type: Source, interarrival: 10s, batch: 2 parts}\n  \
         - {id: a, type: Sink}\n  - {id: b, type: Sink}",
        "  - {from: source, split: {a: 50%, b: 50%}}",
    );
    let r = run(&yaml, 2);
    let draws: Vec<u64> = r
        .routes()
        .iter()
        .map(|(_, _, record)| record.detail)
        .collect();
    assert_eq!(draws.len() as u64, r.report.items.delivered.get());
    // Every draw differs: one per part.
    let mut unique = draws.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), draws.len());
}

#[test]
fn an_on_demand_source_routes_the_part_it_is_about_to_offer() {
    let yaml = document(
        "1min",
        "  - {id: source, type: Source, variants: {red: 50%, blue: 50%}}\n  \
         - {id: a, type: Machine, cycle_time: 7s}\n  - {id: b, type: Machine, cycle_time: 9s}\n  \
         - {id: sink, type: Sink}",
        "  - {from: source, to: a, when: \"variant == 'red'\"}\n  - {from: source, to: b}\n  \
         - {from: [a, b], to: sink}",
    );
    let r = run(&yaml, 6);
    assert!(r.arrival_variants("a").iter().all(|v| v == "red"));
    assert!(r.arrival_variants("b").iter().all(|v| v == "blue"));
    // A part exists only once it is accepted: conservation holds, and every record
    // names one.
    for (_, _, record) in r.routes() {
        assert!(
            r.delivered().iter().any(|(_, item)| *item == record.item)
                || r.records.iter().any(|rec| matches!(
                    rec,
                    taktsim_trace::Record::ItemCreated { item, .. } if *item == record.item
                ))
        );
    }
}

#[test]
fn due_dates_and_priorities_are_one_per_batch() {
    let yaml = document(
        "30min",
        "  - id: source\n    type: Source\n    interarrival: {dist: exponential, mean: 1min}\n    batch: 3 parts\n    \
         priority_mix: [{priority: 2, share: 40%}, {priority: 1, share: 60%}]\n    \
         due_in: {dist: uniform, min: 1h, max: 2h}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let r = run(&yaml, 13);
    let arrivals = r.arrivals("sink");
    assert!(arrivals.len() >= 30);
    let attrs: Vec<_> = arrivals
        .iter()
        .filter_map(arrive_attrs)
        .map(|attrs| (attrs.priority, attrs.due.unwrap()))
        .collect();
    let mut priorities = std::collections::BTreeSet::new();
    for batch in attrs.chunks(3) {
        priorities.insert(batch[0].0);
        for unit in batch {
            assert_eq!(unit, &batch[0], "{batch:?}");
        }
        assert!(batch[0].1 >= t("1h"));
    }
    assert_eq!(priorities.len(), 2);
    // A zero due time is the creation itself.
    let yaml = document(
        "1min",
        "  - {id: source, type: Source, interarrival: 10s, due_in: 0s}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let r = run(&yaml, 13);
    let first = &r.arrivals("sink")[0];
    assert!(matches!(first.message, FlowMessage::Arrive(attrs) if attrs.due == Some(first.time)));
}

#[test]
fn a_due_date_past_the_end_of_time_is_no_due_date() {
    // The clock ends after 213,503.98 days: 213,503 days from a creation at 1 d or
    // later is past it. Every part has no due date, and the run reaches its horizon.
    let yaml = document(
        "3d",
        "  - {id: source, type: Source, interarrival: 1d, due_in: 213503d}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let model = load(&yaml);
    let registry = registry();
    let mut engine = registry
        .build(model.document(), EngineConfig::new(1))
        .unwrap();
    let result = engine.run(taktsim_core::RunLimits::until(t("3d")), &mut NoTrace);
    assert!(result.is_ok(), "{result:?}");
    let r = run(&yaml, 1);
    let arrivals = r.arrivals("sink");
    assert_eq!(arrivals.len(), 3);
    assert!(
        arrivals
            .iter()
            .all(|e| matches!(e.message, FlowMessage::Arrive(attrs) if attrs.due.is_none())),
        "{arrivals:?}"
    );
}

#[test]
fn a_due_date_drawn_past_the_end_of_time_is_no_due_date_and_keeps_its_draw() {
    // Exponential of mean 100,000 days against a clock of 213,503.98 days: about one
    // draw in eight is too long for a duration, and more than that land past the
    // clock once added to the creation. Seed fixed when the test was written.
    let seed = 2026;
    let yaml = document(
        "64d",
        "  - {id: source, type: Source, interarrival: 1d, due_in: {dist: exponential, mean: 100000d}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let r = run(&yaml, seed);
    let distribution =
        taktsim_core::random::DurationDistribution::exponential(d("100000d")).unwrap();
    let mut stream = taktsim_core::random::Stream::new(
        seed,
        taktsim_core::random::StreamKey::new("source", "due_in"),
    );
    let arrivals = r.arrivals("sink");
    assert_eq!(arrivals.len(), 64);
    let (mut dated, mut undated) = (0, 0);
    for e in &arrivals {
        let attrs = arrive_attrs(e).unwrap();
        // The source hands each part on at the instant it makes it.
        let expected = distribution
            .sample(&mut stream)
            .ok()
            .and_then(|after| e.time.checked_add(after).ok());
        assert_eq!(attrs.due, expected, "{e:?}");
        match attrs.due {
            Some(_) => dated += 1,
            None => undated += 1,
        }
    }
    assert!(dated > 0 && undated > 0, "{dated} dated, {undated} undated");
}

#[test]
fn a_time_per_variant_comes_from_the_cycle_time_stream() {
    let yaml = |times: &str| {
        document(
            "10min",
            &format!(
                "  - {{id: source, type: Source, variants: {{red: 50%, blue: 50%}}}}\n  \
                 - {{id: m, type: Machine, cycle_time: {{dist: exponential, mean: 10s}}{times}}}\n  \
                 - {{id: sink, type: Sink}}"
            ),
            "  - [source, m, sink]",
        )
    };
    // Constant for one variant: its cycle takes exactly that.
    let r = run(&yaml(", cycle_time_by_variant: {red: 7s}"), 3);
    let notes = r.machine_notes("m");
    let starts: Vec<_> = r.arrivals("m");
    let reds = starts.iter().zip(starts.iter().skip(1)).filter(|(e, _)| {
        matches!(e.message, FlowMessage::Arrive(a) if r.variant_name(a.variant).as_deref() == Some("red"))
    });
    let mut seen = 0;
    for (start, next) in reds {
        assert_eq!(
            next.time.checked_duration_since(start.time).unwrap(),
            d("7s")
        );
        seen += 1;
    }
    assert!(seen > 2);
    assert_ne!(notes.len(), 0);
    // Absent, no draw shifts: the same run as without the key at all.
    let plain = run(&yaml(""), 3);
    let same = run(&yaml(", cycle_time_by_variant: {}"), 3);
    assert_eq!(
        plain.report.items.delivered.get(),
        same.report.items.delivered.get()
    );
    assert_eq!(plain.events(), same.events());
}

#[test]
fn a_plain_edge_writes_nothing_and_takes_no_route_stream() {
    let yaml = document(
        "10min",
        "  - {id: source, type: Source, interarrival: 10s}\n  - {id: m, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    assert!(r.routes().is_empty() && r.dispatches().is_empty() && r.statuses().is_empty());
}
