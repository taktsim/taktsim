//! Independent tests of M8's experiments (ADR-0025 §4 to §10), written from the ADR and
//! the public documentation only. An experiment's numbers are checked against the
//! single runs of the same seeds and against the t interval worked out here, never
//! against a run of the experiment code itself.

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use taktsim_model::Format;
use taktsim_run::{
    Accuracy, AccuracyKind, Budget, Ended, Failure, FailureClass, MetricValue, ModelSource,
    ResultDocument, RunRequest, RunResponse, SeedSpec, Spent, Store, expand_seeds, run, validate,
};
use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "hand-derived-experiments-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[allow(clippy::expect_used)] // Test helper.
fn store(name: &str) -> Store {
    Store::open(&dir(name)).expect("a store")
}

#[allow(clippy::expect_used)] // Test helper.
fn try_run(model: &str, request: &RunRequest<'_>, store: &Store) -> Result<RunResponse, Failure> {
    let registry = taktsim_components::standard().expect("the standard registry");
    run(
        &registry,
        ModelSource::Text(model, Format::Yaml),
        request,
        store,
    )
}

#[allow(clippy::panic)] // Test helper.
fn run_ok(model: &str, request: &RunRequest<'_>, store: &Store) -> RunResponse {
    try_run(model, request, store).unwrap_or_else(|failure| panic!("{failure:?}"))
}

/// Whether every value is the same number, bit for bit (ADR-0025 §6: "all equal"
/// compares the values).
fn all_equal(xs: &[f64]) -> bool {
    xs.iter().all(|x| x.to_bits() == xs[0].to_bits())
}

/// The `censored` entry's units inside, if there is one.
fn censored(accuracy: &[Accuracy]) -> Option<f64> {
    accuracy.iter().find_map(|a| match &a.kind {
        AccuracyKind::Censored { units_inside, .. } => Some(*units_inside),
        AccuracyKind::Sampling(_)
        | AccuracyKind::None { .. }
        | AccuracyKind::PartlyUndefined { .. }
        | AccuracyKind::Unknown { .. } => None,
    })
}

/// The `partly_undefined` entry's counts, if there is one.
fn partly_undefined(accuracy: &[Accuracy]) -> Option<(u32, u32)> {
    accuracy.iter().find_map(|a| match &a.kind {
        AccuracyKind::PartlyUndefined { undefined, of, .. } => Some((*undefined, *of)),
        AccuracyKind::Sampling(_)
        | AccuracyKind::None { .. }
        | AccuracyKind::Censored { .. }
        | AccuracyKind::Unknown { .. } => None,
    })
}

fn seeds(request_seeds: &[u64]) -> RunRequest<'_> {
    let mut request = RunRequest::new();
    request.seeds = request_seeds;
    request
}

fn budget(max_runs: u32, max_seconds: u64) -> Budget {
    Budget {
        max_runs: NonZeroU32::new(max_runs),
        max_seconds: NonZeroU64::new(max_seconds),
    }
}

/// A row's key: metric name and labels.
type Key = (String, BTreeMap<String, String>);

fn rows(doc: &ResultDocument) -> Vec<(Key, &MetricValue)> {
    doc.metrics
        .iter()
        .flat_map(|m| {
            m.values
                .iter()
                .map(move |v| ((m.name.clone(), v.labels.clone().unwrap_or_default()), v))
        })
        .collect()
}

fn keys(doc: &ResultDocument) -> Vec<Key> {
    rows(doc).into_iter().map(|(k, _)| k).collect()
}

fn row<'a>(
    doc: &'a ResultDocument,
    name: &str,
    labels: &[(&str, &str)],
) -> Option<&'a MetricValue> {
    let want: BTreeMap<String, String> = labels
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    rows(doc)
        .into_iter()
        .find(|((n, l), _)| n == name && *l == want)
        .map(|(_, v)| v)
}

fn rel_close(found: f64, expected: f64, tolerance: f64) -> bool {
    (found - expected).abs() <= tolerance * expected.abs().max(1e-300)
}

/// No document anywhere states a `sampling` entry of width zero (ADR-0025 §6).
fn assert_no_zero_width(doc: &ResultDocument) {
    for (key, value) in rows(doc) {
        for accuracy in &value.accuracy {
            if let AccuracyKind::Sampling(interval) = &accuracy.kind {
                assert!(interval.half_width > 0.0, "zero-width sampling on {key:?}");
            }
        }
    }
}

/// A line that draws: exponential arrivals and cycle, a large buffer.
const RANDOM_LINE: &str = "format: \"1.0\"\nmodel: {name: random-line}\n\
    experiment: {type: terminating, horizon: 2h}\n\
    components:\n\
    - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
    - {id: queue, type: Buffer, capacity: 1000 parts}\n\
    - {id: server, type: Machine, cycle_time: {dist: exponential, mean: 45s}}\n\
    - {id: sink, type: Sink}\n\
    flow: [[source, queue, server, sink]]\n";

/// t(0.975, 4), from any table of Student's t: 2.7764451051977987.
const T_975_DF4: f64 = 2.776_445_105_197_799;

/// A fixed list of five seeds: each row's value and interval follow from the five
/// single runs of the same seeds. For a row defined in all five, with values x₁..x₅:
/// mean x̄ = Σx/5, s² = Σ(x − x̄)²/4, half-width = t(0.975, 4) · s / √5 (ADR-0014). All
/// five equal: no interval, `no_variation_observed` (ADR-0025 §6). The runs of the
/// experiment are the same runs as the singles, so their run ids agree.
#[test]
fn list_matches_its_single_runs() {
    let list = [1, 2, 3, 4, 5];
    let singles_store = store("singles");
    let singles: Vec<ResultDocument> = list
        .iter()
        .map(|s| run_ok(RANDOM_LINE, &seeds(&[*s]), &singles_store).result)
        .collect();
    let response = run_ok(RANDOM_LINE, &seeds(&list), &store("list"));
    let doc = &response.result;

    assert_eq!(response.ended, Ended::Complete);
    assert!(doc.complete);
    assert_eq!(doc.stop_reason, "seeds_finished");
    assert_eq!(doc.provenance.seeds, list);
    assert!(doc.experiment_id.is_some());
    assert_eq!(doc.runs.len(), 5);
    for (entry, single) in doc.runs.iter().zip(&singles) {
        assert_eq!(entry.run_id, single.runs[0].run_id);
        assert_eq!(entry.seed, single.runs[0].seed);
    }
    let progress = response
        .progress
        .expect("an experiment states its progress");
    assert_eq!(
        (progress.counted, progress.planned, progress.spent),
        (5, 5, None)
    );

    let mut checked = 0;
    for (key, value) in rows(doc) {
        let labels: Vec<(&str, &str)> = key
            .1
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let xs: Vec<f64> = singles
            .iter()
            .map(|s| {
                row(s, &key.0, &labels)
                    .unwrap_or_else(|| panic!("{key:?} missing in a single run"))
                    .value
                    .as_f64()
                    .unwrap()
            })
            .collect();
        let mean = xs.iter().sum::<f64>() / 5.0;
        let found = value.value.as_f64().unwrap();
        if all_equal(&xs) {
            assert_eq!(found, xs[0], "{key:?}");
            assert!(
                matches!(&value.accuracy[0].kind, AccuracyKind::None { reason, .. } if reason == "no_variation_observed"),
                "{key:?}: {:?}",
                value.accuracy
            );
            continue;
        }
        let s = (xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / 4.0).sqrt();
        let half_width = T_975_DF4 * s / 5f64.sqrt();
        assert!(
            rel_close(found, mean, 1e-12),
            "{key:?}: value {found}, mean {mean}"
        );
        let AccuracyKind::Sampling(interval) = &value.accuracy[0].kind else {
            panic!("{key:?}: {:?}", value.accuracy)
        };
        assert!(rel_close(interval.mean, mean, 1e-12), "{key:?}");
        assert!(
            rel_close(interval.std_dev, s, 1e-9),
            "{key:?}: sd {} vs {s}",
            interval.std_dev
        );
        assert!(
            rel_close(interval.half_width, half_width, 1e-9),
            "{key:?}: half-width {} vs {half_width}",
            interval.half_width
        );
        assert_eq!(interval.replications, 5);
        assert_eq!(interval.level, 0.95);
        assert_eq!(interval.stopping, None, "a fixed list is not sequential");
        assert_eq!(value.accuracy[0].target, None);
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} rows varied");
    assert_no_zero_width(doc);
}

