//! Coverage of the intervals (ADR-0025 §7, acceptance criterion A2): on the M/M/1
//! reference model, whose expected throughput is exactly 299/600 items/s, about 95 %
//! of fixed-n 95 % intervals cover the true value.
//!
//! The model, the seeds, the count of experiments and both windows are fixed in
//! ADR-0025 and never change. A count outside its window is a finding to report, not
//! a reason to change a seed.
//!
//! The experiments go through `prepare`, `run_traced` to memory, the fold
//! (`metrics_of_trace`) and `Accuracy::from_replications`, so ten thousand store
//! directories are not created; `the_store_path_gives_the_same_numbers` requires the
//! store path to agree.

mod support;

use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use support::TempDir;
use taktsim_components::{Flow, Registry, RunOptions, standard};
use taktsim_model::{Loader, Model};
use taktsim_run::{
    Accuracy, AccuracyKind, AccuracyPlan, Budget, ModelSource, Replicated, RunRequest,
    metrics_of_trace, run,
};
use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus};
use taktsim_units::Ratio;

/// Experiments in each test.
const EXPERIMENTS: u64 = 1_000;
/// Seeds per experiment of A2.
const FIXED_N: u64 = 10;
/// The most seeds per sequential experiment.
const BUDGET: u32 = 100;
/// The expected throughput of `mm1.yaml`, items/s.
const TRUE_THROUGHPUT: f64 = 299.0 / 600.0;
/// The row the tests read.
const ROW: &str = "throughput(sink)";

fn mm1_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/models/mm1.yaml")
}

#[allow(clippy::unwrap_used)] // Test helper.
fn mm1(registry: &Registry<Flow>) -> Model {
    Loader::new(registry.catalog())
        .load_file(&mm1_path())
        .unwrap()
}

/// The throughput of one replication with `seed`, through the trace and the fold.
#[allow(clippy::unwrap_used)] // Test helper.
fn throughput(registry: &Registry<Flow>, model: &Model, seed: u64) -> f64 {
    let prepared = registry
        .prepare(model, RunOptions::new().seed(seed))
        .unwrap();
    let (trace, _) = prepared.run_traced(Vec::new()).unwrap();
    let (metrics, _) = metrics_of_trace(registry, model, trace.as_slice()).unwrap();
    let metric = metrics.iter().find(|m| m.name == ROW).unwrap();
    metric.values[0].value.as_f64().unwrap()
}

/// The interval of a replicated row.
#[allow(clippy::panic)] // Test helper.
fn interval(row: &Replicated) -> (f64, f64, u32, Option<String>) {
    match &row.accuracy[0].kind {
        AccuracyKind::Sampling(interval) => (
            interval.mean,
            interval.half_width,
            interval.replications,
            interval.stopping.clone(),
        ),
        other @ (AccuracyKind::None { .. }
        | AccuracyKind::Censored { .. }
        | AccuracyKind::PartlyUndefined { .. }
        | AccuracyKind::Unknown { .. }) => panic!("no interval: {other:?}"),
    }
}

fn covers(row: &Replicated) -> bool {
    let (mean, half_width, _, _) = interval(row);
    (mean - TRUE_THROUGHPUT).abs() <= half_width
}

/// A fixed-n experiment: seeds `100·i` to `100·i + 9`.
#[allow(clippy::unwrap_used)] // Test helper.
fn fixed(registry: &Registry<Flow>, model: &Model, i: u64) -> Replicated {
    let values: Vec<Option<f64>> = (0..FIXED_N)
        .map(|j| Some(throughput(registry, model, 100 * i + j)))
        .collect();
    Accuracy::from_replications(&values, &AccuracyPlan::fixed())
        .unwrap()
        .unwrap()
}

fn target() -> Ratio {
    "3%".parse().unwrap_or_else(|_| unreachable!("a ratio"))
}

/// A sequential experiment from seed `100·i`: γ = 3 %, n₀ = 10, at most 100 seeds, as
/// the library's sequencer decides for one target row.
#[allow(clippy::unwrap_used)] // Test helper.
fn sequential(registry: &Registry<Flow>, model: &Model, i: u64) -> Replicated {
    let plan =
        ReplicationPlan::new(target(), ReplicationPlan::DEFAULT_MIN_REPLICATIONS, BUDGET).unwrap();
    let mut control = ReplicationControl::new(plan);
    let mut values = Vec::new();
    for j in 0..u64::from(BUDGET) {
        let value = throughput(registry, model, 100 * i + j);
        values.push(Some(value));
        if control.push(value).unwrap() != ReplicationStatus::Continue {
            break;
        }
    }
    Accuracy::from_replications(&values, &AccuracyPlan::sequential().with_target(target()))
        .unwrap()
        .unwrap()
}

