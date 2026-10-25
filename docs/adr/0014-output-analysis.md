# ADR-0014: Output analysis: t quantiles, replication control, time-weighted statistics, MSER-5

- Status: proposed (needs the owner's explicit acceptance: it reads the
  spec's stop rule, records an exception to the unit-type rule, and asks
  three questions; merging the pull request alone does not accept it)
- Date: 2026-10-25
- Blocks: M8, M9 (the statistics; KPI extraction from traces comes with the
  engine)
- Spec: §6 "Output analysis", §4 "Observables and KPIs" (WIP
  time-weighted), §5 Contracts ("every output that contains a number
  contains its uncertainty"), §9 Reproducibility; ADR-0007 (A2, A3,
  Bonferroni rule), ADR-0013 (`libm`, determinism)

## Context

Spec §6 fixes what output analysis must do: confidence intervals from
independent replications, a sequential procedure that reaches a declared
relative precision within a budget and reports whether it did, MSER-5 as
the default warm-up detection, and statistics that exclude the warm-up.
ADR-0007 fixes the acceptance tests: A2 (95 % intervals cover the true
value in 930–970 of 1000 seeded experiments) and A3 (MSER-5 finds a
truncation point in [0.9 L, 2 L] in at least 95 of 100 runs). It leaves
open which sequential procedure, its minimum number of replications n₀,
how "within 2 %" is read, how Student t quantiles are computed under the
no-platform-math rule of ADR-0013, the details of MSER-5, what a
time-weighted statistic over a window is, and the models behind the A2 and
A3 tests. This ADR settles those details. It contains a reading of the
spec (the stop rule, §3 below) and an exception to the rule that
quantities cross function boundaries as unit types (§6 below), so it needs
the owner's explicit acceptance; the questions at the end are about
readings the owner may want otherwise.

## Measured findings, stated plainly

Two results of the prototypes matter beyond this crate:

1. **The sequential procedure under-covers.** Law's procedure with n₀ = 10
   and γ = 5 % on normal results reaches a coverage of **0.9435 ± 0.0005**
   (numpy, 200,000 experiments), not 0.95. Over 1000 experiments that is a
   count of about 943.5 with standard deviation 7.3, so a correct
   implementation fails the A2 window [930, 970] with probability
   **3.1 %**. A2 must therefore be proven on fixed-n intervals (question 2).
2. **MSER-5 is weak on autocorrelated output.** On the A3 models with
   AR(1) noise instead of independent noise, the share of runs that land
   in [0.9 L, 2 L] falls from 99.8 % / 100 % (exponential decay / mean
   shift, independent noise) to **96 % / 87 %** at lag-1 correlation 0.5
   and **76 % / 34 %** at 0.8 (numpy, 2000 runs each). A mean shift of
   only half a noise standard deviation is found in **30 %** of runs even
   with independent noise. Queue output is strongly autocorrelated, so
   ADR-0007's A3 number may not hold on the M9 model (question 3).

## Options

**t quantiles.** (a) A table of critical values: covers only the degrees
of freedom and levels in the table, and Bonferroni levels such as
1 − 0.05/2000 are not in any table. (b) A port of an existing library
routine (R's `qt`, Boost): larger, and R's code is GPL. (c) An own
implementation of published algorithms with `libm` only, checked against
an arbitrary-precision reference.

**Sequential procedure.** (a) Law's relative-precision procedure (Law
2015, §9.4.1; Law, Kelton & Koenig 1981): after n₀ replications, add one at
a time until the half-width is small enough relative to the mean. (b) A
two-stage procedure that estimates the required n once from a pilot: one
estimate, no adaptation, and it may overshoot the budget. (c) Batch-wise
sequential rules: fewer checks but coarser stopping. Option (a) is the
textbook procedure for exactly the spec's wording.

**Warm-up.** MSER-5 is the spec default; the open points are its details
(below).

## Trade-offs

Own numerics (t quantiles) cost a careful implementation and a reference
test, but give identical results on every platform (ADR-0013) and any
degree of freedom. Law's procedure checks the stop rule after every
replication, which is cheap here (a quantile costs microseconds), and its
coverage is slightly below nominal because stopping depends on the sample
variance; n₀ ≥ 10 keeps that small (measured below).