/// A list is sorted before anything uses it, so its order changes no byte (ADR-0025 §4).
#[test]
fn seed_order_changes_no_byte() {
    let sorted = run_ok(RANDOM_LINE, &seeds(&[1, 2, 3, 4, 5]), &store("sorted")).result;
    let shuffled = run_ok(RANDOM_LINE, &seeds(&[5, 3, 1, 4, 2]), &store("shuffled")).result;
    assert_eq!(shuffled.provenance.seeds, [1, 2, 3, 4, 5]);
    assert_eq!(sorted.to_text(), shuffled.to_text());
}

#[allow(clippy::unwrap_used)] // Test helper.
fn files_below(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if relative.starts_with("tmp") {
                continue;
            }
            if path.is_dir() {
                todo.push(path);
            } else {
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// The thread count reaches no byte of the document or the store (ADR-0025 §8).
#[test]
fn one_and_four_threads_store_the_same_bytes() {
    let list = [11, 12, 13, 14, 15, 16, 17, 18];
    let one_store = store("threads-1");
    let four_store = store("threads-4");
    let mut one = seeds(&list);
    one.threads = NonZeroUsize::new(1);
    let mut four = seeds(&list);
    four.threads = NonZeroUsize::new(4);
    let a = run_ok(RANDOM_LINE, &one, &one_store).result;
    let b = run_ok(RANDOM_LINE, &four, &four_store).result;
    assert_eq!(a.to_text(), b.to_text());
    let (fa, fb) = (
        files_below(one_store.root()),
        files_below(four_store.root()),
    );
    assert_eq!(fa.keys().collect::<Vec<_>>(), fb.keys().collect::<Vec<_>>());
    assert!(fa == fb, "the two stores differ in some file's bytes");
}

/// Running the same command again gives the same bytes, from the store.
#[test]
fn rerun_gives_the_same_bytes() {
    let s = store("rerun");
    let first = run_ok(RANDOM_LINE, &seeds(&[21, 22, 23]), &s).result;
    let second = run_ok(RANDOM_LINE, &seeds(&[21, 22, 23]), &s).result;
    assert_eq!(first.to_text(), second.to_text());
}

/// Arrivals 2 h apart on average over a 1 h horizon: P(no arrival) = e^(−1/2) ≈ 0.61, so
/// some seeds deliver nothing. Each row of the list follows from the single runs
/// (ADR-0025 §5): a count or rate is a true 0 in an empty run and is averaged with the
/// others; the lead time is undefined there, so its mean is over the runs that define
/// it, with a `partly_undefined` entry stating how many did not.
const SPARSE: &str = "format: \"1.0\"\nmodel: {name: sparse}\n\
    experiment: {type: terminating, horizon: 1h}\n\
    observe: [\"lead_time(sink)\"]\n\
    components:\n\
    - {id: source, type: Source, interarrival: {dist: exponential, mean: 2h}}\n\
    - {id: cut, type: Machine, cycle_time: 1s}\n\
    - {id: sink, type: Sink}\n\
    flow: [[source, cut, sink]]\n";

#[test]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // At most 20 runs.
fn true_zero_is_averaged_and_partly_undefined_states_the_count() {
    let list: Vec<u64> = (1..=20).collect();
    let singles_store = store("sparse-singles");
    let singles: Vec<ResultDocument> = list
        .iter()
        .map(|s| run_ok(SPARSE, &seeds(&[*s]), &singles_store).result)
        .collect();
    let doc = run_ok(SPARSE, &seeds(&list), &store("sparse-list")).result;

    let throughput: Vec<f64> = singles
        .iter()
        .map(|s| {
            row(s, "throughput(sink)", &[])
                .expect("throughput always has a value")
                .value
                .as_f64()
                .unwrap()
        })
        .collect();
    let mean = throughput.iter().sum::<f64>() / 20.0;
    let found = row(&doc, "throughput(sink)", &[])
        .unwrap()
        .value
        .as_f64()
        .unwrap();
    assert!(
        rel_close(found, mean, 1e-12),
        "throughput {found} vs mean of 20 with zeros {mean}"
    );

    let lead: Vec<Option<f64>> = singles
        .iter()
        .map(|s| {
            row(s, "lead_time(sink)", &[("statistic", "mean")]).map(|v| v.value.as_f64().unwrap())
        })
        .collect();
    let defined: Vec<f64> = lead.iter().flatten().copied().collect();
    let undefined = 20 - defined.len();
    eprintln!("sparse: {} of 20 seeds deliver", defined.len());
    assert!(
        undefined > 0 && defined.len() >= 2,
        "the seeds give no mixed case: {lead:?}"
    );
    let value = row(&doc, "lead_time(sink)", &[("statistic", "mean")]).expect("the lead time row");
    let lead_mean = defined.iter().sum::<f64>() / defined.len() as f64;
    assert!(rel_close(value.value.as_f64().unwrap(), lead_mean, 1e-12));
    match &value.accuracy[0].kind {
        AccuracyKind::Sampling(interval) => {
            assert_eq!(interval.replications as usize, defined.len());
        }
        AccuracyKind::None { reason, .. } => {
            assert!(all_equal(&defined), "{reason}");
        }
        other @ (AccuracyKind::Censored { .. }
        | AccuracyKind::PartlyUndefined { .. }
        | AccuracyKind::Unknown { .. }) => panic!("{other:?}"),
    }
    let partly = partly_undefined(&value.accuracy);
    assert_eq!(partly, Some((undefined as u32, 20)), "{:?}", value.accuracy);
    assert!(doc.unreported.is_empty(), "{:?}", doc.unreported);
    assert_no_zero_width(&doc);

    // The row set comes from the model, not from the lowest seed (ADR-0025 §5): a list
    // whose lowest seed is empty and one whose lowest seed delivers have the same rows.
    let empty: Vec<u64> = list
        .iter()
        .zip(&lead)
        .filter(|(_, l)| l.is_none())
        .map(|(s, _)| *s)
        .collect();
    let full: Vec<u64> = list
        .iter()
        .zip(&lead)
        .filter(|(_, l)| l.is_some())
        .map(|(s, _)| *s)
        .collect();
    let low_empty = [
        empty[0],
        *full
            .iter()
            .find(|s| **s > empty[0])
            .expect("a later delivering seed"),
    ];
    let low_full = [
        full[0],
        *empty
            .iter()
            .find(|s| **s > full[0])
            .expect("a later empty seed"),
    ];
    let a = run_ok(SPARSE, &seeds(&low_empty), &store("sparse-low-empty")).result;
    let b = run_ok(SPARSE, &seeds(&low_full), &store("sparse-low-full")).result;
    assert_eq!(keys(&a), keys(&b));
}

/// One seed of several defines the lead time: its value, `one_defined_value`, and a
/// `partly_undefined` entry (ADR-0025 §5). The seeds are found from single runs.
#[test]
fn one_defined_value_of_several() {
    let singles_store = store("one-defined-singles");
    let mut empty = Vec::new();
    let mut full = Vec::new();
    for s in 1..=20u64 {
        let doc = run_ok(SPARSE, &seeds(&[s]), &singles_store).result;
        match row(&doc, "lead_time(sink)", &[("statistic", "mean")]) {
            Some(v) => full.push((s, v.value.as_f64().unwrap())),
            None => empty.push(s),
        }
    }
    let list = [empty[0], empty[1], full[0].0];
    let doc = run_ok(SPARSE, &seeds(&list), &store("one-defined")).result;
    let value = row(&doc, "lead_time(sink)", &[("statistic", "mean")]).unwrap();
    assert_eq!(value.value.as_f64().unwrap(), full[0].1);
    assert!(
        matches!(&value.accuracy[0].kind, AccuracyKind::None { reason, .. } if reason == "one_defined_value"),
        "{:?}",
        value.accuracy
    );
    assert!(value.accuracy.iter().any(|a| matches!(
        a.kind,
        AccuracyKind::PartlyUndefined {
            undefined: 2,
            of: 3,
            ..
        }
    )));

    // No seed defines it: the rows are left out and the declared observable is
    // `unreported`, `undefined`.
    let none = run_ok(SPARSE, &seeds(&empty[..3]), &store("none-defined")).result;
    assert!(row(&none, "lead_time(sink)", &[("statistic", "mean")]).is_none());
    assert!(
        none.unreported
            .iter()
            .any(|u| u.observable == "lead_time(sink)" && u.reason == "undefined"),
        "{:?}",
        none.unreported
    );
}

/// An on-demand Source in front of a random machine: the machine is never idle and the
/// Source never holds a part, in every replication, so those rows do not vary. They state
/// no interval, `no_variation_observed`, and the value itself (ADR-0025 §6). Throughput
/// varies and has an interval.
#[test]
fn identical_replications_state_no_interval() {
    let model = "format: \"1.0\"\nmodel: {name: saturated}\n\
        experiment: {type: terminating, horizon: 1h}\n\
        components:\n\
        - {id: source, type: Source}\n\
        - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 1min}}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, cut, sink]]\n";
    let doc = run_ok(model, &seeds(&[1, 2, 3, 4]), &store("saturated")).result;
    for (name, labels, expected) in [
        ("held(source)", vec![], 0.0),
        (
            "state_shares(cut)",
            vec![("resource", "cut"), ("state", "idle")],
            0.0,
        ),
        (
            "state_shares(cut)",
            vec![("resource", "cut"), ("state", "busy")],
            1.0,
        ),
        ("utilization(cut)", vec![], 1.0),
        ("wip(cut)", vec![], 1.0),
    ] {
        let value = row(&doc, name, &labels).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(value.value.as_f64(), Some(expected), "{name} {labels:?}");
        assert!(
            matches!(&value.accuracy[0].kind, AccuracyKind::None { reason, .. } if reason == "no_variation_observed"),
            "{name}: {:?}",
            value.accuracy
        );
    }
    let throughput = row(&doc, "throughput(sink)", &[]).unwrap();
    assert!(
        matches!(&throughput.accuracy[0].kind, AccuracyKind::Sampling(i) if i.half_width > 0.0)
    );
    assert_no_zero_width(&doc);
}

/// A model that draws nothing is one run whatever seeds are asked for, with no seeds
/// in its provenance, a warning for the unused flag, and no interval anywhere
/// (ADR-0025 §4, §5).
#[test]
fn no_draw_model_never_gets_an_interval() {
    let model = "format: \"1.0\"\nmodel: {name: fixed}\n\
        experiment: {type: terminating, horizon: 1h}\n\
        components:\n\
        - {id: source, type: Source, interarrival: 50s}\n\
        - {id: cut, type: Machine, cycle_time: 42s}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, cut, sink]]\n";
    let response = run_ok(model, &seeds(&[1, 2, 3, 4, 5]), &store("no-draw"));
    let doc = &response.result;
    assert_eq!(response.ended, Ended::Complete);
    assert_eq!(doc.runs.len(), 1);
    assert_eq!(doc.provenance.seeds, [] as [u64; 0]);
    assert_eq!(doc.runs[0].seed, None);
    assert_ne!(response.warnings, [] as [taktsim_run::Diagnostic; 0]);
    for (key, value) in rows(doc) {
        for accuracy in &value.accuracy {
            assert!(
                !matches!(accuracy.kind, AccuracyKind::Sampling(_)),
                "{key:?}"
            );
        }
        assert!(
            matches!(&value.accuracy[0].kind, AccuracyKind::None { reason, .. } if reason == "no_random_draws"),
            "{key:?}"
        );
    }
}

/// An M/M/1 line with a short horizon and a throughput target.
fn precision_model(target: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: mm1-short, seed: 1}}\n\
         experiment: {{type: terminating, horizon: 300s, precision: {{\"throughput(sink)\": {target}}}}}\n\
         components:\n\
         - {{id: source, type: Source, interarrival: {{dist: exponential, mean: 2s}}}}\n\
         - {{id: queue, type: Buffer, capacity: 1000 parts}}\n\
         - {{id: server, type: Machine, cycle_time: {{dist: exponential, mean: 1s}}}}\n\
         - {{id: sink, type: Sink}}\n\
         flow: [[source, queue, server, sink]]\n"
    )
}

/// The same model without a target: the same runs, so the same values per seed.
fn no_target_model() -> String {
    "format: \"1.0\"\nmodel: {name: mm1-short, seed: 1}\n\
     experiment: {type: terminating, horizon: 300s}\n\
     components:\n\
     - {id: source, type: Source, interarrival: {dist: exponential, mean: 2s}}\n\
     - {id: queue, type: Buffer, capacity: 1000 parts}\n\
     - {id: server, type: Machine, cycle_time: {dist: exponential, mean: 1s}}\n\
     - {id: sink, type: Sink}\n\
     flow: [[source, queue, server, sink]]\n"
        .to_string()
}

#[allow(clippy::unwrap_used)] // Test helper.
fn throughputs(first: u64, count: u64, name: &str) -> Vec<f64> {
    let s = store(name);
    let model = no_target_model();
    (first..first + count)
        .map(|seed| {
            let doc = run_ok(&model, &seeds(&[seed]), &s).result;
            row(&doc, "throughput(sink)", &[])
                .unwrap()
                .value
                .as_f64()
                .unwrap()
        })
        .collect()
}

fn diagnostic_types(failure: &Failure) -> Vec<&str> {
    failure
        .diagnostics
        .iter()
        .map(|d| d.type_name.as_str())
        .collect()
}

/// The budget table of ADR-0025 §4, each refusal before anything runs.
#[test]
fn precision_needs_both_budgets() {
    let model = precision_model("5%");
    let s = store("refusals");
    let failure = try_run(&model, &RunRequest::new(), &s).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    // One line per missing flag.
    assert_eq!(
        diagnostic_types(&failure),
        ["run.no_budget", "run.no_budget"]
    );

    let mut only_runs = RunRequest::new();
    only_runs.budget = Budget {
        max_runs: NonZeroU32::new(50),
        max_seconds: None,
    };
    let failure = try_run(&model, &only_runs, &s).unwrap_err();
    assert_eq!(diagnostic_types(&failure), ["run.no_budget"]);
    let d = &failure.diagnostics[0];
    let said = format!("{} {:?} {:?}", d.message, d.path, d.example);
    assert!(said.contains("--max-seconds"), "{said}");

    // Below n₀ = 10.
    let mut small = RunRequest::new();
    small.budget = budget(5, 60);
    assert_eq!(
        diagnostic_types(&try_run(&model, &small, &s).unwrap_err()),
        ["run.budget_too_small"]
    );

    // A list and a target together.
    let mut list = seeds(&[1, 2]);
    list.budget = budget(50, 60);
    assert_eq!(
        diagnostic_types(&try_run(&model, &list, &s).unwrap_err()),
        ["run.seed_list_with_precision"]
    );

    // A list longer than --max-runs is a refusal, not a silent cut.
    let mut cut = seeds(&[1, 2, 3]);
    cut.budget = Budget {
        max_runs: NonZeroU32::new(2),
        max_seconds: None,
    };
    let failure = try_run(RANDOM_LINE, &cut, &s).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(diagnostic_types(&failure), ["run.budget_too_small"]);

    // A seed named twice.
    let failure = try_run(RANDOM_LINE, &seeds(&[4, 4]), &s).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert_eq!(diagnostic_types(&failure), ["run.duplicate_seed"]);
    // Nothing was stored by any refusal.
    assert!(!s.root().join("runs").exists() || files_below(&s.root().join("runs")).is_empty());
}

/// Sequential stopping (ADR-0014 §3, ADR-0025 §5): seeds 1, 2, ... until the 95 %
/// half-width of throughput is within 4 % of its mean, at least n₀ = 10. The expected
/// n is the first n ≥ 10 at which the interval over the single runs of seeds 1..n meets
/// it. Every interval then states `stopping: sequential` (ADR-0025 §7).
#[test]
#[allow(clippy::cast_possible_truncation)] // At most 100 runs.
fn sequential_stops_where_the_single_runs_say() {
    let values = throughputs(1, 100, "sequential-singles");
    let n = (10..=100)
        .find(|&n| {
            let ci =
                ConfidenceInterval::from_values(&values[..n], ConfidenceLevel::DEFAULT).unwrap();
            ci.relative_half_width().is_some_and(|r| r <= 0.04)
        })
        .expect("the target is reached within 100");
    eprintln!("sequential: expected n = {n}");
    let mut request = RunRequest::new();
    request.budget = budget(100, 3600);
    let response = run_ok(&precision_model("4%"), &request, &store("sequential"));
    let doc = &response.result;
    assert_eq!(response.ended, Ended::Complete);
    assert!(doc.complete);
    assert_eq!(doc.stop_reason, "precision_reached");
    assert_eq!(doc.runs.len(), n);
    assert_eq!(doc.provenance.seeds, (1..=n as u64).collect::<Vec<_>>());
    let progress = response.progress.unwrap();
    assert_eq!(
        (progress.counted, progress.planned, progress.spent),
        (n as u32, 100, None)
    );

    let value = row(doc, "throughput(sink)", &[]).unwrap();
    let ci = ConfidenceInterval::from_values(&values[..n], ConfidenceLevel::DEFAULT).unwrap();
    assert!(rel_close(value.value.as_f64().unwrap(), ci.mean(), 1e-12));
    let first = &value.accuracy[0];
    let AccuracyKind::Sampling(interval) = &first.kind else {
        panic!("{:?}", value.accuracy)
    };
    assert!(rel_close(interval.half_width, ci.half_width(), 1e-9));
    assert_eq!(interval.replications as usize, n);
    assert_eq!(first.target.and_then(|t| t.relative), Some(0.04));
    assert_eq!(first.reached, Some(true));
    for (key, value) in rows(doc) {
        for accuracy in &value.accuracy {
            if let AccuracyKind::Sampling(interval) = &accuracy.kind {
                assert_eq!(interval.stopping.as_deref(), Some("sequential"), "{key:?}");
            }
        }
    }
    assert_no_zero_width(doc);
}

/// A target that 12 runs cannot reach: `--max-runs 12` is used up. Twelve runs, the
/// interval over them, `reached: false`, exit 3, and the document is kept, since the
/// model and the run set determine it (ADR-0025 §5, §10).
#[test]
fn max_runs_used_up_under_a_target() {
    let values = throughputs(1, 12, "exhausted-singles");
    let ci = ConfidenceInterval::from_values(&values, ConfidenceLevel::DEFAULT).unwrap();
    assert!(
        ci.relative_half_width().unwrap() > 0.001,
        "the derivation needs an unreachable target"
    );
    let s = store("exhausted");
    let mut request = RunRequest::new();
    request.budget = budget(12, 3600);
    let response = run_ok(&precision_model("0.1%"), &request, &s);
    let doc = &response.result;
    assert_eq!(response.ended, Ended::BudgetExhausted);
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "budget_exhausted");
    assert_eq!(doc.runs.len(), 12);
    let progress = response.progress.unwrap();
    assert_eq!(
        (progress.counted, progress.planned, progress.spent),
        (12, 12, Some(Spent::MaxRuns))
    );
    let value = row(doc, "throughput(sink)", &[]).unwrap();
    assert!(rel_close(value.value.as_f64().unwrap(), ci.mean(), 1e-12));
    assert_eq!(value.accuracy[0].reached, Some(false));
    let id = doc
        .experiment_id
        .as_ref()
        .expect("twelve finished runs have an experiment id");
    let stored = s
        .root()
        .join("experiments")
        .join(id.as_str())
        .join("result.json");
    assert_eq!(
        std::fs::read_to_string(&stored).expect("the document is kept"),
        doc.to_text()
    );

    // A larger budget reuses the twelve and gives what a fresh store gives.
    let mut more = RunRequest::new();
    more.budget = budget(14, 3600);
    let resumed = run_ok(&precision_model("0.1%"), &more, &s).result;
    let fresh = run_ok(&precision_model("0.1%"), &more, &store("exhausted-fresh")).result;
    assert_eq!(resumed.to_text(), fresh.to_text());
    assert_eq!(resumed.runs.len(), 14);
}

/// A signal before anything finished: exit 4, `cancelled`, not complete, no experiment
/// id and no document in `experiments/` (ADR-0025 §5, §10). The replications are long
/// (about 260,000 parts each, millions of events), so none can finish within the
/// coordinator's 100 ms poll (ADR-0025 §8, §9: "one event plus 100 ms"); the document is
/// then the lowest seed's partial one, with no ids.
#[test]
fn cancelled_before_any_run_finished() {
    let slow = "format: \"1.0\"\nmodel: {name: slow}\n\
        experiment: {type: terminating, horizon: 72h}\n\
        components:\n\
        - {id: source, type: Source, interarrival: {dist: exponential, mean: 1s}}\n\
        - {id: queue, type: Buffer, capacity: 1000 parts}\n\
        - {id: server, type: Machine, cycle_time: {dist: exponential, mean: 500ms}}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, queue, server, sink]]\n";
    let flag = AtomicBool::new(true);
    let s = store("cancelled");
    let mut request = seeds(&[1, 2, 3]);
    request.cancel = Some(&flag);
    let response = run_ok(slow, &request, &s);
    let doc = &response.result;
    assert_eq!(response.ended, Ended::Cancelled);
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "cancelled");
    assert_eq!(doc.experiment_id, None);
    assert_eq!(doc.runs.len(), 1);
    assert_eq!(doc.runs[0].seed, Some(1));
    assert!(doc.runs.iter().all(|r| r.run_id.is_none()));
    let experiments = s.root().join("experiments");
    assert!(!experiments.exists() || files_below(&experiments).is_empty());
}

/// A signal set before the call, on short replications: some may finish within the
/// 100 ms poll and are counted (ADR-0025 §9, C2). Whatever the count, it ends with exit
/// 4, `cancelled`, not complete; an experiment id exactly when every run has a run id
/// (ADR-0024); and no document in `experiments/` (§10).
#[test]
fn cancelled_short_list_keeps_the_rules() {
    let flag = AtomicBool::new(true);
    let s = store("cancelled-short");
    let mut request = seeds(&[1, 2, 3]);
    request.cancel = Some(&flag);
    let response = run_ok(RANDOM_LINE, &request, &s);
    let doc = &response.result;
    assert_eq!(response.ended, Ended::Cancelled);
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "cancelled");
    assert!(!doc.runs.is_empty() && doc.runs.len() <= 3);
    assert_eq!(
        doc.experiment_id.is_some(),
        doc.runs.iter().all(|r| r.run_id.is_some())
    );
    assert_eq!(doc.provenance.seeds.len(), doc.runs.len());
    let experiments = s.root().join("experiments");
    assert!(!experiments.exists() || files_below(&experiments).is_empty());
}