/// Every experiment of a test, on as many threads as the machine has, in index order.
#[allow(clippy::unwrap_used)] // Test helper.
fn experiments(each: fn(&Registry<Flow>, &Model, u64) -> Replicated) -> Vec<Replicated> {
    let registry = standard().unwrap();
    let model = mm1(&registry);
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| {
        (0..EXPERIMENTS)
            .into_par_iter()
            .map(|i| each(&registry, &model, i))
            .collect()
    })
}

/// A2: 1,000 experiments of 10 seeds each; the count of 95 % intervals that cover
/// 299/600 is in [930, 970]. A correct implementation passes with probability about
/// 99.7 % (binomial, n = 1,000, p = 0.95).
#[test]
fn a2_fixed_n_coverage_on_mm1() {
    let rows = experiments(fixed);
    let covered = rows.iter().filter(|row| covers(row)).count();
    for row in &rows {
        let (_, _, n, stopping) = interval(row);
        assert_eq!((n, stopping), (10, None), "fixed n, no stopping rule");
    }
    eprintln!("A2: {covered} of {EXPERIMENTS} intervals cover 299/600");
    assert!((930..=970).contains(&covered), "A2 coverage: {covered}");
}

/// The sequential procedure on the same model and seeds: γ = 3 %, budget 100. No A2
/// claim: the window [900, 970] says that coverage must not collapse under the stopping
/// rule, and every interval says `stopping: sequential`.
#[test]
fn sequential_coverage_on_mm1() {
    let rows = experiments(sequential);
    let covered = rows.iter().filter(|row| covers(row)).count();
    let mut total = 0_u64;
    for row in &rows {
        let (_, _, n, stopping) = interval(row);
        assert_eq!(stopping.as_deref(), Some("sequential"));
        total += u64::from(n);
    }
    eprintln!(
        "sequential: {covered} of {EXPERIMENTS} intervals cover 299/600; mean n {:.2}",
        f64::from(u32::try_from(total).unwrap()) / 1_000.0
    );
    assert!(
        (900..=970).contains(&covered),
        "sequential coverage: {covered}"
    );
}

/// The store path, `taktsim_run::run` with a seed list or a precision, gives the
/// numbers of the in-memory path, so the coverage tests speak for what users run.
#[test]
fn the_store_path_gives_the_same_numbers() {
    let registry = standard().unwrap();
    let model = mm1(&registry);
    let text = std::fs::read_to_string(mm1_path()).unwrap();
    let dir = TempDir::new();
    let store = dir.store();
    let row_of = |result: &taktsim_run::ResultDocument| {
        let metric = result.metrics.iter().find(|m| m.name == ROW).unwrap();
        let value = &metric.values[0];
        Replicated {
            value: value.value.as_f64().unwrap(),
            accuracy: vec![value.accuracy[0].clone()],
        }
    };
    for i in [0, 7] {
        let seeds: Vec<u64> = (0..FIXED_N).map(|j| 100 * i + j).collect();
        let request = RunRequest {
            seeds: &seeds,
            ..RunRequest::new()
        };
        let stored = run(
            &registry,
            ModelSource::Text(&text, taktsim_model::Format::Yaml),
            &request,
            &store,
        )
        .unwrap();
        let memory = fixed(&registry, &model, i);
        assert_eq!(row_of(&stored.result).value, memory.value, "{i}");
        assert_eq!(interval(&row_of(&stored.result)), interval(&memory), "{i}");

        let precise = text.replace(
            "horizon: 600s}",
            "horizon: 600s, precision: {\"throughput(sink)\": 3%}}",
        );
        let first = [100 * i];
        let request = RunRequest {
            seeds: &first,
            budget: Budget {
                max_runs: NonZeroU32::new(BUDGET),
                max_seconds: NonZeroU64::new(3_600),
            },
            ..RunRequest::new()
        };
        let stored = run(
            &registry,
            ModelSource::Text(&precise, taktsim_model::Format::Yaml),
            &request,
            &store,
        )
        .unwrap();
        let memory = sequential(&registry, &model, i);
        let stored_row = row_of(&stored.result);
        assert_eq!(stored_row.value, memory.value, "{i}");
        assert_eq!(interval(&stored_row), interval(&memory), "{i}");
        assert_eq!(
            stored.result.runs.len(),
            interval(&memory).2 as usize,
            "{i}"
        );
    }
}
