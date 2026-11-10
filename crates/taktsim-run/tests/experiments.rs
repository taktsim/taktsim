//! Experiments over several seeds (ADR-0025 §4, §5, §6, §10): one document over the
//! counted runs, rows from the model, and the store keeps only what is determined.

mod support;

use std::collections::BTreeSet;
use std::num::{NonZeroU32, NonZeroU64};

use support::{TempDir, exact_model, model, run_text};
use taktsim_model::{Encode, Json};
use taktsim_run::{
    AccuracyKind, Budget, Ended, FailureClass, Metric, ResultDocument, RunRequest, RunResponse,
};

fn budget(runs: u32, seconds: u64) -> Budget {
    Budget {
        max_runs: NonZeroU32::new(runs),
        max_seconds: NonZeroU64::new(seconds),
    }
}

/// A short random line.
fn line(head: &str, tail: &str) -> String {
    model(
        head,
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 50s}}\n  \
         - {id: sink, type: Sink}",
        &format!("  - [source, cut, sink]{tail}"),
    )
}

fn with_precision(precision: &str) -> String {
    line(", seed: 1", "").replace(
        "horizon: 1h}",
        &format!("horizon: 1h, precision: {{{precision}}}}}"),
    )
}

#[allow(clippy::unwrap_used)] // Test helper.
fn run(yaml: &str, request: &RunRequest<'_>, dir: &TempDir) -> RunResponse {
    run_text(yaml, request, &dir.store()).unwrap()
}

#[allow(clippy::panic)] // Test helper.
fn metric<'a>(result: &'a ResultDocument, name: &str) -> &'a Metric {
    result
        .metrics
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

fn reason(result: &ResultDocument, name: &str, row: usize) -> String {
    match &metric(result, name).values[row].accuracy[0].kind {
        AccuracyKind::None { reason, .. } => reason.clone(),
        AccuracyKind::Sampling(_) => "sampling".to_owned(),
        AccuracyKind::Censored { .. } => "censored".to_owned(),
        AccuracyKind::PartlyUndefined { .. } => "partly_undefined".to_owned(),
        AccuracyKind::Unknown { kind } => kind.clone(),
    }
}

/// Every `sampling` entry in a document, as JSON.
fn sampling_entries(result: &ResultDocument) -> Vec<Json> {
    result
        .metrics
        .iter()
        .flat_map(|m| &m.values)
        .flat_map(|v| &v.accuracy)
        .filter(|a| matches!(a.kind, AccuracyKind::Sampling(_)))
        .map(Encode::encode)
        .collect()
}

#[test]
fn a_list_is_one_document_over_its_runs() {
    let dir = TempDir::new();
    let seeds = [5, 3, 1, 2, 4];
    let request = RunRequest {
        seeds: &seeds,
        ..RunRequest::new()
    };
    let response = run(&line("", ""), &request, &dir);
    let result = &response.result;
    assert_eq!(response.ended, Ended::Complete);
    assert!(result.complete);
    assert_eq!(result.stop_reason, "seeds_finished");
    assert_eq!(result.provenance.seeds, [1, 2, 3, 4, 5], "sorted");
    let run_seeds: Vec<_> = result.runs.iter().map(|r| r.seed).collect();
    assert_eq!(run_seeds, [Some(1), Some(2), Some(3), Some(4), Some(5)]);
    assert!(result.experiment_id.is_some());
    assert_eq!(reason(result, "throughput(sink)", 0), "sampling");
    let AccuracyKind::Sampling(interval) =
        &metric(result, "throughput(sink)").values[0].accuracy[0].kind
    else {
        panic!("sampling");
    };
    assert_eq!(interval.replications, 5);
    assert_eq!(interval.level, 0.95);
    assert_eq!(interval.stopping, None, "a list is fixed n");
    // M7's rows first, in its order.
    let names: Vec<&str> = result.metrics.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names[..3],
        ["created(source)", "delivered(sink)", "items_alive"]
    );
    // The same command again: the same bytes, every run reused.
    let again = run(&line("", ""), &request, &dir);
    assert!(again.reused);
    assert_eq!(again.result.to_text(), result.to_text());
}

