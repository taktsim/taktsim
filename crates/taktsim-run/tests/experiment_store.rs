//! What M7's checks of a stored result (ADR-0024 §6, §8) and M7's ending rule
//! (ADR-0024 §3, a wait past the clock is the horizon) do for M8's experiments
//! (ADR-0025 §5, §10): a stored experiment document and every run an experiment
//! counts are checked against the id they are filed under, the text of a response
//! is the stored bytes, and a replication that a wait past the clock holds counts
//! as complete at its horizon.

mod support;

use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;

use support::{TempDir, model, run_text};
use taktsim_model::{Format, decode_str};
use taktsim_run::{
    Budget, Ended, FailureClass, ResultDocument, RunRequest, RunResponse, Scalar, Store,
};

/// A short random line.
fn line() -> String {
    model(
        ", seed: 1",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 50s}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    )
}

/// A part arrives at about 150,000 d and its cycle of 100,000 d would end past the end
/// of the clock, about 213,504 d (ADR-0022 §6); so would the next arrival. Every
/// replication ends with no event left, the part inside, and a wait that never ends.
fn held_past_the_clock(experiment: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: held, seed: 1}}\n\
         experiment: {{type: terminating, horizon: 200000d{experiment}}}\n\
         components:\n  - {{id: source, type: Source, interarrival: {{dist: uniform, min: 140000d, max: 160000d}}}}\n  \
         - {{id: cut, type: Machine, cycle_time: 100000d}}\n  - {{id: sink, type: Sink}}\n\
         flow:\n  - [source, cut, sink]\n"
    )
}

fn list(seeds: &[u64]) -> RunRequest<'_> {
    RunRequest {
        seeds,
        ..RunRequest::new()
    }
}

#[allow(clippy::unwrap_used)] // Test helper.
fn experiment_file(store: &Store, result: &ResultDocument) -> PathBuf {
    store
        .root()
        .join("experiments")
        .join(result.experiment_id.as_ref().unwrap().as_str())
        .join("result.json")
}

#[allow(clippy::unwrap_used)] // Test helper.
fn read(path: &PathBuf) -> ResultDocument {
    decode_str(&std::fs::read_to_string(path).unwrap(), Format::Json).unwrap()
}

/// The kept document of an experiment comes back as its stored bytes, fresh and reused,
/// as a run's does (ADR-0024 §3).
#[test]
fn the_text_of_an_experiment_is_the_stored_document() {
    let dir = TempDir::new();
    let store = dir.store();
    let seeds = [1, 2, 3];
    let fresh = run_text(&line(), &list(&seeds), &store).unwrap();
    let path = experiment_file(&store, &fresh.result);
    let stored = std::fs::read_to_string(&path).unwrap();
    assert_eq!(fresh.text, stored);
    assert_eq!(fresh.text, fresh.result.to_text());

    // A key a later build writes is kept in what is printed, as for a run.
    let later = stored.replacen('{', "{\"written_later\": 1, ", 1);
    std::fs::write(&path, &later).unwrap();
    let reused = run_text(&line(), &list(&seeds), &store).unwrap();
    assert!(reused.reused);
    assert_eq!(reused.text, later);
    assert_eq!(reused.result.to_text(), fresh.result.to_text());
}

/// One change to a stored document.
type Forgery<'a> = Box<dyn Fn(&mut ResultDocument) + 'a>;

/// A document filed under an experiment id that is not that experiment's is refused at
/// `--store`, exit 2, and never returned as it, whichever part of it is not.
#[test]
fn a_forged_experiment_document_is_refused() {
    let dir = TempDir::new();
    let store = dir.store();
    let elsewhere = line().replace("horizon: 1h", "horizon: 2h");
    let other = run_text(&elsewhere, &list(&[7, 8]), &store).unwrap().result;
    let seeds = [1, 2, 3];
    let kept = run_text(&line(), &list(&seeds), &store).unwrap().result;
    let path = experiment_file(&store, &kept);
    let original = std::fs::read_to_string(&path).unwrap();
    let forgeries: Vec<(&str, Forgery<'_>)> = vec![
        (
            "another experiment id",
            Box::new(|doc| doc.experiment_id.clone_from(&other.experiment_id)),
        ),
        (
            "other runs",
            Box::new(|doc| {
                doc.runs.pop();
            }),
        ),
        (
            "other seeds",
            Box::new(|doc| doc.provenance.seeds = vec![1, 2, 4]),
        ),
        (
            "another model",
            Box::new(|doc| doc.provenance.model_hash = other.provenance.model_hash),
        ),
        ("another `complete`", Box::new(|doc| doc.complete = false)),
    ];
    for (what, forge) in forgeries {
        let mut forged = read(&path);
        forge(&mut forged);
        std::fs::write(&path, forged.to_text()).unwrap();
        let failure = run_text(&line(), &list(&seeds), &store).unwrap_err();
        assert_eq!(failure.class, FailureClass::RunFailed, "{what}");
        let line = &failure.diagnostics[0];
        assert_eq!(line.type_name, "run.stored_run_differs", "{what}");
        assert_eq!(line.path.as_deref(), Some("--store"), "{what}");
        assert!(
            line.message.contains("experiment"),
            "{what}: {}",
            line.message
        );
        std::fs::write(&path, &original).unwrap();
    }
    let again = run_text(&line(), &list(&seeds), &store).unwrap();
    assert_eq!(again.text, original);
}

