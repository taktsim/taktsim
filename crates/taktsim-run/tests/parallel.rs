//! Parallel replications with the same bytes (ADR-0025 §8, §9): the store is a function
//! of the counted runs, whatever the thread count, and the clock and a signal cut a
//! replication that is running.

mod support;

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use support::{TempDir, model, run_text};
use taktsim_run::{Budget, Ended, RunRequest, RunResponse, Spent, Store};

fn budget(runs: u32, seconds: u64) -> Budget {
    Budget {
        max_runs: NonZeroU32::new(runs),
        max_seconds: NonZeroU64::new(seconds),
    }
}

/// A short random line, with `extra` in the experiment.
fn line(extra: &str) -> String {
    model(
        ", seed: 1",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 50s}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    )
    .replace("horizon: 1h}", &format!("horizon: 1h{extra}}}"))
}

/// Every file below `root`, by its path relative to it, with its bytes.
#[allow(clippy::unwrap_used)] // Test helper.
fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn threads(n: usize) -> Option<NonZeroUsize> {
    NonZeroUsize::new(n)
}

/// Runs the same commands with `threads` on a fresh store; returns the documents and
/// the store tree.
#[allow(clippy::unwrap_used)] // Test helper.
fn commands(threads: Option<NonZeroUsize>) -> (Vec<String>, BTreeMap<PathBuf, Vec<u8>>) {
    let dir = TempDir::new();
    let store = dir.store();
    let seeds: Vec<u64> = (1..=12).collect();
    let list = RunRequest {
        seeds: &seeds,
        threads,
        ..RunRequest::new()
    };
    let precise = RunRequest {
        budget: budget(40, 600),
        threads,
        ..RunRequest::new()
    };
    // A list; a precision run that stops before its budget, so seeds past the stop may
    // have run; and an event limit inside a list.
    let limited = RunRequest {
        max_events: Some(400),
        ..list
    };
    let documents = [
        run_text(&line(""), &list, &store).unwrap(),
        run_text(
            &line(", precision: {\"throughput(sink)\": 10%}"),
            &precise,
            &store,
        )
        .unwrap(),
        run_text(&line("").replace("1h", "3h"), &limited, &store).unwrap(),
    ];
    assert_eq!(documents[1].ended, Ended::Complete);
    assert_eq!(documents[1].result.stop_reason, "precision_reached");
    assert!(
        documents[1].result.runs.len() < 40,
        "stopped before the budget"
    );
    assert_eq!(documents[2].ended, Ended::EventLimit);
    let texts = documents
        .iter()
        .map(|response| response.result.to_text())
        .collect();
    let root = store.root();
    let tmp = std::fs::read_dir(root.join("tmp")).unwrap().count();
    assert_eq!(tmp, 0, "nothing is left in tmp/");
    (texts, tree(root))
}

/// One thread or eight: the same documents, and the same store, file for file and byte
/// for byte. No run past a stop is stored.
#[test]
fn threads_1_and_8_store_the_same_bytes() {
    let (one, one_tree) = commands(threads(1));
    let (eight, eight_tree) = commands(threads(8));
    assert_eq!(one, eight);
    assert_eq!(
        one_tree.keys().collect::<Vec<_>>(),
        eight_tree.keys().collect::<Vec<_>>()
    );
    assert!(one_tree == eight_tree, "the stored bytes differ");
    // Only counted runs are stored: twelve, the precision run's, and the event limit's
    // seeds below the one that hit it.
    let runs = one_tree
        .keys()
        .filter(|path| path.ends_with("result.json") && path.starts_with("runs"))
        .count();
    assert!(runs >= 12, "{runs}");
    let (default, _) = commands(None);
    assert_eq!(default, one);
}

/// A replication that would run for minutes: `--max-seconds 1` cuts it while it runs.
/// Nothing finished, so the document is the lowest seed's partial one, and the exit is a
/// budget's.
#[test]
fn the_clock_cuts_a_running_replication() {
    let dir = TempDir::new();
    let yaml = line("").replace("horizon: 1h", "horizon: 100000h");
    let seeds = [1, 2];
    let request = RunRequest {
        seeds: &seeds,
        budget: budget(2, 1),
        threads: threads(2),
        ..RunRequest::new()
    };
    let started = Instant::now();
    let response = run_text(&yaml, &request, &dir.store()).unwrap();
    assert!(started.elapsed() < Duration::from_secs(60));
    assert_eq!(response.ended, Ended::BudgetExhausted);
    assert_eq!(response.result.stop_reason, "budget_exhausted");
    assert_eq!(response.result.runs.len(), 1);
    assert_eq!(response.result.runs[0].seed, Some(1));
    assert_eq!(
        response.result.runs[0].run_id, None,
        "a cut run is not kept"
    );
    assert_eq!(
        response.progress.and_then(|p| p.spent),
        Some(Spent::MaxSeconds)
    );
    assert!(!dir.store().root().join("experiments").exists());
    assert_eq!(runs_in(&dir.store()), 0);
}

