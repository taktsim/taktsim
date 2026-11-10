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

fn budget(max_runs: u32, max_seconds: Option<u64>) -> taktsim_run::Budget {
    taktsim_run::Budget {
        max_runs: std::num::NonZeroU32::new(max_runs),
        max_seconds: max_seconds.and_then(std::num::NonZeroU64::new),
    }
}

/// What `--seed` names is checked once the model is known (ADR-0025 §4, last row): a
/// model that draws nothing is one run whatever the list, its length or the budget,
/// and each unused flag is a warning, never `run.budget_too_small`, `run.no_budget`
/// or `run.budget_too_large`.
#[test]
fn a_model_that_draws_nothing_takes_any_seed_specs_as_one_run() {
    use taktsim_run::SeedSpec::{One, Range};
    let dir = TempDir::new();
    let store = dir.store();
    let cases: [(&[taktsim_run::SeedSpec], taktsim_run::Budget, &str); 4] = [
        (&[Range { first: 1, last: 20 }], budget(5, None), "20 seeds"),
        (
            &[Range {
                first: 1,
                last: 2000,
            }],
            budget(0, None),
            "2000 seeds",
        ),
        (
            &[Range {
                first: 0,
                last: (1 << 53) - 1,
            }],
            budget(0, None),
            "9007199254740992 seeds",
        ),
        (&[One(4), One(4)], budget(0, None), "2 seeds"),
    ];
    for (specs, budget, found) in cases {
        let request = RunRequest {
            seed_specs: specs,
            budget,
            ..RunRequest::new()
        };
        let response = run_text(&exact_model(""), &request, &store).unwrap();
        assert_eq!(response.result.runs.len(), 1, "{specs:?}");
        assert_eq!(response.result.runs[0].seed, None);
        assert_eq!(response.progress, None, "one run, no experiment");
        let warning = &response.warnings[0];
        assert_eq!(warning.type_name, "run.seed_unused");
        assert_eq!(warning.found.as_deref(), Some(found));
        let unused = response
            .warnings
            .iter()
            .any(|w| w.type_name == "run.flag_unused" && w.path.as_deref() == Some("--max-runs"));
        assert_eq!(unused, budget.max_runs.is_some(), "{specs:?}");
    }
    // One seed named by a spec is the one seed, unused here.
    let request = RunRequest {
        seed_specs: &[Range { first: 7, last: 7 }],
        ..RunRequest::new()
    };
    let response = run_text(&exact_model(""), &request, &store).unwrap();
    assert_eq!(response.warnings[0].found.as_deref(), Some("7"));
    // A malformed spec is refused, whatever the model.
    let request = RunRequest {
        seed_specs: &[Range { first: 5, last: 1 }],
        ..RunRequest::new()
    };
    let failure = run_text(&exact_model(""), &request, &store).unwrap_err();
    assert_eq!(failure.diagnostics[0].type_name, "run.seed_out_of_range");
}

/// A precision target with a seed list gets `run.seed_list_with_precision` first, the
/// error that names the fix, not a budget refusal whose example still fails.
#[test]
fn a_precision_target_refuses_seed_specs_before_any_budget() {
    let dir = TempDir::new();
    let yaml = drawing_model(", seed: 1").replace(
        "horizon: 1h}",
        "horizon: 1h, precision: {\"throughput(sink)\": 5%}}",
    );
    let specs = [taktsim_run::SeedSpec::Range { first: 1, last: 20 }];
    for budget in [
        budget(10, Some(60)),
        budget(0, None),
        budget(2000, Some(60)),
    ] {
        let request = RunRequest {
            seed_specs: &specs,
            budget,
            ..RunRequest::new()
        };
        let failure = run_text(&yaml, &request, &dir.store()).unwrap_err();
        assert_eq!(failure.diagnostics.len(), 1, "{budget:?}");
        assert_eq!(
            failure.diagnostics[0].type_name, "run.seed_list_with_precision",
            "{budget:?}"
        );
        assert_eq!(failure.diagnostics[0].found.as_deref(), Some("20 seeds"));
    }
    // One spec of one seed is the first of the sequence.
    let request = RunRequest {
        seed_specs: &[taktsim_run::SeedSpec::One(3)],
        budget: budget(10, Some(600)),
        ..RunRequest::new()
    };
    let response = run_text(&yaml, &request, &dir.store()).unwrap();
    assert_eq!(response.result.provenance.seeds.first(), Some(&3));
}

/// A model that draws, with a list in `seed_specs`: the list's checks hold as they did
/// in `expand_seeds`, and seeds named both ways join one sorted list.
#[test]
fn seed_specs_of_a_drawing_model_are_a_checked_list() {
    use taktsim_run::SeedSpec::{One, Range};
    let dir = TempDir::new();
    let yaml = drawing_model("");
    let request = RunRequest {
        seed_specs: &[Range { first: 1, last: 20 }],
        budget: budget(19, None),
        ..RunRequest::new()
    };
    let failure = run_text(&yaml, &request, &dir.store()).unwrap_err();
    assert_eq!(failure.diagnostics[0].type_name, "run.budget_too_small");
    let request = RunRequest {
        seed_specs: &[Range {
            first: 1,
            last: 1001,
        }],
        ..RunRequest::new()
    };
    let failure = run_text(&yaml, &request, &dir.store()).unwrap_err();
    assert_eq!(failure.diagnostics.len(), 2);
    let request = RunRequest {
        seed_specs: &[Range {
            first: 0,
            last: (1 << 53) - 1,
        }],
        ..RunRequest::new()
    };
    let failure = run_text(&yaml, &request, &dir.store()).unwrap_err();
    assert_eq!(failure.diagnostics[0].type_name, "run.budget_too_large");
    let request = RunRequest {
        seeds: &[3],
        seed_specs: &[One(9), Range { first: 3, last: 4 }],
        ..RunRequest::new()
    };
    let failure = run_text(&yaml, &request, &dir.store()).unwrap_err();
    assert_eq!(failure.diagnostics[0].type_name, "run.duplicate_seed");
    let request = RunRequest {
        seeds: &[2],
        seed_specs: &[One(9), Range { first: 3, last: 4 }],
        ..RunRequest::new()
    };
    let response = run_text(&yaml, &request, &dir.store()).unwrap();
    assert_eq!(response.result.provenance.seeds, [2, 3, 4, 9]);
}
