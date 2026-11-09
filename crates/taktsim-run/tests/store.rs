//! The run store (ADR-0024 §8): atomic, reused, compared, never overwritten.

mod support;

use std::collections::BTreeSet;
use std::path::Path;

use support::{TempDir, drawing_model, exact_model, run_text};
use taktsim_components::standard;
use taktsim_model::ModelHash;
use taktsim_run::{FailureClass, ModelSource, RunRequest, Store, replay_stored, run};

#[allow(clippy::unwrap_used)] // Test helper.
fn listing(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_owned()).collect()
}

#[test]
fn store_layout_is_pinned() {
    let dir = TempDir::new();
    let store = dir.store();
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let root = store.root();
    assert_eq!(
        listing(root),
        set(&[".gitignore", "runs", "store.json", "tmp"])
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("store.json")).unwrap(),
        "{\"store_format\": 1}\n"
    );
    assert_eq!(listing(&root.join("runs")), set(&[id.as_str()]));
    let run_dir = root.join("runs").join(id.as_str());
    assert_eq!(
        listing(&run_dir),
        set(&["model.json", "result.json", "trace.takt"])
    );
    assert_eq!(listing(&root.join("tmp")).len(), 0, "no run in progress");
    let trace = response.result.runs[0].trace.as_ref().unwrap();
    assert_eq!(trace.path, format!("runs/{id}/trace.takt"));
    assert_eq!(
        trace.bytes,
        std::fs::metadata(run_dir.join("trace.takt")).unwrap().len()
    );
    assert_eq!(trace.trace_format, 3);
    assert_eq!(trace.record_format, "taktsim-core/2");
    assert_eq!(trace.message_format, "taktsim-components/1");
    // The document printed and the document stored are the same bytes.
    assert_eq!(
        std::fs::read_to_string(run_dir.join("result.json")).unwrap(),
        response.result.to_text()
    );
    assert!(!response.result.to_text().contains('\r'));
}

#[test]
fn model_json_hashes_to_the_model_hash() {
    let dir = TempDir::new();
    let store = dir.store();
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let bytes = std::fs::read(
        store
            .root()
            .join("runs")
            .join(id.as_str())
            .join("model.json"),
    )
    .unwrap();
    assert_eq!(ModelHash::of(&bytes), response.result.provenance.model_hash);
}

#[test]
fn second_run_is_reused() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    assert!(!first.reused);
    let second = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    assert!(second.reused);
    assert_eq!(second.result, first.result);
    // Another spelling of the same model is the same run.
    let json_like = yaml.replace("mean: 1min", "mean: 60s");
    let third = run_text(&json_like, &RunRequest::new(), &store).unwrap();
    assert!(third.reused);
    // Reuse wins over a limit: nothing runs, so no limit is reached.
    let limited = RunRequest {
        max_events: Some(1),
        ..RunRequest::new()
    };
    let fourth = run_text(&yaml, &limited, &store).unwrap();
    assert!(fourth.reused);
    assert!(fourth.result.complete);
}

#[test]
fn no_reuse_runs_again_and_finds_the_same_trace() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let request = RunRequest {
        reuse: false,
        ..RunRequest::new()
    };
    let again = run_text(&yaml, &request, &store).unwrap();
    assert!(!again.reused);
    assert_eq!(again.result, first.result);
    assert_eq!(listing(&store.root().join("tmp")).len(), 0);
}

#[test]
fn no_reuse_detects_a_different_trace() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    // What an engine that changed its behaviour would find.
    let id = first.result.runs[0].run_id.clone().unwrap();
    let path = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    let hash = first.result.runs[0]
        .trace
        .as_ref()
        .unwrap()
        .stream_hash
        .to_string();
    let altered = std::fs::read_to_string(&path)
        .unwrap()
        .replace(&hash, &"0".repeat(64));
    std::fs::write(&path, &altered).unwrap();
    let request = RunRequest {
        reuse: false,
        ..RunRequest::new()
    };
    let failure = run_text(&yaml, &request, &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.store_differs");
    assert_eq!(failure.diagnostics[0].found.as_deref(), Some(hash.as_str()));
    // Never overwritten.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), altered);
    assert_eq!(listing(&store.root().join("tmp")).len(), 0);
}