/// `--max-events` stops a replication: exit 3, `event_limit`, not complete, and no
/// document in `experiments/` (ADR-0025 §5).
#[test]
fn event_limit_in_a_list() {
    let s = store("event-limit");
    let mut request = seeds(&[1, 2, 3]);
    request.max_events = Some(50);
    let response = run_ok(RANDOM_LINE, &request, &s);
    let doc = &response.result;
    assert_eq!(response.ended, Ended::EventLimit);
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "event_limit");
    let experiments = s.root().join("experiments");
    assert!(!experiments.exists() || files_below(&experiments).is_empty());
}

/// Adding a component that nothing else touches leaves the other streams, and so the
/// other numbers, unchanged: streams are keyed by component id and purpose (ADR-0013,
/// ADR-0022 §8).
#[test]
fn an_unrelated_line_changes_no_other_number() {
    let one = "format: \"1.0\"\nmodel: {name: two-lines, seed: 7}\n\
        experiment: {type: terminating, horizon: 2h}\n\
        components:\n\
        - {id: s1, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
        - {id: m1, type: Machine, cycle_time: {dist: lognormal, mean: 40s, sigma: 0.3}}\n\
        - {id: k1, type: Sink}\n\
        flow: [[s1, m1, k1]]\n";
    let two = "format: \"1.0\"\nmodel: {name: two-lines, seed: 7}\n\
        experiment: {type: terminating, horizon: 2h}\n\
        components:\n\
        - {id: s1, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
        - {id: m1, type: Machine, cycle_time: {dist: lognormal, mean: 40s, sigma: 0.3}}\n\
        - {id: k1, type: Sink}\n\
        - {id: s2, type: Source, interarrival: {dist: exponential, mean: 30s}}\n\
        - {id: m2, type: Machine, cycle_time: {dist: exponential, mean: 20s}, mtbf: {dist: exponential, mean: 30min}, mttr: 2min}\n\
        - {id: k2, type: Sink}\n\
        flow: [[s1, m1, k1], [s2, m2, k2]]\n";
    let a = run_ok(one, &RunRequest::new(), &store("one-line")).result;
    let b = run_ok(two, &RunRequest::new(), &store("two-lines")).result;
    let line1 = |doc: &ResultDocument| -> Vec<(Key, f64)> {
        rows(doc)
            .into_iter()
            .filter(|((name, _), _)| ["(s1)", "(m1)", "(k1)"].iter().any(|id| name.ends_with(id)))
            .map(|(k, v)| (k, v.value.as_f64().unwrap()))
            .collect()
    };
    let (la, lb) = (line1(&a), line1(&b));
    assert!(la.len() >= 10, "{la:?}");
    assert_eq!(la, lb);
}

