# Roadmap

Derived from [spec.md](spec.md) section 11. A tier ships only when every
acceptance criterion in its row is demonstrated by an automated test or a
benchmark report. Milestones are ordered: each builds only on the ones
before it, and each ends with green CI on every tier-1 platform
(ADR-0010: Windows x86-64, Linux x86-64 and ARM64, macOS ARM64).

Status: ADR-0001 to ADR-0010 accepted 2026-10-25. A milestone does not
start while an ADR it depends on is proposed.

## v0.1 Engine

### v0.1 acceptance criteria → where they are proven

| # | Criterion (spec §11) | Proven in |
| --- | --- | --- |
| A1 | Analytical reference suite passes (M/M/1, M/M/c, 3-station serial line) | M10 |
| A2 | 95 % CIs contain the true value in 93–97 % of 1,000 seeded experiments | M8, re-run in M10 |
| A3 | Warm-up detection recovers a known transient | M9 |
| A4 | Deterministic replay test passes on all three OSes (checked on every tier-1 target, x86-64 and ARM64) | M3 (mechanism), M7 (real models) |
| A5 | Schema and stubs committed and fresh | M4 (schema), M12 (stubs) |
| A6 | 100 % branch coverage in `taktsim-core` | M0 (gate), M2 (reached) |
| A7 | clippy pedantic clean, no `unsafe` | M0 (gate), every milestone |
| A8 | README example runs in < 1 s from Rust and from Python | M13 |

---

### M0 — Workspace skeleton and CI gates

Status: in review (pull request). Toolchain and tooling: ADR-0011.

No domain code. The gates exist before there is anything to gate, so no
later milestone can lower them.

- Cargo workspace with the v0.1 crates only: `taktsim-units`, `taktsim-core`,
  `taktsim-trace`, `taktsim-model`, `taktsim-components`, `taktsim-stats`, `taktsim-adapters`
  (feature `csv`), `taktsim-cli`; `python/` package. Later crates are added
  in the tier that needs them.
- Workspace-level lints: `forbid(unsafe_code)`, `deny(missing_docs)`,
  clippy `pedantic`, `unwrap_used`, `expect_used`, `panic` denied
  (exception path: see CLAUDE.md), `-D warnings`.
- Pinned MSRV in `rust-toolchain.toml` / `rust-version`, MSRV job in CI.
- CI in spec §10 order, minus steps for crates that do not exist yet (WASM,
  viewer, benchmarks from M2): rustfmt → clippy → cargo-deny → nextest →
  coverage gate → Python (pytest, ruff, pyright strict on 3.13–3.15) → docs
  build with a doc example on every public item. Schema and stub freshness
  checks arrive with M4 and M12; mutation testing of changed core code runs
  on every PR as advisory.
- Matrix: the tier-1 targets from ADR-0010, all must pass. No `target-cpu=native` anywhere in committed config.
- DCO sign-off check on every commit (ADR-0006).
- Conventional Commits check; `git-cliff` config; "core commit references a
  test" check; `gate-change` label check and CODEOWNERS for gate files;
  actions pinned by SHA with Dependabot.
- `LICENSE-MIT`/`LICENSE-APACHE` (ADR-0006, already added), `README`, `CONTRIBUTING` (CI contract),
  `CODE_OF_CONDUCT`, `SECURITY`, issue templates (bug, component proposal
  with invariants, adapter proposal), `docs/adr/`.

**Proves:** A6 and A7 as gates. **Exit:** CI green on an empty workspace,
and a deliberately broken branch (an `unwrap`, an `unsafe` block, an
uncovered branch in core) fails the matching gate.
**Depends on:** ADR-0006, ADR-0007, ADR-0008, ADR-0010 (all accepted).

### M1 — Units (`taktsim-units`)

- Quantity newtypes per dimension: time, count (`parts`), rate, length,
  dimensionless ratio (`%`). Further dimensions (`kg`, `l`, `m`) only as
  types, used from v0.2.
- Parser for `42s`, `12min`, `6h`, `8h`, `20 parts`, `2%`; `Display` that
  round-trips.
- Simulation time representation per ADR-0001.

**Proves:** spec §7 "units are types". **Tests:** proptest parse ↔ display
round-trip; compile-fail tests (`trybuild`) showing `Duration + Rate` does
not compile; error messages for unknown units name the allowed ones.
**Depends on:** ADR-0001 (time representation).

### M2 — Engine core (`taktsim-core`)

- Event queue per ADR-0001, with deterministic tie-breaking and
  cancellation (needed for failures that interrupt a cycle).
- Simulation clock, run loop, budget and cancellation hook (synchronous;
  the async boundary is a v0.2 ADR).
- Counter-based RNG, one stream per component id, so adding a component
  does not shift the others. Distribution sampling with platform-independent
  math (see "Decisions not in §15" below).
- `Component` trait: typed parameters, state machine over engine events,
  invariant list. Invariant runner and global conservation check at every
  event; violation aborts with the event trace that led to it.
