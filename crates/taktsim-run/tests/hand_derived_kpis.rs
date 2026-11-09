//! Independent tests of M8's KPIs on whole runs (ADR-0025 §2, §3), written from the ADR,
//! ADR-0022 and the concept pages only. Each model draws nothing, so its timeline can
//! be counted by hand; the derivation is in the comment by each test, and nothing is
//! taken from a run of the code under test.

use std::path::PathBuf;

use taktsim_model::Format;
use taktsim_run::{
    AccuracyKind, Metric, ModelSource, ResultDocument, RunRequest, RunResponse, Store, run,
};

#[allow(clippy::expect_used)] // Test helper.
fn store(name: &str) -> Store {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hand-derived-kpis-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Store::open(&dir).expect("a store")
}

#[allow(clippy::expect_used, clippy::panic)] // Test helper.
fn run_model(name: &str, model: &str) -> RunResponse {
    let registry = taktsim_components::standard().expect("the standard registry");
    run(
        &registry,
        ModelSource::Text(model, Format::Yaml),
        &RunRequest::new(),
        &store(name),
    )
    .unwrap_or_else(|failure| panic!("{failure:?}"))
}

#[allow(clippy::panic)] // Test helper.
fn metric<'a>(doc: &'a ResultDocument, name: &str) -> &'a Metric {
    doc.metrics
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no metric {name}; have {:?}", names(doc)))
}

fn names(doc: &ResultDocument) -> Vec<&str> {
    doc.metrics.iter().map(|m| m.name.as_str()).collect()
}

/// The value of `name` with exactly these labels (none for a scalar).
#[allow(clippy::expect_used, clippy::panic)] // Test helper.
fn value(doc: &ResultDocument, name: &str, labels: &[(&str, &str)]) -> f64 {
    let m = metric(doc, name);
    let row = m
        .values
        .iter()
        .find(|v| match &v.labels {
            None => labels.is_empty(),
            Some(found) => {
                found.len() == labels.len()
                    && labels
                        .iter()
                        .all(|(k, v)| found.get(*k).map(String::as_str) == Some(*v))
            }
        })
        .unwrap_or_else(|| panic!("no row {labels:?} of {name}: {:?}", m.values));
    row.value.as_f64().expect("a number")
}

fn assert_close(found: f64, expected: f64, what: &str) {
    assert!(
        (found - expected).abs() <= 1e-12 * expected.abs().max(1.0),
        "{what}: found {found}, expected {expected}"
    );
}

fn check(doc: &ResultDocument, name: &str, labels: &[(&str, &str)], expected: f64) {
    assert_close(
        value(doc, name, labels),
        expected,
        &format!("{name} {labels:?}"),
    );
}

/// The `censored` entry's units inside, if there is one.
fn censored(accuracy: &[taktsim_run::Accuracy]) -> Option<f64> {
    accuracy.iter().find_map(|a| match &a.kind {
        AccuracyKind::Censored { units_inside, .. } => Some(*units_inside),
        AccuracyKind::Sampling(_)
        | AccuracyKind::None { .. }
        | AccuracyKind::PartlyUndefined { .. }
        | AccuracyKind::Unknown { .. } => None,
    })
}

fn state(resource: &'static str, state: &'static str) -> [(&'static str, &'static str); 2] {
    [("resource", resource), ("state", state)]
}

fn statistic(s: &'static str) -> [(&'static str, &'static str); 1] {
    [("statistic", s)]
}