/// Seed lists by hand (ADR-0025 §4).
#[test]
fn seed_lists_and_their_limits() {
    use SeedSpec::{One, Range};
    let none = Budget::none();
    // 1..3 and 7 give [1, 2, 3, 7], sorted whatever the order of the specs.
    assert_eq!(
        expand_seeds(&[One(7), Range { first: 1, last: 3 }], none).unwrap(),
        [1, 2, 3, 7]
    );
    // Exactly 1,000 seeds need no budget; 1,001 need both flags.
    assert_eq!(
        expand_seeds(
            &[Range {
                first: 1,
                last: 1000
            }],
            none
        )
        .unwrap()
        .len(),
        1000
    );
    let long = [Range {
        first: 1,
        last: 1001,
    }];
    let failure = expand_seeds(&long, none).unwrap_err();
    assert_eq!(failure.class, FailureClass::Invalid);
    assert!(
        diagnostic_types(&failure)
            .iter()
            .all(|t| *t == "run.no_budget")
    );
    assert_eq!(failure.diagnostics.len(), 2, "one line per missing flag");
    assert_eq!(expand_seeds(&long, budget(1001, 60)).unwrap().len(), 1001);
    // --max-runs below the list's length.
    assert_eq!(
        diagnostic_types(&expand_seeds(&long, budget(1000, 60)).unwrap_err()),
        ["run.budget_too_small"]
    );
    // 100,000 seeds are the most; 100,001 are refused whatever the budget.
    assert_eq!(
        expand_seeds(
            &[Range {
                first: 0,
                last: 99_999
            }],
            budget(100_000, 60)
        )
        .unwrap()
        .len(),
        100_000
    );
    assert_eq!(
        diagnostic_types(
            &expand_seeds(
                &[Range {
                    first: 0,
                    last: 100_000
                }],
                budget(100_000, 60)
            )
            .unwrap_err()
        ),
        ["run.budget_too_large"]
    );
    // --max-runs above 100,000 is refused whatever else is asked.
    assert_eq!(
        diagnostic_types(&expand_seeds(&[One(1), One(2)], budget(100_001, 60)).unwrap_err()),
        ["run.budget_too_large"]
    );
    // The largest seed is 2^53 − 1; one above it, or a backward range, is out of range.
    let top = (1u64 << 53) - 1;
    assert_eq!(expand_seeds(&[One(top)], none).unwrap(), [top]);
    assert_eq!(
        diagnostic_types(&expand_seeds(&[One(top + 1)], none).unwrap_err()),
        ["run.seed_out_of_range"]
    );
    assert_eq!(
        diagnostic_types(
            &expand_seeds(
                &[Range {
                    first: top - 1,
                    last: top + 1
                }],
                none
            )
            .unwrap_err()
        ),
        ["run.seed_out_of_range"]
    );
    assert_eq!(
        diagnostic_types(&expand_seeds(&[Range { first: 5, last: 3 }], none).unwrap_err()),
        ["run.seed_out_of_range"]
    );
    // Overlapping ranges name 3 twice.
    assert_eq!(
        diagnostic_types(
            &expand_seeds(
                &[Range { first: 1, last: 3 }, Range { first: 3, last: 5 }],
                none
            )
            .unwrap_err()
        ),
        ["run.duplicate_seed"]
    );
    // The range form.
    assert_eq!("3..3".parse(), Ok(Range { first: 3, last: 3 }));
    assert!("3..".parse::<SeedSpec>().is_err());
    assert!("-1".parse::<SeedSpec>().is_err());
}

