//! The run store (ADR-0024 §8): atomic, reused, compared, never overwritten.

mod support;

use std::collections::BTreeSet;
use std::path::Path;

use support::{TempDir, drawing_model, exact_model, run_text};
use taktsim_components::standard;
use taktsim_model::ModelHash;
use taktsim_run::{
    FailureClass, ModelSource, ResultDocument, RunRequest, Scalar, Store, Unreported,
    replay_stored, run,
};

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

/// A trace whose header names another model, another system or another seed, is
/// not that run's trace, even when its records are the same, as they are for two
/// models that differ only in their name (ADR-0024 §1). Replay compares records, so
/// only the header tells these apart.
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

    // Run B's own records, under a header with one field changed. The writer computes
    // the header's CRC, so only the check of that field can refuse it.
    let model = taktsim_model::Loader::new(registry.catalog())
        .load_str(&drawing_model("b, seed: 1"), taktsim_model::Format::Yaml)
        .unwrap();
    let with_header = |edit: &dyn Fn(&mut taktsim_trace::Header)| {
        let mut engine = registry
            .build(model.document(), taktsim_core::EngineConfig::new(1))
            .unwrap();
        let mut header = taktsim_trace::Header::for_engine(
            &engine,
            taktsim_trace::ModelHash::from_bytes(*model.hash().as_bytes()),
            taktsim_trace::SystemHash::from_bytes(*model.system_hash().as_bytes()),
        );
        edit(&mut header);
        let mut writer = taktsim_trace::TraceWriter::<_, taktsim_components::FlowMessage>::new(
            Vec::new(),
            &header,
        )
        .unwrap();
        let horizon = model.document().experiment().horizon().as_nanos();
        let summary = engine
            .run(
                taktsim_core::RunLimits::until(taktsim_units::SimTime::from_nanos(horizon)),
                &mut writer,
            )
            .unwrap();
        writer.run_ended(&summary);
        writer.finish().unwrap().0
    };
    assert_eq!(with_header(&|_| {}), b_trace, "the same run, rebuilt");

    // Another system hash, the model hash and the seed kept (ADR-0024 §1: both
    // hashes of the header are checked).
    let other_system = taktsim_trace::SystemHash::from_bytes([7; 32]);
    let resystemed = with_header(&|header| header.system_hash = other_system);
    assert_ne!(resystemed, b_trace);
    std::fs::write(trace_of(&b_id), &resystemed).unwrap();
    let failure = replay_stored(&registry, &store, &b_id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let line = &failure.diagnostics[0];
    assert_eq!(line.type_name, "run.stored_run_differs");
    assert!(line.message.contains("system hash"), "{}", line.message);
    assert_eq!(
        line.found.as_deref(),
        Some(other_system.to_string().as_str())
    );
    assert_eq!(
        line.allowed.as_deref(),
        Some(b.result.provenance.system_hash.to_string().as_str())
    );

    // No seed: replay checks a seed only where the header states one, and the
    // records do not show it.
    let unseeded = with_header(&|header| header.seed = None);
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

/// The stored result `text` of `response` with one field forged, by name.
#[allow(clippy::unwrap_used)] // Test helper.
fn forged_results(response: &taktsim_run::RunResponse, text: &str) -> Vec<(&'static str, String)> {
    let id = response.result.runs[0].run_id.as_ref().unwrap();
    let other_id = format!("taktsim-core-{}", "a".repeat(64));
    let experiment = response.result.experiment_id.clone().unwrap();
    let provenance = &response.result.provenance;
    let trace = response.result.runs[0].trace.as_ref().unwrap();
    let complete = "\"complete\": true";
    let last = text.rfind(complete).unwrap();
    assert_ne!(text.find(complete), Some(last), "the document and its run");
    vec![
        (
            "document's complete",
            text.replacen(complete, "\"complete\": false", 1),
        ),
        (
            "run's complete",
            format!(
                "{}\"complete\": false{}",
                &text[..last],
                &text[last + complete.len()..]
            ),
        ),
        (
            "experiment id",
            text.replace(experiment.as_str(), &format!("exp-{}", "0".repeat(64))),
        ),
        (
            "run id",
            text.replace(
                &format!("\"run_id\": \"{id}\""),
                &format!("\"run_id\": \"{other_id}\""),
            ),
        ),
        (
            "seeds",
            text.replace(
                "\"seeds\": [\n      1\n    ]",
                "\"seeds\": [\n      43\n    ]",
            ),
        ),
        // Each input of the run id the result states, alone.
        (
            "engine",
            text.replace(
                &format!("\"engine\": \"{}\"", provenance.engine),
                "\"engine\": \"other-core\"",
            ),
        ),
        (
            "engine version",
            text.replace(
                &format!("\"engine_version\": \"{}\"", provenance.engine_version),
                "\"engine_version\": \"9.9.9\"",
            ),
        ),
        (
            "record format",
            text.replace(
                &format!("\"record_format\": \"{}\"", trace.record_format),
                "\"record_format\": \"taktsim-core/1\"",
            ),
        ),
        (
            "message format",
            text.replace(
                &format!("\"message_format\": \"{}\"", trace.message_format),
                "\"message_format\": \"taktsim-components/9\"",
            ),
        ),
        // What the loaded model fixes: its hashes.
        (
            "system hash",
            text.replace(&provenance.system_hash.to_string(), &"0".repeat(64)),
        ),
        (
            "model hash",
            text.replace(&provenance.model_hash.to_string(), &"0".repeat(64)),
        ),
        // The trace filed with it, and no other.
        (
            "trace path",
            text.replace(&trace.path, "runs/other/trace.takt"),
        ),
        ("no trace", {
            let mut document = response.result.clone();
            document.runs[0].trace = None;
            document.to_text()
        }),
        // One run, which its experiment id is over.
        ("second run", {
            let mut document = response.result.clone();
            let mut extra = document.runs[0].clone();
            extra.run_id = None;
            extra.trace = None;
            document.runs.push(extra);
            document.to_text()
        }),
    ]
}

/// A result whose stated run id alone, or whose `provenance.seeds` alone, disagrees
/// is not the run it is filed under, though its other fields still compute to that
/// id: neither reuse nor replay accepts it (ADR-0024 §6). Nor one that says it is
/// partial, since a partial run is never stored (§8), or states an experiment id
/// other than the one over its run alone. Nor one where any one input of the run id
/// is forged, which then computes to another id; nor one that states hashes the
/// model does not have, names no trace or another run's, or holds a second run that
/// its experiment id is not over (§3).
#[test]
fn a_result_that_disagrees_with_itself_in_one_field_is_refused() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let response = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let file = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    let text = std::fs::read_to_string(&file).unwrap();
    let cases = forged_results(&response, &text);
    for (name, forged) in cases {
        assert_ne!(forged, text, "{name}");
        std::fs::write(&file, &forged).unwrap();
        let reused = run_text(&yaml, &RunRequest::new(), &store);
        let replayed = replay_stored(&registry, &store, &id);
        std::fs::write(&file, &text).unwrap();
        for (how, outcome) in [
            ("reuse", reused.map(|_| ())),
            ("replay", replayed.map(|_| ())),
        ] {
            let failure = outcome.expect_err(&format!("{how} accepted a forged {name}"));
            assert_eq!(failure.class, FailureClass::RunFailed, "{how} {name}");
            assert_eq!(
                failure.diagnostics[0].type_name, "run.stored_run_differs",
                "{how} {name}"
            );
        }
    }
    assert!(run_text(&yaml, &RunRequest::new(), &store).unwrap().reused);
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

/// A stored trace that cannot be read, cut to nothing or in half, is a file that
/// failed (exit code 7), as it is in the check run, not a replay that diverged
/// (exit code 2): `replay_stored`'s doc says `Io` for a stored file that cannot be read.
#[test]
fn replay_of_a_trace_that_cannot_be_read_is_an_io_failure() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let path = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("trace.takt");
    let trace = std::fs::read(&path).unwrap();
    // Cut to nothing, the header does not read; cut in half, a record does not.
    for cut in [0, trace.len() / 2] {
        std::fs::write(&path, &trace[..cut]).unwrap();
        let failure = replay_stored(&registry, &store, &id).unwrap_err();
        assert_eq!(failure.class, FailureClass::Io, "cut at {cut}");
        let line = &failure.diagnostics[0];
        assert_eq!(line.type_name, "store.read", "cut at {cut}");
        assert_eq!(line.path.as_deref(), Some("--store"));
        assert_eq!(line.file, Some(path.display().to_string()));
    }
    std::fs::write(&path, &trace).unwrap();
    assert!(replay_stored(&registry, &store, &id).is_ok());
}

