//! Reference experiments (ADR-0025 §11, §12): every file in `benchmarks/experiments/`
//! runs through `taktsim_run::run` with the flags of its `# run:` line, and its result
//! document matches its committed snapshot with only `provenance.platform` masked. The
//! `test` job runs this on every tier-1 target, so one snapshot proves the bytes equal.
//! A new file with no committed snapshot fails, so a case joins with a snapshot and no
//! code. A separate directory from `benchmarks/models/`, whose harness runs each file
//! without a budget.

mod support;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use regex::Regex;
use support::TempDir;
use taktsim_components::standard;
use taktsim_run::{AccuracyKind, Budget, ModelSource, RunRequest, SeedSpec, expand_seeds, run};

fn repo(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        |p, q| p.join(q),
    )
}

/// The YAML files of the directory, sorted.
#[allow(clippy::unwrap_used)] // Test helper.
fn cases() -> Vec<(String, PathBuf)> {
    let mut cases: Vec<(String, PathBuf)> = std::fs::read_dir(repo(&["benchmarks", "experiments"]))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .map(|p| (p.file_stem().unwrap().to_str().unwrap().to_owned(), p))
        .collect();
    cases.sort();
    cases
}

/// The flags of a case: its `# run:` line, of `--seed`, `--max-runs` and
/// `--max-seconds`.
#[allow(clippy::unwrap_used, clippy::panic)] // Test helper.
fn flags(text: &str) -> (Vec<SeedSpec>, Budget) {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("# run:"))
        .unwrap_or_else(|| panic!("a case states its flags on a `# run:` line"));
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut seeds = Vec::new();
    let mut budget = Budget::none();
    for pair in words.chunks(2) {
        match pair {
            ["--seed", value] => seeds.push(value.parse().unwrap()),
            ["--max-runs", value] => budget.max_runs = Some(value.parse().unwrap()),
            ["--max-seconds", value] => budget.max_seconds = Some(value.parse().unwrap()),
            other => panic!("not a flag of a reference experiment: {other:?}"),
        }
    }
    (seeds, budget)
}

/// The document with `provenance.platform` masked, the only field that differs across
/// targets.
fn masked(text: &str) -> String {
    let platform = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
    text.replace(&format!("\"{platform}\""), "\"<platform>\"")
}

#[test]
fn the_directory_holds_a_list_and_a_sequential_case() {
    let stems: Vec<String> = cases().into_iter().map(|(stem, _)| stem).collect();
    assert_eq!(stems, ["chain_list", "mm1_sequential"]);
}

#[test]
fn every_reference_experiment_gives_its_golden_result() {
    let registry = standard().unwrap();
    for (stem, path) in cases() {
        let text = std::fs::read_to_string(&path).unwrap();
        let (specs, budget) = flags(&text);
        let seeds = expand_seeds(&specs, budget).unwrap();
        let dir = TempDir::new();
        let request = RunRequest {
            seeds: &seeds,
            budget,
            ..RunRequest::new()
        };
        let response = run(&registry, ModelSource::Path(&path), &request, &dir.store()).unwrap();
        assert!(response.result.complete, "{stem}");
        assert!(response.result.experiment_id.is_some(), "{stem}");
        let document = response.result.to_text();
        insta::assert_snapshot!(format!("reference_experiment_{stem}"), masked(&document));

        // The same bytes on a second thread count, from a fresh store.
        let other = TempDir::new();
        let request = RunRequest {
            threads: NonZeroUsize::new(3),
            ..request
        };
        let again = run(
            &registry,
            ModelSource::Path(&path),
            &request,
            &other.store(),
        )
        .unwrap();
        assert_eq!(again.result.to_text(), document, "{stem}");
    }
}

/// The two kinds of case say what they are: fixed n, and a sequential stop.
#[test]
fn the_cases_state_how_they_stopped() {
    let registry = standard().unwrap();
    for (stem, path) in cases() {
        let text = std::fs::read_to_string(&path).unwrap();
        let (specs, budget) = flags(&text);
        let seeds = expand_seeds(&specs, budget).unwrap();
        let dir = TempDir::new();
        let request = RunRequest {
            seeds: &seeds,
            budget,
            ..RunRequest::new()
        };
        let result = run(&registry, ModelSource::Path(&path), &request, &dir.store())
            .unwrap()
            .result;
        let stopping: Vec<Option<String>> = result
            .metrics
            .iter()
            .flat_map(|m| &m.values)
            .filter_map(|v| match &v.accuracy[0].kind {
                AccuracyKind::Sampling(interval) => Some(interval.stopping.clone()),
                AccuracyKind::None { .. }
                | AccuracyKind::Censored { .. }
                | AccuracyKind::PartlyUndefined { .. }
                | AccuracyKind::Unknown { .. } => None,
            })
            .collect();
        assert!(!stopping.is_empty(), "{stem}");
        match stem.as_str() {
            "chain_list" => {
                assert_eq!(result.stop_reason, "seeds_finished");
                assert_eq!(result.runs.len(), 10);
                assert!(stopping.iter().all(Option::is_none));
            }
            "mm1_sequential" => {
                assert_eq!(result.stop_reason, "precision_reached");
                assert!((10..100).contains(&result.runs.len()));
                assert!(stopping.iter().all(|s| s.as_deref() == Some("sequential")));
                assert!(result.unreported.is_empty(), "{:?}", result.unreported);
            }
            other => unreachable!("a new case needs its expectations here: {other}"),
        }
    }
}

/// No golden result states a `sampling` entry of width zero (ADR-0025 §6): identical
/// replications state `no_variation_observed` instead. Walks every committed snapshot
/// of the run layer and the CLI.
#[test]
fn no_golden_result_has_a_zero_width_sampling() {
    let zero = Regex::new(r#""half_width":\s*-?0(\.0+)?(e[-+]?\d+)?\s*[,}\n]"#).unwrap();
    let mut documents = 0;
    for crate_dir in ["taktsim-run", "taktsim-cli"] {
        let dir = repo(&["crates", crate_dir, "tests", "snapshots"]);
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            if text.contains("\"half_width\"") {
                documents += 1;
            }
            assert!(
                !zero.is_match(&text),
                "{}: a sampling entry of width zero",
                path.display()
            );
        }
    }
    assert!(
        documents >= 3,
        "the walk finds the golden results: {documents}"
    );
}
