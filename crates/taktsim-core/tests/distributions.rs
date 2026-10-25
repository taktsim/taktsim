//! Duration distributions: parameter checks, model-file display, and
//! samples that follow the stated distribution.
//!
//! Each statistical test draws 200,000 samples with a fixed seed, so it
//! passes or fails the same way on every run. Means and variances must lie
//! within 5 standard errors (estimated from the samples) of theory; where
//! the CDF has a closed form, a Kolmogorov–Smirnov test at the 1 % level is
//! applied as well.

use taktsim_core::random::{DurationDistribution, MAX_ERLANG_PHASES, Stream, StreamKey};
use taktsim_units::{SimDuration, UnitError};

const N: usize = 200_000;

#[allow(clippy::unwrap_used)] // test helper
fn d(text: &str) -> SimDuration {
    text.parse().unwrap()
}

#[allow(clippy::unwrap_used)] // test helper
fn samples(dist: &DurationDistribution, purpose: &str) -> Vec<f64> {
    let mut stream = Stream::new(20_261_025, StreamKey::new("test", purpose));
    (0..N)
        .map(|_| dist.sample(&mut stream).unwrap().as_secs_f64())
        .collect()
}

/// Checks the sample mean and variance against theory.
fn assert_moments(xs: &[f64], mean: f64, var: f64) {
    #[allow(clippy::cast_precision_loss)]
    let n = xs.len() as f64;
    let m = xs.iter().sum::<f64>() / n;
    let s2 = xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n;
    let m4 = xs
        .iter()
        .map(|x| ((x - m) * (x - m)) * ((x - m) * (x - m)))
        .sum::<f64>()
        / n;
    let se_mean = (s2 / n).sqrt();
    let se_var = ((m4 - s2 * s2) / n).sqrt();
    assert!(
        (m - mean).abs() < 5.0 * se_mean,
        "mean {m}, expected {mean} ± {}",
        5.0 * se_mean
    );
    assert!(
        (s2 - var).abs() < 5.0 * se_var,
        "variance {s2}, expected {var} ± {}",
        5.0 * se_var
    );
}

/// Kolmogorov–Smirnov: the largest gap between the empirical and the
/// theoretical CDF stays below the 1 % critical value 1.628 / √n.
fn assert_ks(xs: &mut [f64], cdf: impl Fn(f64) -> f64) {
    xs.sort_by(f64::total_cmp);
    #[allow(clippy::cast_precision_loss)]
    let n = xs.len() as f64;
    let mut gap: f64 = 0.0;
    for (i, &x) in xs.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let i = i as f64;
        let f = cdf(x);
        gap = gap.max((f - i / n).abs()).max(((i + 1.0) / n - f).abs());
    }
    assert!(gap < 1.628 / n.sqrt(), "KS distance {gap}");
}

fn normal_cdf(z: f64) -> f64 {
    f64::midpoint(1.0, libm::erf(z / std::f64::consts::SQRT_2))
}

#[test]
fn constant_always_returns_its_value() {
    let c = DurationDistribution::constant(d("45s"));
    let mut s = Stream::new(0, StreamKey::new("a", "b"));
    assert!((0..100).all(|_| c.sample(&mut s).unwrap() == d("45s")));
}

#[test]
fn exponential_follows_its_distribution() {
    let mut xs = samples(
        &DurationDistribution::exponential(d("12min")).unwrap(),
        "exp",
    );
    assert_moments(&xs, 720.0, 720.0 * 720.0);
    assert_ks(&mut xs, |x| 1.0 - libm::exp(-x / 720.0));
}

#[test]
fn normal_follows_its_distribution() {
    let mut xs = samples(
        &DurationDistribution::normal(d("42s"), d("3s")).unwrap(),
        "normal",
    );
    assert_moments(&xs, 42.0, 9.0);
    assert_ks(&mut xs, |x| normal_cdf((x - 42.0) / 3.0));
}