/// A stored `model.json` that does not load is a stored file that cannot be read
/// (exit code 7), named as the file, not invalid input at a path inside a file the
/// caller never gave (exit code 1).
#[test]
fn replay_of_a_stored_model_that_does_not_load_is_an_io_failure() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let response = run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let path = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("model.json");
    let model = std::fs::read_to_string(&path).unwrap();
    for broken in ["{}", "{\"format\": \"1.0\""] {
        std::fs::write(&path, broken).unwrap();
        let failure = replay_stored(&registry, &store, &id).unwrap_err();
        assert_eq!(failure.class, FailureClass::Io, "{broken}");
        assert_eq!(failure.diagnostics.len(), 1, "{broken}");
        let line = &failure.diagnostics[0];
        assert_eq!(line.type_name, "store.read", "{broken}");
        assert_eq!(line.path.as_deref(), Some("--store"));
        assert_eq!(line.file, Some(path.display().to_string()));
        assert!(line.found.as_deref().is_some_and(|f| !f.is_empty()));
    }
    std::fs::write(&path, &model).unwrap();
    assert!(replay_stored(&registry, &store, &id).is_ok());
}

/// A `store.json` that exists but cannot be read, here a directory, which is one on
/// every target, is refused when the store is opened, before anything is written
/// into it: only a missing one means a store not made yet.
#[test]
fn a_store_json_that_cannot_be_read_is_refused_on_opening() {
    let dir = TempDir::new();
    let root = dir.path().join("store");
    let file = root.join("store.json");
    std::fs::create_dir_all(&file).unwrap();
    let failure = Store::open(&root).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    assert_eq!(failure.diagnostics[0].type_name, "store.read");
    assert_eq!(
        failure.diagnostics[0].file,
        Some(file.display().to_string())
    );
    assert_eq!(listing(&root), set(&["store.json"]));
}