/// The rows come from the model: an empty run first, in the middle or last gives the
/// same rows in the same order.
#[test]
fn row_set_does_not_depend_on_the_lowest_seed() {
    // Arrivals every 2 h on average in 1 h: often nothing at all.
    let yaml = model(
        "",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 2h}}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let dir = TempDir::new();
    let delivered = |seed: u64| -> bool {
        let one = run(
            &yaml,
            &RunRequest {
                seeds: &[seed],
                ..RunRequest::new()
            },
            &dir,
        );
        one.result
            .metrics
            .iter()
            .any(|m| m.name == "lead_time(sink)")
    };
    let (mut empty, mut full) = (Vec::new(), Vec::new());
    for seed in 1..200 {
        if delivered(seed) {
            full.push(seed);
        } else {
            empty.push(seed);
        }
    }
    // An empty seed below two full ones, between them, and above them.
    let lowest = empty[0];
    let above: Vec<u64> = full
        .iter()
        .copied()
        .filter(|s| *s > lowest)
        .take(2)
        .collect();
    let between = empty
        .iter()
        .copied()
        .find(|e| *e > full[0] && full.iter().any(|f| f > e))
        .unwrap();
    let a = full[0];
    let b = full.iter().copied().find(|f| *f > between).unwrap();
    let last = empty.iter().copied().find(|e| *e > full[1]).unwrap();
    let sets = [
        vec![lowest, above[0], above[1]],
        vec![a, between, b],
        vec![full[0], full[1], last],
    ];
    let mut shapes = BTreeSet::new();
    for seeds in &sets {
        let result = run(
            &yaml,
            &RunRequest {
                seeds,
                ..RunRequest::new()
            },
            &dir,
        )
        .result;
        let shape: Vec<String> = result
            .metrics
            .iter()
            .map(|m| {
                let labels: Vec<_> = m.values.iter().map(|v| v.labels.clone()).collect();
                format!("{} {labels:?}", m.name)
            })
            .collect();
        shapes.insert(shape.join("\n"));
        // The lead time is over the runs that delivered, and says so.
        let lead = &metric(&result, "lead_time(sink)").values[0];
        let entry = lead.accuracy.last().unwrap().encode();
        assert_eq!(entry["kind"], "partly_undefined", "{seeds:?}");
        assert_eq!(entry["undefined"], Json::from(1));
        assert_eq!(entry["of"], Json::from(3));
    }
    assert_eq!(shapes.len(), 1, "{shapes:?}");
}

/// A count or a rate a run does not have is a true 0, and is averaged.
#[test]
fn true_zero_is_averaged() {
    let yaml = model(
        "",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 2h}}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let dir = TempDir::new();
    let single = |seed: u64| -> f64 {
        let one = run(
            &yaml,
            &RunRequest {
                seeds: &[seed],
                ..RunRequest::new()
            },
            &dir,
        )
        .result;
        metric(&one, "throughput(sink)").values[0]
            .value
            .as_f64()
            .unwrap()
    };
    let per_run: Vec<f64> = (1..=6).map(single).collect();
    assert!(per_run.contains(&0.0), "{per_run:?}");
    let seeds: Vec<u64> = (1..=6).collect();
    let result = run(
        &yaml,
        &RunRequest {
            seeds: &seeds,
            ..RunRequest::new()
        },
        &dir,
    )
    .result;
    let value = &metric(&result, "throughput(sink)").values[0];
    #[allow(clippy::cast_precision_loss)]
    let mean = per_run.iter().sum::<f64>() / per_run.len() as f64;
    let found = value.value.as_f64().unwrap();
    assert!(
        (found - mean).abs() <= 1e-15 * mean.abs().max(1.0),
        "{found} {mean}"
    );
    // `held` of a Source that created nothing is 0, and defined in every run.
    assert_eq!(metric(&result, "held(source)").values[0].accuracy.len(), 1);
}

/// `35.5 ± 0` would read as certainty: a row that does not vary says so.
#[test]
fn identical_replications_state_no_interval() {
    let seeds = [1, 2, 3];
    let dir = TempDir::new();
    let result = run(
        &line("", ""),
        &RunRequest {
            seeds: &seeds,
            ..RunRequest::new()
        },
        &dir,
    )
    .result;
    let shares = metric(&result, "state_shares(cut)");
    let down = shares
        .values
        .iter()
        .position(|v| v.labels.as_ref().is_some_and(|l| l["state"] == "down"))
        .unwrap();
    assert_eq!(
        reason(&result, "state_shares(cut)", down),
        "no_variation_observed"
    );
    for entry in sampling_entries(&result) {
        assert_ne!(entry["interval"]["half_width"], Json::from(0.0), "{entry}");
    }
}