/// A buffer that fills and a Source that is blocked, counted event by event.
///
/// Source every 2 s (constant), Buffer of 2, Machine 5 s, Sink, horizon 30 s. The first
/// part appears after the first interval, and the next interval counts from the last
/// part leaving the Source (ADR-0022 §7, S2).
///
/// | t | what happens | buffer | machine |
/// | --- | --- | --- | --- |
/// | 2 | p1 created, through the buffer into the machine | 0 | p1 until 7 |
/// | 4 | p2 created, into the buffer | 1 | |
/// | 6 | p3 | 2 | |
/// | 7 | p1 delivered; p2 to the machine until 12 | 1 | p2 |
/// | 8 | p4 into the buffer | 2 | |
/// | 10 | p5 created; buffer full: the Source holds it | 2 | |
/// | 12 | p2 delivered; p3 to the machine; p5 into the buffer; next interval from 12 | 2 | p3 |
/// | 14 | p6 created and held | | |
/// | 17 | p3 delivered; p4 in; p6 into the buffer | 2 | p4 |
/// | 19 | p7 held | | |
/// | 22 | p4 delivered; p5 in; p7 into the buffer | 2 | p5 |
/// | 24 | p8 held | | |
/// | 27 | p5 delivered; p6 in; p8 into the buffer | 2 | p6 |
/// | 29 | p9 held, until the end at 30 | | |
#[test]
fn buffer_fills_and_the_source_is_blocked() {
    let doc = run_model(
        "blocked-source",
        "format: \"1.0\"\nmodel: {name: blocked-source}\n\
         experiment: {type: terminating, horizon: 30s}\n\
         components:\n\
         - {id: source, type: Source, interarrival: 2s}\n\
         - {id: queue, type: Buffer, capacity: 2 parts}\n\
         - {id: server, type: Machine, cycle_time: 5s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, queue, server, sink]]\n",
    )
    .result;
    assert!(doc.complete);
    // Created p1..p9; delivered p1..p5 at 7, 12, 17, 22, 27.
    check(&doc, "created(source)", &[], 9.0);
    check(&doc, "delivered(sink)", &[], 5.0);
    // Alive at 30: p6 in the machine, p7 and p8 in the buffer, p9 held.
    check(&doc, "items_alive", &[], 4.0);
    // Throughput 5 / 30 s.
    check(&doc, "throughput(sink)", &[], 5.0 / 30.0);
    // Held: p5 [10, 12] 2 s, p6 [14, 17] 3, p7 [19, 22] 3, p8 [24, 27] 3, p9 [29, 30] 1:
    // 12 item-seconds over 30 s.
    check(&doc, "held(source)", &[], 12.0 / 30.0);
    // Source states: blocked exactly while it holds a part, the same 12 s; waiting 18 s.
    check(
        &doc,
        "state_shares(source)",
        &state("source", "blocked"),
        12.0 / 30.0,
    );
    check(
        &doc,
        "state_shares(source)",
        &state("source", "waiting"),
        18.0 / 30.0,
    );
    // Buffer level: 1 over [4, 6], 2 over [6, 7], 1 over [7, 8], 2 over [8, 30]:
    // 2 + 2 + 1 + 44 = 49 item-seconds. It never exceeds its capacity of 2.
    check(&doc, "wip(queue)", &[], 49.0 / 30.0);
    // Machine: holds a part over [2, 30], always in its cycle (the sink always takes):
    // 28 s. Utilization at capacity 1 equals the busy share (ADR-0025 §2).
    check(&doc, "wip(server)", &[], 28.0 / 30.0);
    check(&doc, "utilization(server)", &[], 28.0 / 30.0);
    check(
        &doc,
        "state_shares(server)",
        &state("server", "busy"),
        28.0 / 30.0,
    );
    // Before 2 s it waits; a Source reaches it through a buffer only, so idle, not
    // starved (ADR-0022 §5, I1).
    check(
        &doc,
        "state_shares(server)",
        &state("server", "idle"),
        2.0 / 30.0,
    );
    for s in ["blocked", "starved", "down", "setup"] {
        check(&doc, "state_shares(server)", &state("server", s), 0.0);
    }
    // System WIP, released and not disposed: buffer plus machine, 49 + 28 = 77.
    check(&doc, "wip()", &[], 77.0 / 30.0);
    // Lead time, release (first move out of the Source) to delivery:
    // p1 2 -> 7 = 5, p2 4 -> 12 = 8, p3 6 -> 17 = 11, p4 8 -> 22 = 14, p5 12 -> 27 = 15
    // (p5 was created at 10 and held 2 s, which does not count). Sorted 5 8 11 14 15:
    // mean 53 / 5 = 10.6; p50 = sorted[⌈2.5⌉ − 1] = 11; p90, p95, p99 = sorted[4] = 15.
    check(&doc, "lead_time(sink)", &statistic("mean"), 10.6);
    check(&doc, "lead_time(sink)", &statistic("min"), 5.0);
    check(&doc, "lead_time(sink)", &statistic("p50"), 11.0);
    check(&doc, "lead_time(sink)", &statistic("p90"), 15.0);
    check(&doc, "lead_time(sink)", &statistic("p95"), 15.0);
    check(&doc, "lead_time(sink)", &statistic("p99"), 15.0);
    check(&doc, "lead_time(sink)", &statistic("max"), 15.0);
    // Censoring (ADR-0025 §3): released units still inside at the end, p6, p7, p8: 3.
    // p9 is held, never released, so a lead time could not have seen it either way.
    for row in &metric(&doc, "lead_time(sink)").values {
        let censored = censored(&row.accuracy);
        assert_eq!(censored, Some(3.0), "{row:?}");
    }
    // A model that draws nothing: every value says so, and none has an interval.
    for m in &doc.metrics {
        for row in &m.values {
            assert!(
                matches!(&row.accuracy[0].kind, AccuracyKind::None { reason, .. } if reason == "no_random_draws"),
                "{}: {:?}",
                m.name,
                row.accuracy
            );
        }
    }
    assert_eq!(doc.unreported, [] as [taktsim_run::Unreported; 0]);
}