/// A stored `result.json` that exists but cannot be read, here a directory, is a
/// store that failed (exit code 7), for replay and for a run with or without reuse:
/// only a missing one means the run is not in the store.
#[test]
fn a_stored_result_that_cannot_be_opened_is_an_io_failure() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let response = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let id = response.result.runs[0].run_id.clone().unwrap();
    let file = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    let no_reuse = RunRequest {
        reuse: false,
        ..RunRequest::new()
    };
    for (how, outcome) in [
        ("replay", replay_stored(&registry, &store, &id).map(|_| ())),
        (
            "reuse",
            run_text(&yaml, &RunRequest::new(), &store).map(|_| ()),
        ),
    ] {
        let failure = outcome.expect_err(how);
        assert_eq!(failure.class, FailureClass::Io, "{how}");
        assert_eq!(failure.diagnostics[0].type_name, "store.read", "{how}");
        assert_eq!(
            failure.diagnostics[0].file,
            Some(file.display().to_string()),
            "{how}"
        );
    }
    // Without reuse it runs, and cannot put the run in place: its directory is there
    // without a result that reads, and a stored run is never overwritten.
    let failure = run_text(&yaml, &no_reuse, &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::Io);
    assert_eq!(failure.diagnostics[0].type_name, "store.write");
    assert!(file.is_dir(), "nothing was written over it");
}

/// A `tmp/` that cannot be written is a store that failed (exit code 7), not a
/// directory to retry in forever: only a directory that is there already is skipped.
/// Where the mode binds; a privileged user writes through it, and the run goes
/// through.
#[cfg(unix)]
#[test]
fn a_tmp_that_cannot_be_written_is_an_io_failure() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    let store = dir.store();
    run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store).unwrap();
    let tmp = store.root().join("tmp");
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = tmp.join("probe");
    let mode_binds = std::fs::create_dir(&probe).is_err();
    let _ = std::fs::remove_dir(&probe);
    let outcome = run_text(&drawing_model(", seed: 2"), &RunRequest::new(), &store);
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    if mode_binds {
        let failure = outcome.unwrap_err();
        assert_eq!(failure.class, FailureClass::Io);
        assert_eq!(failure.diagnostics[0].type_name, "store.write");
    } else {
        assert!(outcome.unwrap().result.complete);
    }
}