/// A model that draws nothing has no seeds to replicate over: one run, and a warning
/// for each unused flag.
#[test]
fn no_draw_model_never_gets_an_interval() {
    let dir = TempDir::new();
    let seeds = [1, 2, 3, 4, 5];
    let request = RunRequest {
        seeds: &seeds,
        budget: budget(10, 60),
        ..RunRequest::new()
    };
    let response = run(&exact_model(""), &request, &dir);
    assert_eq!(response.result.runs.len(), 1);
    assert_eq!(response.result.provenance.seeds.len(), 0);
    assert_eq!(sampling_entries(&response.result), Vec::<Json>::new());
    let types: Vec<(&str, Option<&str>)> = response
        .warnings
        .iter()
        .map(|w| (w.type_name.as_str(), w.path.as_deref()))
        .collect();
    assert_eq!(
        types,
        [
            ("run.seed_unused", Some("--seed")),
            ("run.flag_unused", Some("--max-runs")),
            ("run.flag_unused", Some("--max-seconds")),
        ]
    );
    assert_eq!(
        reason(&response.result, "throughput(sink)", 0),
        "no_random_draws"
    );
}

/// With a target and no interval to compare, a model that draws nothing states the
/// target and no `reached`.
#[test]
fn a_target_on_a_no_draw_model_is_stated_not_reached() {
    let yaml = exact_model("").replace(
        "horizon: 1h}",
        "horizon: 1h, precision: {\"throughput(sink)\": 2%}}",
    );
    let dir = TempDir::new();
    let result = run(&yaml, &RunRequest::new(), &dir).result;
    let first = &metric(&result, "throughput(sink)").values[0].accuracy[0];
    assert_eq!(first.target.unwrap().relative, Some(0.02));
    assert_eq!(first.reached, None);
}

#[test]
fn overlapping_observables_are_all_answered() {
    let yaml = line(
        "",
        "\nobserve:\n  - state_shares(cut)\n  - state_shares(cut, source)\n  - wip()",
    );
    let dir = TempDir::new();
    let seeds = [1, 2];
    let result = run(
        &yaml,
        &RunRequest {
            seeds: &seeds,
            ..RunRequest::new()
        },
        &dir,
    )
    .result;
    assert_eq!(
        metric(&result, "state_shares(cut)").observables,
        ["state_shares(cut)", "state_shares(cut, source)"]
    );
    assert_eq!(
        metric(&result, "state_shares(source)").observables,
        ["state_shares(cut, source)"]
    );
    assert_eq!(metric(&result, "wip()").observables, ["wip()"]);
    assert_eq!(result.unreported, []);
}

/// One row, several targets: the tightest governs, and the others can be read off.
#[test]
fn tightest_target_governs() {
    let yaml = with_precision("\"state_shares(cut)\": 2%, \"state_shares(cut, source)\": 50%");
    let dir = TempDir::new();
    let request = RunRequest {
        budget: budget(10, 600),
        ..RunRequest::new()
    };
    let result = run(&yaml, &request, &dir).result;
    let target = |name: &str| {
        metric(&result, name).values[0].accuracy[0]
            .target
            .unwrap()
            .relative
    };
    assert_eq!(target("state_shares(cut)"), Some(0.02));
    assert_eq!(target("state_shares(source)"), Some(0.5));
    assert!(
        metric(&result, "throughput(sink)").values[0].accuracy[0]
            .target
            .is_none()
    );
    // Every interval of a sequential experiment says its level is nominal.
    for entry in sampling_entries(&result) {
        assert_eq!(entry["interval"]["stopping"], "sequential", "{entry}");
    }
}

#[test]
fn system_wip_and_held_reach_their_targets() {
    let yaml = with_precision("\"wip()\": 50%, \"held(source)\": 100%");
    let dir = TempDir::new();
    let request = RunRequest {
        budget: budget(50, 600),
        ..RunRequest::new()
    };
    let response = run(&yaml, &request, &dir);
    assert_eq!(response.ended, Ended::Complete);
    assert_eq!(response.result.stop_reason, "precision_reached");
    let wip = &metric(&response.result, "wip()").values[0].accuracy[0];
    assert_eq!(wip.reached, Some(true));
}

