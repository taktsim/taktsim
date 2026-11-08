//! What M7 reports: one fold over core's records, keyed on `by` (ADR-0024 §3).

mod support;

use support::{TempDir, model, run_text, run_with, test_registry};
use taktsim_components::{RunOptions, standard};
use taktsim_core::NoTrace;
use taktsim_model::{Format, Loader};
use taktsim_run::{Metric, Over, RunRequest, Scalar};

#[allow(clippy::panic)] // Test helper.
fn value(metrics: &[Metric], name: &str) -> Option<u64> {
    metrics
        .iter()
        .find(|m| m.name == name)
        .map(|m| match m.values[0].value {
            Scalar::Integer(n) => n,
            Scalar::Number(_) | Scalar::Flag(_) => panic!("{name} is not a count"),
        })
}

#[test]
fn metrics_equal_item_counts() {
    let yaml = model(
        ", seed: 5",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 2min}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    );
    let dir = TempDir::new();
    let response = run_text(&yaml, &RunRequest::new(), &dir.store()).unwrap();

    let registry = standard().unwrap();
    let loaded = Loader::new(registry.catalog())
        .load_str(&yaml, Format::Yaml)
        .unwrap();
    let report = registry
        .prepare(&loaded, RunOptions::new())
        .unwrap()
        .run(&mut NoTrace)
        .unwrap();
    let metrics = &response.result.metrics;
    assert_eq!(
        value(metrics, "created(source)"),
        Some(report.items.created.get())
    );
    assert_eq!(
        value(metrics, "delivered(sink)"),
        Some(report.items.delivered.get())
    );
    let alive = report.items.held.get() + report.items.in_transit.get();
    assert_eq!(value(metrics, "items_alive"), Some(alive));
    assert!(alive > 0, "a slow machine leaves parts in the system");
}

#[test]
fn a_zero_row_is_stated_and_units_are_items() {
    // A run too short for any part to reach the Sink.
    let yaml = model(
        "",
        "  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 2h}\n  - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    );
    let dir = TempDir::new();
    let response = run_text(&yaml, &RunRequest::new(), &dir.store()).unwrap();
    let metrics = &response.result.metrics;
    assert_eq!(value(metrics, "delivered(sink)"), Some(0));
    assert!(value(metrics, "created(source)").is_some());
    // A Machine is shown only if it did something itself.
    assert_eq!(value(metrics, "created(cut)"), None);
    assert_eq!(value(metrics, "scrapped(sink)"), None);
    for metric in metrics {
        assert_eq!(metric.unit, "items", "{}", metric.name);
    }
    let names: Vec<&str> = metrics.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["created(source)", "delivered(sink)", "items_alive"]);
    assert!(matches!(metrics[0].over, Over::Window { .. }));
    assert!(matches!(metrics[2].over, Over::At { .. }));
}

/// `Ports` chooses zero rows and never who may create: a type with `Ports::Both`
/// and one with `Ports::None` are counted like a Source.
#[test]
fn any_component_may_create_items() {
    let yaml = model(
        "",
        "  - {id: burst, type: Burst, count: 2}\n  - {id: relay, type: Relay}\n  \
         - {id: keep, type: Keep, how: delivered}\n  - {id: lonely, type: Lonely}",
        "  - [burst, relay, keep]",
    );
    let dir = TempDir::new();
    let response = run_with(&test_registry(), &yaml, &RunRequest::new(), &dir.store()).unwrap();
    let metrics = &response.result.metrics;
    assert_eq!(value(metrics, "created(burst)"), Some(2));
    assert_eq!(value(metrics, "created(relay)"), Some(2));
    assert_eq!(value(metrics, "created(lonely)"), Some(2));
    assert_eq!(value(metrics, "delivered(keep)"), Some(4));
    assert_eq!(value(metrics, "scrapped(lonely)"), Some(2));
    assert_eq!(value(metrics, "items_alive"), Some(0));
}

#[test]
fn scrapped_and_consumed_are_counted_by_who_disposed() {
    for (how, name) in [
        ("scrapped", "scrapped(keep)"),
        ("consumed", "consumed(keep)"),
    ] {
        let yaml = model(
            "",
            &format!(
                "  - {{id: burst, type: Burst, count: 3}}\n  - {{id: keep, type: Keep, how: {how}}}"
            ),
            "  - [burst, keep]",
        );
        let dir = TempDir::new();
        let response = run_with(&test_registry(), &yaml, &RunRequest::new(), &dir.store()).unwrap();
        assert_eq!(value(&response.result.metrics, name), Some(3));
        assert_eq!(value(&response.result.metrics, "items_alive"), Some(0));
        assert_eq!(value(&response.result.metrics, "delivered(keep)"), Some(0));
    }
}