/// Blocking between two stations with no buffer, counted by hand.
///
/// On-demand Source, `cut` 2 s, `weld` 3 s, Sink, horizon 18.5 s. On demand, a part is
/// created and released at the instant `cut` takes it (ADR-0022 §7).
///
/// | part | into cut | cut done | into weld | delivered | lead |
/// | --- | --- | --- | --- | --- | --- |
/// | p1 | 0 | 2 | 2 | 5 | 5 |
/// | p2 | 2 | 4 | 5 | 8 | 6 |
/// | p3 | 5 | 7 | 8 | 11 | 6 |
/// | p4 | 8 | 10 | 11 | 14 | 6 |
/// | p5 | 11 | 13 | 14 | 17 | 6 |
/// | p6 | 14 | 16 | 17 | (20) | |
/// | p7 | 17 | (19) | | | |
///
/// `cut` is busy over [0, 4], [5, 7], [8, 10], [11, 13], [14, 16], [17, 18.5] = 13.5 s
/// and blocked over [4, 5], [7, 8], [10, 11], [13, 14], [16, 17] = 5 s. `weld` is
/// starved over [0, 2] (a station is upstream) and busy over [2, 18.5] = 16.5 s.
#[test]
fn blocking_between_two_stations() {
    let doc = run_model(
        "two-stations",
        "format: \"1.0\"\nmodel: {name: two-stations}\n\
         experiment: {type: terminating, horizon: 18500ms}\n\
         components:\n\
         - {id: source, type: Source}\n\
         - {id: cut, type: Machine, cycle_time: 2s}\n\
         - {id: weld, type: Machine, cycle_time: 3s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, weld, sink]]\n",
    )
    .result;
    let h = 18.5;
    check(&doc, "created(source)", &[], 7.0);
    check(&doc, "delivered(sink)", &[], 5.0);
    check(&doc, "items_alive", &[], 2.0);
    check(&doc, "throughput(sink)", &[], 5.0 / h);
    check(&doc, "held(source)", &[], 0.0);
    check(&doc, "state_shares(cut)", &state("cut", "busy"), 13.5 / h);
    check(&doc, "state_shares(cut)", &state("cut", "blocked"), 5.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "idle"), 0.0);
    check(
        &doc,
        "state_shares(weld)",
        &state("weld", "starved"),
        2.0 / h,
    );
    check(&doc, "state_shares(weld)", &state("weld", "busy"), 16.5 / h);
    check(&doc, "state_shares(weld)", &state("weld", "idle"), 0.0);
    check(&doc, "utilization(cut)", &[], 13.5 / h);
    check(&doc, "utilization(weld)", &[], 16.5 / h);
    // `cut` always holds a part (in its cycle or blocked): level 1. `weld` holds one
    // over [2, 18.5]. System WIP is their sum: (18.5 + 16.5) / 18.5.
    check(&doc, "wip(cut)", &[], 1.0);
    check(&doc, "wip(weld)", &[], 16.5 / h);
    check(&doc, "wip()", &[], 35.0 / h);
    // Lead times 5 6 6 6 6: mean 29 / 5 = 5.8; p50 = sorted[2] = 6.
    check(&doc, "lead_time(sink)", &statistic("mean"), 5.8);
    check(&doc, "lead_time(sink)", &statistic("min"), 5.0);
    check(&doc, "lead_time(sink)", &statistic("p50"), 6.0);
    check(&doc, "lead_time(sink)", &statistic("max"), 6.0);
}

/// A failure and repair timeline with fixed durations.
///
/// On-demand Source, Machine 10 s with `mtbf: 23s`, `mttr: 5s` (constants), Sink,
/// horizon 58 s. Calendar clock from time zero; up time restarts at each repair end;
/// an interrupted cycle resumes with what remains (ADR-0022 §6).
///
/// | t | what happens |
/// | --- | --- |
/// | 0 to 10 | p1 |
/// | 10 to 20 | p2 |
/// | 20 | p3 starts |
/// | 23 | failure, p3 has 7 s left; repair until 28 |
/// | 28 to 35 | p3 resumes; next failure at 28 + 23 = 51 |
/// | 35 to 45 | p4 |
/// | 45 | p5 starts |
/// | 51 | failure, p5 has 4 s left; repair until 56 |
/// | 56 to 58 | p5 resumes, unfinished at the end |
///
/// Busy 23 + 23 + 2 = 48 s, down 5 + 5 = 10 s. Deliveries at 10, 20, 35, 45. Lead times
/// 10, 10, 15, 10.
#[test]
fn failures_and_repairs_with_fixed_durations() {
    let doc = run_model(
        "failures",
        "format: \"1.0\"\nmodel: {name: failures}\n\
         experiment: {type: terminating, horizon: 58s}\n\
         components:\n\
         - {id: source, type: Source}\n\
         - {id: cut, type: Machine, cycle_time: 10s, mtbf: 23s, mttr: 5s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, sink]]\n",
    )
    .result;
    let h = 58.0;
    check(&doc, "created(source)", &[], 5.0);
    check(&doc, "delivered(sink)", &[], 4.0);
    check(&doc, "throughput(sink)", &[], 4.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "busy"), 48.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "down"), 10.0 / h);
    for s in ["idle", "blocked", "starved", "setup"] {
        check(&doc, "state_shares(cut)", &state("cut", s), 0.0);
    }
    // Repair time never counts as work: units in cycle is 0 while down (ADR-0022 §5).
    check(&doc, "utilization(cut)", &[], 48.0 / h);
    // The machine holds a part the whole time, down or not.
    check(&doc, "wip(cut)", &[], 1.0);
    check(&doc, "wip()", &[], 1.0);
    // Sorted 10 10 10 15: mean 45 / 4 = 11.25; p50 = sorted[⌈2⌉ − 1] = 10;
    // p90 = sorted[⌈3.6⌉ − 1] = 15.
    check(&doc, "lead_time(sink)", &statistic("mean"), 11.25);
    check(&doc, "lead_time(sink)", &statistic("p50"), 10.0);
    check(&doc, "lead_time(sink)", &statistic("p90"), 15.0);
    check(&doc, "lead_time(sink)", &statistic("max"), 15.0);
}