/// A model and a trace from another run, filed together as this run's: the trace's
/// header names the swapped model, and the two models differ only in their name, so
/// their system hashes and records are the same. Only the result's model hash tells
/// the stored model is not this run's (ADR-0024 §1).
#[test]
fn replay_refuses_the_model_and_trace_of_another_run_filed_together() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let a = run_text(&drawing_model("a, seed: 1"), &RunRequest::new(), &store).unwrap();
    let b = run_text(&drawing_model("b, seed: 1"), &RunRequest::new(), &store).unwrap();
    assert_eq!(
        a.result.provenance.system_hash, b.result.provenance.system_hash,
        "the names are outside the system"
    );
    let a_id = a.result.runs[0].run_id.clone().unwrap();
    let b_id = b.result.runs[0].run_id.clone().unwrap();
    let run_dir = |id: &taktsim_run::RunId| store.root().join("runs").join(id.as_str());
    for name in ["model.json", "trace.takt"] {
        std::fs::copy(run_dir(&a_id).join(name), run_dir(&b_id).join(name)).unwrap();
    }
    let failure = replay_stored(&registry, &store, &b_id).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    assert_eq!(failure.diagnostics[0].type_name, "run.stored_run_differs");
    assert!(
        failure.diagnostics[0].message.contains("hashes"),
        "{}",
        failure.diagnostics[0].message
    );
    assert!(replay_stored(&registry, &store, &a_id).is_ok());
}

/// A change to a stored result.
type Edit<'a> = dyn Fn(&mut ResultDocument) + 'a;

/// `--no-reuse` is how a stored run is checked (ADR-0024 §8): the run it makes must
/// give the stored result, not only the stored stream. A stored result whose numbers
/// are not what the run gives, with its stream hash left alone, is exit 2 and is
/// never overwritten.
#[test]
fn no_reuse_compares_the_whole_stored_result() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let id = first.result.runs[0].run_id.clone().unwrap();
    let file = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    let no_reuse = RunRequest {
        reuse: false,
        ..RunRequest::new()
    };
    let delivered = first
        .result
        .metrics
        .iter()
        .position(|m| m.name == "delivered(sink)")
        .unwrap();
    let edits: [(&str, &Edit<'_>); 4] = [
        ("metrics", &|d| {
            d.metrics[delivered].values[0].value = Scalar::Integer(5);
        }),
        ("runs", &|d| d.runs[0].events = 1),
        ("runs", &|d| d.runs[0].end_time = "3h".parse().unwrap()),
        ("unreported", &|d| {
            d.unreported.push(Unreported {
                observable: "throughput(sink)".to_owned(),
                reason: "not_computed".to_owned(),
            });
        }),
    ];
    for (key, edit) in edits {
        let mut forged = first.result.clone();
        edit(&mut forged);
        std::fs::write(&file, forged.to_text()).unwrap();
        let failure = run_text(&yaml, &no_reuse, &store).unwrap_err();
        assert_eq!(failure.class, FailureClass::RunFailed, "{key}");
        let line = &failure.diagnostics[0];
        assert_eq!(line.type_name, "run.store_differs", "{key}");
        assert!(
            line.message.contains(&format!("`{key}`")),
            "{}",
            line.message
        );
        assert_eq!(line.path.as_deref(), Some("--no-reuse"));
        assert_ne!(line.found, line.allowed, "{key}");
        // Never overwritten.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), forged.to_text());
        assert_eq!(listing(&store.root().join("tmp")).len(), 0);
    }
    std::fs::write(&file, &first.text).unwrap();
    assert_eq!(run_text(&yaml, &no_reuse, &store).unwrap().text, first.text);
}

/// A `--no-reuse` run that matches the stored run returns the stored bytes, as reuse
/// does: a key this build does not read is kept, and the platform the run was stored
/// on is the only field that may differ (ADR-0024 §3, §8).
#[test]
fn no_reuse_returns_the_stored_bytes() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = drawing_model(", seed: 1");
    let first = run_text(&yaml, &RunRequest::new(), &store).unwrap();
    let id = first.result.runs[0].run_id.clone().unwrap();
    let file = store
        .root()
        .join("runs")
        .join(id.as_str())
        .join("result.json");
    let platform = format!("\"platform\": \"{}\"", first.result.provenance.platform);
    let other = if first.result.provenance.platform == "aarch64-macos" {
        "x86_64-windows"
    } else {
        "aarch64-macos"
    };
    let stored = first
        .text
        .replace(&platform, &format!("\"platform\": \"{other}\""))
        .replacen("{\n", "{\n  \"later\": 1,\n", 1);
    assert_ne!(stored, first.text);
    std::fs::write(&file, &stored).unwrap();
    let no_reuse = RunRequest {
        reuse: false,
        ..RunRequest::new()
    };
    let again = run_text(&yaml, &no_reuse, &store).unwrap();
    assert!(!again.reused);
    assert_eq!(again.text, stored);
    assert_eq!(again.result.provenance.platform, other);
}
