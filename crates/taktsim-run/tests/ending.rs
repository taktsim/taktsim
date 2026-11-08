//! How a run ends (ADR-0024 §3): `complete` decides, `stop_reason` only explains,
//! and items left with no event are a failure with nothing stored.

mod support;

use support::{TempDir, drawing_model, model, run_with, test_registry};
use taktsim_run::{FailureClass, RunRequest};

fn burst_into(how: &str) -> String {
    model(
        "",
        &format!(
            "  - {{id: burst, type: Burst, count: 3}}\n  - {{id: keep, type: Keep, how: {how}}}"
        ),
        "  - [burst, keep]",
    )
}

#[test]
fn drained_is_complete() {
    let dir = TempDir::new();
    let response = run_with(
        &test_registry(),
        &burst_into("delivered"),
        &RunRequest::new(),
        &dir.store(),
    )
    .unwrap();
    assert!(response.result.complete);
    assert_eq!(response.result.stop_reason, "drained");
    assert_eq!(response.result.runs[0].stop_reason, "drained");
    assert!(response.result.runs[0].run_id.is_some());
    // The clock runs on to the horizon: the system stands empty until then.
    assert_eq!(response.result.runs[0].end_time.to_string(), "1h");
    let alive = response.result.metrics.last().unwrap();
    assert_eq!(alive.name, "items_alive");
    assert_eq!(alive.values[0].value, taktsim_run::Scalar::Integer(0));
}

#[test]
fn stall_is_exit_2_and_not_kept() {
    let dir = TempDir::new();
    let store = dir.store();
    let failure = run_with(
        &test_registry(),
        &burst_into("hold"),
        &RunRequest::new(),
        &store,
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let error = &failure.diagnostics[0];
    assert_eq!(error.type_name, "run.stalled");
    assert_eq!(error.items_alive, Some(3));
    assert_eq!(error.example, None, "a broken run has no valid example");
    assert_eq!(error.time.map(|t| t.to_string()).as_deref(), Some("0s"));
    assert!(
        error.recent.iter().any(|line| line.contains("keep")),
        "{:?}",
        error.recent
    );
    let runs = std::fs::read_dir(store.root().join("runs"))
        .unwrap()
        .count();
    assert_eq!(runs, 0, "the wrong numbers can never be reused");
    let temp = std::fs::read_dir(store.root().join("tmp")).unwrap().count();
    assert_eq!(temp, 0);
}

/// A reader decides from `complete`, whatever word `stop_reason` holds: each of
/// these words goes with the value of the table, and a new word changes nothing a
/// reader does.
#[test]
fn complete_decides_and_stop_reason_only_explains() {
    let registry = test_registry();
    let dir = TempDir::new();
    let store = dir.store();
    let cases: [(&str, RunRequest<'_>, bool, &str); 3] = [
        (&burst_into("delivered"), RunRequest::new(), true, "drained"),
        (
            &drawing_model(", seed: 1"),
            RunRequest::new(),
            true,
            "horizon",
        ),
        (
            &drawing_model(", seed: 2"),
            RunRequest {
                max_events: Some(1),
                ..RunRequest::new()
            },
            false,
            "event_limit",
        ),
    ];
    for (yaml, request, complete, reason) in cases {
        let response = run_with(&registry, yaml, &request, &store).unwrap();
        assert_eq!(response.result.complete, complete, "{reason}");
        assert_eq!(response.result.runs[0].complete, complete, "{reason}");
        assert_eq!(response.result.stop_reason, reason);
        assert_eq!(response.result.experiment_id.is_some(), complete);
    }
    // A cancelled run is partial too, under its own word.
    let flag = std::sync::atomic::AtomicBool::new(true);
    let request = RunRequest {
        cancel: Some(&flag),
        ..RunRequest::new()
    };
    let response = run_with(&registry, &drawing_model(", seed: 3"), &request, &store).unwrap();
    assert!(!response.result.complete);
    assert_eq!(response.result.stop_reason, "cancelled");
    // An unknown word reads as an explanation, and `complete` still decides.
    let text = response
        .result
        .to_text()
        .replace("cancelled", "a_later_word");
    let read: taktsim_run::ResultDocument =
        taktsim_model::decode_str(&text, taktsim_model::Format::Json).unwrap();
    assert!(!read.complete);
    assert_eq!(read.stop_reason, "a_later_word");
}

/// An error of exit code 2 has no valid example. It says where, when and what came
/// last, and nothing is kept (ADR-0024 §8, §9).
#[test]
fn a_failed_run_says_where_when_and_what_came_last() {
    let registry = test_registry();
    for (how, type_name) in [
        ("error", "run.component_failed"),
        ("invariant", "run.invariant_violated"),
        ("lost", "run.conservation_violated"),
        ("unreceived", "run.item_not_received"),
    ] {
        let yaml = model(
            "",
            &format!(
                "  - {{id: burst, type: Burst, count: 1}}\n  - {{id: keep, type: Keep, how: delivered}}\n  \
                 - {{id: faulty, type: Faulty, how: {how}}}"
            ),
            "  - [burst, keep]",
        );
        let dir = TempDir::new();
        let store = dir.store();
        let failure = run_with(&registry, &yaml, &RunRequest::new(), &store).unwrap_err();
        assert_eq!(failure.class, FailureClass::RunFailed, "{how}");
        let error = &failure.diagnostics[0];
        assert_eq!(error.type_name, type_name, "{how}: {}", error.message);
        assert_eq!(error.path.as_deref(), Some("components[2]"), "{how}");
        assert_eq!(
            error.time.map(|t| t.to_string()).as_deref(),
            Some("1min"),
            "{how}"
        );
        assert_eq!(error.example, None, "{how}");
        assert!(error.recent.len() > 1, "{how}");
        assert!(error.recent.last().unwrap().contains("faulty"), "{how}");
        let runs = std::fs::read_dir(store.root().join("runs"))
            .unwrap()
            .count();
        assert_eq!(runs, 0, "{how}: not kept");
        let temp = std::fs::read_dir(store.root().join("tmp")).unwrap().count();
        assert_eq!(temp, 0, "{how}: nothing left behind");
    }
}