/// Setup on a variant change, counted by hand, and throughput per class.
///
/// Source `a` makes variant `basic` (x below) every 20 s, Source `b` variant `premium`
/// (y below) every 30 s; both feed
/// one Machine of 2 s with `setup: {per_variant: 3s}`; horizon 59 s. One variant per
/// Source draws nothing (ADR-0022 §8). The first unit costs no setup (§7).
///
/// | t | what happens |
/// | --- | --- |
/// | 20 | x arrives; no setup for the first unit; busy until 22 |
/// | 30 | y arrives; change x -> y: setup [30, 33], busy [33, 35] |
/// | 40 | x arrives; change y -> x: setup [40, 43], busy [43, 45] |
///
/// Busy 6 s, setup 6 s, idle 47 s (fed by Sources directly: idle, not starved).
/// Lead times 2, 5, 5: a part waits in the machine during its changeover.
#[test]
fn setup_on_variant_change() {
    let doc = run_model(
        "setup",
        "format: \"1.0\"\nmodel: {name: setup}\n\
         experiment: {type: terminating, horizon: 59s}\n\
         components:\n\
         - {id: a, type: Source, interarrival: 20s, variants: {basic: 100%}}\n\
         - {id: b, type: Source, interarrival: 30s, variants: {premium: 100%}}\n\
         - {id: press, type: Machine, cycle_time: 2s, setup: {per_variant: 3s}}\n\
         - {id: sink, type: Sink}\n\
         flow: [[a, press, sink], [b, press]]\n",
    )
    .result;
    let h = 59.0;
    check(&doc, "delivered(sink)", &[], 3.0);
    check(
        &doc,
        "state_shares(press)",
        &state("press", "busy"),
        6.0 / h,
    );
    check(
        &doc,
        "state_shares(press)",
        &state("press", "setup"),
        6.0 / h,
    );
    check(
        &doc,
        "state_shares(press)",
        &state("press", "idle"),
        47.0 / h,
    );
    check(&doc, "state_shares(press)", &state("press", "starved"), 0.0);
    // No unit is in a cycle during a changeover: utilization 6 / 59.
    check(&doc, "utilization(press)", &[], 6.0 / h);
    // Level: x [20, 22] 2, y [30, 35] 5, x [40, 45] 5 = 12 item-seconds.
    check(&doc, "wip(press)", &[], 12.0 / h);
    check(&doc, "wip()", &[], 12.0 / h);
    // Throughput: 3 in total; with two classes in the dimension, one row per class
    // (ADR-0025 §2): x 2, y 1.
    check(&doc, "throughput(sink)", &[], 3.0 / h);
    check(&doc, "throughput(sink)", &[("variant", "basic")], 2.0 / h);
    check(&doc, "throughput(sink)", &[("variant", "premium")], 1.0 / h);
    // Sorted 2 5 5: mean 4, min 2, p50 = sorted[⌈1.5⌉ − 1] = 5.
    check(&doc, "lead_time(sink)", &statistic("mean"), 4.0);
    check(&doc, "lead_time(sink)", &statistic("min"), 2.0);
    check(&doc, "lead_time(sink)", &statistic("p50"), 5.0);
    // Neither Source is ever blocked: the machine takes each part as it arrives.
    check(&doc, "held(a)", &[], 0.0);
    check(&doc, "held(b)", &[], 0.0);
    check(&doc, "state_shares(b)", &state("b", "waiting"), 1.0);
}