## Recommendation

### 1. Student t quantiles (`StudentT`)

- ν = 1 and ν = 2: closed forms (`tan` from `libm`; a square root).
- ν ≥ 3000 and z² ≤ ν/100 (z the normal quantile): the Cornish–Fisher
  expansion of Abramowitz & Stegun 26.7.5 to the ν⁻⁴ term; its truncation
  error there is below 2·10⁻¹⁴ relative (measured with mpmath).
- Otherwise: Hill's approximation (Hill 1970, CACM Algorithm 396) as the
  start, then Newton's method on the exact CDF, safeguarded by bisection.
  The CDF is the regularized incomplete beta function by its continued
  fraction (modified Lentz), with the prefactor written through the t
  density and Γ((ν+1)/2)/Γ(ν/2) from an exact recurrence (ν ≤ 400) or its
  asymptotic series. Far tails are solved for ln P(T > t) in ln t, near
  the median for P(|T| < t), each of which is computed without
  cancellation.
- The normal quantile is Wichura's AS 241 (PPND16, 1988).
- Verification: 850 reference values (ν from 1 to 2³² − 1, p from 0.6 to
  1 − 10⁻⁹ and Bonferroni levels 1 − 0.05/(2k) for k ≤ 1000) computed with
  mpmath at 60 digits and cross-checked with scipy; the largest relative
  error is 2.4·10⁻¹⁴ (required: 10⁻¹⁰). Script and values are in
  `crates/taktsim-stats/tests/t_quantiles.rs`.

### 2. Confidence intervals (`Summary`, `ConfidenceInterval`)

- The t interval mean ± t(n − 1, 1 − α/2)·s/√n from n ≥ 2 independent
  replications; fewer is a typed error.
- Mean and variance by Welford's update in the order the results are
  pushed. That order must be the replication (seed) order, never the
  completion order of parallel replications, or results would depend on
  thread scheduling.
- The default confidence level is 95 %. A level is stored as α, so a
  Bonferroni level α/k (`ConfidenceLevel::bonferroni`, for the ADR-0007
  analytical suite) loses no digits; a percentage gives α = (100 − p)/100,
  so `95%` is exactly the default; levels order by level and print as
  100 − 100α, without rounding noise.
- The smallest supported tail probability is the smallest normal `f64`
  (about 2.2·10⁻³⁰⁸): a quantile's p must be at least that, and α/2 too.
  Below it a t quantile with 1 degree of freedom overflows and a halved
  subnormal α can vanish, so the constructors refuse such values with a
  typed error.
- `ConfidenceInterval` has no constructor without its half-width, level
  and replication count, so the number cannot be reported without its
  uncertainty. The seed list (spec §5) belongs to the run that produced the
  results and is attached there.

### 3. Sequential replication control (`ReplicationPlan`, `ReplicationControl`)

