//! The real types keep every pinned vector of the loader, and the normalized form
//! of what they add is pinned (ADR-0022 §1).

mod support;

use std::path::Path;

use support::{load, registry, try_load};
use taktsim_model::{Format, Loader};

const MODEL_HASH: &str = "56d63bb349f4eef8da1e437c48f4e01206eaee6315c97c5913637f56ff462dae";
const SYSTEM_HASH: &str = "18c3cbaad4e836fd4c29a17ba0a88ff553dfb516a69886db10ed2efb09d89904";

#[allow(clippy::unwrap_used)] // Test helper.
fn model_data(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("taktsim-model")
        .join("tests")
        .join("data")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn spec_example_keeps_its_pinned_hashes() {
    let model = load(&model_data("valid/spec-line-a.yaml"));
    assert_eq!(
        model.canonical_json(),
        model_data("spec-line-a.canonical.json").trim_end()
    );
    assert_eq!(model.hash().to_string(), MODEL_HASH);
    assert_eq!(model.system_hash().to_string(), SYSTEM_HASH);
    // The canonical form is itself a model with the same hash.
    let registry = registry();
    let again = Loader::new(registry.catalog())
        .load_str(model.canonical_json(), Format::Json)
        .unwrap();
    assert_eq!(again.hash(), model.hash());
}

#[test]
fn the_spec_example_runs() {
    let r = support::run(&model_data("valid/spec-line-a.yaml"), 42);
    assert!(r.report.items.delivered.get() > 300);
    // `cut` fails once, at 6 h, in the 8 h of this model.
    let failures = r.times("cut", |m| {
        matches!(m, taktsim_components::FlowMessage::Failure)
    });
    assert_eq!(failures, [support::t("6h")]);
}

const FULL: &str = r#"
format: "1.0"
model: {name: full, seed: 1}
experiment: {type: terminating, horizon: 8h}
components:
  - id: source
    type: Source
    interarrival: { dist: exponential, mean: 40s }
    batch: 2 parts
    variants: { premium: 20%, basic: 80% }
    priority: 1
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.08 }
    mtbf: 6h
    mttr: { dist: exponential, mean: 12min }
    setup:
      per_variant: 4min
      matrix:
        - { from: premium, to: basic, time: 0s }
        - { from: basic, to: premium, time: 6min }
    capacity: 2 parts
  - id: buf1
    type: Buffer
    capacity: 20 parts
    discipline: fifo
  - id: sink
    type: Sink
flow:
  - [source, cut, buf1, sink]
"#;

/// The normalized form of the full vocabulary. The hash is SHA-256 of the
/// `CANONICAL` text below, by another tool (`sha256sum`).
const CANONICAL: &str = r#"{"components":[{"batch":"2 parts","id":"source","interarrival":{"dist":"exponential","mean":"40s"},"priority":1,"type":"Source","variants":{"basic":"80%","premium":"20%"}},{"capacity":"2 parts","cycle_time":{"dist":"lognormal","mean":"42s","sigma":0.08},"id":"cut","mtbf":"6h","mttr":{"dist":"exponential","mean":"12min"},"setup":{"matrix":[{"from":"basic","time":"6min","to":"premium"},{"from":"premium","time":"0s","to":"basic"}],"per_variant":"4min"},"type":"Machine"},{"capacity":"20 parts","discipline":"fifo","id":"buf1","type":"Buffer"},{"id":"sink","type":"Sink"}],"experiment":{"horizon":"8h","type":"terminating"},"flow":[{"from":["source"],"to":["cut"]},{"from":["cut"],"to":["buf1"]},{"from":["buf1"],"to":["sink"]}],"format":"1.0","model":{"name":"full","seed":1}}"#;

const FULL_HASH: &str = "8a68a35fdbf391a0587e36be9ff4104a049ddf35f6c926d8e25e399479a473f3";

#[test]
fn full_vocabulary_hash_is_pinned() {
    let model = load(FULL);
    assert_eq!(model.canonical_json(), CANONICAL);
    assert_eq!(model.hash().to_string(), FULL_HASH);
    // Its canonical form loads and keeps the hash.
    let registry = registry();
    let again = Loader::new(registry.catalog())
        .load_str(CANONICAL, Format::Json)
        .unwrap();
    assert_eq!(again.hash(), model.hash());
}