/// Utilization against the busy share at capacity 2 (ADR-0025 §2).
///
/// Source every 10 s, Machine of capacity 2 and 15 s, Sink, horizon 97 s. Arrivals at
/// 10, 20, ..., 90; at most two are ever in a cycle (one starts every 10 s, each takes
/// 15 s), so none waits. Units in cycle integrate to 8 · 15 (starts 10 to 80) + 7 (start
/// 90) = 127 unit-seconds. Utilization 127 / (2 · 97); busy share, at least one unit in
/// a cycle, [10, 97] = 87 / 97; idle 10 / 97. Deliveries at 25, 35, ..., 95: 8.
#[test]
fn utilization_is_not_the_busy_share_at_capacity_two() {
    let doc = run_model(
        "capacity-two",
        "format: \"1.0\"\nmodel: {name: capacity-two}\n\
         experiment: {type: terminating, horizon: 97s}\n\
         components:\n\
         - {id: source, type: Source, interarrival: 10s}\n\
         - {id: oven, type: Machine, cycle_time: 15s, capacity: 2 parts}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, oven, sink]]\n",
    )
    .result;
    check(&doc, "utilization(oven)", &[], 127.0 / 194.0);
    check(
        &doc,
        "state_shares(oven)",
        &state("oven", "busy"),
        87.0 / 97.0,
    );
    check(
        &doc,
        "state_shares(oven)",
        &state("oven", "idle"),
        10.0 / 97.0,
    );
    check(&doc, "state_shares(oven)", &state("oven", "blocked"), 0.0);
    check(&doc, "wip(oven)", &[], 127.0 / 97.0);
    check(&doc, "delivered(sink)", &[], 8.0);
    check(&doc, "lead_time(sink)", &statistic("mean"), 15.0);
    check(&doc, "lead_time(sink)", &statistic("max"), 15.0);
}

/// Line A of concept page 1: on-demand Source, cut 42 s, a buffer of 20, weld 45 s,
/// horizon 8 h. The page states: first part out at 87 s, 639 parts, WIP 22 once full, a
/// part entering the full line needs 16.5 min.
///
/// Expected values come from the textbook recurrence of a two-station line with
/// blocking after service and a buffer of B places, in whole seconds:
/// `s1(n) = d1(n−1)` (cut takes the next part when the last leaves it, on demand),
/// `d1(n) = max(s1(n) + 42, s2(n − B))` (it leaves when the buffer has room, that is
/// when part n − B has started at weld), `s2(n) = max(d1(n), d2(n−1))`,
/// `d2(n) = s2(n) + 45` (the sink always takes). Lead time `d2 − s1`.
#[test]
#[allow(clippy::cast_precision_loss)] // Seconds and counts far below 2^52.
fn line_a_of_page_one() {
    const H: i64 = 8 * 3600;
    const B: usize = 20;
    let (mut s1, mut d1, mut s2, mut d2) = (vec![], vec![], vec![], vec![]);
    loop {
        let n = s1.len();
        let start = if n == 0 { 0 } else { d1[n - 1] };
        if start >= H {
            break;
        }
        let room = if n >= B { s2[n - B] } else { 0 };
        let leave = (start + 42).max(room);
        let weld = leave.max(if n == 0 { 0 } else { d2[n - 1] });
        s1.push(start);
        d1.push(leave);
        s2.push(weld);
        d2.push(weld + 45);
    }
    // No event of the recurrence at exactly the horizon, so no count depends on
    // whether one at H is processed.
    assert!(!s1.contains(&H) && !d1.contains(&H) && !s2.contains(&H) && !d2.contains(&H));
    let delivered: Vec<usize> = (0..s1.len()).filter(|&n| d2[n] <= H).collect();
    let mut lead: Vec<i64> = delivered.iter().map(|&n| d2[n] - s1[n]).collect();
    lead.sort_unstable();
    // The page's own numbers, as a check of the recurrence.
    assert_eq!(delivered.len(), 639);
    assert_eq!(d2[0], 87);
    assert_eq!(lead[lead.len() - 1], 990);

    let clip = |a: i64, b: i64| (b.min(H) - a.min(H)).max(0);
    let wip: i64 = (0..s1.len()).map(|n| clip(s1[n], d2[n])).sum();
    let in_cut: i64 = (0..s1.len()).map(|n| clip(s1[n], d1[n])).sum();
    let in_buffer: i64 = (0..s1.len()).map(|n| clip(d1[n], s2[n])).sum();
    let in_weld: i64 = (0..s1.len()).map(|n| clip(s2[n], d2[n])).sum();
    let cut_busy: i64 = (0..s1.len()).map(|n| clip(s1[n], s1[n] + 42)).sum();
    let weld_busy: i64 = (0..s1.len()).map(|n| clip(s2[n], d2[n])).sum();
    let rank = |p: usize| lead[(p * lead.len()).div_ceil(100) - 1] as f64;
    let h = H as f64;

    let doc = run_model(
        "line-a",
        "format: \"1.0\"\nmodel: {name: line-a, time_unit: s}\n\
         experiment: {type: terminating, horizon: 8h}\n\
         components:\n\
         - {id: source, type: Source}\n\
         - {id: cut, type: Machine, cycle_time: 42s}\n\
         - {id: buf1, type: Buffer, capacity: 20 parts}\n\
         - {id: weld, type: Machine, cycle_time: 45s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, buf1, weld, sink]]\n",
    )
    .result;
    check(&doc, "delivered(sink)", &[], 639.0);
    check(&doc, "created(source)", &[], s1.len() as f64);
    check(&doc, "items_alive", &[], (s1.len() - 639) as f64);
    check(&doc, "throughput(sink)", &[], 639.0 / h);
    check(&doc, "wip()", &[], wip as f64 / h);
    check(&doc, "wip(cut)", &[], in_cut as f64 / h);
    check(&doc, "wip(buf1)", &[], in_buffer as f64 / h);
    check(&doc, "wip(weld)", &[], in_weld as f64 / h);
    check(&doc, "held(source)", &[], 0.0);
    check(
        &doc,
        "state_shares(cut)",
        &state("cut", "busy"),
        cut_busy as f64 / h,
    );
    check(
        &doc,
        "state_shares(cut)",
        &state("cut", "blocked"),
        (H - cut_busy) as f64 / h,
    );
    check(
        &doc,
        "state_shares(weld)",
        &state("weld", "busy"),
        weld_busy as f64 / h,
    );
    check(
        &doc,
        "state_shares(weld)",
        &state("weld", "starved"),
        (H - weld_busy) as f64 / h,
    );
    check(&doc, "utilization(weld)", &[], weld_busy as f64 / h);
    let lead_sum: i64 = lead.iter().sum();
    check(
        &doc,
        "lead_time(sink)",
        &statistic("mean"),
        lead_sum as f64 / lead.len() as f64,
    );
    check(&doc, "lead_time(sink)", &statistic("min"), 87.0);
    check(&doc, "lead_time(sink)", &statistic("max"), 990.0);
    for (p, label) in [(50, "p50"), (90, "p90"), (95, "p95"), (99, "p99")] {
        check(&doc, "lead_time(sink)", &statistic(label), rank(p));
    }
}

