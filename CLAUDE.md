# CLAUDE.md

Simulation-as-code: a Rust library (with Python bindings, CLI and MCP
server) for discrete-event simulation of production systems. Licensed
MIT OR Apache-2.0 (ADR-0006).

The spec still uses the placeholder `simcode`; read it as `taktsim`
(crates `taktsim-*`, CLI `taktsim`), and its model header `simcode: "1.0"`
as `format: "1.0"` (ADR-0008).

## Sources of truth

- `docs/spec.md`: the specification. Read the relevant section before
  working on a feature.
- `docs/roadmap.md`: milestones and the acceptance criteria each proves.
  Work on the current milestone only.
- `docs/adr/`: decisions. Only the owner accepts an ADR, in one of two ways:
  - **Explicitly**, by saying so. Required for an ADR that changes the spec
    or an accepted ADR.
  - **By merging** the milestone PR that proposes it. Allowed only for an ADR
    that settles a spec *default* or a detail the milestone needs (e.g.
    ADR-0012, units). The ADR says `proposed (accepted when the Mn pull
    request is merged)`.

  Never mark an ADR `accepted` before the owner has done one of these; after
  a merge, record the status in a later commit. A milestone does not start
  while an ADR from an earlier milestone that it depends on is `proposed`.
- A tool or library the spec marks *default* is used only once an ADR
  accepts it. A number marked *initial target* is confirmed in an ADR with
  measurements.
- If code and spec disagree, stop and raise it; do not silently pick one.

## Design principles (spec §2)

Every change is checked against these. A PR that breaks one states the
exception explicitly in its description.

1. **Models are code.** A model is a text file under version control.
   Anything a person can do, an agent can do through the same API, and the
   other way round.
2. **Typed end to end.** Every model, parameter, event, result and tool call
   has a schema. Invalid input is rejected at load time with the field path
   and the fix, never during a run.
3. **Deterministic by default.** Same model, seed, engine version: same
   trace byte for byte, on every platform. Nondeterminism is opt-in and
   labelled. In practice:
   - no `HashMap`/`HashSet` iteration where order can reach a trace or
     result (use `BTreeMap`, `IndexMap`, or sort);
   - no wall-clock time, thread scheduling or environment in engine logic;
   - no platform math library for transcendental functions in sampling;
   - parallelism only across replications; one replication is single-threaded.
4. **Verifiable.** Reference models with analytical solutions, runtime
   invariants, error bounds. A result without an accuracy statement is not
   a result.
5. **Self-improving within limits.** Loops have stopping criteria, budgets
   and thresholds, and report what changed and why. Never silently rewrite a
   model; return new versions instead of overwriting.
6. **Open standards first** (JSON Schema, Parquet, OPC UA, MQTT, FMI,
   AutomationML).
7. **Small, composable core.** The engine knows nothing about machines.
   New domains are new component packages, not engine changes.

## Architecture rules

- `taktsim-core` has no I/O and depends only on `taktsim-units`. Every other
  crate depends on core, never the reverse.
- The Python bindings, CLI and MCP server are thin layers over the same
  public Rust API. No logic lives only in one client.
- No network code in engine crates. Adapters are opt-in Cargo features and
  read-only toward production systems: no code path writes to a controller.
- Models are data, never executed. Components are compiled in, never loaded
  from model files.
- No telemetry of any kind.

## Platforms and performance (ADR-0010)

It must run well on ordinary PCs, not only on the developer's Mac.

- Tier-1 targets: Windows x86-64, Linux x86-64 and ARM64, macOS ARM64.
  Code that works only on macOS, or only on one CPU architecture, is a bug.
- No `-C target-cpu=native` or other machine-specific flags in committed
  config. CPU-specific code only with runtime detection and a
  bit-identical scalar fallback. No `f64::mul_add` in engine or sampling
  code.
- Paths, line endings and file names must work on Windows (no hard-coded
  `/`, no case-only differences, no symlinks in the repo).
- Performance and memory claims are measured on the reference hardware or
  the CI proxy runner (ADR-0010). Never cite numbers from a developer's
  machine as evidence that a target is met.
- Keep memory within the targets: traces stream to disk, and parallel
  replications are limited by memory as well as cores.

## Rust rules (spec §7 Layer 1)

- Rust stable, MSRV declared in the workspace `Cargo.toml` and tested in CI.
- `#![forbid(unsafe_code)]` in every crate. An exception needs an ADR, an
  isolated module and Miri in CI.
- `clippy::pedantic` with `-D warnings`, configured once at the workspace
  level. Do not add `#[allow]` to get past a lint; fix the code. If an
  allow is truly needed, it is local and has a comment saying why.
- No `unwrap`, `expect` or `panic!` in library code (denied by clippy).
  Panics are only for violated internal invariants, through the single
  documented invariant-violation path (ADR-0007), never for bad input.
  Tests may use `unwrap`/`expect`.