#[test]
fn normal_is_truncated_at_zero_not_clamped() {
    // Mean 1 s, sd 2 s: about 31 % of raw draws are negative and redrawn.
    // Moments of the truncated normal, computed exactly with Python.
    let mut xs = samples(
        &DurationDistribution::normal(d("1s"), d("2s")).unwrap(),
        "trunc",
    );
    assert!(xs.iter().all(|&x| x >= 0.0));
    assert_moments(&xs, 2.018_320_867_674_067, 1.944_701_742_785_468);
    let below_zero = normal_cdf(-0.5);
    assert_ks(&mut xs, |x| {
        (normal_cdf((x - 1.0) / 2.0) - below_zero) / (1.0 - below_zero)
    });
}

#[test]
fn lognormal_follows_its_distribution() {
    let sigma: f64 = 0.08;
    let mut xs = samples(
        &DurationDistribution::lognormal(d("42s"), sigma).unwrap(),
        "lognormal",
    );
    assert_moments(&xs, 42.0, 42.0 * 42.0 * (libm::exp(sigma * sigma) - 1.0));
    let mu = libm::log(42.0) - sigma * sigma / 2.0;
    assert_ks(&mut xs, |x| normal_cdf((libm::log(x) - mu) / sigma));
}

#[test]
fn uniform_follows_its_distribution() {
    let mut xs = samples(
        &DurationDistribution::uniform(d("40s"), d("50s")).unwrap(),
        "uniform",
    );
    assert!(xs.iter().all(|&x| (40.0..=50.0).contains(&x)));
    assert_moments(&xs, 45.0, 100.0 / 12.0);
    assert_ks(&mut xs, |x| ((x - 40.0) / 10.0).clamp(0.0, 1.0));
}

fn triangular_cdf(x: f64, a: f64, c: f64, b: f64) -> f64 {
    if x <= a {
        0.0
    } else if x <= c {
        (x - a) * (x - a) / ((b - a) * (c - a))
    } else if x < b {
        1.0 - (b - x) * (b - x) / ((b - a) * (b - c))
    } else {
        1.0
    }
}

#[test]
fn triangular_follows_its_distribution() {
    let (a, c, b) = (40.0, 42.0, 50.0);
    let dist = DurationDistribution::triangular(d("40s"), d("42s"), d("50s")).unwrap();
    let mut xs = samples(&dist, "triangular");
    assert!(xs.iter().all(|&x| (a..=b).contains(&x)));
    let var = (a * a + b * b + c * c - a * b - a * c - b * c) / 18.0;
    assert_moments(&xs, (a + b + c) / 3.0, var);
    assert_ks(&mut xs, |x| triangular_cdf(x, a, c, b));
}

#[test]
fn triangular_allows_the_mode_at_either_end() {
    for (mode, purpose) in [("40s", "tri_low"), ("50s", "tri_high")] {
        let m: f64 = d(mode).as_secs_f64();
        let dist = DurationDistribution::triangular(d("40s"), d(mode), d("50s")).unwrap();
        let mut xs = samples(&dist, purpose);
        assert_moments(&xs, (90.0 + m) / 3.0, triangular_var(40.0, m, 50.0));
        assert_ks(&mut xs, |x| triangular_cdf(x, 40.0, m, 50.0));
    }
}

fn triangular_var(a: f64, c: f64, b: f64) -> f64 {
    (a * a + b * b + c * c - a * b - a * c - b * c) / 18.0
}

#[test]
fn gamma_follows_its_distribution() {
    // cv 0.5 is shape 4 (direct Marsaglia–Tsang); cv 2 is shape 0.25,
    // which uses the u^(1/k) boost.
    for (cv, purpose) in [(0.5, "gamma_k4"), (2.0, "gamma_k025")] {
        let xs = samples(
            &DurationDistribution::gamma(d("12min"), cv).unwrap(),
            purpose,
        );
        assert_moments(&xs, 720.0, (720.0 * cv) * (720.0 * cv));
    }
}

