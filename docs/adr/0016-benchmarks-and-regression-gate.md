# ADR-0016: Benchmarks and the instruction-count regression gate

- Status: accepted (2026-10-26, by the owner, by merging the Track C pull request #12)
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
  `ci.yml` has `gungraun-runner` up to 0.20.0 in its manifest, with a
  SHA-256 checksum per target. A version released after that SHA is not in
  it; the action then stops (`fallback: none`) or builds it with `cargo
  install --locked` (`fallback: cargo-install`; read in its `main.sh` at
  that SHA).
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
`taktsim-bench` only (criterion behind a feature would be a normal
dependency, and its `syn` 2 next to the workspace's `syn` 3 fails
`cargo deny`'s duplicate check, which exempts only dev-dependencies).
`gungraun-runner` is installed with exactly the version `Cargo.lock`
resolves for `gungraun` (`ci/bench-runner-version.sh`), by
`taiki-e/install-action`: a version in the action's manifest comes as a
checksummed release binary; a newer one is built from crates.io with
`cargo install --locked` (`fallback: cargo-install`), whose sources cargo
checks against the registry's checksums. The version is pinned by the
lockfile, so a Dependabot bump of the library also bumps the runner
without editing the workflow, and base and head each get their own
version. Valgrind is installed from apt at `1:3.22.0-0ubuntu3` and its
version is printed in the job summary. The scripts need bash and jq 1.6 or
later (`ci/bench-compare.sh` checks the version).

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

`ci/bench-targets.sh --strict` fails on the head if a bench target is in
another package or is named neither `instructions_*` nor `wall_*`, so a
benchmark cannot drop out of the gate by its name or place. The crate is a
gate path (`ci/check-gates.sh`, `CODEOWNERS`): what the gate measures
changes only with the `gate-change` label and the owner's review, like the
gate itself.

The gungraun benchmarks sit in a private module of each bench file: the
macros generate public items without doc comments, which `missing_docs`
rejects at the crate root, and gungraun refuses any other attribute on a
benchmark function (tried: a lint `expect` and a doc comment both fail
with "Invalid attribute"; on the group macro the attribute is ignored). In
a private module the items are not exported, so no lint exception is
needed, and the benchmark names do not change (gungraun builds them from
the file, the group and the function). Each benchmark returns its inputs
to its teardown, so freeing them is not counted.

**Base against head, same runner.** The `bench` workflow
(`.github/workflows/bench.yml`, job `benchmarks (instruction counts, > 10 %
regression fails)`) runs on every pull request on `ubuntu-24.04`, the CI
proxy runner of ADR-0010:

1. Checks out the merge commit with its parents; the base is `HEAD^1`.
2. Takes the gate (`ci/bench-*.sh`, its threshold and fixtures) from the
   base when the base has `ci/bench-compare.sh`, and from the head only
   when it does not (the pull request that introduces the gate). A pull
   request that changes the gate is judged by the gate it changes; the
   change applies from the next pull request on. This makes CLAUDE.md's
   "never loosen a gate in the same PR as the code that would fail the old
   one" mechanical. The workflow file itself comes from the head, as for
   every `pull_request` workflow; it is a gate file under the same label
   and review. Because a later head's workflow calls the base's scripts,
   their arguments and exit codes stay stable.
3. Runs the tests of the gate in use, and of the head's scripts if they
   differ (`ci/bench-test.sh`).
4. Measures the base, then the head, one after the other in the same job,
   each with its own `gungraun-runner` version and output directory
   (`ci/bench-run.sh`).
5. Compares them (`ci/bench-compare.sh`, with the head's
   `ci/bench-removed.txt`), writes a table (benchmark, base, head, change,
   result) to the job summary, and uploads the summaries as the
   `instruction-counts` artifact.

**The rule** (ADR-0007): a benchmark fails if its head count is more than
10 % above its base count, `head × 100 > base × 110`, computed exactly
(counts up to 2^53 / 110 ≈ 8 × 10^13; larger ones are rejected). Exactly
+10 % passes. Each benchmark is judged on its own; there is no average
that could hide one regression behind improvements. Only `Ir` is gated;
cache and cycle estimates are simulated and are not part of ADR-0007.

- A benchmark is identified by gungraun's name for it, `file::group::function`
  and its id, not by the package directory, so moving the crate still
  compares every benchmark.
- A benchmark only in the head is reported as **new** and passes. A base
  from before this ADR has no benchmarks, so everything is new.
- A benchmark only in the base fails as **removed, not listed**, unless
  the head's `ci/bench-removed.txt` names it: the pull request that removes
  or renames a benchmark adds its old name there, and since `ci/` is a gate
  path that needs the label and the owner's review. Entries stay as a
  record; a listed name that the head measures again is an error until its
  entry is deleted, so a later removal of it has to be listed afresh.
- Input the gate cannot trust fails (exit 2): an unreadable summary, an
  unsupported schema version, a missing or wrongly typed field, a missing
  or non-integer count, a benchmark measured twice, a head without
  benchmarks, or a base with benchmarks of which none is in the head
  (nothing was compared). A gate that cannot read its input never reports
  success.

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

## Verification

The full job on GitHub's `ubuntu-24.04` runner (x86-64), with Valgrind
3.22.0 and gungraun-runner 0.20.0, on 2026-10-25:

| Run | Base → head | Result |
| --- | --- | --- |
| [40239198298](https://github.com/taktsim/taktsim/actions/runs/40239198298) | `main` (no benchmarks) → this pull request (#6) | 21 benchmarks, all new; passed in 1 min 20 s |
| [40239440977](https://github.com/taktsim/taktsim/actions/runs/40239440977) | #6 → #6 plus a changed comment in `taktsim-units` (scratch #7) | all 21 counts identical on base and head, and identical to the run above, which ran on another runner instance; passed in 1 min 39 s |
| [40239650506](https://github.com/taktsim/taktsim/actions/runs/40239650506) | #6 → #6 plus `SimDuration::from_str` splitting its input twice (scratch #7) | `parse_duration` 3,080 → 4,784 instructions (+55.32 %), reported as a regression, job failed; the other 20 identical |

Pull request #7 was a scratch pull request and was closed without merging.
These runs predate the review changes (removal list, base's gate, name
without package directory). The reviewed version, on 2026-10-26:

| Run | Base → head | Result |
| --- | --- | --- |
| [40244226409](https://github.com/taktsim/taktsim/actions/runs/40244226409) | `main` → this pull request | judged by the pull request's own `ci/` (the base has none); 21 new; passed |
| [40244316501](https://github.com/taktsim/taktsim/actions/runs/40244316501) | #6 → #6 without the `parse_ratio` benchmark (scratch #11) | judged by the base's `ci/`, both test suites run; `parse_ratio` "removed, not listed", job failed; the other 20 compared; the second runner install skipped (same version) |
| [40244521385](https://github.com/taktsim/taktsim/actions/runs/40244521385) | as above, with `parse_ratio` in `ci/bench-removed.txt` (scratch #11) | "removed (listed)"; passed |

Pull request #11 was a scratch pull request, closed without merging.

The gate's tests (`ci/bench-test.sh`) pass with jq 1.7.1 and with the
official jq 1.6 binary. Every non-equivalent mutant of
`ci/bench-compare.sh` tried by hand fails at least one of them, among
them: thresholds 9 and 11, `>=` for `>`, a ratio instead of the integer
comparison, the old instead of the new count, the package directory in
the name, `($s.id // null)`, each dropped input check, an ignored removal
list, an unlisted removal that passes, no "nothing compared" check, and
accepting jq 1.5. The one surviving mutant is equivalent: multiplying the
base by `1.1` gives the same verdict on every whole count in range.

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
- One local lint `expect` in the benchmark binaries, with its reason:
  `clippy::expect_used` on `benches/support/mod.rs::built` (a constant
  input that fails must stop the run; setup functions cannot return an
  error). The criterion benchmarks count failed iterations and fail the
  run afterwards, for the same reason.

## Questions for the owner

1. **An intended regression.** Some changes should cost instructions, e.g.
   an invariant check. The gate has no override, on purpose. Proposed: the
   pull request explains the regression and the owner merges it past the
   failing check (a ruleset bypass), rather than a label that anyone with
   triage rights could set. Is that how you want it handled?

## Owner's answers (2026-10-26)

The owner took the recommendations:

1. An intended regression above 10 % is not merged past the failing
   check (the ruleset has no bypass). Instead, a committed file of
   accepted regressions (benchmark, new limit, reason) is read by
   `ci/bench-compare.sh`; adding an entry is a gate change reviewed by the
   owner. **Follow-up:** not implemented yet; it loosens the gate, so it
   is its own pull request with the `gate-change` label.

## Implementation note: accepted regressions (2026-10-26)

Implements answer 1. It loosens the gate (a benchmark may rise by more
than 10 %), so it came in a pull request of its own with the
`gate-change` label. The rule of ADR-0007 is unchanged for every benchmark
without an entry.

**The file.** `ci/bench-accepted.txt`, one entry per line:

```text
<benchmark> +<N>% <reason>
instructions_random::stream::next_u64::draws +25% every draw now checks the stream's position (ADR-0007)
```

A line whose first word starts with `#` is a comment; white space between
the words is free, and a `#` later in a line belongs to the reason. The
file is committed with comments only.

**Rules, and how `ci/bench-compare.sh --accepted <head's list>
--accepted-base <base's list>` enforces each:**

| Rule | On violation |
| --- | --- |
| The benchmark is one name, `file::group::function[::id]` of letters, digits and `_`; no pattern | exit 2, line number, what was found, an example |
| It is measured by both the base and the head (an unknown, new or removed benchmark has nothing to accept); an entry for `f::g::x` does not cover `f::g::x::a` | exit 2 |
| A benchmark has at most one entry | exit 2, both line numbers |
| The limit is `+N%`, N a whole number from 11 to 1000; up to +10 % needs no entry | exit 2 |
| A reason (the rest of the line) is present | exit 2 |
| The head may rise to `base × (100 + N) / 100`, exactly: `head × 100 > base × (100 + N)` fails, as for the 10 % rule | exit 1, `regression above the accepted +N %` |
| A new entry must be needed: a benchmark that rose by 10 % or less (or fell) fails with its entry | exit 1, `accepted entry not needed` |
| Only entries the pull request adds apply; an entry also in the base's list is stale and never applies | the benchmark is judged by +10 % alone |
| A pull request that changes the list deletes every stale entry | exit 2 |
| A product `base × (100 + N)` of 2^53 or more cannot be compared exactly | exit 2 |

Every malformed line is reported in one run, with the fix and a valid
example. Exit codes keep their meaning: 1 a verdict, 2 input the gate
cannot trust. Without `--accepted` the script behaves exactly as before,
so a later head's workflow can still call an older gate.

**Why at most +1000 %.** An entry must never mean "anything". A percentage
of the base is bounded by construction; the cap makes the bound a fixed
number the gate checks, not only the reviewer. A change that makes a
benchmark more than eleven times as expensive measures a different
workload: it becomes a new benchmark, with the old name in
`ci/bench-removed.txt`.

**Which copy is read, and why the owner still reviews every entry.** The
workflow keeps judging a pull request by the base's scripts (step 2 of the
Decision). The list itself is read from the **head**, like
`ci/bench-removed.txt`: the pull request that makes a change declares the
regression it asks to accept, beside the code that causes it. Reading the
base's list instead would need the entry merged before the change, where
it could not be checked against a measurement. The head's list cannot
slip through unreviewed:

- `ci/bench-accepted.txt` is under `ci/`, a gate path (`ci/check-gates.sh`
  now also names it, so narrowing `ci/` cannot drop it) and owned by the
  owner in `CODEOWNERS`; adding an entry fails the `gates` check without
  the `gate-change` label.
- The script that interprets the list is the base's, so a pull request
  cannot relax the rules and use the relaxed rules at once.
- The base's copy (`git show <base>:ci/bench-accepted.txt`, empty if
  absent) decides which entries are new. Only those apply, so an entry
  merged with an earlier pull request never covers a later regression,
  and every acceptance is a line added in the pull request it applies to,
  visible in its diff. Changing an entry's limit or reason makes it a new
  entry, which again needs the label and the review, and must be needed.

**After the merge: stale entries.** Once the regression is on `main`, the
base's count includes it and the entry is in the base's list: it is
stale. Stale entries are identified by their words (benchmark, limit,
reason; white space ignored), never applied, and listed under
**Stale:** in the job summary. They fail the check (exit 2) in any pull
request that changes the list, which must delete them; in a pull request
that does not touch the list they are reported but do not fail.

Failing every pull request while a stale entry is on `main` was
considered and rejected. The pull request that adds an entry cannot also
delete it, so after every accepted regression all open pull requests
would fail until a separate clean-up pull request is merged, and each
author would have to edit a gate file and carry the `gate-change` label
for a change that is not theirs; that would wear down the meaning of
both the label and a red check. Failing gives nothing more for safety:
a stale entry already cannot accept anything. What failing would add,
keeping the file short, is reached by the rule above: the file never
holds more than the entries of the last pull request that changed it,
and the next acceptance removes them under the owner's review.

**The first pull request.** The base of the pull request that adds the
feature has a gate that does not know `--accepted`. The workflow passes
the lists only when the gate in use has `ci/bench-accepted.txt` (the file
and the option arrive together), and writes "Accepted regressions: not
read" in the summary otherwise. That pull request is therefore judged by
+10 % alone, as every pull request that changes the gate is judged by the
gate it changes, and its own tests run as the head's tests. The first
pull request that can accept a regression is the one after it.

**Job summary.** An applied entry shows in the table as
`accepted (limit +N %)`, and below it under **Accepted:** with its change,
limit, line and reason; the closing line reads "no benchmark rose by more
than 10 % (ADR-0007), except the accepted ones above". Failures show as
`regression above the accepted +N %` and `accepted entry not needed`.
The summary also says whether the gate in use read the list.

**Tests.** `ci/bench-test.sh` has 56 new cases (115 in all): within,
exactly at and one above the limit (also near 2^53, and a product too
large to compare exactly); an entry covering only its own benchmark and
not an id under it; comments, spacing, `#` in the reason and CRLF line
ends; not needed (+10 % exactly, and an improvement); a zero base; the
highest limit; the committed (empty) list changing nothing; each
malformed form (pattern, short name, no limit, no reason, blank reason,
missing `+` or `%`, a space before `%`, fraction, leading zero, word,
negative, +10 %, +1001 %, a 20-digit limit), every bad line named in one
run, duplicates, an unknown, a new and a removed benchmark; stale entries
warning, never applying, of a removed benchmark, left beside a new entry,
deleted beside a new entry, deleted alone, left while only a comment
changes, re-spaced; an edited entry being new and having to be needed; a
malformed base list that can still be fixed; the committed list parsing;
missing, directory and empty-path lists, `--accepted-base` alone, a
repeated option, the base's list from a pipe, options in any order. Each
expected value is worked out by hand from the rule. The test helper also
checks that a line is absent (`!text`). The tests pass with jq 1.8.2; the
jq programs use nothing newer than 1.6 (every `if` has an `else`), but
the official 1.6 binary was not run again for this change.

Each of 28 hand mutants of the new code fails at least one test: stale
entries applied, `>=` for `>` and one more percent at the limit, no
"not needed" verdict or one that passes, an exceeded entry that passes,
each half of the "measured on both sides" check and the whole check
dropped, no stale check, stale entries failing every pull request, the
list never counted as changed, no exactness or duplicate check, a minimum
limit of +10 %, a maximum of +1001 % or ten times higher, any name or a
two-part name accepted, the `+` optional, leading zeros allowed, no
reason required, `#` anywhere making a comment, comments not skipped,
entries keyed by benchmark only, the base's list read strictly,
`--accepted-base` alone allowed, and a missing list accepted.

**Checked locally** (Linux x86-64, Valgrind 3.25.1, gungraun-runner
0.20.0; these runs show what the gate decides, not speed, ADR-0010):
`ci/bench-run.sh` on `main` (6834c25) and on this branch measured the same
21 counts; `main`'s `ci/bench-compare.sh` and the new one, given the
committed list, printed byte-identical tables ("21 compared", passed).
With a scratch change that parses each duration twice,
`parse_duration` rose from 3,113 to 4,721 instructions (+51.65 %): no
entry failed it as a regression; a new `+60%` entry passed it as
`accepted (limit +60 %)` with its reason; a `+30%` entry failed it as
`regression above the accepted +30 %`; the `+60%` entry in both lists,
with the regressed counts as base and head, passed with it listed as
stale. The scratch change was not committed.

**Open questions for the owner.**

1. The `bench` check is not yet a required check on `main` (CLAUDE.md,
   Status), and the owner's review of gate files relies on `CODEOWNERS`
   being enforced by the ruleset ("require review from code owners"). Both
   are ruleset settings; until they are on, an accepted entry is held back
   by the `gates` check and the label only.
2. Stale entries fail only a pull request that changes the list (above).
   If you prefer every pull request to fail until they are deleted, it is
   one condition in `ci/bench-compare.sh` and its tests.
3. The cap of +1000 % is a judgement; a lower cap (say +100 %) would make
   larger acceptances a deliberate rename instead.