- Trace events emitted through a sink trait; core does no I/O.
- Event-queue benchmark on synthetic workloads; ADR-0001 updated with
  numbers.

**Proves:** A6 reached (100 % branch coverage in core). **Tests:** unit
tests per primitive; proptest: queue pops in non-decreasing time with
stable tie order, same seed → same event sequence, adding an unrelated
component leaves other streams unchanged; conservation violation is
detected and reported.
**Depends on:** ADR-0001.

### M3 — Trace format and replay

- Trace writer and reader per ADR-0004 in a new `taktsim-trace` crate,
  streaming to disk (never fully in memory).
- 10 M synthetic events: peak memory and file size recorded on the CI
  proxy runner (ADR-0010).
- Replay mode: re-execute from a trace and assert identical state at every
  event.
- Golden-trace harness (`insta`) and a cross-platform job that runs the
  reference models on every tier-1 target and compares trace hashes.

**Proves:** A4 mechanism. **Tests:** replay of a toy model; golden trace
snapshot; the cross-platform hash job fails if one target differs.
**Depends on:** ADR-0004.

### M4 — Model format and loader (`taktsim-model`)

- Model document types: header (`format: "1.0"`, ADR-0008), `model`, `experiment`,
  `components`, `flow` (chain shorthand plus edge list with branch, merge
  and loop-back), `observe`.
- Loader per ADR-0002: strict parsing, unknown keys rejected, id uniqueness
  and reference checks, unit checks, `include:` with content hashing, model
  hash.
- Distributions per ADR-0003.
- Error type: field path, what was found, what is allowed, one valid
  example.
- JSON Schema generated from the Rust types, committed under `schemas/`,
  freshness check in CI.

**Proves:** A5 (schema part); principle 2 (rejection at load time).
**Tests:** every documented example in the spec and docs loads; a corpus
of malformed models each fails with the expected path; proptest: valid
document → normalize → hash is stable, and YAML vs JSON forms of the same
model have the same hash; schema freshness check fails on a stale schema.
**Depends on:** ADR-0002, ADR-0003.

### M5 — Components: Source, Machine, Buffer, Sink (`taktsim-components`)

- Source: arrival distribution, variant mix, batch size.
- Machine: cycle time, MTBF, MTTR, setup matrix by variant, capacity;
  states idle / busy / blocked / starved / down / setup as an enum with
  exhaustive matches.
- Buffer: capacity, discipline FIFO / LIFO / priority.
- Sink.
- Compile-time registration; schema generator picks components up
  automatically.
- Runtime invariants from the spec §4 component table for these four.

**Proves:** per-component invariants. **Tests:** unit tests per state
transition; proptest on generated chain models: conservation, `0 ≤ fill ≤
capacity`, never more than `capacity` units in process; schema contains
every registered component.

### M6 — Routing and control

- Expression language per ADR-0009 for edge conditions: parser, type
  checker, evaluator.
- Edge conditions, split ratios, resource pools with choice rules (first
  free, shortest queue, lowest utilization, preferred with fallback).
- Dispatching rules as typed enums: FIFO, EDD, SPT, minimum setup,
  priority class.
- Every control decision written to the trace with the rule and the
  candidates.

**Proves:** routing part of the v0.1 scope. **Tests:** expression
grammar exhaustively tested (every production, every type error); proptest
that split ratios converge to their declared shares; each dispatching rule
picks the expected unit on hand-built queues; trace contains a decision
record for every routing choice.
**Depends on:** ADR-0009. EDD and priority rules need due dates and
priorities before order lists exist (v0.2). In v0.1 a Source can assign
them: an optional `due_in` (duration or distribution from creation) and a
priority-class mix. Units without a due date sort after all units with
one, then FIFO. Order lists replace this in v0.2 without breaking it.

### M7 — CLI `validate` and `run` (`taktsim-cli`), first end-to-end runs

- `taktsim validate <model>`: machine-readable errors (JSON) with
  `--format json`, human-readable rendering by default; both from the same
  typed error.
- `taktsim run <model> --seed … --horizon …`: results and trace reference
  with provenance (model hash, engine version, seed list, platform).
- Reference models in `benchmarks/` (chain, branch/merge, rework loop,
  pool).

**Proves:** A4 on real models. **Tests:** CLI snapshot tests; golden
traces for every reference model on every tier-1 target; replay of every
reference model.

### M8 — Statistics: KPIs and terminating experiments (`taktsim-stats`)

- KPIs: throughput (per product), state shares (per resource), WIP
  (time-weighted, per buffer and system), lead time per unit as a
  distribution.
- Independent replications in parallel (`rayon`), each single-threaded.
- Confidence intervals from replications; sequential replication control
  to a declared relative precision within the budget; report states the
  achieved half-width and whether precision was reached.

