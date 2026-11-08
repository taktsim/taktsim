//! A claim of drawing nothing labels a number exact, so it has three guards
//! (ADR-0024 §5): parameters destructured without `..`, this property test over
//! the standard catalog, and the check run on every such run.

mod support;

use proptest::prelude::*;
use support::{TempDir, exact_model, model, run_with, test_registry};
use taktsim_components::{RunOptions, standard};
use taktsim_core::RunOutcome;
use taktsim_model::{Format, Loader};
use taktsim_run::{FailureClass, RunRequest};

/// A time: mostly a constant, so many generated models draw nothing.
fn time() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => (1_u32..200).prop_map(|s| format!("{s}s")),
        1 => (1_u32..200).prop_map(|s| format!("{{dist: exponential, mean: {s}s}}")),
        1 => (1_u32..200).prop_map(|s| format!("{{dist: uniform, min: {s}s, max: {}s}}", s + 10)),
    ]
}

fn source() -> impl Strategy<Value = String> {
    (
        prop::option::of(time()),
        prop_oneof![
            8 => Just(""),
            1 => Just(", variants: {default: 100%}"),
            1 => Just(", variants: {default: 50%, b: 50%}"),
        ],
        any::<bool>(),
    )
        .prop_map(|(interarrival, variants, priority)| {
            let priority = if priority { ", priority: 2" } else { "" };
            let interarrival =
                interarrival.map_or_else(String::new, |t| format!(", interarrival: {t}"));
            format!("{{id: source, type: Source{interarrival}{variants}{priority}}}")
        })
}

fn machine(id: &'static str) -> impl Strategy<Value = String> {
    (
        time(),
        prop::option::of((time(), time())),
        prop::option::of(time()),
        prop::option::of(time()),
        any::<bool>(),
    )
        .prop_map(move |(cycle, failures, per_variant, change, double)| {
            let failures =
                failures.map_or_else(String::new, |(f, r)| format!(", mtbf: {f}, mttr: {r}"));
            let matrix =
                change.map(|t| format!("matrix: [{{from: default, to: other, time: {t}}}]"));
            let setup = match (per_variant, matrix) {
                (Some(p), Some(m)) => format!(", setup: {{per_variant: {p}, {m}}}"),
                (Some(p), None) => format!(", setup: {{per_variant: {p}}}"),
                (None, Some(m)) => format!(", setup: {{{m}}}"),
                (None, None) => String::new(),
            };
            let capacity = if double { ", capacity: 2 parts" } else { "" };
            format!("{{id: {id}, type: Machine, cycle_time: {cycle}{failures}{setup}{capacity}}}")
        })
}

/// A whole model: two Sources, so a matrix may name `default` and `other`.
fn generated() -> impl Strategy<Value = String> {
    (source(), machine("cut"), machine("weld"), 1_u32..4).prop_map(|(source, cut, weld, room)| {
        model(
            "",
            &format!(
                "  - {source}\n  - {{id: second, type: Source, interarrival: 90s, variants: {{other: 100%}}}}\n  \
                 - {cut}\n  - {{id: buf, type: Buffer, capacity: {room} parts}}\n  - {weld}\n  - {{id: sink, type: Sink}}"
            ),
            "  - {from: [source, second], to: cut}\n  - [cut, buf, weld, sink]",
        )
    })
}

/// A run with no seed, under this internal seed.
#[allow(clippy::unwrap_used)] // Test helper.
fn trace_hash(yaml: &str, seed: u64) -> (String, bool) {
    let registry = standard().unwrap();
    let model = Loader::new(registry.catalog())
        .load_str(yaml, Format::Yaml)
        .unwrap();
    let (_, report) = registry
        .prepare(&model, RunOptions::new().seed(seed))
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    assert_ne!(report.summary.outcome, RunOutcome::EventLimit);
    (
        report.trace.unwrap().hash.to_string(),
        registry.draws_random(model.document()),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Every generated model that claims to draw nothing gives the same trace
    /// under two seeds.
    #[test]
    fn no_draw_claim_holds_on_generated_models(yaml in generated()) {
        let (one, draws) = trace_hash(&yaml, 1);
        if !draws {
            let (two, _) = trace_hash(&yaml, 2);
            prop_assert_eq!(one, two, "{}", yaml);
        }
    }
}

/// The generator does produce both, so the property above is not vacuous, and
/// the claim is tested the other way too: a model with a varying time that claims
/// to draw would differ.
#[test]
fn the_generator_gives_both_kinds_of_model() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let mut runner = TestRunner::deterministic();
    let (mut drawing, mut exact) = (0, 0);
    for _ in 0..200 {
        let yaml = generated().new_tree(&mut runner).unwrap().current();
        if trace_hash(&yaml, 1).1 {
            drawing += 1;
        } else {
            exact += 1;
        }
    }
    assert!(drawing >= 20 && exact >= 20, "{drawing} and {exact}");
}

#[test]
fn false_no_draw_claim_exits_2_and_is_not_kept() {
    let yaml = model(
        "",
        "  - {id: liar, type: Liar}\n  - {id: keep, type: Keep, how: delivered}",
        "  - [liar, keep]",
    );
    let dir = TempDir::new();
    let store = dir.store();
    let failure = run_with(&test_registry(), &yaml, &RunRequest::new(), &store).unwrap_err();
    assert_eq!(failure.class, FailureClass::RunFailed);
    let error = &failure.diagnostics[0];
    assert_eq!(error.type_name, "run.no_draw_claim_false");
    assert_eq!(error.example, None);
    assert!(error.time.is_some());
    assert!(error.recent.len() > 1);
    let runs = std::fs::read_dir(store.root().join("runs"))
        .unwrap()
        .count();
    assert_eq!(runs, 0);
    let temp = std::fs::read_dir(store.root().join("tmp")).unwrap().count();
    assert_eq!(temp, 0);
}

/// A run stopped by a limit has not reached its end: it is not checked, and it is
/// not kept either, so nothing false is stored.
#[test]
fn a_partial_run_of_a_liar_is_not_kept() {
    let yaml = model(
        "",
        "  - {id: liar, type: Liar}\n  - {id: keep, type: Keep, how: delivered}",
        "  - [liar, keep]",
    );
    let dir = TempDir::new();
    let store = dir.store();
    let request = RunRequest {
        max_events: Some(3),
        ..RunRequest::new()
    };
    let response = run_with(&test_registry(), &yaml, &request, &store).unwrap();
    assert!(!response.result.complete);
    assert_eq!(
        std::fs::read_dir(store.root().join("runs"))
            .unwrap()
            .count(),
        0
    );
}

/// The check costs one more run of that model, once.
#[test]
fn a_true_claim_is_kept_and_then_reused() {
    let dir = TempDir::new();
    let store = dir.store();
    let yaml = exact_model("");
    let registry = standard().unwrap();
    let first = run_with(&registry, &yaml, &RunRequest::new(), &store).unwrap();
    assert!(first.result.complete);
    assert!(first.result.runs[0].run_id.is_some());
    let second = run_with(&registry, &yaml, &RunRequest::new(), &store).unwrap();
    assert!(second.reused);
}
