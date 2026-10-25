//! The benchmark workloads do the work they claim, on inputs that never
//! fail. The gungraun benchmarks rely on this: they stop the run if an input
//! fails (`benches/support/mod.rs`), and a failure found here first is
//! easier to read.
//!
//! Expected values come from the distributions' theory or from hand
//! arithmetic on the inputs, not from running the workloads.

use proptest::prelude::*;
use taktsim_bench::random::{self, DRAWS, DistributionCase, SEED};
use taktsim_bench::units::{
    self, COUNT_TEXTS, COUNTS, DURATION_TEXTS, DURATIONS, RATE_TEXTS, RATIO_TEXTS,
};
use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
use taktsim_units::{Count, Rate, Ratio, SimDuration};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Theoretical mean and standard deviation of one sample, in seconds.
///
/// Truncating the normal at zero changes nothing visible: zero is 14
/// standard deviations below the mean. Lognormal: sd = mean × √(e^σ² − 1).
/// Triangular: var = (a² + b² + c² − ab − ac − bc) / 18 = 84 / 18. Gamma
/// and Erlang: sd = cv × mean, cv = 1/√k for Erlang. Weibull with scale λ
/// and shape k: mean = λ Γ(1 + 1/k), var = λ² (Γ(1 + 2/k) − Γ(1 + 1/k)²),
/// with Γ(5/3) = 0.902745292950934 and Γ(7/3) = (4/3) Γ(4/3) =
/// (4/3) × 0.892979511569249 (Abramowitz and Stegun, table 6.1).
fn theory(case: DistributionCase) -> (f64, f64) {
    match case {
        DistributionCase::Constant => (45.0, 0.0),
        DistributionCase::Exponential => (720.0, 720.0),
        DistributionCase::Normal => (42.0, 3.0),
        DistributionCase::LogNormal => (42.0, 42.0 * (libm::exp(0.08 * 0.08) - 1.0).sqrt()),
        DistributionCase::Uniform => (45.0, 10.0 / 12.0_f64.sqrt()),
        DistributionCase::Triangular => (44.0, (84.0_f64 / 18.0).sqrt()),
        DistributionCase::Gamma => (720.0, 0.5 * 720.0),
        DistributionCase::GammaLowShape => (720.0, 2.0 * 720.0),
        DistributionCase::Erlang => (720.0, 720.0 / 3.0_f64.sqrt()),
        DistributionCase::Weibull => {
            let (lambda, g1) = (21_600.0, 0.902_745_292_950_934);
            let g2 = 4.0 / 3.0 * 0.892_979_511_569_249;
            (lambda * g1, lambda * (g2 - g1 * g1).sqrt())
        }
    }
}

#[test]
fn every_case_builds_with_the_parameters_its_docs_name() {
    let expected = [
        "45s",
        "{dist: exponential, mean: 12min}",
        "{dist: normal, mean: 42s, sd: 3s}",
        "{dist: lognormal, mean: 42s, sigma: 0.08}",
        "{dist: uniform, min: 40s, max: 50s}",
        "{dist: triangular, min: 40s, mode: 42s, max: 50s}",
        "{dist: gamma, mean: 12min, cv: 0.5}",
        "{dist: gamma, mean: 12min, cv: 2}",
        "{dist: erlang, mean: 12min, k: 3}",
        "{dist: weibull, scale: 6h, shape: 1.5}",
    ];
    for (case, text) in DistributionCase::ALL.into_iter().zip(expected) {
        assert_eq!(case.distribution().unwrap().to_string(), text, "{case:?}");
    }
}

#[test]
fn case_names_are_unique_benchmark_ids() {
    let mut names: Vec<&str> = DistributionCase::ALL.iter().map(|c| c.name()).collect();
    assert!(
        names
            .iter()
            .all(|n| { !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_') })
    );
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), DistributionCase::ALL.len());
}