/// The observable signatures of ADR-0025 §2, checked at load time with a field path.
#[test]
fn observable_signatures_are_checked_with_a_path() {
    let registry = taktsim_components::standard().unwrap();
    let model = |observe: &str, precision: &str| {
        format!(
            "format: \"1.0\"\nmodel: {{name: sig, seed: 1}}\n\
             experiment: {{type: terminating, horizon: 1h{precision}}}\n\
             observe: [{observe}]\n\
             components:\n\
             - {{id: source, type: Source}}\n\
             - {{id: cut, type: Machine, cycle_time: 42s}}\n\
             - {{id: buf1, type: Buffer, capacity: 5 parts}}\n\
             - {{id: weld, type: Machine, cycle_time: 45s}}\n\
             - {{id: sink, type: Sink}}\n\
             flow: [[source, cut, buf1, weld, sink]]\n"
        )
    };
    let accepted = model(
        "\"wip()\", \"wip(buf1)\", \"wip(cut, buf1, weld)\", \"held(source)\", \"utilization(weld)\", \
         \"state_shares(cut, source)\", \"throughput(sink)\", \"lead_time(sink)\"",
        ", precision: {\"wip()\": 5%, \"held(source)\": 5%}",
    );
    let report = validate(&registry, ModelSource::Text(&accepted, Format::Yaml));
    assert!(report.valid, "{:?}", report.errors);

    for wrong in [
        "\"held(cut)\"",
        "\"utilization(source)\"",
        "\"utilization(buf1)\"",
        "\"wip(sink)\"",
        "\"wip(source)\"",
        "\"lead_time(cut)\"",
        "\"throughput()\"",
        "\"state_shares()\"",
        "\"state_shares(buf1)\"",
        "\"held()\"",
    ] {
        let text = model(wrong, "");
        let report = validate(&registry, ModelSource::Text(&text, Format::Yaml));
        assert!(!report.valid, "{wrong} was accepted");
        let path = report.errors[0].path.as_deref().unwrap_or_default();
        assert!(path.starts_with("observe"), "{wrong}: path {path:?}");
    }
    let text = model("", ", precision: {\"wip(sink)\": 5%}");
    let report = validate(&registry, ModelSource::Text(&text, Format::Yaml));
    assert!(!report.valid);
    let path = report.errors[0].path.as_deref().unwrap_or_default();
    assert!(path.starts_with("experiment.precision"), "path {path:?}");
}

