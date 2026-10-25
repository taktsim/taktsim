//! Acceptance criterion A3 at the statistics level (spec §6, ADR-0007):
//! on a series with a known transient of length L, MSER-5 must detect a
//! truncation point between 0.9 L and 2 L in at least 95 of 100 seeded runs.
//!
//! Series model: `x(i) = μ + b(i) + σ ε(i)` for i = 0, …, n − 1, with
//! n = 5000, μ = 10, σ = 1, i.i.d. standard normal noise `ε(i)` (one stream
//! per run, seed = run number) and a bias `b` that is the transient:
//!
//! - exponential decay, `b(i) = A e^(−i/τ)` with A = 10σ and τ = 100. The
//!   transient lasts while the bias exceeds the noise standard deviation:
//!   L = τ ln(A/σ) = 230.3.
//! - mean shift, `b(i) = A` for i < L and 0 after, with A = σ and L = 500.
//!
//! A prototype in numpy (2000 runs per model, independent of this crate)
//! found MSER-5 inside [0.9 L, 2 L] in 99.8 % of runs for the decay
//! (truncation 1 %–99 % quantiles 240–410) and in 100 % for the shift
//! (480–555), so ≥ 95 of 100 holds with probability > 0.999999. The same
//! prototype shows the limits recorded in ADR-0014: with AR(1) noise of
//! lag-1 correlation 0.5 the rates drop to 96 % and 87 %, with 0.8 to 76 %
//! and 34 %, and a shift of only σ/2 is found in 30 % of runs.

use taktsim_core::random::{Stream, StreamKey};
use taktsim_stats::{mser5, mser5_timed};
use taktsim_units::SimTime;

const RUNS: u64 = 100;
const N: u32 = 5000;
const MU: f64 = 10.0;
const SIGMA: f64 = 1.0;

/// One run of the series model with bias `bias`.
fn series(seed: u64, purpose: &str, bias: impl Fn(f64) -> f64) -> Vec<f64> {
    let mut stream = Stream::new(seed, StreamKey::new("warm-up", purpose));
    (0..N)
        .map(|i| MU + bias(f64::from(i)) + SIGMA * stream.next_standard_normal())
        .collect()
}

/// How many runs put the truncation point in [0.9 L, 2 L]; prints the
/// spread of the truncation points (`cargo test -- --nocapture`).
#[allow(clippy::unwrap_used, clippy::print_stdout)] // test helper
fn recovered(purpose: &str, length: f64, bias: impl Fn(f64) -> f64) -> u64 {
    let mut found = Vec::new();
    let hits = (0..RUNS)
        .filter(|&seed| {
            let cut = mser5(&series(seed, purpose, &bias)).unwrap();
            let d = f64::from(u32::try_from(cut.deleted_observations()).unwrap());
            found.push(d);
            (0.9 * length..=2.0 * length).contains(&d)
        })
        .count();
    found.sort_by(f64::total_cmp);
    println!(
        "{purpose}: L = {length:.1}, {hits} of {RUNS} in [0.9 L, 2 L]; min {}, median {}, max {}",
        found[0],
        found[found.len() / 2],
        found[found.len() - 1]
    );
    hits.try_into().unwrap()
}

#[test]
fn mser5_recovers_an_exponential_transient() {
    let (amplitude, tau) = (10.0 * SIGMA, 100.0);
    let length = tau * libm::log(amplitude / SIGMA);
    let hits = recovered("exponential", length, |i| amplitude * libm::exp(-i / tau));
    assert!(hits >= 95, "{hits} of {RUNS}");
}

#[test]
fn mser5_recovers_a_mean_shift() {
    let (amplitude, length) = (SIGMA, 500.0);
    let hits = recovered(
        "shift",
        length,
        |i| if i < length { amplitude } else { 0.0 },
    );
    assert!(hits >= 95, "{hits} of {RUNS}");
}

/// Without a transient, MSER-5 deletes little: in the numpy prototype
/// (4000 runs of the same i.i.d. series without bias) at most 2 % of the
/// series in 99.9 % of runs and never more than 115 observations. Here:
/// at most 5 % (250 observations) in every run.
#[test]
fn mser5_deletes_little_from_a_stationary_series() {
    for seed in 0..RUNS {
        let cut = mser5(&series(seed, "stationary", |_| 0.0)).unwrap();
        assert!(cut.deleted_observations() <= 250, "seed {seed}: {cut:?}");
    }
}

/// With one observation per minute, the warm-up ends at the time of the
/// first kept observation.
#[test]
fn timed_truncation_reports_the_end_of_the_warm_up() {
    let values = series(0, "shift", |i| if i < 500.0 { SIGMA } else { 0.0 });
    let timed: Vec<(SimTime, f64)> = values
        .iter()
        .zip(0_u64..)
        .map(|(&x, i)| (SimTime::from_nanos(i * 60_000_000_000), x))
        .collect();
    let cut = mser5_timed(&timed).unwrap();
    assert_eq!(cut.truncation(), mser5(&values).unwrap());
    let deleted = u64::try_from(cut.truncation().deleted_observations()).unwrap();
    assert!(deleted > 0);
    assert_eq!(
        cut.warm_up_end(),
        Some(SimTime::from_nanos(deleted * 60_000_000_000))
    );
}
