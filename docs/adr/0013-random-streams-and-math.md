# ADR-0013: Random streams and platform-independent math

- Status: proposed. Needs the owner's **explicit** acceptance: it settles
  how ADR-0001 and ADR-0003 are read on truncation, and how spec §3's
  dependency rule for `taktsim-core` is read (see "Questions for the
  owner").
- Date: 2026-10-25
- Blocks: M2
- Spec: §3 ("`simcode-core` has no I/O and depends only on the units
  crate"), §9 Reproducibility ("counter-based (default: Philox or PCG64),
  one stream per component"; "no floating-point operation whose result
  depends on platform"); ADR-0001, ADR-0003, ADR-0010

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
2. **The math.** Sampling needs `ln`, `exp`, `sin` and `cos`. Rust's
   `f64::ln` and friends call the platform's math library, whose results
   can differ in the last bit between glibc, Apple's libm and the MSVC
   runtime, so the same seed could give different durations on different
   machines.

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
- `clippy.toml` forbids the platform-library functions (`ln`, `exp`,
  `powf`, `powi`, `sin`, `cos`, the hyperbolic and inverse hyperbolic
  functions, …) for both `f64` and `f32`, and `mul_add` (ADR-0010), in the
  whole workspace, tests included. It bans the `HashMap` and `HashSet`
  types, whose random seeding would make iteration order differ between
  runs; banning the types catches `new`, `default`, `from` and `collect`
  alike.

**Sampling algorithms** (ADR-0003 parameters):

| Distribution | Method |
| --- | --- |
| exponential | inverse CDF |
| normal | Box–Muller, both values of each pair used; truncated at zero by rejection |
| lognormal | exp(μ + σz), μ = ln(mean) − σ²/2 |
| uniform, triangular | inverse CDF, clamped to `[min, max]` after rounding to nanoseconds |
| gamma (mean, cv) | Marsaglia–Tsang; for shape < 1, shape + 1 and the u^(1/shape) boost |
| erlang (mean, k) | gamma with integer shape k (the same distribution), so any k ≥ 1 costs the same |
| weibull | inverse CDF, `pow` from libm |

Samples are rounded once to the nearest nanosecond (ADR-0001). A sample
beyond simulation time (about 584 years) is an error, not a clamp.

**Truncation of the normal.** ADR-0003 says the normal is "truncated at 0".
This ADR reads that as statistical truncation: negative draws are rejected
and drawn again, so the result is the normal conditioned on being
non-negative. A positive mean keeps at least half of all draws, so the
loop is short. ADR-0001's "clamped at ≥ 0" is the separate safety net of
the conversion `SimDuration::from_secs_f64`, which turns any negative
seconds into zero; with rejection, the normal never reaches it. The two
readings differ visibly: for mean 1 s and sd 2 s, clamping would put
about 31 % of samples exactly at zero, while truncation gives a mean of
2.018 s and no point mass.

**Parameters that cannot be sampled.** Besides the ADR-0003 rules on the
parameters themselves, a constructor rejects parameters whose derived
values overflow or vanish in `f64`: a gamma `cv` beyond about 10⁻¹⁵⁴ or
10¹⁵⁴, a lognormal `sigma` large enough to make μ infinite, a Weibull
`shape` so small that 1/shape is infinite. This is representability, not
a new range rule: without it, gamma with `cv = 1e-160` looped forever,
which no run budget could interrupt.

## Verification

- Philox: known-answer vectors (Random123, NumPy).
- Each distribution: 200,000 samples with a fixed seed; mean and variance
  within 5 standard errors of theory, and a Kolmogorov–Smirnov test at the
  1 % level where the CDF has a closed form (all but gamma). Erlang is
  also checked with a million phases.
- Properties: uniform and triangular samples stay in their support over
  the whole duration range; every constructible distribution, with
  parameters from 10⁻³⁰⁰ to 10³⁰⁰, either samples or reports an overflow.
- Cross-platform golden test: digests of 10,000 samples per distribution
  and of the raw stream, identical on all four tier-1 targets in CI.

## Consequences

- `libm` functions are somewhat slower than the platform library; the
  M2b benchmark measures the effect on events per second.
- Changing an algorithm or a constant changes every trace; the golden
  test makes that a deliberate, visible change.
- Statistics (M8) use the same rule, so results, not only traces, are
  identical across platforms.
- `taktsim-core` depends on `taktsim-units` and on two external crates
  that do pure computation: `libm` (required by spec §9's determinism
  rule) and `thiserror` (the spec's default for typed errors, §7).

## Questions for the owner

1. **Truncation:** accept the reading above (ADR-0003's "truncated" is
   statistical truncation; ADR-0001's clamp is the conversion's safety
   net)?
2. **Core dependencies:** spec §3 says core "depends only on the units
   crate". Read it as "the only *taktsim* crate core depends on is units,
   and core does no I/O", so pure-computation crates such as `libm` and
   `thiserror` are allowed? The alternative is to vendor `libm` (about
   15,000 lines) and hand-write the error impls.
