//! Golden traces of reference models (spec §7 Layer 2, ADR-0021): each run must
//! reproduce its committed snapshot, stream hash included, on every tier-1
//! target, and each replays. A snapshot changes only with an intended engine
//! change, and the commit says why (CLAUDE.md).

mod support;

use support::{check_records, document, load, registry, run_model, t};
use taktsim_core::EngineConfig;
use taktsim_trace::golden::{self, Case};
use taktsim_trace::{ModelHash, Record, SystemHash, TraceHash};

/// The SHA-256 of every record's line of case `name`, by the code of M5
/// (ADR-0022), before any decision was written. A record of M5 keeps its line for
/// ever under this format, and no event moves (ADR-0023 §10).
const M5_EVENT_LINES: [(&str, &str); 6] = [
    (
        "m5_same_instant_chain",
        "3cd18211597ec93ac1effc46cacfc1daa6d9ddbd1f353a690123d366e7379d32",
    ),
    (
        "m5_chain",
        "2eaaed55391579195db144cd5e7028e9d0119007bfe20b479255c155fb3bb87a",
    ),
    (
        "m5_setup_mix",
        "f537f473597ad399fb628ddf75389e197b0889fa1546d0c798570631585ceb0c",
    ),
    (
        "m5_capacity2",
        "a0328ce33add50b8099e13ace9a0eb6f84ff761a6a291db18d7ca18bcb42d5d6",
    ),
    (
        "m5_branch_merge",
        "fc3c2befdab4046b877e7d5d1b527d25c7a8b821fcac0e43cdcfd8ebb5ed7156",
    ),
    (
        "m5_failures",
        "9b8bbbc6e4eeb04fe4273b1a3395c572aa3187af81337e6a44e11b1c96ed97f3",
    ),
];

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
    // Every record of every case explains itself (ADR-0023 §9).
    let run = run_model(&model, seed);
    check_records(&run);
    if let Some((_, pinned)) = M5_EVENT_LINES.iter().find(|(case, _)| *case == name) {
        let mut lines = String::new();
        for record in &run.records {
            // A decision is what M6 adds; everything else stays as it was.
            if !matches!(record, Record::Decision { .. }) {
                lines.push_str(&record.display(&run.names).to_string());
                lines.push('\n');
            }
        }
        assert_eq!(
            TraceHash::sha256(lines.as_bytes()).to_string(),
            *pinned,
            "{name}: an event line of M5 changed"
        );
    }
}

/// Every case of M5 passes through [`case`], which compares its lines.
#[test]
fn m5_event_lines_are_unchanged() {
    assert_eq!(M5_EVENT_LINES.len(), 6);
    chain();
    failures();
    setup_mix();
    capacity2();
    branch_merge();
    same_instant_chain();
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

/// Every word at once, with constant times that tie often: conditions, a split, a
/// pool, a rule on a Buffer's taker, due dates and priorities.
#[test]
fn m6_branch_merge() {
    let yaml = format!(
        "{}control:\n  dispatch: {{a: earliest_due_date, b: priority_class, c: minimum_setup}}\n",
        document(
            "1h",
            "  - {id: s1, type: Source, interarrival: {dist: exponential, mean: 20s}, variants: {red: 50%, blue: 50%}, due_in: {dist: uniform, min: 1h, max: 2h}, priority_mix: [{priority: 2, share: 25%}, {priority: 0, share: 75%}]}\n  \
             - {id: s2, type: Source, interarrival: 30s, variants: {green: 100%}, priority: 1}\n  \
             - {id: buf, type: Buffer, capacity: 6 parts, discipline: lifo}\n  \
             - {id: a, type: Machine, cycle_time: {dist: exponential, mean: 25s}, cycle_time_by_variant: {red: 20s}}\n  \
             - {id: b, type: Machine, cycle_time: 30s, mtbf: {dist: exponential, mean: 15min}, mttr: 1min}\n  \
             - {id: c, type: Machine, cycle_time: {dist: uniform, min: 10s, max: 20s}, setup: {per_variant: 30s, matrix: [{from: red, to: blue, time: 0s}]}}\n  \
             - {id: sink1, type: Sink}\n  - {id: sink2, type: Sink}",
            "  - {from: [s1, s2], to: buf}\n  \
             - {from: buf, to: a, when: \"variant == 'red'\"}\n  \
             - {from: buf, split: {a: 40%, b: 60%}}\n  \
             - {from: [a, b], to: c}\n  - {from: c, to: [sink1, sink2], choose: preferred}",
        )
    );
    case("m6_branch_merge", 61, "1h", &yaml);
}

/// A pool of each rule, with failures and a merge, so offers move and `Accept`s cross.
#[test]
fn m6_pool() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 4s}\n  \
         - {id: other, type: Source, interarrival: {dist: exponential, mean: 9s}}\n  \
         - {id: a, type: Machine, cycle_time: 13s}\n  \
         - {id: b, type: Machine, cycle_time: 22s, mtbf: {dist: exponential, mean: 10min}, mttr: 30s}\n  \
         - {id: c, type: Machine, cycle_time: 7s, capacity: 2 parts}\n  \
         - {id: buf, type: Buffer, capacity: 3 parts}\n  \
         - {id: d, type: Machine, cycle_time: 5s}\n  \
         - {id: e, type: Machine, cycle_time: 6s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: preferred}\n  \
         - {from: other, to: [c, b], choose: shortest_queue}\n  \
         - {from: [a, b, c], to: buf}\n  \
         - {from: buf, to: [d, e], choose: lowest_utilization}\n  - {from: [d, e], to: sink}",
    );
    case("m6_pool", 62, "1h", &yaml);
}

/// A rework loop with a return target that fails, and a rule on it.
#[test]
fn m6_rework() {
    let yaml = format!(
        "{}control:\n  dispatch: {{weld: fifo}}\n",
        document(
            "1h",
            "  - {id: source, type: Source}\n  \
             - {id: other, type: Source, interarrival: 12s}\n  \
             - {id: weld, type: Machine, cycle_time: {dist: exponential, mean: 6s}, mtbf: {dist: exponential, mean: 10min}, mttr: 40s}\n  \
             - {id: buf, type: Buffer, capacity: 2 parts}\n  \
             - {id: qc, type: Machine, cycle_time: 3s, capacity: 2 parts}\n  - {id: sink, type: Sink}",
            "  - {from: [source, other], to: weld}\n  - [weld, buf, qc]\n  \
             - {from: qc, split: {weld: 30%, sink: 70%}}",
        )
    );
    case("m6_rework", 63, "1h", &yaml);
}