- Units are types: quantities use the newtypes from `taktsim-units`; never
  pass a bare `f64` for a quantity across a function boundary.
- Component states are enums handled with exhaustive `match`, **no wildcard
  arms** (`_ =>`), so adding a state forces every handler to change.
- Errors are typed enums (`thiserror` by default), never strings. Each
  variant can produce a field path, what was found, what is allowed and one
  valid example.
- `#![deny(missing_docs)]` in every crate; every public item has a doc
  comment with a doc test.
- The Rust model types are the single source of truth. JSON Schema
  (`schemas/`) and Python stubs (`.pyi`) are generated from them and
  committed. Never hand-edit generated files; regenerate and commit them in
  the same change as the types.

## Python and TypeScript

- Python: `ruff`, `pyright --strict` against the generated stubs, `pytest`.
- Viewer (from v0.2): TypeScript `strict`, ESLint; types generated from the
  JSON Schema.

## Tests (spec §7 Layer 2)

Every change comes with tests. Required kinds, by default tool:

| Kind | Tool | Required for |
| --- | --- | --- |
| Unit | `cargo nextest` | every component and engine primitive |
| Property | `proptest` | invariants: conservation, bounds, ordering, determinism under replay |
| Analytical reference | `rstest` | M/M/1, M/M/c, tandem and serial lines vs closed form |
| Golden traces | `insta` | reference models, byte for byte per engine version |
| Schema contract | `cargo test`, `pytest` | every documented example accepted; every malformed one rejected with a path |
| Mutation | `cargo-mutants` | core (gate level per ADR-0007) |
| Performance | `criterion` locally, instruction counts in CI | > 10 % regression fails |
| Fuzz | `cargo-fuzz` | nightly: loader and engine never crash |

- Coverage floors: ≥ 90 % lines overall, 100 % branches in `taktsim-core`
  (`cargo-llvm-cov`). Coverage is a floor; the property and analytical
  tests are what give confidence.
- Never weaken, skip or delete a test, snapshot or gate to make CI pass. A
  golden trace changes only with an intended engine change, and the commit
  says why.
- Seeds in statistical tests are fixed when the test is written and never
  changed to make it pass.
- Runtime invariant checks are on by default; `--fast` exists only for
  calibration inner loops after one checked run.

## CI gates (spec §10)

Order, each a required check on `main` (steps for crates that do not exist
yet are added with the crate):

rustfmt → clippy → cargo-deny → unit and property tests → analytical and
golden tests → schema and stub freshness → coverage gate → Python binding
tests → WebAssembly build → viewer type check and tests → mutation (core) →
benchmarks → docs build.

CI runs on every tier-1 target (ADR-0010); prebuilt wheels for
the three latest Python versions, so `pip install` never needs a compiler.
Nightly: fuzzing, full calibration benchmark, `cargo audit`.

Before calling work done, run locally what CI runs:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
ci/check-crates.sh
cargo nextest run --workspace --exclude taktsim-python --all-features
cargo test --workspace --exclude taktsim-python --all-features --doc
```

Coverage, docs and Python commands are in `CONTRIBUTING.md`;
`.github/workflows/ci.yml` is the authority on exact commands.

Every gate change (lints, toolchain, MSRV, `deny.toml`, workflows, `ci/`)
carries the `gate-change` label and a reason in the PR description.

- **Loosening** a gate (removing or weakening a check, allowing a lint,
  lowering a threshold, a laxer `deny.toml`) is its own PR. Never loosen a
  gate in the same PR as the code that would fail the old one.
- **Tightening** a gate (a new check, a stricter setting) may go in the PR
  with the code it checks.

If unsure whether a change loosens or tightens, treat it as loosening.

## Commits and PRs

- Conventional Commits (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`,
  `chore:`, `perf:`, `ci:`); the changelog is generated from them.
- Every commit carries a DCO `Signed-off-by:` line (`git commit -s`).
- A commit that touches `taktsim-core` references the test that covers it.
- Commit or push only when asked. Never commit to `main` directly once CI
  exists; use a branch and a PR.
- A PR description lists any exception to the design principles above.

## Repository layout (spec §10)

```text
crates/   taktsim-units, -core, -trace, -model, -components, -stats,
          -calibrate, -adapters, -share, -wasm, -mcp, -cli (added per tier)
python/   PyO3 bindings, generated .pyi, pytest
viewer/   TypeScript app (v0.2)
benchmarks/  reference models, analytical cases, datasets
docs/     spec, roadmap, ADRs, tutorial
schemas/  committed JSON Schemas, one per format version
```

## Status

M0 and M1 done. M2 (engine core) in progress, in two PRs: M2a (random
streams and distributions), M2b (event queue and engine). ADR-0001 to
ADR-0012 accepted. `main` is protected: PRs only, rebase merge, all CI jobs
except mutants required.