- Law's procedure. Stop rule: n ≥ n₀ and half-width ≤ γ·|mean|, written
  without a division (a mean of 0 with no spread counts as reached; a mean
  of 0 with spread never does). This is the spec's reading of
  `precision: { throughput(sink): 2% }` ("replications until the CI
  half-width is within 2 %"). Law states the rule with γ/(1 + γ) in place
  of γ, which bounds the error relative to the true mean by γ; see
  question 1.
- n₀ default 10 (`DEFAULT_MIN_REPLICATIONS`), always explicit in the plan:
  Law recommends n₀ ≥ 10 and γ ≤ 0.15 after the coverage study of Law,
  Kelton & Koenig (1981); below 10 the sample standard deviation is too
  unreliable for a stop rule. n₀ must be at least 2 and at most the budget.
- The budget is the maximum number of replications. The status after each
  result is `Continue`, `PrecisionReached` or `BudgetExhausted`; the report
  holds the interval, the achieved relative half-width and whether the
  precision was reached. A result beyond the budget is an error. The
  status is recomputed after every result, so with several target KPIs the
  run keeps one control per KPI and stops when none says `Continue`.
- Measured (numpy, 200,000 experiments, normal results, γ = 5 %, n₀ = 10):
  coverage 0.9435 ± 0.0005, mean 62.4 replications against 61.5 from
  theory. The test `tests/coverage.rs` reproduces both in 1000 seeded
  experiments.

### 4. Time-weighted statistics (`Window`, `TimeWeightedAccumulator`)

- A signal is given by change points (time, value) and is constant from
  each change to the next. Several changes at one instant collapse to the
  last of them before anything is weighted, so a zero-length blip to
  another value and back changes no bit of the result.
- A window is [start, end] with an optional warm-up [start, w) given as a
  time (e.g. from MSER-5) or a fixed period. The statistics cover [w, end];
  the signal must have a value at w (a change at or before w), or the
  result is an error rather than a guess. Changes after the end are
  checked and ignored.
- Mean (1/T)∫v dt and variance (1/T)∫(v − mean)² dt, the spread of the path
  (not of an estimator), plus minimum and maximum, by West's (1979)
  weighted form of Welford's update with weights in seconds. Equal
  consecutive values are merged before weighting, so splitting a segment
  never changes a bit of the result. The accumulator streams, so the
  engine need not store a buffer's history.

### 5. MSER-5 (`mser5`, `mser5_timed`)

- MSER (White 1997) on batch means of 5 consecutive observations (White,
  Cobb & Spratt 2000): d* minimizes MSER(d) = Σ_{j>d} (Z_j − Z̄(d))²/(k − d)²,
  the form in Franklin & White (2008).
- Observations after the last complete batch are not used. At least 3
  batches (15 observations) are required, so the largest truncation
  searched leaves 2 batches.
- The search covers d ≤ ⌊k/2⌋. Near the end of the series the statistic
  rests on a few batches and becomes small by chance; a truncation point in
  the second half is treated in MSER-5 practice as a sign that the run is
  too short. The result says whether the minimum lies at the limit
  (`reached_search_limit`): then the transient may be longer than half the
  series. Ties go to the smallest d.
- With times, the warm-up ends at the time of the first kept observation
  (`None` if nothing is deleted), which a `Window` takes as its warm-up end.
- Finite observations whose batch sum or statistic overflows `f64` are an
  error, never skipped.

### 6. Exception: observed values are bare `f64`

The rule (CLAUDE.md, spec §7 Layer 1) is that quantities cross function
boundaries as unit types. Replication results (`Summary`,
`ConfidenceInterval`, `ReplicationControl`), signal values
(`time_weighted`, `TimeWeightedAccumulator`) and observations (`mser5`)
are bare `f64` instead, deliberately:

- The statistics are unit-agnostic: a mean, a variance or a truncation
  point is computed the same way for throughput in parts per hour, lead
  time in seconds or WIP in parts. Generic code over every unit type would
  add nothing but conversions, and some KPIs (shares, OEE) have no unit
  type.
- The caller declares one unit per KPI and passes all its values in it
  (documented on the crate and on each function). The M8 KPI layer, which
  knows the unit of each KPI, wraps the results in typed quantities again.
- Everything that is a quantity of this crate's own is typed: times and
  windows (`SimTime`, `SimDuration`), confidence levels, probabilities and
  the target precision (`Ratio`).

### 7. Test models for A2 and A3

- A2 (`tests/coverage.rs`): 1000 experiments, one stream each, seed =
  experiment number. Normal results with n = 5 (coverage exactly 95 %;
  with the normal quantile instead of t it would be 87.8 %), and means of
  50 exponentials with n = 10 (coverage 0.9487 by a 2,000,000-experiment
  numpy simulation). Both counts must lie in [930, 970]; a correct
  implementation passes with probability 99.7 % and 99.5 %. Seeded counts:
  939 and 944.
- A3 (`tests/warm_up_recovery.rs`): x(i) = 10 + b(i) + ε(i), ε i.i.d.
  N(0, 1), n = 5000, 100 runs. Exponential decay b(i) = 10 e^(−i/100) with
  L = 100 ln 10 = 230.3, the point where the bias falls to the noise
  standard deviation; and a mean shift b(i) = 1 for i < L = 500. A numpy
  prototype (2000 runs each) found 99.8 % and 100 % of runs in
  [0.9 L, 2 L]; the seeded test finds 100 of 100 for both.
- Prototype evidence on the limits of MSER-5, for ADR-0007's A3 number
  (which it says is "validated with a prototype in M9"): see "Measured
  findings" above. Without a transient, MSER-5 deleted at most 2 % of the
  series in 99.9 % of runs.

## Consequences

- Every statistic is a pure function of its input built from IEEE
  operations and `libm`, with no fused multiply-add and fixed summation
  order: identical results on every tier-1 platform (ADR-0010, ADR-0013).
- `taktsim-stats` depends on `taktsim-units`, `libm` and `thiserror`, not
  on the engine; `taktsim-core` is a dev-dependency for seeded test data
  (added to `[workspace.dependencies]`).
- M8/M9 integration: the run layer pushes per-replication KPI values in
  seed order, keeps one `TimeWeightedAccumulator` per WIP signal, and
  attaches the seed list to every reported interval. Which series MSER-5
  is applied to (one pilot replication, or the mean over replications) is
  decided in M9.
- Out of scope here: KPI definitions over traces (need the engine), batch
  means with an autocorrelation check (deferred, spec allows it), Bonferroni
  comparison of designs (v0.4).

## Questions for the owner

1. **Stop rule.** Keep the spec's literal reading, half-width ≤ γ·|mean|
   (implemented), or adopt Law's γ/(1 + γ), which guarantees a relative
   error of at most γ against the *true* mean (for γ = 2 % the threshold
   becomes 1.96 %)? Recommended: keep the literal reading, because the spec
   defines precision as the half-width and the report states it directly.
2. **A2 through the sequential procedure (M10).** Its coverage is about
   94.3 % (n₀ = 10, γ = 5 %), so 1000 experiments would land near the lower
   edge of [930, 970] and a correct implementation would fail about 3 % of
   the time. Recommended: prove A2 on fixed-n intervals, as here, and test
   the sequential procedure against its own simulated coverage.
3. **A3 on a real model (M9).** Queue output is strongly autocorrelated,
   and the prototype shows MSER-5's recovery rate falling with
   autocorrelation. The ≥ 95 % criterion may then fail because of the
   method, not the implementation. Options: apply MSER-5 to the mean over
   several replications (noise reduced by √R), require the M9 model's
   transient to be strong relative to its noise, or revise the criterion
   with M9 measurements. Recommended: decide with the M9 prototype; this
   ADR only records the evidence.
4. **Unit types at the statistics boundary.** Accept observed values as
   bare `f64` in one declared unit per KPI (§6), with the M8 KPI layer
   wrapping results in typed quantities? Recommended: yes; the
   alternative is a generic numeric trait over every unit type for no gain
   in safety inside the statistics.

## References

- Abramowitz, M. & Stegun, I. A. (1964). *Handbook of Mathematical
  Functions*, 26.7.5.
- Franklin, W. W. & White, K. P. (2008). Stationarity tests and MSER-5:
  exploring the intuition behind mean-squared-error reduction in
  detecting and correcting initialization bias. *Proc. Winter Simulation
  Conference*, 541–546.
- Hill, G. W. (1970). Algorithm 396: Student's t-quantiles. *Comm. ACM*
  13(10), 619–620.
- Law, A. M. (2015). *Simulation Modeling and Analysis*, 5th ed.,
  McGraw-Hill, §9.4.1.
- Law, A. M., Kelton, W. D. & Koenig, L. W. (1981). Relative width
  sequential confidence intervals for the mean. *Communications in
  Statistics – Simulation and Computation* B10, 29–39.
- Press, W. H. et al. (2007). *Numerical Recipes*, 3rd ed., §6.4
  (incomplete beta function, continued fraction).
- West, D. H. D. (1979). Updating mean and variance estimates: an improved
  method. *Comm. ACM* 22(9), 532–535.
- White, K. P. (1997). An effective truncation heuristic for bias
  reduction in simulation output. *Simulation* 69(6), 323–334.
- White, K. P., Cobb, M. J. & Spratt, S. C. (2000). A comparison of five
  steady-state truncation heuristics for simulation. *Proc. Winter
  Simulation Conference*, 755–760.
- Wichura, M. J. (1988). Algorithm AS 241: the percentage points of the
  normal distribution. *Applied Statistics* 37(3), 477–484.
