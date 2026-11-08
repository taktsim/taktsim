//! Reference models (ADR-0024 §11): every file in `benchmarks/models/` runs through
//! `taktsim_run::run`, gives the same stream as the golden harness, matches its
//! committed snapshot, replays from the store and answers for its claim of drawing
//! nothing. This test has no list of its own: a new file with no committed snapshot
//! fails, so a model joins with a snapshot and no code.
//!
//! The snapshots also feed `TAKTSIM_TRACE_HASHES`, and so `ci/trace-hashes.sh` (A4).

mod support;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use support::TempDir;
use taktsim_components::{Flow, FlowMessage, MachineState, Registry, standard};
use taktsim_core::{ComponentId, EngineConfig};
use taktsim_model::{Format, Loader, Model};
use taktsim_run::{ModelSource, RunRequest, Store, replay_stored, run};
use taktsim_trace::golden::{self, Case, Golden};
use taktsim_trace::{ModelHash, Record, SystemHash, TraceReader};
use taktsim_units::SimTime;

fn repo(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        |p, q| p.join(q),
    )
}

/// The YAML files of the directory, sorted.
#[allow(clippy::unwrap_used)] // Test helper.
fn models() -> Vec<(String, PathBuf)> {
    let mut models: Vec<(String, PathBuf)> = std::fs::read_dir(repo(&["benchmarks", "models"]))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .map(|p| (p.file_stem().unwrap().to_str().unwrap().to_owned(), p))
        .collect();
    models.sort();
    models
}

#[allow(clippy::unwrap_used)] // Test helper.
fn load(registry: &Registry<Flow>, path: &Path) -> Model {
    Loader::new(registry.catalog()).load_file(path).unwrap()
}

/// The golden case of `model`, under this name.
fn case<'a>(name: &'a str, model: &Model) -> Case<'a> {
    Case {
        name,
        model_hash: ModelHash::from_bytes(*model.hash().as_bytes()),
        system_hash: SystemHash::from_bytes(*model.system_hash().as_bytes()),
        until: SimTime::from_nanos(model.document().experiment().horizon().as_nanos()),
    }
}

#[test]
fn the_directory_holds_the_reference_models() {
    let stems: Vec<String> = models().into_iter().map(|(stem, _)| stem).collect();
    assert!(stems.contains(&"chain".to_owned()), "{stems:?}");
    assert!(stems.contains(&"chain_failures".to_owned()), "{stems:?}");
    for stem in &stems {
        assert!(
            stem.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{stem}: a stem is a golden case name"
        );
    }
}

#[test]
fn every_reference_model_reproduces_its_golden_trace() {
    let registry = standard().unwrap();
    for (stem, path) in models() {
        let dir = TempDir::new();
        let store = dir.store();
        let response = run(
            &registry,
            ModelSource::Path(&path),
            &RunRequest::new(),
            &store,
        )
        .unwrap();
        let result = &response.result;
        assert!(result.complete, "{stem}");
        assert_eq!(result.stop_reason, "horizon", "{stem}");

        let model = load(&registry, &path);
        let seed = result.runs[0].seed;
        let name = format!("reference_{stem}");
        let case = case(&name, &model);
        let build = |seed| {
            let engine = registry
                .build(model.document(), EngineConfig::new(seed))
                .unwrap();
            Ok(engine)
        };
        let golden: Golden = match seed {
            Some(seed) => golden::record(&case, || build(seed)).unwrap(),
            None => golden::record_without_seed(&case, || build(0)).unwrap(),
        };
        let trace = result.runs[0].trace.as_ref().unwrap();
        assert_eq!(golden.summary().hash, trace.stream_hash, "{stem}");
        assert_eq!(golden.summary().records, result.runs[0].records, "{stem}");
        assert_eq!(golden.summary().events, result.runs[0].events, "{stem}");
        insta::assert_snapshot!(name.clone(), golden.snapshot());

        // The stored file is the golden run, byte for byte.
        let id = result.runs[0].run_id.as_ref().unwrap();
        let stored = store.root().join(&trace.path);
        let bytes = std::fs::read(&stored).unwrap();
        assert_eq!(
            taktsim_trace::TraceHash::sha256(&bytes),
            golden.file_hash(),
            "{stem}"
        );

        let replayed = replay_stored(&registry, &store, id).unwrap();
        assert_eq!(replayed.stream_hash, trace.stream_hash, "{stem}");
    }
}