/// The benchmarks' workload, run exactly as a gungraun benchmark runs it
/// (a fresh stream, `DRAWS` samples): it succeeds, and its mean is within
/// 5 standard errors of the theoretical mean. For a normal sample mean,
/// that bound fails with probability 6e-7; the seed is fixed, so the
/// result never changes.
#[test]
fn sample_n_draws_from_each_distribution() {
    for case in DistributionCase::ALL {
        let distribution = case.distribution().unwrap();
        let total = random::sample_n(&distribution, &mut random::stream(), DRAWS).unwrap();
        let (mean, sd) = theory(case);
        let n = f64::from(DRAWS);
        let observed = total.as_secs_f64() / n;
        let bound = 5.0 * sd / n.sqrt();
        assert!(
            (observed - mean).abs() <= bound,
            "{case:?}: sample mean {observed} s, theory {mean} s ± {bound} s"
        );
    }
}

#[test]
fn constant_total_is_exact() {
    let d = DistributionCase::Constant.distribution().unwrap();
    let total = random::sample_n(&d, &mut random::stream(), DRAWS).unwrap();
    assert_eq!(total.as_nanos(), 45 * u64::from(DRAWS) * NANOS_PER_SEC);
}

#[test]
fn sample_n_reports_an_overflowing_total() {
    // 2^64 ns is about 584 years; 1,000 samples of a year cannot fit.
    let year = SimDuration::from_secs(365 * 86_400).unwrap();
    let d = DurationDistribution::constant(year);
    assert!(random::sample_n(&d, &mut random::stream(), 1_000).is_err());
    assert!(random::sample_n(&d, &mut random::stream(), 584).is_ok());
}

/// Means of `DRAWS` uniforms and normals within 5 standard errors:
/// sd(U(0, 1)) = 1/√12, sd(N(0, 1)) = 1.
#[test]
fn stream_workloads_draw_uniforms_and_normals() {
    let n = f64::from(DRAWS);
    let u = random::next_open01_n(&mut random::stream(), DRAWS) / n;
    assert!((u - 0.5).abs() <= 5.0 / 12.0_f64.sqrt() / n.sqrt(), "{u}");
    let z = random::next_standard_normal_n(&mut random::stream(), DRAWS) / n;
    assert!(z.abs() <= 5.0 / n.sqrt(), "{z}");
    // Top bit of each u64: a fair coin, mean 1/2, sd 1/2.
    let mut s = random::stream();
    let ones = (0..DRAWS).filter(|_| s.next_u64() >> 63 == 1).count();
    let share = f64::from(u32::try_from(ones).unwrap()) / n;
    assert!((share - 0.5).abs() <= 5.0 * 0.5 / n.sqrt(), "{share}");
}

#[test]
fn the_benchmark_stream_is_fixed() {
    let mut a = random::stream();
    let mut b = Stream::new(SEED, StreamKey::new("bench", "draws"));
    assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
}

proptest! {
    /// Each stream workload draws exactly `n` values: the stream is then at
    /// the same place as one that drew `n` by hand, and the folded value is
    /// the fold of those draws.
    #[test]
    fn stream_workloads_draw_exactly_n(n in 0_u32..200) {
        let mut by_hand = random::stream();
        let expected = (0..n).fold(0_u64, |s, _| s.wrapping_add(by_hand.next_u64()));
        let mut stream = random::stream();
        prop_assert_eq!(random::next_u64_n(&mut stream, n), expected);
        prop_assert_eq!(stream.next_u64(), by_hand.next_u64());

        // Same additions in the same order, so the sums agree bit for bit.
        let mut by_hand = random::stream();
        let expected = (0..n).fold(0.0, |s, _| s + by_hand.next_open01());
        let mut stream = random::stream();
        prop_assert_eq!(random::next_open01_n(&mut stream, n).to_bits(), expected.to_bits());
        prop_assert_eq!(stream.next_u64(), by_hand.next_u64());

        let mut by_hand = random::stream();
        let expected = (0..n).fold(0.0, |s, _| s + by_hand.next_standard_normal());
        let mut stream = random::stream();
        prop_assert_eq!(
            random::next_standard_normal_n(&mut stream, n).to_bits(),
            expected.to_bits()
        );
        prop_assert_eq!(
            stream.next_standard_normal().to_bits(),
            by_hand.next_standard_normal().to_bits()
        );
    }

    /// A constant's total is the constant times `n`, by integer arithmetic.
    #[test]
    fn constant_total_is_n_times_the_constant(nanos in 0_u64..1_000_000_000_000, n in 0_u32..1_000) {
        let d = DurationDistribution::constant(SimDuration::from_nanos(nanos));
        let total = random::sample_n(&d, &mut random::stream(), n).unwrap();
        prop_assert_eq!(total.as_nanos(), nanos * u64::from(n));
    }

    /// Formatting replaces the buffer's contents with the values' `Display`
    /// text, whatever the buffer held.
    #[test]
    fn format_all_writes_the_display_text(nanos in prop::collection::vec(0_u64..u64::MAX, 0..10), old in ".{0,20}") {
        let values: Vec<SimDuration> = nanos.into_iter().map(SimDuration::from_nanos).collect();
        let mut buffer = old;
        let length = units::format_all(&values, &mut buffer).unwrap();
        let expected: String = values.iter().map(ToString::to_string).collect();
        prop_assert_eq!(length, expected.len());
        prop_assert_eq!(buffer, expected);
    }
}

