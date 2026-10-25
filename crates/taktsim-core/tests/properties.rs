//! Properties of streams and distributions over generated inputs.

use proptest::prelude::*;
use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
use taktsim_units::{SimDuration, UnitError};

/// Durations from 1 ms to 100 days.
fn duration() -> impl Strategy<Value = SimDuration> {
    (1_000_000_u64..8_640_000_000_000_000).prop_map(SimDuration::from_nanos)
}

/// Any duration simulation time can hold, from 1 ns to ~584 years.
fn any_duration() -> impl Strategy<Value = SimDuration> {
    (1_u64..=u64::MAX).prop_map(SimDuration::from_nanos)
}

/// Positive finite numbers across every magnitude, 1e-300 to 1e300.
fn any_positive() -> impl Strategy<Value = f64> {
    (-300.0_f64..300.0).prop_map(|e| libm::pow(10.0, e))
}

proptest! {
    #[test]
    fn uniforms_stay_inside_the_open_interval(seed: u64, component in ".{0,12}", purpose in ".{0,12}") {
        let mut s = Stream::new(seed, StreamKey::new(&component, &purpose));
        for _ in 0..64 {
            let u = s.next_open01();
            prop_assert!(u > 0.0 && u < 1.0);
        }
    }

    #[test]
    fn a_stream_is_reproducible_and_independent_of_others(seed: u64, n in 1_usize..64) {
        let key = StreamKey::new("cut", "cycle_time");
        let mut alone = Stream::new(seed, key);
        let expected: Vec<u64> = (0..n).map(|_| alone.next_u64()).collect();
        // Interleave draws from another stream: this one is unaffected.
        let mut a = Stream::new(seed, key);
        let mut other = Stream::new(seed, StreamKey::new("weld", "cycle_time"));
        let interleaved: Vec<u64> = (0..n).map(|_| { other.next_u64(); a.next_u64() }).collect();
        prop_assert_eq!(interleaved, expected);
    }

    #[test]
    fn uniform_and_triangular_samples_stay_in_their_support(
        a in any_duration(), b in any_duration(), c in any_duration(), seed: u64
    ) {
        let mut sorted = [a, b, c];
        sorted.sort();
        let [min, mode, max] = sorted;
        prop_assume!(min < max);
        let mut s = Stream::new(seed, StreamKey::new("p", "support"));
        let uniform = DurationDistribution::uniform(min, max).unwrap();
        let triangular = DurationDistribution::triangular(min, mode, max).unwrap();
        for _ in 0..32 {
            let x = uniform.sample(&mut s).unwrap();
            prop_assert!(min <= x && x <= max, "uniform {x} outside [{min}, {max}]");
            let y = triangular.sample(&mut s).unwrap();
            prop_assert!(min <= y && y <= max, "triangular {y} outside [{min}, {max}]");
        }
    }

    // Realistic tails only: a Weibull with shape 0.05 or a lognormal with a
    // large sigma can exceed the ~584 years simulation time can hold, which
    // is an error tested in tests/distributions.rs.
    #[test]
    fn positive_parameters_always_sample(
        mean in duration(), spread in 0.05_f64..1.0, shape in 0.5_f64..20.0, k in 1_u32..50, seed: u64
    ) {
        let mut s = Stream::new(seed, StreamKey::new("p", "any"));
        let dists = [
            DurationDistribution::exponential(mean).unwrap(),
            DurationDistribution::normal(mean, mean).unwrap(),
            DurationDistribution::lognormal(mean, spread).unwrap(),
            DurationDistribution::gamma(mean, 2.0 * spread).unwrap(),
            DurationDistribution::erlang(mean, k).unwrap(),
            DurationDistribution::weibull(mean, shape).unwrap(),
        ];
        for dist in &dists {
            for _ in 0..8 {
                prop_assert!(dist.sample(&mut s).is_ok(), "{dist}");
            }
        }
    }

    // Every constructible distribution samples or reports an overflow; it
    // never panics or hangs, whatever its parameters. (This catches the
    // gamma loop that never ended for cv = 1e-160.)
    #[test]
    fn extreme_parameters_sample_or_report_overflow(
        mean in any_duration(), x in any_positive(), k in 1_u32..=1000, seed: u64
    ) {
        let mut s = Stream::new(seed, StreamKey::new("p", "extreme"));
        let dists = [
            DurationDistribution::exponential(mean).ok(),
            DurationDistribution::normal(mean, mean).ok(),
            DurationDistribution::lognormal(mean, x).ok(),
            DurationDistribution::gamma(mean, x).ok(),
            DurationDistribution::erlang(mean, k).ok(),
            DurationDistribution::weibull(mean, x).ok(),
        ];
        for dist in dists.iter().flatten() {
            for _ in 0..4 {
                match dist.sample(&mut s) {
                    Ok(_) | Err(UnitError::Overflow { .. } | UnitError::NotFinite { .. }) => {}
                    Err(other) => prop_assert!(false, "{dist}: {other}"),
                }
            }
        }
    }
}