#[test]
fn erlang_follows_its_distribution() {
    let mut xs = samples(
        &DurationDistribution::erlang(d("12min"), 3).unwrap(),
        "erlang",
    );
    assert_moments(&xs, 720.0, 720.0 * 720.0 / 3.0);
    let rate = 3.0 / 720.0;
    assert_ks(&mut xs, |x| {
        let y = rate * x;
        1.0 - libm::exp(-y) * (1.0 + y + y * y / 2.0)
    });
}

#[test]
fn weibull_follows_its_distribution() {
    let mut xs = samples(
        &DurationDistribution::weibull(d("6h"), 1.5).unwrap(),
        "weibull",
    );
    // λΓ(1 + 1/k) and λ²(Γ(1 + 2/k) − Γ(1 + 1/k)²), computed with Python.
    assert_moments(&xs, 19_499.298_327_740_165, 175_282_059.282_787_98);
    assert_ks(&mut xs, |x| 1.0 - libm::exp(-libm::pow(x / 21_600.0, 1.5)));
}

#[test]
fn invalid_parameters_are_rejected_with_a_fix() {
    let zero = SimDuration::ZERO;
    let one = d("1s");
    let checks = [
        (
            DurationDistribution::exponential(zero).unwrap_err(),
            "exponential: `mean` must be longer than 0s, found 0s; \
             example: {dist: exponential, mean: 12min}",
        ),
        (
            DurationDistribution::normal(zero, one).unwrap_err(),
            "normal: `mean` must be longer than 0s, found 0s; \
             example: {dist: normal, mean: 42s, sd: 3s}",
        ),
        (
            DurationDistribution::normal(one, zero).unwrap_err(),
            "normal: `sd` must be longer than 0s, found 0s; \
             example: {dist: normal, mean: 42s, sd: 3s}",
        ),
        (
            DurationDistribution::lognormal(zero, 0.1).unwrap_err(),
            "lognormal: `mean` must be longer than 0s, found 0s; \
             example: {dist: lognormal, mean: 42s, sigma: 0.08}",
        ),
        (
            DurationDistribution::lognormal(one, f64::NAN).unwrap_err(),
            "lognormal: `sigma` must be a finite number > 0, found NaN; \
             example: {dist: lognormal, mean: 42s, sigma: 0.08}",
        ),
        (
            DurationDistribution::uniform(one, one).unwrap_err(),
            "uniform: `max` must be longer than `min`, found 1s; \
             example: {dist: uniform, min: 40s, max: 50s}",
        ),
        (
            DurationDistribution::triangular(d("2s"), one, d("3s")).unwrap_err(),
            "triangular: `mode` must be at least `min`, found 1s; \
             example: {dist: triangular, min: 40s, mode: 42s, max: 50s}",
        ),
        (
            DurationDistribution::triangular(one, d("3s"), d("2s")).unwrap_err(),
            "triangular: `max` must be at least `mode` and longer than `min`, found 2s; \
             example: {dist: triangular, min: 40s, mode: 42s, max: 50s}",
        ),
        (
            DurationDistribution::triangular(one, one, one).unwrap_err(),
            "triangular: `max` must be at least `mode` and longer than `min`, found 1s; \
             example: {dist: triangular, min: 40s, mode: 42s, max: 50s}",
        ),
        (
            DurationDistribution::gamma(zero, 0.5).unwrap_err(),
            "gamma: `mean` must be longer than 0s, found 0s; \
             example: {dist: gamma, mean: 12min, cv: 0.5}",
        ),
        (
            DurationDistribution::gamma(one, 0.0).unwrap_err(),
            "gamma: `cv` must be a finite number > 0, found 0; \
             example: {dist: gamma, mean: 12min, cv: 0.5}",
        ),
        (
            DurationDistribution::erlang(zero, 3).unwrap_err(),
            "erlang: `mean` must be longer than 0s, found 0s; \
             example: {dist: erlang, mean: 12min, k: 3}",
        ),
        (
            DurationDistribution::erlang(one, 0).unwrap_err(),
            "erlang: `k` must be a whole number from 1 to 1000, found 0; \
             example: {dist: erlang, mean: 12min, k: 3}",
        ),
        (
            DurationDistribution::erlang(one, MAX_ERLANG_PHASES + 1).unwrap_err(),
            "erlang: `k` must be a whole number from 1 to 1000, found 1001; \
             example: {dist: erlang, mean: 12min, k: 3}",
        ),
        (
            DurationDistribution::weibull(zero, 1.5).unwrap_err(),
            "weibull: `scale` must be longer than 0s, found 0s; \
             example: {dist: weibull, scale: 6h, shape: 1.5}",
        ),
        (
            DurationDistribution::weibull(one, f64::INFINITY).unwrap_err(),
            "weibull: `shape` must be a finite number > 0, found inf; \
             example: {dist: weibull, scale: 6h, shape: 1.5}",
        ),
    ];
    for (err, message) in checks {
        assert_eq!(err.to_string(), message);
    }
    assert!(DurationDistribution::erlang(one, MAX_ERLANG_PHASES).is_ok());
}

