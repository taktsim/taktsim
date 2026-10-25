# ADR-0016: Benchmarks and the instruction-count regression gate

- Status: proposed (accepted when the Track C pull request is merged)
- Date: 2026-10-25
- Blocks: M2 (the benchmark gate of ADR-0007), M2b (engine benchmarks), M13
  (release benchmark report)
- Spec: §7 Layer 2 (Performance: "`criterion` locally, instruction counts in
  CI (`iai-callgrind`)"; "no regression > 10 % on reference models;
  instruction counts avoid noisy CI runners"), §9 Performance, §10 CI
  pipeline order; ADR-0007, ADR-0010, ADR-0011

## Context

ADR-0007 makes "> 10 % regression fails, measured as instruction counts on
Linux CI" a required gate from M2. ADR-0010 says absolute numbers come from
the reference hardware or the CI proxy runner (GitHub-hosted x86-64 Linux,
median of at least 10 runs), never from a developer's machine, and that
Valgrind runs on Linux only. ADR-0011 pins tool versions and actions by
commit SHA. The engine's event loop arrives in M2b; what exists now is
`taktsim_core::random` and `taktsim_units`.

Facts checked on 2026-10-25 (crates.io, the gungraun 0.20.0 guide and
source):

- `iai-callgrind` was renamed `gungraun`; its last release is 0.16.1
  (July 2025). `gungraun` 0.20.0 (2026-09-26, MSRV 1.88, MIT OR Apache-2.0)
  is current. It runs each benchmark in its own process under Callgrind
  and reports the instructions executed (`Ir`) in the benchmark function
  only; setup and teardown functions are not counted.
- The library and the `gungraun-runner` binary must have exactly the same
  version, or the run stops with an error ("When updating the gungraun
  library, you'll also need to update gungraun-runner and vice-versa",
  guide, Installation). `taiki-e/install-action` at the SHA pinned in
  `ci.yml` has `gungraun-runner` 0.20.0 in its manifest, with a SHA-256
  checksum per target.
- gungraun can compare against a saved baseline (`--save-baseline`,
  `--baseline`) and fail on a limit (`--callgrind-limits='ir=10%'`, exit
  code 3). It writes a `summary.json` per benchmark (schema v7 from 0.20)
  with the `Ir` count at `.profiles[].data.total.metrics.Ir.values.new`.
- `criterion` 0.8.2 (MSRV 1.86, MIT OR Apache-2.0) is current. Its default
  features pull in `plotters` and `rayon`; neither is needed.
- Ubuntu 24.04 ships Valgrind `1:3.22.0-0ubuntu3` in its release pocket,
  which is never changed; gungraun is tested with Valgrind 3.20 and later.

## Options

**Where the benchmarks live.**

1. `benches/` in each measured crate (`taktsim-core`, `taktsim-units`).
2. One unpublished crate, `crates/taktsim-bench`, that uses the engine
   crates' public API.

**How base and head are compared.**

1. gungraun's baselines: run the base with `--save-baseline=base`, the head
   with `--baseline=base --callgrind-limits='ir=10%'`.
2. Two independent runs, base and head, each into its own directory, then a
   small script that compares the two sets of `summary.json`.
3. Store a baseline from `main` (an artifact or a branch) and compare each
   pull request with it.

**Which commits.** The pull request's head commit, or the merge commit
GitHub builds for the pull request (what `ci.yml` tests).

## Trade-offs

- Benchmarks inside `taktsim-core` would put `criterion`, `gungraun` and
  their dependencies into core's manifest, which ADR-0013 keeps minimal,
  and every benchmark change would be a core change (mutation job, "core
  commit names a test"). In-crate benchmarks also see inlining that users
  across a crate boundary do not get (gungraun guide, best practices).
  A separate crate only reaches the public API, which is what users call.
- gungraun's baselines need base and head in one output directory with the
  same gungraun version, and they do not report a benchmark that was
  removed. Version bumps of gungraun (Dependabot) would break the gate
  exactly when it is needed, and a removed benchmark would vanish silently.
- A stored baseline from `main` is measured on a different runner, on a
  different day, perhaps with a different image; that reintroduces the
  machine noise instruction counts are meant to avoid.
- The head commit alone ignores what landed on `main` since the branch was
  cut; the merge commit's first parent is exactly the base it is compared
  with, so the difference between the two runs is the pull request's
  change and nothing else.

## Decision

**Tools.** `gungraun` 0.20.0 for instruction counts in CI, `criterion`
0.8.2 (`default-features = false`, `cargo_bench_support` only) for
wall-clock times on a developer's machine. Both are dev-dependencies of
`taktsim-bench` only. `gungraun-runner` is installed with exactly the
version `Cargo.lock` resolves for `gungraun` (`ci/bench-runner-version.sh`),
by `taiki-e/install-action` with checksums and `fallback: none`: the
version is pinned by the lockfile, so a Dependabot bump of the library also
bumps the runner, without editing the workflow, and a version missing from
the action's manifest fails instead of being built from an unchecked
source. Valgrind is installed from apt at `1:3.22.0-0ubuntu3` and its
version is printed in the job summary.

**Where.** `crates/taktsim-bench`, `publish = false`, under the workspace
lints and `ci/check-crates.sh`:

- `src/`: the workloads, shared by both harnesses so they measure the same
  work. Each is a fixed amount of work with a fixed seed, folds its results
  into one value (so nothing is optimised away), and is tested in
  `tests/workloads.rs` against theory or hand arithmetic, which also proves
  the constant inputs never fail.
- `benches/instructions_<area>.rs`: gungraun benchmarks, gated in CI.
- `benches/wall_<area>.rs`: criterion benchmarks, run locally.
- Today: `Stream::next_u64`, `next_open01`, `next_standard_normal` (1,000
  draws each), `DurationDistribution::sample` for each distribution of
  ADR-0003 plus a gamma with shape below 1 (its second code path), 1,000
  samples each; parsing and formatting of durations, counts, rates and
  percentages as written in models.

`ci/bench-run.sh --strict` fails on the head if a bench target is in
another package or is named neither `instructions_*` nor `wall_*`, so a
benchmark cannot drop out of the gate by its name or place.

**Base against head, same runner.** The `bench` workflow
(`.github/workflows/bench.yml`, job `benchmarks (instruction counts, > 10 %
regression fails)`) runs on every pull request on `ubuntu-24.04`, the CI
proxy runner of ADR-0010:

1. Runs the gate's own tests (`ci/bench-compare-test.sh`) first.
2. Checks out the merge commit with its parents; the base is `HEAD^1`.
3. Measures the base, then the head, one after the other in the same job,
   each with its own `gungraun-runner` version and output directory
   (`ci/bench-run.sh`). Both are measured by the head's scripts.
4. Compares them (`ci/bench-compare.sh`), writes a table (benchmark, base,
   head, change, result) to the job summary, and uploads the summaries as
   the `instruction-counts` artifact.

**The rule** (ADR-0007): a benchmark fails if its head count is more than
10 % above its base count, `head × 100 > base × 110`, computed exactly
(counts up to 2^53 / 110 ≈ 8 × 10^13; larger ones are rejected). Exactly
+10 % passes. Each benchmark is judged on its own; there is no average
that could hide one regression behind improvements. Only `Ir` is gated;
cache and cycle estimates are simulated and are not part of ADR-0007.

- A benchmark only in the head is reported as **new** and passes; only in
  the base, **removed**, and passes. A base from before this ADR has no
  benchmarks, so everything is new.
- Input the gate cannot trust fails (exit 2): an unreadable summary, an
  unsupported schema version, a missing or non-integer count, a benchmark
  measured twice, or a head without benchmarks. A gate that cannot read its
  input never reports success.

**Why it does not flake.** Callgrind counts the instructions the program
executes, not time, so runner speed, load and frequency scaling do not
enter. The workloads are deterministic (fixed seeds, no hash maps, no
threads), gungraun clears the environment and disables address-space
randomisation, and base and head are built by the same toolchain (unless
the pull request changes it) and measured under the same Valgrind in the
same job. An unchanged benchmark therefore gives the same count on both
sides; the 10 % threshold only has to absorb real code-generation changes,
such as inlining decisions moved by an unrelated edit.

**Gated versus measured on the CI proxy (ADR-0010).** The gate makes no
speed claim: it compares two counts on one machine. Absolute numbers
(events per second against the §9 targets, peak memory, the README
example time) are measured separately on the CI proxy runner, median of at
least 10 runs, with the engine and the reference line model (M2b and
later); the criterion benchmarks here are the starting point for that
measurement but are not run in CI by this ADR. Numbers from a developer's
machine, including any criterion output on a Mac, are never evidence that
a target is met.

## How to benchmark

For `CONTRIBUTING.md`.

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
  `ci/bench-compare.sh target/ic-before target/ic-after`. On macOS and
  Windows, rely on the CI job.
- **In CI:** the `bench` workflow runs on every pull request. Its job
  summary has the table; a regression above 10 % fails the check. The
  `instruction-counts` artifact has every `summary.json`.
- **Adding a benchmark** (no workflow change):
  1. Add the workload to `crates/taktsim-bench/src/` (fixed inputs and
     seed, results folded into one value, a doc example) and test it in
     `tests/workloads.rs`, with expected values from theory or by hand.
  2. Add a `#[library_benchmark]` to `benches/instructions_<area>.rs`, or a
     new file of that name; put setup in a `setup` function so it is not
     counted, and use `support::built` / `support::check` so a failing
     input stops the run instead of measuring less work.
  3. Add the same workload to `benches/wall_<area>.rs`.
  4. Register a new file in `crates/taktsim-bench/Cargo.toml` as
     `[[bench]] name = "…" harness = false`.

  A renamed benchmark (file, group, function or id) shows up as one
  removed and one new benchmark, so it is not compared in that pull
  request: rename in a pull request of its own, without code changes.

## Verification

The full job on GitHub's `ubuntu-24.04` runner (x86-64), with Valgrind
3.22.0 and gungraun-runner 0.20.0, on 2026-10-25:

| Run | Base → head | Result |
| --- | --- | --- |
| [40239198298](https://github.com/taktsim/taktsim/actions/runs/40239198298) | `main` (no benchmarks) → this pull request (#6) | 21 benchmarks, all new; passed in 1 min 20 s |
| [40239440977](https://github.com/taktsim/taktsim/actions/runs/40239440977) | #6 → #6 plus a changed comment in `taktsim-units` (scratch #7) | all 21 counts identical on base and head, and identical to the run above, which ran on another runner instance; passed in 1 min 39 s |
| [40239650506](https://github.com/taktsim/taktsim/actions/runs/40239650506) | #6 → #6 plus `SimDuration::from_str` splitting its input twice (scratch #7) | `parse_duration` 3,080 → 4,784 instructions (+55.32 %), reported as a regression, job failed; the other 20 identical |

Pull request #7 was a scratch pull request and was closed without merging.
The comparison script's own tests (`ci/bench-compare-test.sh`, 30 cases,
one on a summary from the first run) fail for every non-equivalent mutant
of the rule tried by hand: thresholds 9 and 11, `>=` for `>`, a ratio
instead of the integer comparison, the old instead of the new count, and
each dropped input check.

## Consequences

- Every pull request builds the benchmarks twice in release mode (thin
  LTO, one codegen unit, as shipped) and runs them under Callgrind; about
  1.5 minutes today.
- The gate covers the random streams, the distributions and unit parsing
  today. Spec §7 asks for "no regression > 10 % on reference models"; the
  event-queue benchmark and the reference line model join with M2b by
  adding `benches/instructions_engine.rs` and its workloads.
- The job is a separate workflow so that it could be added without editing
  `ci.yml` while other pull requests change it. It does not wait for the
  `lint` job as the jobs in `ci.yml` do; it can move into `ci.yml` with
  `needs: lint` later without changing what it checks.
- Making it a required check on `main` is a branch-ruleset change for the
  owner (check name: `benchmarks (instruction counts, > 10 % regression
  fails)`).
- A new gungraun summary schema fails the gate until
  `ci/bench-compare.sh` is checked against it and its version added: the
  gate fails closed.
- Two local `expect`s of lints in the benchmark binaries, each with its
  reason: `missing_docs` (gungraun's macros generate undocumented public
  items) and `clippy::expect_used` in `benches/support/mod.rs` (a constant
  input that fails must stop the run; setup functions cannot return an
  error).

## Questions for the owner

1. **An intended regression.** Some changes should cost instructions, e.g.
   an invariant check. The gate has no override, on purpose. Proposed: the
   pull request explains the regression and the owner merges it past the
   failing check (a ruleset bypass), rather than a label that anyone with
   triage rights could set. Is that how you want it handled?
