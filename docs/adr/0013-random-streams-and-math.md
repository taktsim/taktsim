# ADR-0013: Random streams and platform-independent math

- Status: proposed (accepted when the M2a pull request is merged)
- Date: 2026-10-25
- Blocks: M2
- Spec: §9 Reproducibility ("counter-based (default: Philox or PCG64), one
  stream per component"; "no floating-point operation whose result depends
  on platform"); ADR-0001, ADR-0003, ADR-0010

## Context

Same model, seed and engine version must give the same trace byte for
byte on Windows x86-64, Linux x86-64 and ARM64, and macOS ARM64. Two parts
of sampling decide whether that holds:

1. **The generator.** The spec asks for a counter-based generator with one
   stream per component, so that adding a component does not shift the
   others, and names "Philox or PCG64". Only Philox is counter-based: its
   n-th output is a pure function of a key and n. PCG64 is a permuted
   linear congruential generator; it has streams and jump-ahead, but its
   state is sequential, and stream selection by an increment parameter is
   known to give correlated streams for related increments.
2. **The math.** Sampling needs `ln`, `exp` and `cos`. Rust's `f64::ln` and
   friends call the platform's math library, whose results can differ in
   the last bit between glibc, Apple's libm and the MSVC runtime, so the
   same seed could give different durations on different machines.

## Decision

**Generator: Philox4x64-10** (Salmon et al., SC 2011), as in Random123 and
`numpy.random.Philox`.

- Key = (seed, stream key); counter = (block index, 0, 0, 0). Each block
  gives four 64-bit values.
- Verified against Random123's published known-answer vectors and against
  NumPy 2.5.4 (`crates/taktsim-core/src/random/philox.rs`).

**Streams per component and purpose.** A `StreamKey` is FNV-1a (64 bit)
over the component id, a `0xff` separator and a purpose, e.g.
`("cut", "cycle_time")`. Keys come from names, not positions, so adding a
component changes no other stream. Separate purposes (cycle time,
failures, repairs) mean that changing a failure parameter leaves cycle
times unchanged, which sharpens the comparisons in v0.4 (common random
numbers). M2b checks at run start that no two streams of a run share a key.

**Uniform values.** `(m + ½) × 2⁻⁵²` for a random 52-bit `m`: strictly
inside (0, 1), so `ln(u)` is always finite, and computed with exact float
operations only.

**Math.**

- All transcendental functions come from the `libm` crate (a pure-Rust
  port of musl's libm) with default features off, so no hardware-specific
  code path is used. `sqrt` comes from `std`: IEEE 754 requires it to be
  correctly rounded, so it is identical everywhere.
- `clippy.toml` forbids `f64::ln`, `exp`, `powf`, `powi`, `sin`, `cos` and
  the other platform-library functions, and `mul_add` (ADR-0010), in the
  whole workspace, tests included. It also forbids `HashMap::new` and
  `HashSet::new`, whose random seeding would make iteration order differ
  between runs.

**Sampling algorithms** (ADR-0003 parameters):

| Distribution | Method |
| --- | --- |
| exponential | inverse CDF |
| normal | Box–Muller (one value per pair), truncated at zero by rejection; a positive mean keeps at least half of the draws |
| lognormal | exp(μ + σz), μ = ln(mean) − σ²/2 |
| uniform, triangular | inverse CDF |
| gamma (mean, cv) | Marsaglia–Tsang; for shape < 1, shape + 1 and the u^(1/k) boost |
| erlang (mean, k) | sum of k exponential phases; k at most 1000 |
| weibull | inverse CDF |

Samples are rounded once to the nearest nanosecond (ADR-0001). A sample
beyond simulation time (about 584 years) is an error, not a clamp.

**Bound on Erlang phases.** ADR-0003 allows any whole `k ≥ 1`; each sample
draws `k` values, so a huge `k` would stall a run in a way the budget
cannot interrupt. `k` is limited to 1000; larger `k` is better modelled as
a gamma or normal.

## Verification

- Philox: known-answer vectors (Random123, NumPy).
- Each distribution: 200,000 samples with a fixed seed; mean and variance
  within 5 standard errors of theory, and a Kolmogorov–Smirnov test at the
  1 % level where the CDF has a closed form (all but gamma).
- Cross-platform golden test: digests of 10,000 samples per distribution
  and of the raw stream, identical on all four tier-1 targets in CI.

## Consequences

- `libm` functions are somewhat slower than the platform library; the
  M2b benchmark measures the effect on events per second.
- Changing an algorithm or a constant changes every trace; the golden
  test makes that a deliberate, visible change.
- Statistics (M8) use the same rule, so results, not only traces, are
  identical across platforms.