/// Declared observables are answered, and overlapping declarations each name the
/// metric that answers them (ADR-0025 §2, "Several declarations, one metric").
#[test]
fn overlapping_declarations_are_all_answered() {
    let doc = run_model(
        "overlap",
        "format: \"1.0\"\nmodel: {name: overlap}\n\
         experiment: {type: terminating, horizon: 1h}\n\
         observe: [\"state_shares(weld, cut)\", \"wip()\", \"state_shares(cut)\", \"wip(cut, buf1)\"]\n\
         components:\n\
         - {id: source, type: Source}\n\
         - {id: cut, type: Machine, cycle_time: 42s}\n\
         - {id: buf1, type: Buffer, capacity: 20 parts}\n\
         - {id: weld, type: Machine, cycle_time: 45s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, buf1, weld, sink]]\n",
    )
    .result;
    assert_eq!(
        metric(&doc, "state_shares(cut)").observables,
        ["state_shares(cut)", "state_shares(weld, cut)"]
    );
    assert_eq!(
        metric(&doc, "state_shares(weld)").observables,
        ["state_shares(weld, cut)"]
    );
    assert_eq!(metric(&doc, "wip()").observables, ["wip()"]);
    assert_eq!(metric(&doc, "wip(cut)").observables, ["wip(cut, buf1)"]);
    assert_eq!(metric(&doc, "wip(buf1)").observables, ["wip(cut, buf1)"]);
    assert_eq!(
        metric(&doc, "wip(weld)").observables,
        [] as [std::string::String; 0]
    );
    assert_eq!(doc.unreported, [] as [taktsim_run::Unreported; 0]);
    // Every KPI row is listed, declared or not (ADR-0025 §2, "Which are listed").
    for name in [
        "held(source)",
        "utilization(cut)",
        "utilization(weld)",
        "throughput(sink)",
    ] {
        metric(&doc, name);
    }
}

/// A run that delivers nothing has no lead time; the declared `lead_time(sink)` goes to
/// `unreported` as `undefined`, while throughput is a true 0 and `held` of a Source
/// that created nothing is a true 0 (ADR-0025 §2, "Undefined values").
///
/// Source every 2 h, horizon 1 h: the first part would appear at 2 h, so nothing is
/// created at all.
#[test]
fn nothing_delivered_leaves_lead_time_undefined() {
    let doc = run_model(
        "nothing-delivered",
        "format: \"1.0\"\nmodel: {name: nothing}\n\
         experiment: {type: terminating, horizon: 1h}\n\
         observe: [\"lead_time(sink)\", \"throughput(sink)\", \"held(source)\"]\n\
         components:\n\
         - {id: source, type: Source, interarrival: 2h}\n\
         - {id: cut, type: Machine, cycle_time: 1min}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, sink]]\n",
    )
    .result;
    check(&doc, "created(source)", &[], 0.0);
    check(&doc, "throughput(sink)", &[], 0.0);
    check(&doc, "held(source)", &[], 0.0);
    check(&doc, "wip()", &[], 0.0);
    check(&doc, "state_shares(cut)", &state("cut", "idle"), 1.0);
    assert!(
        doc.metrics.iter().all(|m| m.name != "lead_time(sink)"),
        "{:?}",
        names(&doc)
    );
    assert_eq!(doc.unreported.len(), 1, "{:?}", doc.unreported);
    assert_eq!(doc.unreported[0].observable, "lead_time(sink)");
    assert_eq!(doc.unreported[0].reason, "undefined");
}