#[test]
fn display_uses_the_model_file_form() {
    let shown = [
        (DurationDistribution::constant(d("45s")), "45s"),
        (
            DurationDistribution::exponential(d("12min")).unwrap(),
            "{dist: exponential, mean: 12min}",
        ),
        (
            DurationDistribution::normal(d("42s"), d("3s")).unwrap(),
            "{dist: normal, mean: 42s, sd: 3s}",
        ),
        (
            DurationDistribution::lognormal(d("42s"), 0.08).unwrap(),
            "{dist: lognormal, mean: 42s, sigma: 0.08}",
        ),
        (
            DurationDistribution::uniform(d("40s"), d("50s")).unwrap(),
            "{dist: uniform, min: 40s, max: 50s}",
        ),
        (
            DurationDistribution::triangular(d("40s"), d("42s"), d("50s")).unwrap(),
            "{dist: triangular, min: 40s, mode: 42s, max: 50s}",
        ),
        (
            DurationDistribution::gamma(d("12min"), 0.5).unwrap(),
            "{dist: gamma, mean: 12min, cv: 0.5}",
        ),
        (
            DurationDistribution::erlang(d("12min"), 3).unwrap(),
            "{dist: erlang, mean: 12min, k: 3}",
        ),
        (
            DurationDistribution::weibull(d("6h"), 1.5).unwrap(),
            "{dist: weibull, scale: 6h, shape: 1.5}",
        ),
    ];
    for (dist, text) in shown {
        assert_eq!(dist.to_string(), text);
    }
}

#[test]
fn samples_beyond_simulation_time_are_errors() {
    let mut s = Stream::new(1, StreamKey::new("a", "huge"));
    // (-ln u)^(1/shape) overflows to infinity for about 13 % of draws with
    // shape 0.001. (A lognormal cannot: its mean is at most ~584 years, and
    // a large sigma pushes most draws towards zero, not infinity.)
    let steep = DurationDistribution::weibull(d("1s"), 0.001).unwrap();
    let not_finite = (0..1000)
        .filter(|_| matches!(steep.sample(&mut s), Err(UnitError::NotFinite { .. })))
        .count();
    assert!(not_finite > 0);
    // A mean of 100 years: about 0.3 % of draws exceed the ~584-year limit,
    // so 10,000 draws contain some (the chance of none is below 10^-12).
    let long = DurationDistribution::exponential(d("36500d")).unwrap();
    let overflows = (0..10_000)
        .filter(|_| matches!(long.sample(&mut s), Err(UnitError::Overflow { .. })))
        .count();
    assert!(overflows > 0);
}
