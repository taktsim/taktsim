//! `Distribution::mean`: the mean of what the sampler draws (ADR-0023 §6). The
//! pinned values were computed apart from this crate, by Python's `math` module
//! from the formulas of the ADR.

use taktsim_core::random::{Stream, StreamKey};
use taktsim_model::Distribution;
use taktsim_units::SimDuration;

#[allow(clippy::unwrap_used)] // Test helper: the texts are valid.
fn d(text: &str) -> SimDuration {
    text.parse().unwrap()
}

#[test]
fn means_are_pinned_per_distribution() {
    let cases = [
        (Distribution::Constant(d("45s")), 45_000_000_000),
        (
            Distribution::Exponential { mean: d("12min") },
            720_000_000_000,
        ),
        (
            Distribution::LogNormal {
                mean: d("42s"),
                sigma: 0.08,
            },
            42_000_000_000,
        ),
        (
            Distribution::Gamma {
                mean: d("12min"),
                cv: 0.5,
            },
            720_000_000_000,
        ),
        (
            Distribution::Erlang {
                mean: d("12min"),
                k: 3,
            },
            720_000_000_000,
        ),
        // Rounded down in integers.
        (
            Distribution::Uniform {
                min: SimDuration::from_nanos(1),
                max: SimDuration::from_nanos(4),
            },
            2,
        ),
        (
            Distribution::Triangular {
                min: SimDuration::from_nanos(1),
                mode: SimDuration::from_nanos(2),
                max: SimDuration::from_nanos(4),
            },
            2,
        ),
        // No overflow in the sum.
        (
            Distribution::Uniform {
                min: SimDuration::MAX,
                max: SimDuration::MAX,
            },
            u64::MAX,
        ),
        (
            Distribution::Weibull {
                scale: d("6h"),
                shape: 1.5,
            },
            WEIBULL,
        ),
        (
            Distribution::Normal {
                mean: d("10s"),
                sd: d("20s"),
            },
            NORMAL_WIDE,
        ),
        (
            Distribution::Normal {
                mean: d("42s"),
                sd: d("3s"),
            },
            NORMAL_NARROW,
        ),
        (
            Distribution::Normal {
                mean: d("1s"),
                sd: d("1s"),
            },
            NORMAL_EQUAL,
        ),
        // No spread at all: the parameter.
        (
            Distribution::Normal {
                mean: d("10s"),
                sd: SimDuration::ZERO,
            },
            10_000_000_000,
        ),
    ];
    for (distribution, nanos) in cases {
        assert_eq!(
            distribution.mean(),
            Some(SimDuration::from_nanos(nanos)),
            "{distribution}"
        );
    }
}

// `floor(21600e9 * math.gamma(1 + 1 / 1.5))`.
const WEIBULL: u64 = 19_499_298_327_740;
// `floor(m + s * phi(z) / Phi(z))` with `z = m / s`, as in the ADR.
const NORMAL_WIDE: u64 = 20_183_208_676;
const NORMAL_NARROW: u64 = 42_000_000_000;
const NORMAL_EQUAL: u64 = 1_287_599_970;

#[test]
fn a_mean_beyond_a_duration_has_none() {
    let huge = Distribution::Weibull {
        scale: SimDuration::MAX,
        shape: 0.1,
    };
    assert_eq!(huge.mean(), None);
    assert_eq!(
        Distribution::Normal {
            mean: SimDuration::MAX,
            sd: SimDuration::MAX,
        }
        .mean(),
        None
    );
}

#[test]
fn truncated_normal_mean_matches_its_draws() {
    // A fixed seed, and a bound of about four standard errors of the sample mean.
    let distribution = Distribution::Normal {
        mean: d("10s"),
        sd: d("20s"),
    };
    let sampler = distribution.sampler().unwrap();
    let mut stream = Stream::new(20_231_108, StreamKey::new("weld", "cycle_time"));
    let n = 400_000_u32;
    let mut sum = 0.0;
    for _ in 0..n {
        sum += sampler.sample(&mut stream).unwrap().as_secs_f64();
    }
    let drawn = sum / f64::from(n);
    let mean = distribution.mean().unwrap().as_secs_f64();
    // The truncated distribution's standard deviation is below 15 s.
    let bound = 4.0 * 15.0 / f64::from(n).sqrt();
    assert!((drawn - mean).abs() < bound, "{drawn} against {mean}");
    // Far from the declared mean: the difference is what the rule is for.
    assert!((drawn - 10.0).abs() > 10.0, "{drawn}");
}

#[test]
fn every_mean_of_a_loaded_distribution_matches_its_draws() {
    let cases = [
        Distribution::Exponential { mean: d("12s") },
        Distribution::LogNormal {
            mean: d("42s"),
            sigma: 0.5,
        },
        Distribution::Gamma {
            mean: d("12s"),
            cv: 0.5,
        },
        Distribution::Erlang {
            mean: d("12s"),
            k: 3,
        },
        Distribution::Uniform {
            min: d("1s"),
            max: d("3s"),
        },
        Distribution::Triangular {
            min: d("1s"),
            mode: d("2s"),
            max: d("6s"),
        },
        Distribution::Weibull {
            scale: d("10s"),
            shape: 1.5,
        },
        Distribution::Normal {
            mean: d("2s"),
            sd: d("3s"),
        },
    ];
    let n = 200_000_u32;
    for distribution in cases {
        let sampler = distribution.sampler().unwrap();
        let mut stream = Stream::new(7, StreamKey::new("m", "t"));
        let mut sum = 0.0;
        for _ in 0..n {
            sum += sampler.sample(&mut stream).unwrap().as_secs_f64();
        }
        let drawn = sum / f64::from(n);
        let mean = distribution.mean().unwrap().as_secs_f64();
        assert!(
            (drawn - mean).abs() < 0.01 * mean,
            "{distribution}: {drawn} against {mean}"
        );
    }
}