/// Two variants, half and half, arriving 2 h apart on average over 1 h, so many runs
/// deliver nothing or one variant only. In every run each class row exists (the
/// Source names its variants at the start, so every run has them) and the class rows
/// sum to the total. In the list, each class row is the mean of the singles, with a
/// true 0 where a run delivered none of that class (ADR-0025 §2, §5).
#[test]
#[allow(clippy::cast_precision_loss)] // 12 runs.
fn class_rows_sum_to_the_total_and_average_with_zeros() {
    let model = "format: \"1.0\"\nmodel: {name: mix}\n\
        experiment: {type: terminating, horizon: 1h}\n\
        components:\n\
        - {id: source, type: Source, interarrival: {dist: exponential, mean: 20min}, variants: {basic: 50%, premium: 50%}}\n\
        - {id: cut, type: Machine, cycle_time: 1s}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, cut, sink]]\n";
    let list: Vec<u64> = (1..=12).collect();
    let singles_store = store("mix-singles");
    let mut per_class = [Vec::new(), Vec::new()];
    for s in &list {
        let doc = run_ok(model, &seeds(&[*s]), &singles_store).result;
        let total = row(&doc, "throughput(sink)", &[])
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        let basic = row(&doc, "throughput(sink)", &[("variant", "basic")])
            .unwrap_or_else(|| panic!("seed {s}: no basic row: {:?}", keys(&doc)))
            .value
            .as_f64()
            .unwrap();
        let premium = row(&doc, "throughput(sink)", &[("variant", "premium")])
            .unwrap_or_else(|| panic!("seed {s}: no premium row: {:?}", keys(&doc)))
            .value
            .as_f64()
            .unwrap();
        assert!(
            rel_close(basic + premium, total, 1e-12) || (total == 0.0 && basic + premium == 0.0)
        );
        per_class[0].push(basic);
        per_class[1].push(premium);
    }
    eprintln!("mix: basic {:?}, premium {:?}", per_class[0], per_class[1]);
    let doc = run_ok(model, &seeds(&list), &store("mix-list")).result;
    for (variant, values) in [("basic", &per_class[0]), ("premium", &per_class[1])] {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        let found = row(&doc, "throughput(sink)", &[("variant", variant)])
            .unwrap()
            .value
            .as_f64()
            .unwrap();
        assert!(
            rel_close(found, mean, 1e-12) || (mean == 0.0 && found == 0.0),
            "{variant}: {found} vs {mean}"
        );
    }
    assert_no_zero_width(&doc);

    // One variant: no class rows (a dimension needs two or more classes).
    let single = run_ok(RANDOM_LINE, &seeds(&[1]), &store("one-variant")).result;
    let throughput = single
        .metrics
        .iter()
        .find(|m| m.name == "throughput(sink)")
        .unwrap();
    assert_eq!(throughput.values.len(), 1);
}

