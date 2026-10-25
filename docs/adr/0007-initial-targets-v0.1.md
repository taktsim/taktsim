# ADR-0007: Initial targets and gate levels for v0.1

- Status: accepted (2026-10-25)
- Date: 2026-10-25
- Blocks: M0 (CI gate configuration), M13 (release acceptance)
- Spec: §15 "Initial targets", §7 Layer 2, §9, §10, §11

## Context

Spec §15 asks that every number marked *initial target* be validated with
a prototype and recorded in an ADR. This ADR covers the targets that touch
v0.1. It also settles places where the spec gives two different answers
for when a gate applies, because CI has to be configured in M0. Later
targets (viewer frame times, calibration success rate, library
aggregation threshold) get their own ADRs in the tiers that need them.

## Inconsistencies in the spec

1. **Mutation gate.** §7 says "required for core" and §10 lists mutation
   as a required check on `main`; §11 lists "mutation gate on core" and
   "≥ 80 %" as v1.0 scope.
2. **`unwrap` / `expect`.** §7 says they are denied in library code; §11
   repeats "no `unwrap` or `expect`" as a v1.0 criterion. Not a conflict
   if it holds from day one; a conflict if it was meant to start at v1.0.
3. **Branch coverage on stable.** §7 requires Rust stable and 100 %
   branch coverage in core with `cargo-llvm-cov`. As far as I know (to be
   checked in M0), branch coverage in `cargo-llvm-cov` still needs a
   nightly toolchain.
4. **Analytical tests at 95 %.** "Simulation within the confidence
   interval of theory", applied to many cases at 95 % each, means a
   correct engine is expected to fail about one case in twenty. Runs are
   seeded, so the failure is not flaky but permanent, and changing seeds
   until it passes would defeat the test.

## Proposed gate levels

| Item | Spec | Proposed for v0.1 | Becomes required |
| --- | --- | --- | --- |
| `unwrap`, `expect`, `panic!` in library code | denied (§7) | denied via clippy from M0; one documented exception path for internal-invariant panics | M0 |
| Line coverage overall | ≥ 90 % (initial) | ≥ 90 %, gate from M2 | M2 |
| Branch coverage `taktsim-core` | 100 % (initial) | 100 %, measured in a separate CI job on a pinned nightly; all other jobs on stable | M2 |
| Mutation score `taktsim-core` | ≥ 80 % (initial), required | per-PR `cargo mutants --in-diff` on core, reported but not blocking; full run nightly | v1.0 (or earlier, see question 1) |
| Benchmark regression | > 10 % fails (§7) | instruction counts on Linux CI, > 10 % regression fails, from M2 | M2 |
| Throughput ≥ 1 M events/s/core (checks off), ≥ 200 k (checks on) | initial | measured on the reference hardware (ADR-0010) and recorded at M13, not gated | v0.2 (as spec §11 says) |
| 1,000 components, 10 M events, ≤ 2 GB | initial | measured in M3 (trace) and M13 (full run) on the reference hardware, recorded | v0.2 |
| README example < 1 s | v0.1 criterion | Rust: release binary, run only, excluding build; Python: `import` plus the call, on the slowest CI runner | M13 |
| CI coverage 93–97 % of 1,000 experiments | v0.1 criterion | as written; seeded, so deterministic | M8 |

## Proposals for open details

- **Analytical tests (inconsistency 4).** Each case checks that theory lies
  within the simulated confidence interval at level 1 − 0.05/k, where k is
  the number of cases in the suite (Bonferroni, as the spec already uses
  for design comparisons). Seeds are fixed once when a case is added and
  never changed to make a case pass.
- **Warm-up recovery (A3).** On a model with a known transient of length
  L, MSER-5 must detect a truncation point between 0.9 L and 2 L in at
  least 95 % of 100 seeded runs. Validated with a prototype in M9 before
  this number is accepted.
- **Invariant-violation panics vs 100 % branch coverage.** Each panic path
  is reached by a test that injects the violation through a test-only
  constructor, so no coverage exclusions are needed.

## Recommendation

Adopt the table above. Performance and memory targets apply to the
reference hardware in ADR-0010, never to a developer's machine. Every target marked "measured and recorded" gets
its measurement appended to this ADR, with machine and commit, before the
release that gates on it.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Mutation gate: blocking on core from v0.1 (strict reading of §7/§10),
   or advisory until v1.0 (reading of §11)? Recommended: advisory, with a
   per-PR report, because a blocking full mutation run is slow and the
   core is still changing quickly.
2. Accept a pinned nightly toolchain for the coverage job only?
3. Accept the Bonferroni rule for the analytical suite?
