//! Which seed a run uses (ADR-0024 §5): a model that draws nothing has none; the
//! option wins over the model; nothing is refused, never entropy.

mod support;

use support::{TempDir, drawing_model, exact_model, run_text};
use taktsim_model::Json;
use taktsim_run::{DiagnosticKind, FailureClass, RunRequest};

#[test]
fn no_seed_refused_when_the_model_draws() {
    let dir = TempDir::new();
    let failure = run_text(&drawing_model(""), &RunRequest::new(), &dir.store()).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    let error = &failure.diagnostics[0];
    assert_eq!(error.type_name, "run.no_seed");
    assert_eq!(error.path.as_deref(), Some("model.seed"));
    assert_eq!(error.example.as_deref(), Some("--seed 42"));
    assert!(error.found.is_some() && error.allowed.is_some());
    assert!(!dir.path().join("store").exists(), "nothing ran");
}

#[test]
fn draws_nothing_runs_with_empty_seeds() {
    let dir = TempDir::new();
    let response = run_text(&exact_model(""), &RunRequest::new(), &dir.store()).unwrap();
    assert_eq!(response.result.provenance.seeds.len(), 0);
    assert_eq!(response.result.runs[0].seed, None);
    assert_eq!(response.warnings.len(), 0);
    let value = &response.result.metrics[0].values[0];
    assert_eq!(
        taktsim_model::Encode::encode(&value.accuracy[0])["reason"],
        "no_random_draws"
    );
}

/// A given seed is unused, with a warning that names it, and one run stays one
/// run: both ways give the same id.
#[test]
fn a_seed_for_a_model_that_draws_nothing_is_unused_and_named() {
    let dir = TempDir::new();
    let store = dir.store();
    let bare = run_text(&exact_model(""), &RunRequest::new(), &store).unwrap();
    let request = RunRequest {
        seeds: &[7],
        ..RunRequest::new()
    };
    let with_option = run_text(&exact_model(""), &request, &store).unwrap();
    assert!(with_option.reused, "the same run");
    assert_eq!(with_option.result, bare.result);
    let warning = &with_option.warnings[0];
    assert_eq!(warning.kind, DiagnosticKind::Warning);
    assert_eq!(warning.type_name, "run.seed_unused");
    assert_eq!(warning.path.as_deref(), Some("--seed"));
    assert_eq!(warning.found.as_deref(), Some("7"));

    let with_model = run_text(&exact_model(", seed: 9"), &RunRequest::new(), &store).unwrap();
    assert_eq!(with_model.result.provenance.seeds.len(), 0);
    assert_eq!(with_model.warnings[0].path.as_deref(), Some("model.seed"));
}

#[test]
fn seed_option_is_recorded_over_model_seed() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let from_model = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let request = RunRequest {
        seeds: &[2],
        ..RunRequest::new()
    };
    let from_option = run_text(&yaml, &request, &store).unwrap();
    assert_eq!(from_model.result.provenance.seeds, [1]);
    assert_eq!(from_option.result.provenance.seeds, [2]);
    assert_eq!(from_option.result.runs[0].seed, Some(2));
    assert_ne!(
        from_model.result.runs[0].run_id,
        from_option.result.runs[0].run_id
    );
    // The stored model keeps what the file said: same model, different seed.
    assert_eq!(
        from_model.result.provenance.model_hash,
        from_option.result.provenance.model_hash
    );
    let value = &from_option.result.metrics[0].values[0];
    let accuracy = taktsim_model::Encode::encode(&value.accuracy[0]);
    assert_eq!(accuracy["reason"], "single_replication");
    assert_eq!(accuracy["interval"], Json::Null);
}

#[test]
fn a_seed_beyond_the_exact_range_is_invalid() {
    let dir = TempDir::new();
    let request = RunRequest {
        seeds: &[1 << 53],
        ..RunRequest::new()
    };
    let failure = run_text(&drawing_model(""), &request, &dir.store()).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].path.as_deref(), Some("--seed"));
}