#[test]
fn every_text_parses_to_the_value_it_names() {
    let s = |n: u64| n * NANOS_PER_SEC;
    let durations: Vec<SimDuration> = units::parsed(&DURATION_TEXTS).unwrap();
    let nanos: Vec<u64> = durations.iter().map(|d| d.as_nanos()).collect();
    assert_eq!(
        nanos,
        [
            s(42),
            s(720),
            s(1_800),
            s(28_800),
            s(172_800),
            250_000_000,
            1_250_000_000,
            s(5_400)
        ]
    );

    let counts: Vec<Count> = units::parsed(&COUNT_TEXTS).unwrap();
    assert_eq!(
        counts.iter().map(|c| c.get()).collect::<Vec<_>>(),
        [20, 1, 500, 12]
    );

    let rates: Vec<Rate> = units::parsed(&RATE_TEXTS).unwrap();
    let per_hour: Vec<f64> = rates.iter().map(|r| r.as_per_hour()).collect();
    for (got, want) in per_hour.iter().zip([120.0, 120.0, 1_800.0, 1.0]) {
        assert!((got - want).abs() < 1e-9, "{got} parts/h, expected {want}");
    }

    let ratios: Vec<Ratio> = units::parsed(&RATIO_TEXTS).unwrap();
    let percent: Vec<f64> = ratios.iter().map(|r| r.as_percent()).collect();
    assert_eq!(percent, [2.0, 0.5, 95.0, 100.0]);

    assert!(units::parse_all::<SimDuration>(&DURATION_TEXTS).is_ok());
    assert!(units::parse_all::<Count>(&COUNT_TEXTS).is_ok());
    assert!(units::parse_all::<Rate>(&RATE_TEXTS).is_ok());
    assert!(units::parse_all::<Ratio>(&RATIO_TEXTS).is_ok());
}

#[test]
fn parse_all_stops_at_the_first_bad_text() {
    assert!(units::parse_all::<SimDuration>(&["42s", "42 parsecs", "1h"]).is_err());
    assert!(units::parsed::<Count>(&["1 part", "1.5 parts"]).is_err());
}

#[test]
fn formats_name_the_values_in_the_model_syntax() {
    let mut buffer = String::new();
    units::format_all(&DURATIONS, &mut buffer).unwrap();
    assert_eq!(buffer, "42s12min30min8h2d0.25s1.25s0.000000001s");
    units::format_all(&COUNTS, &mut buffer).unwrap();
    assert_eq!(buffer, "20 parts1 parts500 parts12 parts");
}

#[test]
fn formatted_rates_and_ratios_parse_back() {
    let mut buffer = String::new();
    for rate in units::parsed::<Rate>(&RATE_TEXTS).unwrap() {
        units::format_all(&[rate], &mut buffer).unwrap();
        assert_eq!(buffer.parse::<Rate>().unwrap(), rate);
    }
    for ratio in units::parsed::<Ratio>(&RATIO_TEXTS).unwrap() {
        units::format_all(&[ratio], &mut buffer).unwrap();
        assert_eq!(buffer.parse::<Ratio>().unwrap(), ratio);
    }
}