/// The `censored` entry of an experiment's lead time states the mean, over the counted
/// runs, of the units inside at each end (ADR-0025 §3; `AccuracyKind::Censored`: "over
/// several runs, their mean"). A run with none inside counts as 0.
#[test]
fn censored_units_are_the_mean_over_the_runs() {
    let list = [31, 32, 33, 34, 35];
    let singles_store = store("censored-singles");
    let inside: Vec<f64> = list
        .iter()
        .map(|s| {
            let doc = run_ok(RANDOM_LINE, &seeds(&[*s]), &singles_store).result;
            let wip_end = row(&doc, "items_alive", &[])
                .unwrap()
                .value
                .as_f64()
                .unwrap()
                - 0.0; // nothing is held: the Source is never blocked by a 1,000-place buffer
            let units = censored(
                &row(&doc, "lead_time(sink)", &[("statistic", "mean")])
                    .unwrap()
                    .accuracy,
            )
            .unwrap_or(0.0);
            assert_eq!(units, wip_end, "seed {s}");
            units
        })
        .collect();
    let mean = inside.iter().sum::<f64>() / 5.0;
    let doc = run_ok(RANDOM_LINE, &seeds(&list), &store("censored-list")).result;
    for value in &doc
        .metrics
        .iter()
        .find(|m| m.name == "lead_time(sink)")
        .unwrap()
        .values
    {
        let found = censored(&value.accuracy);
        if mean > 0.0 {
            assert!(
                found.is_some_and(|f| rel_close(f, mean, 1e-12)),
                "{found:?} vs {mean} from {inside:?}"
            );
        } else {
            assert_eq!(found, None);
        }
    }
}

/// A machine that fails at random, with `state_shares(cut)` targeted at 20 % and
/// `state_shares(cut, source)` at 50 %: every row of `cut` takes the tightest target
/// that covers it, 20 %, and every row of `source` 50 % (ADR-0025 §5). Rows that never
/// vary (idle, blocked, starved, setup: always 0 with an on-demand Source) state no
/// interval, `reached: true`. The experiment stops at the first n ≥ 10 where every
/// varying target row's relative half-width is within its target, which the single
/// runs give.
#[test]
#[allow(clippy::too_many_lines)] // One derivation and its checks, kept together.
fn every_state_row_is_a_target_and_the_tightest_governs() {
    let base = |precision: &str| {
        format!(
            "format: \"1.0\"\nmodel: {{name: flaky, seed: 1}}\n\
             experiment: {{type: terminating, horizon: 8h{precision}}}\n\
             components:\n\
             - {{id: source, type: Source}}\n\
             - {{id: cut, type: Machine, cycle_time: 30s, mtbf: {{dist: exponential, mean: 1h}}, mttr: {{dist: exponential, mean: 10min}}}}\n\
             - {{id: sink, type: Sink}}\n\
             flow: [[source, cut, sink]]\n"
        )
    };
    let plain = base("");
    let singles_store = store("flaky-singles");
    let states = ["idle", "busy", "blocked", "starved", "down", "setup"];
    let mut matrix: Vec<Vec<f64>> = Vec::new();
    for s in 1..=40u64 {
        let doc = run_ok(&plain, &seeds(&[s]), &singles_store).result;
        let mut line: Vec<f64> = states
            .iter()
            .map(|st| {
                row(
                    &doc,
                    "state_shares(cut)",
                    &[("resource", "cut"), ("state", st)],
                )
                .unwrap()
                .value
                .as_f64()
                .unwrap()
            })
            .collect();
        for st in ["waiting", "blocked"] {
            line.push(
                row(
                    &doc,
                    "state_shares(source)",
                    &[("resource", "source"), ("state", st)],
                )
                .unwrap()
                .value
                .as_f64()
                .unwrap(),
            );
        }
        matrix.push(line);
    }
    let targets = [0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.5, 0.5];
    let met = |n: usize| {
        (0..targets.len()).all(|j| {
            let column: Vec<f64> = matrix[..n].iter().map(|line| line[j]).collect();
            all_equal(&column)
                || ConfidenceInterval::from_values(&column, ConfidenceLevel::DEFAULT)
                    .unwrap()
                    .relative_half_width()
                    .is_some_and(|r| r <= targets[j])
        })
    };
    let expected = (10..=40).find(|&n| met(n));
    eprintln!("state targets: expected stop at {expected:?}");

    let targeted =
        base(", precision: {\"state_shares(cut)\": 20%, \"state_shares(cut, source)\": 50%}");
    let mut request = RunRequest::new();
    request.budget = budget(40, 3600);
    let response = run_ok(&targeted, &request, &store("flaky"));
    let doc = &response.result;
    if let Some(n) = expected {
        assert_eq!(response.ended, Ended::Complete);
        assert_eq!(doc.stop_reason, "precision_reached");
        assert_eq!(doc.runs.len(), n);
    } else {
        assert_eq!(response.ended, Ended::BudgetExhausted);
        assert_eq!(doc.runs.len(), 40);
    }
    for (j, st) in states.iter().enumerate() {
        let value = row(
            doc,
            "state_shares(cut)",
            &[("resource", "cut"), ("state", st)],
        )
        .unwrap();
        assert_eq!(
            value.accuracy[0].target.and_then(|t| t.relative),
            Some(0.2),
            "cut {st}"
        );
        let column: Vec<f64> = matrix[..doc.runs.len()]
            .iter()
            .map(|line| line[j])
            .collect();
        if all_equal(&column) {
            assert_eq!(value.accuracy[0].reached, Some(true), "cut {st}");
        }
    }
    for st in ["waiting", "blocked"] {
        let value = row(
            doc,
            "state_shares(source)",
            &[("resource", "source"), ("state", st)],
        )
        .unwrap();
        assert_eq!(
            value.accuracy[0].target.and_then(|t| t.relative),
            Some(0.5),
            "source {st}"
        );
    }
    // Rows no target covers state no target.
    assert_eq!(
        row(doc, "throughput(sink)", &[]).unwrap().accuracy[0].target,
        None
    );
    assert_no_zero_width(doc);
}