/// A run an experiment counts is checked against the id it is filed under, reused or
/// just run: a result copied into another run's directory is refused.
#[test]
fn a_forged_run_is_not_counted() {
    let dir = TempDir::new();
    let store = dir.store();
    let seeds = [1, 2];
    let kept = run_text(&line(), &list(&seeds), &store).unwrap().result;
    let runs = store.root().join("runs");
    let file = |i: usize| {
        runs.join(kept.runs[i].run_id.as_ref().unwrap().as_str())
            .join("result.json")
    };
    std::fs::copy(file(0), file(1)).unwrap();
    std::fs::remove_dir_all(store.root().join("experiments")).unwrap();
    let failure = run_text(&line(), &list(&seeds), &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");

    // With `--no-reuse` the run is made again, and the stored one is compared.
    let again = RunRequest {
        reuse: false,
        ..list(&seeds)
    };
    let failure = run_text(&line(), &again, &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");
}

#[allow(clippy::unwrap_used)] // Test helper.
fn assert_held_at_the_horizon(response: &RunResponse) {
    for run in &response.result.runs {
        assert!(run.complete, "seed {:?}", run.seed);
        assert_eq!(run.stop_reason, "horizon", "seed {:?}", run.seed);
        assert_eq!(run.end_time.to_string(), "200000d", "seed {:?}", run.seed);
        assert!(run.run_id.is_some(), "seed {:?}: kept", run.seed);
    }
    let alive = response
        .result
        .metrics
        .iter()
        .find(|m| m.name == "items_alive")
        .unwrap();
    // The part is still inside in every replication.
    let one = match alive.values[0].value {
        Scalar::Integer(n) => n == 1,
        Scalar::Number(x) => (x - 1.0).abs() < 1e-12,
        Scalar::Flag(_) => false,
    };
    assert!(one, "{:?}", alive.values[0].value);
}

/// ADR-0024 §3 with ADR-0025 §5: a replication that a wait past the clock holds is no
/// stall but complete at its horizon, so a list counts every seed, over one window.
#[test]
fn a_wait_past_the_clock_completes_every_replication_of_a_list() {
    let dir = TempDir::new();
    let store = dir.store();
    let seeds = [1, 2, 3, 4];
    let response = run_text(&held_past_the_clock(""), &list(&seeds), &store).unwrap();
    assert_eq!(response.ended, Ended::Complete);
    assert!(response.result.complete);
    assert_eq!(response.result.stop_reason, "seeds_finished");
    assert_eq!(response.result.runs.len(), 4);
    assert_held_at_the_horizon(&response);
    assert!(experiment_file(&store, &response.result).is_file());
}

/// The same under a precision it cannot reach: the budget ends it, every counted
/// replication complete at its horizon, and the document is kept (ADR-0025 §5, §10).
#[test]
fn a_wait_past_the_clock_counts_toward_a_budget() {
    let dir = TempDir::new();
    let store = dir.store();
    let request = RunRequest {
        budget: Budget {
            max_runs: NonZeroU32::new(10),
            max_seconds: NonZeroU64::new(600),
        },
        ..RunRequest::new()
    };
    let yaml = held_past_the_clock(", precision: {wip(): 0.1%}");
    let response = run_text(&yaml, &request, &store).unwrap();
    assert_eq!(response.ended, Ended::BudgetExhausted);
    assert!(!response.result.complete);
    assert_eq!(response.result.stop_reason, "budget_exhausted");
    assert_eq!(response.result.runs.len(), 10);
    assert_held_at_the_horizon(&response);
    assert!(experiment_file(&store, &response.result).is_file());
}