/// The metrics of M7 come first, in M7's order, then the KPIs sorted by name and then
/// in model order (ADR-0025 §2).
#[test]
fn rows_follow_the_stated_order() {
    let doc = run_model(
        "order",
        "format: \"1.0\"\nmodel: {name: order}\n\
         experiment: {type: terminating, horizon: 1h}\n\
         components:\n\
         - {id: source, type: Source}\n\
         - {id: weld, type: Machine, cycle_time: 42s}\n\
         - {id: buf1, type: Buffer, capacity: 20 parts}\n\
         - {id: cut, type: Machine, cycle_time: 45s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, weld, buf1, cut, sink]]\n",
    )
    .result;
    assert_eq!(
        names(&doc),
        [
            "created(source)",
            "delivered(sink)",
            "items_alive",
            "held(source)",
            "lead_time(sink)",
            "state_shares(source)",
            "state_shares(weld)",
            "state_shares(cut)",
            "throughput(sink)",
            "utilization(weld)",
            "utilization(cut)",
            "wip()",
            "wip(weld)",
            "wip(buf1)",
            "wip(cut)",
        ]
    );
    // Each state_shares row list is its type's, in its order (ADR-0025 §1, ADR-0022 §5).
    let states: Vec<_> = metric(&doc, "state_shares(weld)")
        .values
        .iter()
        .map(|v| v.labels.as_ref().unwrap()["state"].clone())
        .collect();
    assert_eq!(
        states,
        ["idle", "busy", "blocked", "starved", "down", "setup"]
    );
    let states: Vec<_> = metric(&doc, "state_shares(source)")
        .values
        .iter()
        .map(|v| v.labels.as_ref().unwrap()["state"].clone())
        .collect();
    assert_eq!(states, ["waiting", "blocked"]);
    // Units (ADR-0025 §2).
    for (name, unit) in [
        ("throughput(sink)", "items/s"),
        ("lead_time(sink)", "s"),
        ("state_shares(cut)", "share"),
        ("utilization(cut)", "share"),
        ("wip()", "items"),
        ("held(source)", "items"),
    ] {
        assert_eq!(metric(&doc, name).unit, unit, "{name}");
    }
}

/// A failure while blocked, counted by hand (ADR-0022 §5, §6: calendar clock, `down`
/// wins over everything, `down -> blocked` on repair).
///
/// On-demand Source, `cut` 2 s with `mtbf: 3s`, `mttr: 2s`, `weld` 10 s, Sink, horizon
/// 14.5 s.
///
/// | t | cut | weld |
/// | --- | --- | --- |
/// | 0 | p1 busy | starved |
/// | 2 | p1 to weld; p2 busy | p1 busy until 12 |
/// | 3 | fails, p2 has 1 s left; down until 5 | |
/// | 5 | repaired; p2 resumes; next failure at 5 + 3 = 8 | |
/// | 6 | p2 done, weld busy: blocked | |
/// | 8 | fails while blocked: down until 10 | |
/// | 10 | repaired: blocked again; next failure at 13 | |
/// | 12 | p2 to weld; p3 busy | p1 delivered; p2 busy until 22 |
/// | 13 | fails, p3 has 1 s left; down to the end | |
///
/// cut: busy 3 + 1 + 1 = 5, down 2 + 2 + 1.5 = 5.5, blocked 2 + 2 = 4 (sum 14.5).
/// weld: starved 2, busy 10 + 2.5 = 12.5. One delivery, p1, lead time 12.
#[test]
fn failure_while_blocked() {
    let doc = run_model(
        "fail-blocked",
        "format: \"1.0\"\nmodel: {name: fail-blocked}\n\
         experiment: {type: terminating, horizon: 14500ms}\n\
         components:\n\
         - {id: source, type: Source}\n\
         - {id: cut, type: Machine, cycle_time: 2s, mtbf: 3s, mttr: 2s}\n\
         - {id: weld, type: Machine, cycle_time: 10s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, weld, sink]]\n",
    )
    .result;
    let h = 14.5;
    check(&doc, "state_shares(cut)", &state("cut", "busy"), 5.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "down"), 5.5 / h);
    check(&doc, "state_shares(cut)", &state("cut", "blocked"), 4.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "idle"), 0.0);
    check(&doc, "utilization(cut)", &[], 5.0 / h);
    check(
        &doc,
        "state_shares(weld)",
        &state("weld", "starved"),
        2.0 / h,
    );
    check(&doc, "state_shares(weld)", &state("weld", "busy"), 12.5 / h);
    check(&doc, "delivered(sink)", &[], 1.0);
    check(&doc, "lead_time(sink)", &statistic("mean"), 12.0);
    // cut holds a part all the time; weld from 2 on.
    check(&doc, "wip(cut)", &[], 1.0);
    check(&doc, "wip(weld)", &[], 12.5 / h);
    check(&doc, "created(source)", &[], 3.0);
    check(&doc, "items_alive", &[], 2.0);
}