/// A target that cannot be reached in the budget: fourteen runs, an id over them, and
/// a larger budget reuses them.
#[test]
fn fourteen_of_twenty() {
    let yaml = with_precision("\"throughput(sink)\": 0.1%");
    let dir = TempDir::new();
    let request = RunRequest {
        budget: budget(14, 600),
        ..RunRequest::new()
    };
    let response = run(&yaml, &request, &dir);
    let result = &response.result;
    assert_eq!(response.ended, Ended::BudgetExhausted);
    assert!(!result.complete);
    assert_eq!(result.stop_reason, "budget_exhausted");
    assert_eq!(result.runs.len(), 14);
    assert_eq!(result.provenance.seeds, (1..=14).collect::<Vec<_>>());
    let first = &metric(result, "throughput(sink)").values[0].accuracy[0];
    assert_eq!(first.reached, Some(false));
    let AccuracyKind::Sampling(interval) = &first.kind else {
        panic!("sampling");
    };
    assert_eq!(interval.replications, 14);
    // Kept, because the model and the run set determine it.
    let id = result.experiment_id.as_ref().unwrap();
    let kept = dir
        .store()
        .root()
        .join("experiments")
        .join(id.as_str())
        .join("result.json");
    assert_eq!(std::fs::read_to_string(kept).unwrap(), result.to_text());

    let more = RunRequest {
        budget: budget(20, 600),
        ..RunRequest::new()
    };
    let larger = run(&yaml, &more, &dir);
    assert_eq!(larger.result.runs.len(), 20);
    assert_eq!(larger.result.runs[..14], result.runs[..]);
    let runs = std::fs::read_dir(dir.store().root().join("runs"))
        .unwrap()
        .count();
    assert_eq!(runs, 20, "the fourteen were reused");
}

/// The store keeps an experiment only when the model and the run set determine it.
#[test]
fn kept_only_when_determined() {
    let dir = TempDir::new();
    let experiments = dir.store().root().join("experiments");
    let seeds = [1, 2, 3];
    let complete = run(
        &line("", ""),
        &RunRequest {
            seeds: &seeds,
            ..RunRequest::new()
        },
        &dir,
    );
    let id = complete.result.experiment_id.clone().unwrap();
    assert!(experiments.join(id.as_str()).join("result.json").is_file());
    let listed = std::fs::read_dir(&experiments).unwrap().count();
    assert_eq!(listed, 1);

    // An event limit in a later seed: the earlier ones count, nothing is kept.
    let limited = RunRequest {
        seeds: &seeds,
        max_events: Some(150),
        ..RunRequest::new()
    };
    let other = line("", "").replace("horizon: 1h", "horizon: 2h");
    let partial = run(&other, &limited, &dir);
    assert_eq!(partial.ended, Ended::EventLimit);
    assert!(!partial.result.complete);
    assert_eq!(std::fs::read_dir(&experiments).unwrap().count(), 1);

    // With --no-reuse, the kept document is compared, not overwritten.
    let again = RunRequest {
        seeds: &seeds,
        reuse: false,
        ..RunRequest::new()
    };
    let compared = run(&line("", ""), &again, &dir);
    assert_eq!(compared.result.to_text(), complete.result.to_text());
}

/// No replication finished: the document is the lowest seed's partial one, M7's shape.
#[test]
fn zero_finished_replications_state_a_run() {
    let dir = TempDir::new();
    let seeds = [7, 8, 9];
    let request = RunRequest {
        seeds: &seeds,
        max_events: Some(3),
        ..RunRequest::new()
    };
    let response = run(&line("", ""), &request, &dir);
    assert_eq!(response.ended, Ended::EventLimit);
    let result = &response.result;
    assert_eq!(result.runs.len(), 1);
    assert_eq!(result.runs[0].seed, Some(7));
    assert_eq!(result.runs[0].run_id, None);
    assert_eq!(result.experiment_id, None);
    assert_eq!(result.stop_reason, "event_limit");
}