#[test]
fn written_default_is_part_of_the_hash() {
    // N1: `capacity: 1 parts` and nothing are two hashes.
    let bare = load(&support::document(
        "1h",
        "  - {id: source, type: Source}\n  - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    ));
    let written = load(&support::document(
        "1h",
        "  - {id: source, type: Source}\n  - {id: m, type: Machine, cycle_time: 1min, capacity: 1 parts}\n  - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    ));
    assert_ne!(bare.hash(), written.hash());
    assert_ne!(bare.system_hash(), written.system_hash());
    // And so for each default of the vocabulary.
    let plain = support::document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min}\n  - {id: buf, type: Buffer, capacity: 2 parts}\n  - {id: sink, type: Sink}",
        "  - [source, buf, sink]",
    );
    let hash = |text: &str| load(text).hash();
    for (from, to) in [
        ("interarrival: 1min}", "interarrival: 1min, batch: 1 parts}"),
        ("interarrival: 1min}", "interarrival: 1min, priority: 0}"),
        (
            "interarrival: 1min}",
            "interarrival: 1min, variants: {default: 100%}}",
        ),
        ("capacity: 2 parts}", "capacity: 2 parts, discipline: fifo}"),
    ] {
        let changed = plain.replace(from, to);
        assert_ne!(changed, plain);
        assert_ne!(hash(&changed), hash(&plain), "{to}");
    }
}

#[test]
fn spellings_of_one_value_have_one_hash() {
    let a = load(FULL);
    let b = load(
        &FULL
            .replace(
                "variants: { premium: 20%, basic: 80% }",
                "variants: { basic: 80.0%, premium: 20% }",
            )
            .replace("capacity: 20 parts", "capacity: 20.0 parts")
            .replace("per_variant: 4min", "per_variant: 240s"),
    );
    assert_eq!(a.hash(), b.hash());
    assert!(try_load(FULL).is_ok());
}

/// Spec §4's routing example, completed: `paint` needs a way out and `pack` an entry
/// (ADR-0023, disagreement 6). The rest is as the spec writes it.
#[test]
fn spec_routing_example_loads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("docs")
        .join("spec.md");
    let spec = std::fs::read_to_string(path).unwrap();
    let start = spec
        .find("flow:\n  - { from: source, to: cut }")
        .expect("the example is in the spec");
    let end = spec[start..].find("```").unwrap();
    let routing = &spec[start..start + end];
    assert!(routing.contains("choose: shortest_queue"));
    assert!(routing.contains("when: \"variant == 'premium'\""));
    assert!(routing.contains("dispatch: { weld-1: earliest_due_date"));
    let completed = routing.replace("control:", "  - [paint, pack, sink]\ncontrol:");
    // `qc` has its last edge to `pack`, so `pack` has its way in, and `paint` joins it.
    let yaml = format!(
        "format: \"1.0\"\nmodel: {{name: routing, seed: 1}}\nexperiment: {{type: terminating, horizon: 2h}}\n\
         components:\n  \
         - {{id: source, type: Source, interarrival: {{dist: exponential, mean: 40s}}, variants: {{basic: 80%, premium: 20%}}, due_in: {{dist: uniform, min: 2h, max: 8h}}}}\n  \
         - {{id: cut, type: Machine, cycle_time: 30s}}\n  \
         - {{id: weld-1, type: Machine, cycle_time: 45s, cycle_time_by_variant: {{premium: 60s}}}}\n  \
         - {{id: weld-2, type: Machine, cycle_time: 45s}}\n  \
         - {{id: qc, type: Machine, cycle_time: 10s}}\n  \
         - {{id: paint, type: Machine, cycle_time: 1min}}\n  \
         - {{id: pack, type: Machine, cycle_time: 20s}}\n  \
         - {{id: sink, type: Sink}}\n{completed}"
    );
    let model = load(&yaml);
    let flow = model.document().flow();
    assert_eq!(
        flow[1].choose(),
        Some(taktsim_model::ChoiceRule::ShortestQueue)
    );
    assert!(flow[3].when().is_some());
    assert_eq!(model.document().control().dispatch().len(), 2);
    // It runs, every branch is used, and a premium part goes to `paint`.
    let r = support::run_model(&model, 3);
    assert!(r.report.items.delivered.get() > 50);
    for name in ["weld-1", "weld-2", "paint", "pack"] {
        assert_ne!(r.arrivals(name).len(), 0, "{name}");
    }
    assert!(r.arrival_variants("paint").iter().all(|v| v == "premium"));
    support::check_records(&r);
    // The ADR's own form of it: a split on `qc`'s last edge sends rework back to `cut`.
    let rework = yaml.replace(
        "{ from: qc, to: pack }",
        "{ from: qc, split: { cut: 5%, pack: 95% } }",
    );
    assert_ne!(rework, yaml);
    let r = support::run(&rework, 3);
    assert!(r.report.items.delivered.get() > 50);
    assert_ne!(
        r.times("cut", |m| matches!(
            m,
            taktsim_components::FlowMessage::Return(_)
        ))
        .len(),
        0
    );
    support::check_records(&r);
}