/// Where a model claims to draw nothing, two seeds give one trace. Over the
/// reference models and the fixture, whose `seed:` must make no difference.
#[test]
fn no_draw_claim_is_true() {
    let registry = standard().unwrap();
    let fixture = fixture();
    let mut claims = 0;
    for path in models().into_iter().map(|(_, p)| p).chain([fixture]) {
        let model = load(&registry, &path);
        if registry.draws_random(model.document()) {
            continue;
        }
        claims += 1;
        let hash = |seed| {
            let options = taktsim_components::RunOptions::new().seed(seed);
            let (_, report) = registry
                .prepare(&model, options)
                .unwrap()
                .run_traced(Vec::new())
                .unwrap();
            report.trace.unwrap().hash
        };
        assert_eq!(hash(1), hash(987_654_321), "{}", path.display());
    }
    assert!(claims > 0, "the fixture claims it");
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("no_draw_chain.yaml")
}

/// A run with no seed through the library and the golden harness: the header says
/// none, the seed in the file is unused, and the check run passes.
#[test]
fn the_fixture_runs_with_no_seed() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let path = fixture();
    let response = run(
        &registry,
        ModelSource::Path(&path),
        &RunRequest::new(),
        &store,
    )
    .unwrap();
    let result = &response.result;
    assert_eq!(result.provenance.seeds.len(), 0);
    assert_eq!(result.runs[0].seed, None);
    let warning = &response.warnings[0];
    assert_eq!(warning.type_name, "run.seed_unused");
    assert_eq!(warning.path.as_deref(), Some("model.seed"));

    let model = load(&registry, &path);
    let case = case("no_draw_chain", &model);
    let golden = golden::record_without_seed(&case, || {
        Ok(registry
            .build(model.document(), EngineConfig::new(0))
            .unwrap())
    })
    .unwrap();
    assert_eq!(
        golden.summary().hash,
        result.runs[0].trace.as_ref().unwrap().stream_hash
    );
    insta::assert_snapshot!("no_draw_chain", golden.snapshot());
    assert_eq!(golden.header().seed, None);

    let id = result.runs[0].run_id.as_ref().unwrap();
    assert!(replay_stored(&registry, &store, id).unwrap().events > 0);
}

/// `chain_failures` shows what it is for: within its horizon a machine fails, the
/// buffer reaches its capacity, and the machine before it is blocked.
#[test]
fn the_failures_model_fills_its_buffer_and_fails() {
    let registry = standard().unwrap();
    let dir = TempDir::new();
    let store = Store::open(&dir.path().join("store")).unwrap();
    let path = repo(&["benchmarks", "models", "chain_failures.yaml"]);
    let response = run(
        &registry,
        ModelSource::Path(&path),
        &RunRequest::new(),
        &store,
    )
    .unwrap();
    let trace = response.result.runs[0].trace.as_ref().unwrap();
    let file = File::open(store.root().join(&trace.path)).unwrap();
    let mut reader = TraceReader::<_, FlowMessage>::new(BufReader::new(file)).unwrap();
    let names = reader.header().components.clone();
    let id = |name: &str| ComponentId::from_index(names.iter().position(|n| n == name).unwrap());
    let (cut, buffer, weld) = (id("cut"), id("buf1"), id("weld"));

    let mut level: i64 = 0;
    let mut highest = 0;
    let mut states: BTreeSet<(usize, String)> = BTreeSet::new();
    while let Some(record) = reader.next_record().unwrap() {
        match record {
            Record::Event {
                target, from, item, ..
            } if item.is_some() => {
                if target == buffer {
                    level += 1;
                }
                if from == buffer {
                    level -= 1;
                }
                highest = highest.max(level);
            }
            Record::Note { by, message, .. } => {
                if let FlowMessage::MachineState(note) = message {
                    states.insert((by.index(), format!("{:?}", note.state)));
                }
            }
            Record::Event { .. } | Record::ItemCreated { .. } | Record::ItemDisposed { .. } => {}
        }
    }
    assert_eq!(highest, 3, "the buffer reaches its capacity");
    let down = format!("{:?}", MachineState::Down);
    let blocked = format!("{:?}", MachineState::Blocked);
    assert!(states.contains(&(weld.index(), down.clone())), "{states:?}");
    assert!(states.contains(&(cut.index(), down)), "{states:?}");
    assert!(states.contains(&(cut.index(), blocked)), "{states:?}");
}

/// A model written only for a test does not reach the directory: it is what CI
/// compares across targets.
#[test]
fn a_reference_model_is_valid_with_its_own_seed_and_horizon() {
    let registry = standard().unwrap();
    for (stem, path) in models() {
        let model = load(&registry, &path);
        assert!(model.document().model().seed().is_some(), "{stem}");
        let text = std::fs::read_to_string(&path).unwrap();
        let again = Loader::new(registry.catalog())
            .load_str(&text, Format::Yaml)
            .unwrap();
        assert_eq!(again.hash(), model.hash(), "{stem}");
    }
}
