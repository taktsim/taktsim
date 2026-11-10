//! Golden traces of reference models (spec §7 Layer 2, ADR-0021): each run must
//! reproduce its committed snapshot, stream hash included, on every tier-1
//! target, and each replays. A snapshot changes only with an intended engine
//! change, and the commit says why (CLAUDE.md).

mod support;

use support::{document, load, registry, t};
use taktsim_core::EngineConfig;
use taktsim_trace::golden::{self, Case};
use taktsim_trace::{ModelHash, SystemHash};

/// Records case `name` from `yaml` with `seed` and compares its snapshot.
#[allow(clippy::unwrap_used)] // Test helper.
fn case(name: &str, seed: u64, horizon: &str, yaml: &str) {
    let model = load(yaml);
    let registry = registry();
    let case = Case {
        name,
        model_hash: ModelHash::from_bytes(*model.hash().as_bytes()),
        system_hash: SystemHash::from_bytes(*model.system_hash().as_bytes()),
        until: t(horizon),
    };
    let golden = golden::record(&case, || {
        Ok(registry
            .build(model.document(), EngineConfig::new(seed))
            .unwrap())
    })
    .unwrap();
    insta::assert_snapshot!(name, golden.snapshot());
}

#[test]
fn chain() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}}\n  \
         - {id: buf1, type: Buffer, capacity: 5 parts}\n  \
         - {id: weld, type: Machine, cycle_time: 45s}\n  - {id: sink, type: Sink}",
        "  - [source, cut, buf1, weld, sink]",
    );
    case("m5_chain", 42, "1h", &yaml);
}

#[test]
fn failures() {
    let yaml = document(
        "8h",
        "  - {id: source, type: Source}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.5}, mtbf: {dist: exponential, mean: 1h}, mttr: {dist: exponential, mean: 3min}}\n  \
         - {id: buf1, type: Buffer, capacity: 5 parts}\n  \
         - {id: weld, type: Machine, cycle_time: {dist: lognormal, mean: 45s, sigma: 0.5}, mtbf: 2h, mttr: 2min}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, buf1, weld, sink]",
    );
    case("m5_failures", 7, "8h", &yaml);
}

#[test]
fn setup_mix() {
    let yaml = document(
        "4h",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 5min}, batch: 4 parts, variants: {basic: 60%, premium: 30%, special: 10%}, priority: 2}\n  \
         - {id: buf, type: Buffer, capacity: 12 parts, discipline: priority}\n  \
         - id: weld\n    type: Machine\n    cycle_time: {dist: uniform, min: 40s, max: 50s}\n    \
         setup:\n      per_variant: {dist: triangular, min: 2min, mode: 4min, max: 6min}\n      \
         matrix:\n        - {from: basic, to: premium, time: 6min}\n        \
         - {from: premium, to: basic, time: 0s}\n  - {id: sink, type: Sink}",
        "  - [source, buf, weld, sink]",
    );
    case("m5_setup_mix", 2026, "4h", &yaml);
}

#[test]
fn capacity2() {
    let yaml = document(
        "4h",
        "  - {id: source, type: Source}\n  \
         - {id: m, type: Machine, cycle_time: {dist: exponential, mean: 1min}, capacity: 2 parts, mtbf: {dist: gamma, mean: 40min, cv: 0.5}, mttr: {dist: erlang, mean: 2min, k: 3}}\n  \
         - {id: buf, type: Buffer, capacity: 3 parts, discipline: lifo}\n  \
         - {id: press, type: Machine, cycle_time: {dist: weibull, scale: 40s, shape: 1.5}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, buf, press, sink]",
    );
    case("m5_capacity2", 11, "4h", &yaml);
}

#[test]
fn branch_merge() {
    let yaml = document(
        "2h",
        "  - {id: s1, type: Source, interarrival: {dist: exponential, mean: 15s}, variants: {red: 50%, blue: 50%}}\n  \
         - {id: s2, type: Source, interarrival: {dist: normal, mean: 40s, sd: 5s}}\n  \
         - {id: a, type: Machine, cycle_time: {dist: exponential, mean: 25s}}\n  \
         - {id: b, type: Machine, cycle_time: {dist: exponential, mean: 35s}, mtbf: {dist: exponential, mean: 20min}, mttr: 1min}\n  \
         - {id: buf, type: Buffer, capacity: 2 parts}\n  \
         - {id: c, type: Machine, cycle_time: {dist: exponential, mean: 15s}}\n  \
         - {id: sink1, type: Sink}\n  - {id: sink2, type: Sink}",
        "  - {from: s1, to: [a, b]}\n  - {from: s2, to: b}\n  - {from: [a, b], to: buf}\n  \
         - {from: buf, to: c}\n  - {from: c, to: [sink1, sink2]}",
    );
    case("m5_branch_merge", 3, "2h", &yaml);
}

/// Constant times tie often: room freed through a chain at one instant, room
/// opening at two targets at one instant, and a merge served at that instant.
/// Event order decides (ADR-0022 §3, §8), and this pins it.
#[test]
fn same_instant_chain() {
    let yaml = document(
        "4min",
        "  - {id: source, type: Source, interarrival: 4s}\n  \
         - {id: a, type: Machine, cycle_time: 13s}\n  \
         - {id: b, type: Machine, cycle_time: 22s}\n  \
         - {id: buf, type: Buffer, capacity: 1 parts}\n  \
         - {id: c, type: Machine, cycle_time: 26s}\n  \
         - {id: d, type: Machine, cycle_time: 26s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b]}\n  - {from: [a, b], to: buf}\n  - [buf, c, d, sink]",
    );
    case("m5_same_instant_chain", 1, "4min", &yaml);
}