/// A shorter stream says nothing about the engine: a limited `--no-reuse` run
/// compares nothing and is partial (exit code 3 in the CLI).
#[test]
fn truncated_no_reuse_exits_3_without_comparing() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    // A stored hash that nothing could match: a comparison would fail.
    let id = first.result.runs[0].run_id.clone().unwrap();
    let path = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    let hash = first.result.runs[0]
        .trace
        .as_ref()
        .unwrap()
        .stream_hash
        .to_string();
    let altered = std::fs::read_to_string(&path)
        .unwrap()
        .replace(&hash, &"0".repeat(64));
    std::fs::write(&path, altered).unwrap();
    let request = RunRequest {
        reuse: false,
        max_events: Some(2),
        ..RunRequest::new()
    };
    let response = run_text(&yaml, &request, &store).unwrap();
    assert!(!response.result.complete);
    assert_eq!(response.result.stop_reason, "event_limit");
    assert_eq!(response.result.runs[0].run_id, None);
}

#[test]
fn partial_run_is_not_kept() {
    let dir = TempDir::new();
    let store = dir.store();
    let request = RunRequest {
        max_events: Some(2),
        ..RunRequest::new()
    };
    let response = run_text(&drawing_model(", seed: 1"), &request, &store).unwrap();
    assert!(!response.result.complete);
    assert_eq!(listing(&store.root().join("runs")).len(), 0);
    assert_eq!(listing(&store.root().join("tmp")).len(), 0);
    // So the next run without the limit really runs.
    let full = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    assert!(!full.reused);
    assert!(full.result.complete);
}

#[test]
fn unknown_store_format_is_refused() {
    let dir = TempDir::new();
    let root = dir.path().join("store");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("store.json"), "{\"store_format\": 99}").unwrap();
    let failure = Store::open(&root).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    assert_eq!(failure.diagnostics[0].type_name, "store.format");
    assert_eq!(failure.diagnostics[0].example, None);
    std::fs::write(root.join("store.json"), "not json").unwrap();
    assert_eq!(Store::open(&root).unwrap_err().class, FailureClass::Io);
    // A later key beside the format is fine: the file is open.
    std::fs::write(
        root.join("store.json"),
        "{\"store_format\": 1, \"later\": true}",
    )
    .unwrap();
    assert!(Store::open(&root).is_ok());
}

#[test]
fn a_store_that_cannot_be_written_is_an_io_failure() {
    let dir = TempDir::new();
    let file = dir.path().join("a-file");
    std::fs::write(&file, "x").unwrap();
    // Refused on opening where a file has no children, on writing elsewhere.
    let failure = Store::open(&file)
        .and_then(|store| run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store))
        .unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    assert!(failure.diagnostics[0].type_name.starts_with("store."));
    assert_eq!(failure.diagnostics[0].example, None);
}

#[test]
fn a_stored_result_that_does_not_read_is_an_io_failure() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let id = first.result.runs[0].run_id.clone().unwrap();
    std::fs::write(
        store
            .root()
            .join("runs")
            .join(id.as_str())
            .join("result.json"),
        "{}",
    )
    .unwrap();
    let failure = run_text(&yaml, &RunRequest::new(), &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    assert_eq!(failure.diagnostics[0].type_name, "store.result");
}

#[test]
fn replay_of_a_stored_run_checks_what_it_replays() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let replayed = replay_stored(&registry, &store, &id).unwrap();
    assert_eq!(
        Some(replayed.stream_hash),
        response.result.runs[0]
            .trace
            .as_ref()
            .map(|t| t.stream_hash)
    );

    // A run with no seed is replayed under another internal seed.
    let exact = run_text(&exact_model(""), &RunRequest::new(), &store).unwrap();
    let exact_id = exact.result.runs[0].run_id.clone().unwrap();
    assert!(replay_stored(&registry, &store, &exact_id).unwrap().events > 0);

    // An id that is not in the store is invalid input.
    let unknown = format!("taktsim-core-{}", "0".repeat(64)).parse().unwrap();
    let failure = replay_stored(&registry, &store, &unknown).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].type_name, "run.unknown_id");

    // A stored model that is not the one the result states.
    let run_dir = store.root().join("runs").join(id.as_str());
    let model = std::fs::read_to_string(run_dir.join("model.json")).unwrap();
    std::fs::write(run_dir.join("model.json"), model.replace("1h", "2h")).unwrap();
    let failure = replay_stored(&registry, &store, &id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");
    std::fs::write(run_dir.join("model.json"), &model).unwrap();

    // A result of another package version.
    let result = std::fs::read_to_string(run_dir.join("result.json")).unwrap();
    let other = result.replace(
        &format!("\"package_version\": \"{}\"", env!("CARGO_PKG_VERSION")),
        "\"package_version\": \"999.0.0\"",
    );
    assert_ne!(other, result);
    std::fs::write(run_dir.join("result.json"), other).unwrap();
    let failure = replay_stored(&registry, &store, &id).unwrap_err();
    assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");
    assert!(failure.diagnostics[0].message.contains("999.0.0"));
    std::fs::write(run_dir.join("result.json"), result).unwrap();

    // A trace that was changed.
    let path = run_dir.join("trace.takt");
    let mut trace = std::fs::read(&path).unwrap();
    let last = trace.len() - 1;
    trace[last] ^= 1;
    std::fs::write(&path, trace).unwrap();
    assert!(replay_stored(&registry, &store, &id).is_err());
}

