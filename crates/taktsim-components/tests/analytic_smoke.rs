//! Smoke checks against closed forms. M10 owns the full analytical suite; these
//! only catch a protocol that gives wrong answers.

mod support;

use support::{document, run};
use taktsim_components::RunOptions;
use taktsim_core::NoTrace;

/// Parts delivered per second over the run, with checks on and no trace.
#[allow(clippy::unwrap_used)] // Test helper.
fn throughput(yaml: &str, seed: u64) -> f64 {
    let model = support::load(yaml);
    let registry = support::registry();
    let report = registry
        .prepare(&model, RunOptions::new().seed(seed))
        .unwrap()
        .run(&mut NoTrace)
        .unwrap();
    #[allow(clippy::cast_precision_loss)] // Far below 2^53 parts.
    let delivered = report.items.delivered.get() as f64;
    delivered / report.summary.now.as_secs_f64()
}

#[test]
fn two_stations_without_buffer_give_two_thirds() {
    // A known result for blocking after service: two exponential stations with
    // the same rate and no place between them make 2/3 of a station's rate.
    // About 72,000 parts at this horizon, so 1.5 % is several standard errors.
    let yaml = document(
        "300h",
        "  - {id: source, type: Source}\n  \
         - {id: a, type: Machine, cycle_time: {dist: exponential, mean: 10s}}\n  \
         - {id: b, type: Machine, cycle_time: {dist: exponential, mean: 10s}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, a, b, sink]",
    );
    let rate = throughput(&yaml, 2026);
    let expected = (2.0 / 3.0) / 10.0;
    assert!(
        (rate / expected - 1.0).abs() < 0.015,
        "{rate} against {expected}"
    );
}

#[test]
fn saturated_throughput_is_a_over_cycle() {
    // Page 8's formula: availability over cycle time. A = 60 / 63.
    let yaml = document(
        "2000h",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: 42s, mtbf: {dist: exponential, mean: 1h}, mttr: {dist: exponential, mean: 3min}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let rate = throughput(&yaml, 2026);
    let expected = (60.0 / 63.0) / 42.0;
    assert!(
        (rate / expected - 1.0).abs() < 0.01,
        "{rate} against {expected}"
    );
}

#[test]
fn a_saturated_line_runs_at_the_bottleneck() {
    let yaml = document(
        "8h",
        "  - {id: source, type: Source}\n  - {id: a, type: Machine, cycle_time: 40s}\n  \
         - {id: buf, type: Buffer, capacity: 5 parts}\n  \
         - {id: b, type: Machine, cycle_time: 60s}\n  - {id: sink, type: Sink}",
        "  - [source, a, buf, b, sink]",
    );
    let r = run(&yaml, 1);
    // 100 s for the first part, then one a minute.
    assert_eq!(r.report.items.delivered.get(), 479);
}

#[test]
fn parallel_machines_add_up() {
    // A machine of capacity 2 and two machines in parallel: both make two parts
    // per cycle.
    let capacity = document(
        "1h",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: 1min, capacity: 2 parts}\n  - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    assert_eq!(run(&capacity, 1).report.items.delivered.get(), 120);
    let parallel = document(
        "1h",
        "  - {id: source, type: Source}\n  - {id: a, type: Machine, cycle_time: 1min}\n  \
         - {id: b, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b]}\n  - {from: [a, b], to: sink}",
    );
    assert_eq!(run(&parallel, 1).report.items.delivered.get(), 120);
}