#[test]
fn what_a_command_cannot_ask_for_is_refused() {
    let dir = TempDir::new();
    let store = dir.store();
    let refused =
        |yaml: &str, request: &RunRequest<'_>| run_text(yaml, request, &store).unwrap_err();

    // Precision without a budget: one line per missing flag.
    let precise = with_precision("\"throughput(sink)\": 2%");
    let failure = refused(&precise, &RunRequest::new());
    assert_eq!(failure.class, FailureClass::Invalid);
    let lines: Vec<(&str, Option<&str>)> = failure
        .diagnostics
        .iter()
        .map(|d| (d.type_name.as_str(), d.path.as_deref()))
        .collect();
    assert_eq!(
        lines,
        [
            ("run.no_budget", Some("--max-runs")),
            ("run.no_budget", Some("--max-seconds")),
        ]
    );
    for line in &failure.diagnostics {
        assert!(line.found.is_some() && line.allowed.is_some() && line.example.is_some());
    }
    // A list with precision asks for two things.
    let seeds = [1, 2];
    let request = RunRequest {
        seeds: &seeds,
        budget: budget(20, 60),
        ..RunRequest::new()
    };
    assert_eq!(
        refused(&precise, &request).diagnostics[0].type_name,
        "run.seed_list_with_precision"
    );
    // Fewer runs than n₀.
    let request = RunRequest {
        budget: budget(9, 60),
        ..RunRequest::new()
    };
    assert_eq!(
        refused(&precise, &request).diagnostics[0].type_name,
        "run.budget_too_small"
    );
    // A seed twice.
    let twice = [3, 3];
    let request = RunRequest {
        seeds: &twice,
        ..RunRequest::new()
    };
    assert_eq!(
        refused(&line("", ""), &request).diagnostics[0].type_name,
        "run.duplicate_seed"
    );
    // A sequence that would run past 2^53 − 1.
    let top = [(1_u64 << 53) - 5];
    let request = RunRequest {
        seeds: &top,
        budget: budget(10, 60),
        ..RunRequest::new()
    };
    assert_eq!(
        refused(&precise, &request).diagnostics[0].type_name,
        "run.seed_out_of_range"
    );
    // --max-runs above the limit, whatever else is asked.
    let request = RunRequest {
        budget: budget(100_001, 60),
        ..RunRequest::new()
    };
    assert_eq!(
        refused(&precise, &request).diagnostics[0].type_name,
        "run.budget_too_large"
    );
    assert!(!store.root().join("runs").exists(), "nothing ran");
}

/// A precision run's document reads back as itself.
#[test]
fn an_experiment_document_round_trips() {
    let dir = TempDir::new();
    let yaml = with_precision("\"throughput(sink)\": 20%");
    let request = RunRequest {
        budget: budget(12, 600),
        ..RunRequest::new()
    };
    let result = run(&yaml, &request, &dir).result;
    let text = result.to_text();
    let read: ResultDocument =
        taktsim_model::decode_str(&text, taktsim_model::Format::Json).unwrap();
    assert_eq!(read.to_text(), text);
}

/// A run that delivered nothing has no lead time row, but the units inside at its end
/// count all the same (ADR-0025 §3): seed 6 delivers nothing with one unit in `cut`,
/// seeds 1 and 2 deliver their one unit each and end empty, so every lead time row
/// over the three carries `censored` with 1/3 unit inside. Run alone, seed 4 states
/// its own entry, and a part held at a Source is no unit inside.
#[test]
fn censored_counts_the_units_inside_a_run_that_delivered_nothing() {
    let dir = TempDir::new();
    let yaml = "format: \"1.0\"\nmodel: {name: rare}\nexperiment: {type: terminating, horizon: 30min}\n\
                components:\n\
                - {id: source, type: Source, interarrival: {dist: exponential, mean: 30min}}\n\
                - {id: cut, type: Machine, cycle_time: 10min}\n\
                - {id: sink, type: Sink}\n\
                flow: [[source, cut, sink]]\nobserve: [\"lead_time(sink)\"]\n";
    let store = dir.store();
    let inside = |seeds: &[u64]| -> Vec<Option<f64>> {
        let request = RunRequest {
            seeds,
            ..RunRequest::new()
        };
        let response = run_text(yaml, &request, &store).unwrap();
        let lead = response
            .result
            .metrics
            .iter()
            .find(|m| m.name == "lead_time(sink)")
            .unwrap();
        assert_eq!(lead.values.len(), 7);
        lead.values
            .iter()
            .map(|value| {
                value.accuracy.iter().find_map(|a| match a.kind {
                    AccuracyKind::Censored { units_inside, .. } => Some(units_inside),
                    AccuracyKind::Sampling(_)
                    | AccuracyKind::None { .. }
                    | AccuracyKind::PartlyUndefined { .. }
                    | AccuracyKind::Unknown { .. } => None,
                })
            })
            .collect()
    };
    assert_eq!(inside(&[1, 2, 6]), vec![Some(1.0 / 3.0); 7]);
    // Fresh or reused, the same: the second call reads every run from the store.
    assert_eq!(inside(&[1, 2, 6]), vec![Some(1.0 / 3.0); 7]);
    // Seed 4 delivers one unit and ends with another inside: its own entry counts.
    assert_eq!(inside(&[1, 4]), vec![Some(0.5); 7]);
    // Seed 45 delivers nothing, with one unit in `cut` and one part held at the Source:
    // one unit inside, not the two items alive.
    assert_eq!(inside(&[1, 45]), vec![Some(0.5); 7]);
    // Seed 3 delivers nothing and ends empty.
    assert_eq!(inside(&[1, 3]), vec![None; 7]);
    assert_eq!(inside(&[1, 2]), vec![None; 7]);
}