#[test]
fn a_model_file_that_cannot_be_read_is_an_io_failure() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let missing = dir.path().join("missing.yaml");
    let failure = run(
        &registry,
        ModelSource::Path(&missing),
        &RunRequest::new(),
        &dir.store(),
    )
    .unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    assert_eq!(failure.diagnostics[0].type_name, "model.io");
    assert_eq!(failure.diagnostics[0].path.as_deref(), Some("<model>"));
    assert_eq!(failure.diagnostics[0].example, None);
}

#[test]
fn a_zero_horizon_is_invalid_with_its_path() {
    let dir = TempDir::new();
    let request = RunRequest {
        horizon: Some("0s".parse().unwrap()),
        ..RunRequest::new()
    };
    let failure = run_text(&drawing_model(", seed: 1"), &request, &dir.store()).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(failure.diagnostics[0].path.as_deref(), Some("--horizon"));
    assert_eq!(failure.diagnostics[0].example.as_deref(), Some("8h"));
}

#[test]
fn discover_finds_the_nearest_store_upward() {
    let dir = TempDir::new();
    let below = dir.path().join("a").join("b");
    std::fs::create_dir_all(&below).unwrap();
    std::fs::create_dir_all(dir.path().join(".taktsim")).unwrap();
    std::fs::create_dir_all(dir.path().join("a").join(".taktsim")).unwrap();
    assert_eq!(
        Store::discover(&below).unwrap().root(),
        dir.path().join("a").join(".taktsim")
    );
}

/// A file that states a zero horizon is wrong itself: that stays at the file's path, even
/// with a flag that would have replaced it.
#[test]
fn a_zero_horizon_in_the_file_is_at_the_files_path() {
    let dir = TempDir::new();
    let text = drawing_model(", seed: 1").replace("1h", "0s");
    let request = RunRequest {
        horizon: Some("1h".parse().unwrap()),
        ..RunRequest::new()
    };
    let failure = run_text(&text, &request, &dir.store()).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(
        failure.diagnostics[0].path.as_deref(),
        Some("experiment.horizon")
    );
}

/// Run ids are computed, so a result that belongs to another run is refused where it would
/// be returned as this one: on reuse and after a `--no-reuse` run.
#[test]
fn a_result_filed_under_another_run_is_not_returned() {
    let dir = TempDir::new();
    let store = dir.store();
    let first = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let second = run_text(&drawing_model(", seed: 2"), &RunRequest::new(), &store).unwrap();
    let second_id = second.result.runs[0].run_id.as_ref().unwrap();
    let file = store
        .root()
        .join("runs")
        .join(second_id.as_str())
        .join("result.json");
    std::fs::write(&file, &first.text).unwrap();
    let no_reuse = RunRequest {
        reuse: false,
        ..RunRequest::new()
    };
    for request in [RunRequest::new(), no_reuse] {
        let failure = run_text(&drawing_model(", seed: 2"), &request, &store).unwrap_err();
        assert_eq!(failure.class, FailureClass::RunFailed);
        assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");
    }
    // The first run's own result is still its own.
    let again = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    assert!(again.reused);
}