/// `wip()` and `held(<id>)` can be targeted (ADR-0025 §2), and a sequence starts at the
/// seed asked for, over `model.seed`: `--seed 5 --max-runs 10` runs 5..14.
#[test]
fn system_wip_and_held_targets_and_the_first_seed() {
    let model = "format: \"1.0\"\nmodel: {name: held-line, seed: 1}\n\
        experiment: {type: terminating, horizon: 1h, precision: {\"wip()\": 0.01%, \"held(source)\": 0.01%}}\n\
        components:\n\
        - {id: source, type: Source, interarrival: {dist: exponential, mean: 50s}}\n\
        - {id: queue, type: Buffer, capacity: 2 parts}\n\
        - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 45s}}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, queue, cut, sink]]\n";
    let mut request = seeds(&[5]);
    request.budget = budget(10, 3600);
    let response = run_ok(model, &request, &store("held-targets"));
    let doc = &response.result;
    assert_eq!(doc.provenance.seeds, (5..=14).collect::<Vec<u64>>());
    assert_eq!(response.ended, Ended::BudgetExhausted);
    for name in ["wip()", "held(source)"] {
        let value = row(doc, name, &[]).unwrap();
        assert_eq!(
            value.accuracy[0].target.and_then(|t| t.relative),
            Some(0.0001),
            "{name}"
        );
        assert_eq!(value.accuracy[0].reached, Some(false), "{name}");
    }
}

/// `--no-reuse` on an experiment reruns every seed and compares; the same engine gives
/// the same bytes, so it succeeds with the same document. A rerun from the store takes
/// no thread: `at_once` is `None` (Progress docs).
#[test]
fn no_reuse_reruns_and_agrees() {
    let s = store("no-reuse");
    let first = run_ok(RANDOM_LINE, &seeds(&[41, 42, 43]), &s);
    let reused = run_ok(RANDOM_LINE, &seeds(&[41, 42, 43]), &s);
    assert_eq!(reused.progress.and_then(|p| p.at_once), None);
    let mut again = seeds(&[41, 42, 43]);
    again.reuse = false;
    let rerun = run_ok(RANDOM_LINE, &again, &s);
    assert_eq!(first.result.to_text(), rerun.result.to_text());
    assert_eq!(first.result.to_text(), reused.result.to_text());
}

/// `--max-seconds` on a list (ADR-0025 §5): replications of tens of seconds each, a
/// clock of one second. Nothing can finish, so the document is the lowest seed's
/// partial one: top `stop_reason` `budget_exhausted`, the run's own `cancelled`, exit
/// 3, the clock named as the budget spent, and nothing in `experiments/`.
#[test]
fn clock_on_a_list_cuts_with_budget_exhausted() {
    let slow = "format: \"1.0\"\nmodel: {name: slow}\n\
        experiment: {type: terminating, horizon: 72h}\n\
        components:\n\
        - {id: source, type: Source, interarrival: {dist: exponential, mean: 1s}}\n\
        - {id: queue, type: Buffer, capacity: 1000 parts}\n\
        - {id: server, type: Machine, cycle_time: {dist: exponential, mean: 500ms}}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, queue, server, sink]]\n";
    let s = store("clock");
    let mut request = seeds(&[1, 2, 3]);
    request.budget = Budget {
        max_runs: None,
        max_seconds: NonZeroU64::new(1),
    };
    let response = run_ok(slow, &request, &s);
    let doc = &response.result;
    assert_eq!(response.ended, Ended::BudgetExhausted);
    assert!(!doc.complete);
    assert_eq!(doc.stop_reason, "budget_exhausted");
    assert_eq!(doc.runs.len(), 1);
    assert_eq!(doc.runs[0].stop_reason, "cancelled");
    assert_eq!(doc.experiment_id, None);
    assert_eq!(
        response.progress.and_then(|p| p.spent),
        Some(Spent::MaxSeconds)
    );
    let experiments = s.root().join("experiments");
    assert!(!experiments.exists() || files_below(&experiments).is_empty());
}

/// A single run cut before time moves has an empty window: no time-weighted row, and
/// each declared observable that has no value goes to `unreported`, `undefined`
/// (ADR-0025 §2, "Undefined values"). The cancel flag is set before the call, so the
/// run stops at its first chance, at time 0, where nothing has happened yet (the first
/// arrival is drawn after an exponential interval, which is above 0).
#[test]
fn cut_at_time_zero_reports_undefined() {
    let model = "format: \"1.0\"\nmodel: {name: cut-at-zero, seed: 3}\n\
        experiment: {type: terminating, horizon: 1h}\n\
        observe: [\"wip()\", \"state_shares(cut)\", \"utilization(cut)\", \"held(source)\"]\n\
        components:\n\
        - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
        - {id: cut, type: Machine, cycle_time: 45s}\n\
        - {id: sink, type: Sink}\n\
        flow: [[source, cut, sink]]\n";
    let flag = AtomicBool::new(true);
    let mut request = RunRequest::new();
    request.cancel = Some(&flag);
    let response = run_ok(model, &request, &store("cut-at-zero"));
    let doc = &response.result;
    assert_eq!(response.ended, Ended::Cancelled);
    assert!(!doc.complete);
    assert_eq!(doc.runs[0].end_time.as_nanos(), 0, "{:?}", doc.runs[0]);
    for name in [
        "wip()",
        "state_shares(cut)",
        "utilization(cut)",
        "held(source)",
    ] {
        assert!(
            doc.metrics.iter().all(|m| m.name != name),
            "{name} has a row over an empty window"
        );
        assert!(
            doc.unreported
                .iter()
                .any(|u| u.observable == name && u.reason == "undefined"),
            "{name}: {:?}",
            doc.unreported
        );
    }
}

/// `--horizon` makes a new version of the model: every run of the list ends there and
/// every window metric covers [0, 30 min] (ADR-0024 §5, ADR-0025 §5).
#[test]
fn another_horizon_applies_to_every_run_of_a_list() {
    use taktsim_model::Encode;
    let mut request = seeds(&[1, 2, 3]);
    request.horizon = Some("30min".parse().unwrap());
    let doc = run_ok(RANDOM_LINE, &request, &store("horizon")).result;
    assert!(
        doc.runs.iter().all(|r| r.end_time.to_string() == "30min"),
        "{:?}",
        doc.runs
    );
    for m in &doc.metrics {
        let over = m.over.encode();
        if m.name == "items_alive" {
            assert_eq!(over["at"], "30min");
        } else {
            assert_eq!(over["to"], "30min", "{}", m.name);
            assert_eq!(over["from"], "0s", "{}", m.name);
        }
    }
}