/// A batch Source: three parts at once, offered one at a time, the next interval from
/// the last part leaving (ADR-0022 §7).
///
/// Source every 10 s, batch 3, Machine 2 s, Sink, horizon 25 s.
///
/// | t | what happens |
/// | --- | --- |
/// | 10 | p1, p2, p3 created; p1 to the machine until 12; p2, p3 held |
/// | 12 | p1 delivered; p2 released, until 14 |
/// | 14 | p2 delivered; p3 released, until 16; the batch has left: next at 24 |
/// | 16 | p3 delivered |
/// | 24 | p4, p5, p6; p4 to the machine until 26; p5, p6 held to the end |
///
/// Held: p2 2 + p3 4 + p5 1 + p6 1 = 8 item-seconds. Source blocked while it holds:
/// [10, 14] and [24, 25] = 5 s. WIP: p1..p3 2 s each, p4 1 s = 7. Lead times 2, 2, 2.
#[test]
fn batch_source_holds_the_rest_of_its_batch() {
    let doc = run_model(
        "batch",
        "format: \"1.0\"\nmodel: {name: batch}\n\
         experiment: {type: terminating, horizon: 25s}\n\
         components:\n\
         - {id: source, type: Source, interarrival: 10s, batch: 3 parts}\n\
         - {id: cut, type: Machine, cycle_time: 2s}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, cut, sink]]\n",
    )
    .result;
    let h = 25.0;
    check(&doc, "created(source)", &[], 6.0);
    check(&doc, "delivered(sink)", &[], 3.0);
    check(&doc, "items_alive", &[], 3.0);
    check(&doc, "held(source)", &[], 8.0 / h);
    check(
        &doc,
        "state_shares(source)",
        &state("source", "blocked"),
        5.0 / h,
    );
    check(
        &doc,
        "state_shares(source)",
        &state("source", "waiting"),
        20.0 / h,
    );
    check(&doc, "wip()", &[], 7.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "busy"), 7.0 / h);
    check(&doc, "state_shares(cut)", &state("cut", "idle"), 18.0 / h);
    check(&doc, "lead_time(sink)", &statistic("mean"), 2.0);
    check(&doc, "lead_time(sink)", &statistic("max"), 2.0);
    // One released unit inside at the end, p4; p5 and p6 are held, not released.
    let row = &metric(&doc, "lead_time(sink)").values[0];
    let censored = censored(&row.accuracy);
    assert_eq!(censored, Some(1.0), "{row:?}");
}

/// A last-in-first-out buffer changes which parts are delivered, so the lead times
/// (ADR-0022 §7, Buffer: it chooses at `Accept`).
///
/// Source every 2 s, Buffer of 10, `lifo`, Machine 5.5 s, Sink, horizon 19.5 s.
/// Arrivals p1..p9 at 2, 4, ..., 18.
///
/// | t | machine takes | buffer after |
/// | --- | --- | --- |
/// | 2 | p1, until 7.5 | |
/// | 7.5 | p3 (last in of p2, p3), until 13 | p2 |
/// | 13 | p6 (last in of p2, p4, p5, p6), until 18.5 | p2 p4 p5 |
/// | 18.5 | p9, until 24 | p2 p4 p5 p7 p8 |
///
/// Delivered p1 at 7.5 (lead 5.5), p3 at 13 (lead 7), p6 at 18.5 (lead 6.5): sorted
/// 5.5 6.5 7, mean 19 / 3, p50 6.5. Buffer level: 1 [4, 6], 2 [6, 7.5], 1 [7.5, 8],
/// 2 [8, 10], 3 [10, 12], 4 [12, 13], 3 [13, 14], 4 [14, 16], 5 [16, 18], 6 [18, 18.5],
/// 5 [18.5, 19.5]: 2 + 3 + 0.5 + 4 + 6 + 4 + 3 + 8 + 10 + 3 + 5 = 48.5 item-seconds.
/// Machine level [2, 19.5] = 17.5.
#[test]
fn lifo_buffer_by_hand() {
    let doc = run_model(
        "lifo",
        "format: \"1.0\"\nmodel: {name: lifo}\n\
         experiment: {type: terminating, horizon: 19500ms}\n\
         components:\n\
         - {id: source, type: Source, interarrival: 2s}\n\
         - {id: queue, type: Buffer, capacity: 10 parts, discipline: lifo}\n\
         - {id: server, type: Machine, cycle_time: 5500ms}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, queue, server, sink]]\n",
    )
    .result;
    let h = 19.5;
    check(&doc, "created(source)", &[], 9.0);
    check(&doc, "delivered(sink)", &[], 3.0);
    check(&doc, "items_alive", &[], 6.0);
    check(&doc, "lead_time(sink)", &statistic("min"), 5.5);
    check(&doc, "lead_time(sink)", &statistic("p50"), 6.5);
    check(&doc, "lead_time(sink)", &statistic("max"), 7.0);
    check(&doc, "lead_time(sink)", &statistic("mean"), 19.0 / 3.0);
    check(&doc, "wip(queue)", &[], 48.5 / h);
    check(&doc, "wip(server)", &[], 17.5 / h);
    check(&doc, "wip()", &[], 66.0 / h);
}