/// A trace whose header names another model, or another seed, is not that run's
/// trace, even when its records are the same, as they are for two models that differ
/// only in their name (ADR-0024 §1). Replay compares records, so only the header
/// tells these apart.
#[test]
fn replay_refuses_a_trace_whose_header_names_another_run() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let a = run_text(&drawing_model("a, seed: 1"), &RunRequest::new(), &store).unwrap();
    let b = run_text(&drawing_model("b, seed: 1"), &RunRequest::new(), &store).unwrap();
    let a_id = a.result.runs[0].run_id.clone().unwrap();
    let b_id = b.result.runs[0].run_id.clone().unwrap();
    assert_ne!(a_id, b_id);
    assert_ne!(
        a.result.provenance.model_hash,
        b.result.provenance.model_hash
    );
    let trace_of = |id: &taktsim_run::RunId| {
        store
            .root()
            .join("runs")
            .join(id.as_str())
            .join("trace.takt")
    };
    let a_trace = std::fs::read(trace_of(&a_id)).unwrap();
    let b_trace = std::fs::read(trace_of(&b_id)).unwrap();
    assert_ne!(a_trace, b_trace, "the headers name other model hashes");

    // Run A's trace filed as run B's.
    std::fs::write(trace_of(&b_id), &a_trace).unwrap();
    let failure = replay_stored(&registry, &store, &b_id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let line = &failure.diagnostics[0];
    assert_eq!(line.type_name, "run.stored_run_differs");
    assert!(line.message.contains("model hash"), "{}", line.message);
    assert_eq!(
        line.found.as_deref(),
        Some(a.result.provenance.model_hash.to_string().as_str())
    );
    assert_eq!(
        line.allowed.as_deref(),
        Some(b.result.provenance.model_hash.to_string().as_str())
    );

    // Run B's own records, under a header that states no seed: replay checks a seed
    // only where the header states one, and the records do not show it.
    let model = taktsim_model::Loader::new(registry.catalog())
        .load_str(&drawing_model("b, seed: 1"), taktsim_model::Format::Yaml)
        .unwrap();
    let mut engine = registry
        .build(model.document(), taktsim_core::EngineConfig::new(1))
        .unwrap();
    let mut header = taktsim_trace::Header::for_engine(
        &engine,
        taktsim_trace::ModelHash::from_bytes(*model.hash().as_bytes()),
        taktsim_trace::SystemHash::from_bytes(*model.system_hash().as_bytes()),
    );
    header.seed = None;
    let mut writer =
        taktsim_trace::TraceWriter::<_, taktsim_components::FlowMessage>::new(Vec::new(), &header)
            .unwrap();
    let horizon = model.document().experiment().horizon().as_nanos();
    let summary = engine
        .run(
            taktsim_core::RunLimits::until(taktsim_units::SimTime::from_nanos(horizon)),
            &mut writer,
        )
        .unwrap();
    writer.run_ended(&summary);
    let (unseeded, _) = writer.finish().unwrap();
    assert_ne!(unseeded, b_trace);
    std::fs::write(trace_of(&b_id), &unseeded).unwrap();
    let failure = replay_stored(&registry, &store, &b_id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let line = &failure.diagnostics[0];
    assert!(line.message.contains("seed"), "{}", line.message);
    assert_eq!(line.found.as_deref(), Some("no seed"));
    assert_eq!(line.allowed.as_deref(), Some("1"));

    // Its own trace replays.
    std::fs::write(trace_of(&b_id), &b_trace).unwrap();
    assert!(replay_stored(&registry, &store, &b_id).is_ok());
}

/// The result states the records and events of its trace, as it states its hash.
#[test]
fn replay_refuses_a_result_with_other_counts_than_its_trace() {
    let dir = TempDir::new();
    let store = dir.store();
    let registry = standard().unwrap();
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let file = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    let run = &response.result.runs[0];
    for (key, count) in [("events", run.events), ("records", run.records)] {
        let forged = response.text.replace(
            &format!("\"{key}\": {count},"),
            &format!("\"{key}\": {},", count + 1),
        );
        assert_ne!(forged, response.text, "{key}");
        std::fs::write(&file, forged).unwrap();
        let failure = replay_stored(&registry, &store, &id).unwrap_err();
        assert_eq!(failure.class, FailureClass::RunFailed, "{key}");
        assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");
    }
    std::fs::write(&file, &response.text).unwrap();
    assert!(replay_stored(&registry, &store, &id).is_ok());
}

/// A kept run's text is the stored file, and a partial run's is its only copy.
#[test]
fn the_text_of_a_response_is_the_document() {
    let dir = TempDir::new();
    let store = dir.store();
    let kept = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    assert_eq!(kept.text, kept.result.to_text());
    let id = kept.result.runs[0].run_id.as_ref().unwrap();
    let stored = std::fs::read_to_string(
        store
            .root()
            .join("runs")
            .join(id.as_str())
            .join("result.json"),
    )
    .unwrap();
    assert_eq!(kept.text, stored);
    let limited = RunRequest {
        seed: Some(2),
        max_events: Some(1),
        ..RunRequest::new()
    };
    let partial = run_text(&drawing_model(", seed: 1"), &limited, &store).unwrap();
    assert!(!partial.result.complete);
    assert_eq!(partial.text, partial.result.to_text());
}