**Proves:** A2. **Tests:** KPIs against hand-computed traces; CI coverage
test with 1,000 seeded experiments on an analytical case. Because seeds
are fixed, this test is deterministic, not flaky.

### M9 — Steady-state experiments and warm-up

- Loader rejects steady-state experiments without a warm-up rule.
- Fixed warm-up and MSER-5 detection; detected length reported; statistics
  exclude it.
- Batch means with the autocorrelation check: deferred unless you want it
  in v0.1 (the spec allows, not requires, it).

**Proves:** A3. **Tests:** a model with a known injected transient; MSER-5
recovers its end within a declared tolerance; loader test for the missing
warm-up rule.

### M10 — Analytical reference suite

- M/M/1, M/M/c, 3-station serial line with closed-form throughput and WIP
  (rstest tables over load levels).
- Simulation result within the CI of theory; CI coverage test (A2) re-run
  on these cases.

**Proves:** A1, A2.

### M11 — CSV adapter (`taktsim-adapters`, feature `csv`)

- Results and traces exported as CSV (spec §8, "Out" v0.1).
- CSV input read into the internal observation table (`timestamp`,
  `entity`, `signal`, `value`, `unit`), timestamps in UTC with source zone
  as metadata. Held in memory in v0.1; Parquet storage of the table comes
  with the Parquet adapter in v0.2. Nothing consumes observations until
  `compare` in v0.2.
- Recorded fixtures; no hardware.

**Proves:** CSV adapter scope. **Tests:** fixture round-trips; malformed
rows rejected with row and column.

### M12 — Python bindings for `validate` and `run`

- PyO3 + maturin, abi3 wheels for every tier-1 target, so
  `pip install` never needs a compiler, API shape per ADR-0005.
- `.pyi` stubs generated and committed; freshness check in CI; `pyright`
  strict and `ruff` in CI.
- `pytest` schema-contract suite: every documented example accepted, every
  malformed example rejected with a path-level error, same results as the
  Rust API for the same model and seed.

**Proves:** A5 (stubs part). **Depends on:** ADR-0005.

### M13 — v0.1 release

- README 30-second example; CI test that times it from Rust and from
  Python (< 1 s) on the slowest tier-1 runner.
- Performance baseline on the reference line model (events/s with checks
  on and off, peak memory), measured on the CI proxy runner per ADR-0010,
  never on a developer Mac; ADR-0007 updated with the measurements.
- 1,000-component model completes on a 2-core, 4 GB runner without
  swapping (ADR-0010 minimum machine).
- `cargo-mutants` run on core as advisory (per ADR-0007).
- Release dry run: tag → crates.io, PyPI trusted publishing, docs
  (mdBook + rustdoc), changelog, benchmark report on the GitHub release.
- Acceptance checklist: each of A1–A8 linked to its test or report.

**Proves:** A8; tier acceptance.

### Decisions not in §15 that v0.1 also needs

The spec marks these as *defaults* to confirm in an ADR. They will be
written at the start of the milestone that uses them:

- MSRV value (M0).
- Coverage tooling: branch coverage in `cargo-llvm-cov` needs a nightly
  toolchain, while the project targets stable (M0, covered in ADR-0007).
- RNG algorithm and platform-independent float math (M2): the spec lists
  "Philox or PCG64" as counter-based, but only Philox is counter-based;
  and `f64::ln`/`exp` call the platform math library, which can differ
  between OSes, so sampling must use a pure-Rust implementation (e.g. the
  `libm` crate) to meet A4 on both x86-64 and ARM64.
- Units implementation: `uom` vs own newtypes (M1).
- YAML parser crate (M4, inside ADR-0002).
- Component registry (`inventory`) and schema generator (`schemars`)
  (M4/M5).
- Python stub generator (M12, inside ADR-0005).

---

## Later tiers (outline)

- **v0.2 Agent.** MCP server (`schema.get`, `model.validate`,
  `model.render`, `run`, `trace.query`, `compare`); remaining v1 components;
  materials, BOMs and lineage; order lists; release control; OEE and other
  KPIs; Parquet, OPC UA, MQTT adapters; WASM engine; first viewer (line and
  station levels, state glow, live reload). Needs ADRs: async boundary,
  viewer rendering stack, performance targets confirmed on the physical
  reference laptop (ADR-0010).
- **v0.3 Calibrate.** Observation format, metrics, `calibrate`,
  certificates, data-quality report, sharing client, parameter-library
  priors. Needs ADRs: optimizers, benchmark data, parameter library.
- **v0.4 Optimize.** `optimize`, `experiment`, `explain`, Operator, common
  random numbers, viewer playback and scrubbing, comparison views,
  parameter write-back.
- **v0.5 Plant.** Transport resources, maintenance, energy and cost,
  hierarchical models and scenarios, result charts, semantic zoom,
  GOVERNANCE.
- **v1.0 Stable.** Format 1.0 frozen, migrations, AutomationML import,
  complete docs, mutation gate on core, two external component
  contributions.