fn runs_in(store: &Store) -> usize {
    std::fs::read_dir(store.root().join("runs")).map_or(0, Iterator::count)
}

/// The caller's flag, set while replications run, cuts them within the poll interval.
#[test]
fn a_signal_cuts_running_replications() {
    let dir = TempDir::new();
    let yaml = line("").replace("horizon: 1h", "horizon: 100000h");
    let seeds = [1, 2, 3];
    let cancel = AtomicBool::new(false);
    let request = RunRequest {
        seeds: &seeds,
        threads: threads(3),
        cancel: Some(&cancel),
        ..RunRequest::new()
    };
    let started = Instant::now();
    let response: RunResponse = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::Relaxed);
        });
        run_text(&yaml, &request, &dir.store()).unwrap()
    });
    assert!(started.elapsed() < Duration::from_secs(60));
    assert_eq!(response.ended, Ended::Cancelled);
    assert_eq!(response.result.stop_reason, "cancelled");
    assert_eq!(response.result.runs[0].seed, Some(1));
    assert_eq!(runs_in(&dir.store()), 0);
    let progress = response.progress.unwrap();
    assert_eq!((progress.counted, progress.planned), (0, 3));
}

/// The caller's flag set before the call: nothing runs to its end, however short the
/// replications, and the lowest seed's partial document is returned, as one run's is
/// (ADR-0025 §5, §9). Covers the check before each replication starts.
#[test]
fn a_signal_before_the_call_runs_nothing_to_its_end() {
    let dir = TempDir::new();
    let seeds = [1, 2, 3];
    let cancel = AtomicBool::new(true);
    let request = RunRequest {
        seeds: &seeds,
        threads: threads(3),
        cancel: Some(&cancel),
        ..RunRequest::new()
    };
    let response = run_text(&line(""), &request, &dir.store()).unwrap();
    assert_eq!(response.ended, Ended::Cancelled);
    assert_eq!(response.result.stop_reason, "cancelled");
    assert!(!response.result.complete);
    assert_eq!(response.result.experiment_id, None);
    assert_eq!(response.result.runs.len(), 1);
    assert_eq!(response.result.runs[0].seed, Some(1));
    assert_eq!(response.result.runs[0].run_id, None);
    assert_eq!(runs_in(&dir.store()), 0);
    assert!(!dir.store().root().join("experiments").exists());
    let progress = response.progress.unwrap();
    assert_eq!((progress.counted, progress.planned), (0, 3));
}

/// Signalled before the call with the lowest seed's run in the store: that run counts,
/// and nothing else starts.
#[test]
fn a_signal_before_the_call_counts_a_stored_lowest_seed() {
    let dir = TempDir::new();
    let first = run_text(&line(""), &RunRequest::new(), &dir.store()).unwrap();
    assert_eq!(runs_in(&dir.store()), 1);
    let seeds = [1, 2, 3];
    let cancel = AtomicBool::new(true);
    let request = RunRequest {
        seeds: &seeds,
        threads: threads(3),
        cancel: Some(&cancel),
        ..RunRequest::new()
    };
    let response = run_text(&line(""), &request, &dir.store()).unwrap();
    assert_eq!(response.ended, Ended::Cancelled);
    assert!(!response.result.complete);
    assert_eq!(response.result.runs.len(), 1);
    assert_eq!(response.result.runs[0].run_id, first.result.runs[0].run_id);
    assert!(response.result.experiment_id.is_some());
    assert_eq!(runs_in(&dir.store()), 1);
    assert!(!dir.store().root().join("experiments").exists());
    let progress = response.progress.unwrap();
    assert_eq!((progress.counted, progress.planned), (1, 3));
}

/// What ran first, alone, sets how many run at once; every one from the store takes no
/// thread.
#[test]
fn progress_states_the_count_and_the_budget() {
    let dir = TempDir::new();
    let yaml = line(", precision: {\"throughput(sink)\": 0.1%}");
    let request = RunRequest {
        budget: budget(12, 600),
        threads: threads(2),
        ..RunRequest::new()
    };
    let response = run_text(&yaml, &request, &dir.store()).unwrap();
    let progress = response.progress.unwrap();
    assert_eq!(progress.spent, Some(Spent::MaxRuns));
    assert_eq!((progress.counted, progress.planned), (12, 12));
    let at_once = progress.at_once.unwrap().get();
    assert!((1..=2).contains(&at_once), "{at_once}");
    let again = run_text(&yaml, &request, &dir.store()).unwrap();
    assert!(again.reused);
    assert_eq!(again.progress.unwrap().at_once, None, "nothing ran");
    // One seed is one run: no progress of an experiment.
    let one = run_text(&line(""), &RunRequest::new(), &dir.store()).unwrap();
    assert_eq!(one.progress, None);
}
