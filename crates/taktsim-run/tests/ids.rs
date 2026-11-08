//! Run ids and experiment ids (ADR-0024 §6): opaque, pinned, and covering all that
//! decides a run.

mod support;

use support::{TempDir, drawing_model, run_text};
use taktsim_model::ModelHash;
use taktsim_run::{ExperimentId, RunId, RunIdInputs, RunRequest};

fn inputs() -> RunIdInputs<'static> {
    RunIdInputs {
        engine: "taktsim-core",
        engine_version: "0.1.0",
        record_format: "taktsim-core/2",
        package: "taktsim-components",
        package_version: "0.1.0",
        message_format: "taktsim-components/1",
        model_hash: ModelHash::of(b"model"),
        seed: Some(42),
    }
}

/// The hash here was computed from ADR-0024 §6's definition with Python's
/// `hashlib`, over the canonical JSON written out here:
/// `{"engine":"taktsim-core","engine_version":"0.1.0","message_format":
/// "taktsim-components/1","model_hash":"<SHA-256 of `model`>","package":
/// "taktsim-components","package_version":"0.1.0","record_format":
/// "taktsim-core/2","run_id_format":1,"seed":42}`. A change here makes every store
/// stale, so it is a change of `run_id_format`.
#[test]
fn run_id_is_pinned() {
    assert_eq!(
        RunId::of(&inputs()).unwrap().as_str(),
        "taktsim-core-8df75e4a51ae6bf3bc62e6fc045c70ec0d78053ff3a7c935dc909e8d1de0b09b"
    );
    let none = RunIdInputs {
        seed: None,
        ..inputs()
    };
    assert_eq!(
        RunId::of(&none).unwrap().as_str(),
        "taktsim-core-7e56c4785abdae333c253c6f50b573cbd9fbf04cda68f6adcb389b5a97bfad77"
    );
}

#[test]
fn run_id_names_the_engine() {
    let other = RunIdInputs {
        engine: "other-engine",
        ..inputs()
    };
    assert!(
        RunId::of(&other)
            .unwrap()
            .as_str()
            .starts_with("other-engine-")
    );
    assert_ne!(RunId::of(&other).unwrap(), RunId::of(&inputs()).unwrap());
}

#[test]
fn every_input_changes_the_run_id() {
    let base = RunId::of(&inputs()).unwrap();
    let changed = [
        RunIdInputs {
            engine_version: "0.1.1",
            ..inputs()
        },
        RunIdInputs {
            record_format: "taktsim-core/3",
            ..inputs()
        },
        RunIdInputs {
            package: "other",
            ..inputs()
        },
        RunIdInputs {
            message_format: "taktsim-components/2",
            ..inputs()
        },
        RunIdInputs {
            model_hash: ModelHash::of(b"other"),
            ..inputs()
        },
        RunIdInputs {
            seed: Some(43),
            ..inputs()
        },
        RunIdInputs {
            seed: None,
            ..inputs()
        },
    ];
    for other in changed {
        assert_ne!(RunId::of(&other).unwrap(), base, "{other:?}");
    }
}

#[test]
fn package_version_changes_the_run_id() {
    let fixed = RunIdInputs {
        package_version: "0.1.1",
        ..inputs()
    };
    assert_ne!(RunId::of(&fixed).unwrap(), RunId::of(&inputs()).unwrap());
}

#[test]
fn horizon_changes_the_run_id() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let base = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let request = RunRequest {
        horizon: Some("30min".parse().unwrap()),
        ..RunRequest::new()
    };
    let shorter = run_text(&yaml, &request, &store).unwrap();
    assert_ne!(base.result.runs[0].run_id, shorter.result.runs[0].run_id);
    assert_ne!(
        base.result.provenance.model_hash,
        shorter.result.provenance.model_hash
    );
    assert_eq!(
        base.result.provenance.system_hash,
        shorter.result.provenance.system_hash
    );
    assert_eq!(shorter.result.runs[0].end_time.to_string(), "30min");
}

#[test]
fn experiment_id_is_over_sorted_runs() {
    let a = RunId::of(&inputs()).unwrap();
    let b = RunId::of(&RunIdInputs {
        seed: Some(7),
        ..inputs()
    })
    .unwrap();
    let forward = ExperimentId::over([a.clone(), b.clone()]).unwrap();
    assert_eq!(forward, ExperimentId::over([b.clone(), a.clone()]).unwrap());
    assert_ne!(forward, ExperimentId::over([a]).unwrap());
    assert_eq!(
        ExperimentId::over([RunId::of(&inputs()).unwrap()])
            .unwrap()
            .as_str(),
        "exp-32ee013295b6910ab659bb5b427e38a3f7e79b5b2e7d6d71c815ecd63d2e1aee"
    );
    let _ = b;
}

/// A run stopped by a limit has no id and its document no experiment id, so an id
/// always names one document that later commands can find (ADR-0024 §8).
#[test]
fn partial_document_has_no_ids() {
    let dir = TempDir::new();
    let request = RunRequest {
        max_events: Some(2),
        ..RunRequest::new()
    };
    let response = run_text(&drawing_model(", seed: 1"), &request, &dir.store()).unwrap();
    let result = &response.result;
    assert!(!result.complete);
    assert_eq!(result.experiment_id, None);
    assert_eq!(result.runs[0].run_id, None);
    assert_eq!(result.runs[0].trace, None);
    // What it was is still said.
    assert_eq!(result.runs[0].seed, Some(1));
    assert_eq!(result.provenance.seeds, [1]);
    let text = result.to_text();
    assert!(
        !text.contains("run_id") && !text.contains("experiment_id"),
        "{text}"
    );
}

#[test]
fn a_complete_document_has_an_experiment_id_over_its_runs() {
    let dir = TempDir::new();
    let response = run_text(
        &drawing_model(", seed: 1"),
        &RunRequest::new(),
        &dir.store(),
    )
    .unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    assert_eq!(
        response.result.experiment_id,
        Some(ExperimentId::over([id.clone()]).unwrap())
    );
    assert!(id.as_str().starts_with("taktsim-core-"));
}

#[test]
fn ids_stay_in_one_path_segment() {
    for bad in ["", "abc", "a/b-0", "../x", &format!("A-{}", "0".repeat(64))] {
        assert!(bad.parse::<RunId>().is_err(), "{bad}");
    }
    let sneaky = format!("../{}", "0".repeat(64));
    assert!(sneaky.parse::<RunId>().is_err());
    assert!(format!("x-{}", "0".repeat(64)).parse::<RunId>().is_ok());
    assert!("exp-1".parse::<ExperimentId>().is_err());
}
