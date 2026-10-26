# Contributing to taktsim

Thanks for helping. This page is the contract every pull request is checked
against. The rules come from [the specification](docs/spec.md) (sections 2, 7
and 10) and the [architecture decisions](docs/adr/).

## Before you start

- Read the design principles in [CLAUDE.md](CLAUDE.md#design-principles-spec-2).
  They apply to people and agents alike.
- Work on something in the current milestone of the [roadmap](docs/roadmap.md),
  or open an issue first.
- New components and adapters start as an issue from the matching template. A
  component proposal must list its invariants.

## Setting up a machine

`rust-toolchain.toml` pins the toolchain, and rustup installs it on first
use. Also install the MSRV toolchain (`workspace.package.rust-version` in
`Cargo.toml`), the pinned nightly with `llvm-tools` (for coverage and
`compile_fail` doc tests; its date is in `.github/workflows/ci.yml`), and
the CI tools in the versions `ci.yml` installs:

```sh
cargo install --locked cargo-nextest@0.9.149 cargo-deny@0.20.2 \
  cargo-llvm-cov@0.9.1 cargo-mutants@27.1.0
```

The Python commands below create their own virtual environment.

## Every commit

- **Conventional Commits** subject: `feat(core): add event queue`,
  `fix(model): reject duplicate ids`, `docs: ...`, `test: ...`, `ci: ...`.
  The changelog is generated from them.
- **DCO sign-off**: commit with `git commit -s`. The `Signed-off-by:` line
  certifies the [Developer Certificate of Origin](https://developercertificate.org/).
- A commit that changes `crates/taktsim-core` also adds or changes a test, or
  names the covering test in a `Test: <name>` trailer.

## Every pull request

CI runs these gates; all must be green:

| Gate | What it checks |
| --- | --- |
| lint | `rustfmt`, `clippy` (pedantic, `-D warnings`, no `unwrap`/`expect`/`panic!` in library code, no wildcard arms on enums), `cargo-deny` (licenses, advisories, duplicates), crate rules (`forbid(unsafe_code)`, `deny(missing_docs)`) |
| test | unit, property, golden-trace and doc tests on Windows x86-64, Linux x86-64 and ARM64, macOS ARM64 |
| trace hashes | every golden-trace case has the same stream and file SHA-256 on all four targets (ADR-0021) |
| msrv | the workspace builds with the minimum supported Rust version |
| coverage | ≥ 90 % lines overall, 100 % branches in `taktsim-core` |
| python | pytest, ruff, pyright strict (warnings fail) on Python 3.13, 3.14, 3.15 |
| docs | rustdoc without warnings; a code example on every public item; `compile_fail` examples fail with the stated error code |
| commits | Conventional Commits, DCO, core changes come with tests |
| gates | changes to lints, CI, toolchain or `deny.toml` carry the `gate-change` label and a reason in the description |
| benchmarks | instruction counts of `taktsim-bench` against the base branch; a rise above 10 % or an unlisted removed benchmark fails (ADR-0016) |
| mutants | mutation testing of changed core code; advisory until v1.0. Timeouts count as caught; known equivalent mutants are listed with reasons in `.cargo/mutants.toml` (adding one is a gate change) |

Run the same checks locally before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny check
ci/check-crates.sh
cargo nextest run --workspace --exclude taktsim-python --all-features
cargo test --workspace --exclude taktsim-python --all-features --doc
# coverage (needs the pinned nightly from .github/workflows/ci.yml):
cargo +nightly-2026-10-24 llvm-cov nextest --workspace --exclude taktsim-python \
  --all-features --branch \
  --json --summary-only --output-path target/coverage.json
ci/coverage-gate.sh target/coverage.json
# compile_fail doc tests with their error codes checked (nightly only):
cargo +nightly-2026-10-24 test --workspace --exclude taktsim-python --all-features --doc
# Python (from python/):
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop
.venv/bin/ruff check . && .venv/bin/ruff format --check .
.venv/bin/pyright --warnings --pythonpath .venv/bin/python
.venv/bin/pytest -q
```

Never weaken a test, snapshot or gate to make CI pass. Every gate change
carries the `gate-change` label and a reason. Loosening a gate is its own PR;
tightening one (a new or stricter check) may come with the code it checks.

If your PR breaks one of the design principles, say so in its description and
explain why; the PR template asks.

## Golden traces

Golden-trace tests (ADR-0021) run a model, check that its trace replays,
and compare a text snapshot, with the SHA-256 of every record, against
`tests/snapshots/*.snap`. A snapshot changes only with an intended engine
or model change, and the commit says why. To add a case, call
`taktsim_trace::golden::record` (feature `golden`) in a test and pass the
result to `insta::assert_snapshot!`; write a new or changed snapshot with
`INSTA_UPDATE=always cargo nextest run -p <crate> --all-features`, then
read the diff before committing it. CI also collects every case's hashes
from every target (`TAKTSIM_TRACE_HASHES`) and fails if one differs;
`ci/trace-memory.sh` measures the memory of a 10 M event trace (Linux).

## Benchmarks

- **Wall-clock, locally (any OS):**
  `cargo bench -p taktsim-bench --bench 'wall_*'`. Compare two states of
  your code with `-- --save-baseline before` on the first and
  `-- --baseline before` on the second. Use it to tune; do not cite the
  numbers as evidence (ADR-0010).
- **Instruction counts, locally (Linux with Valgrind only):** install the
  runner once,
  `cargo install --locked gungraun-runner --version "$(ci/bench-runner-version.sh)"`,
  then on each of two commits run `ci/bench-run.sh target/ic-before` and
  `ci/bench-run.sh target/ic-after`, and compare with
  `ci/bench-compare.sh --removed ci/bench-removed.txt target/ic-before target/ic-after`.
  On macOS and Windows, rely on the CI job. The gate's own tests run
  anywhere with bash and jq: `ci/bench-test.sh`.
- **In CI:** the `bench` workflow runs on every pull request. Its job
  summary has the table; a regression above 10 % or an unlisted removal
  fails the check. The `instruction-counts` artifact has every
  `summary.json`.
- **Adding a benchmark** (no workflow change; `crates/taktsim-bench` is a
  gate path, so the pull request needs the `gate-change` label):
  1. Add the workload to `crates/taktsim-bench/src/` (fixed inputs and
     seed, results folded into one value, a doc example) and test it in
     `tests/workloads.rs`, with expected values from theory or by hand.
  2. Add a `#[library_benchmark]` to `benches/instructions_<area>.rs`, or a
     new file of that name, inside its private `benchmarks` module; put
     setup in a `setup` function so it is not counted, return the inputs
     with the result to `teardown = support::check`, and use
     `support::built` in setup, so a failing input stops the run instead
     of measuring less work.
  3. Add the same workload to `benches/wall_<area>.rs`.
  4. Register a new file in `crates/taktsim-bench/Cargo.toml` as
     `[[bench]] name = "…" harness = false`.

  A renamed benchmark (file, group, function or id) shows up as one
  removed and one new benchmark: add the old name to
  `ci/bench-removed.txt`, and rename in a pull request of its own,
  without code changes, since the renamed one is not compared there.
  Renaming every benchmark at once fails (nothing compared); do it in two
  steps.

## Decisions

Larger choices are recorded as [ADRs](docs/adr/). Propose one in a PR with
status `proposed`; the project owner accepts it, either explicitly or, for an
ADR that settles a detail of the milestone in the same PR, by merging that
PR.

## License

Contributions are dual licensed under MIT OR Apache-2.0, without additional
terms (see [README](README.md#license)).
