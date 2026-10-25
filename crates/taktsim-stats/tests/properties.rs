//! Properties of the statistics over generated inputs.

use proptest::collection::vec;
use proptest::prelude::*;
use taktsim_stats::{
    ConfidenceInterval, ConfidenceLevel, StudentT, Summary, TimeWeighted, Window, mser5,
    time_weighted,
};
use taktsim_units::SimTime;

fn relative_error(found: f64, expected: f64) -> f64 {
    if expected == 0.0 {
        found.abs()
    } else {
        ((found - expected) / expected).abs()
    }
}

/// `n` as f64; exact for every count used here.
#[allow(clippy::unwrap_used)] // test helper
fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap())
}

/// Replication results with some spread.
fn results() -> impl Strategy<Value = Vec<f64>> {
    vec(-1e3..1e3_f64, 2..60).prop_filter("spread", |xs| {
        let (lo, hi) = xs
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| {
                (lo.min(x), hi.max(x))
            });
        hi - lo > 1e-3
    })
}

#[allow(clippy::unwrap_used)] // test helper
fn levels() -> impl Strategy<Value = ConfidenceLevel> {
    prop_oneof![
        Just(ConfidenceLevel::DEFAULT),
        (0.5..0.9999_f64).prop_map(|l| ConfidenceLevel::new(l).unwrap()),
        (1_u32..1000).prop_map(|k| ConfidenceLevel::DEFAULT.bonferroni(k).unwrap()),
    ]
}

proptest! {
    /// The half-width is t(n − 1, 1 − α/2) · s/√n.
    #[test]
    fn half_width_is_t_times_the_standard_error(xs in results(), level in levels()) {
        let ci = ConfidenceInterval::from_values(&xs, level).unwrap();
        let n = u32::try_from(xs.len()).unwrap();
        let t = StudentT::new(n - 1).unwrap().critical_value(level);
        let expected = t * ci.std_dev() / f64::from(n).sqrt();
        prop_assert!(relative_error(ci.half_width(), expected) < 1e-14);
        prop_assert!(ci.lower() <= ci.mean() && ci.mean() <= ci.upper());
        prop_assert!(ci.contains(ci.mean()));
    }

    /// The half-width shrinks like 1/√n: the same results repeated k times
    /// (same spread, k times as many replications) give a half-width
    /// smaller by t(kn − 1)/t(n − 1) · √((n − 1)/(kn − 1)), which tends to
    /// 1/√k as n grows.
    #[test]
    fn half_width_shrinks_like_one_over_root_n(xs in results(), k in 1_usize..20) {
        let level = ConfidenceLevel::DEFAULT;
        let one = ConfidenceInterval::from_values(&xs, level).unwrap();
        let repeated: Vec<f64> = xs.iter().copied().cycle().take(k * xs.len()).collect();
        let many = ConfidenceInterval::from_values(&repeated, level).unwrap();
        let n = xs.len();
        let t = |df: usize| StudentT::new(u32::try_from(df).unwrap()).unwrap().critical_value(level);
        let ratio = t(k * n - 1) / t(n - 1) * (count(n - 1) / count(k * n - 1)).sqrt();
        prop_assert!(relative_error(many.half_width(), ratio * one.half_width()) < 1e-10);
        prop_assert!(relative_error(many.mean(), one.mean()) < 1e-12);
        // Never wider, and at least as narrow as 1/√k for k ≥ 2.
        prop_assert!(many.half_width() <= one.half_width() * (1.0 + 1e-12));
        prop_assert!(many.half_width() <= one.half_width() / count(k).sqrt() * (1.0 + 1e-12));
    }

    /// Shifting and scaling the results shifts and scales the interval.
    #[test]
    fn intervals_follow_affine_changes(
        xs in results(),
        scale in prop_oneof![-1e3..-1e-3_f64, 1e-3..1e3_f64],
        shift in -1e3..1e3_f64,
    ) {
        let level = ConfidenceLevel::DEFAULT;
        let ci = ConfidenceInterval::from_values(&xs, level).unwrap();
        let moved: Vec<f64> = xs.iter().map(|x| scale * x + shift).collect();
        let moved_ci = ConfidenceInterval::from_values(&moved, level).unwrap();
        let tolerance = 1e-9 * (1.0 + (shift / scale).abs() / ci.std_dev());
        prop_assert!((moved_ci.mean() - (scale * ci.mean() + shift)).abs()
            <= tolerance * scale.abs() * ci.std_dev());
        prop_assert!(relative_error(moved_ci.half_width(), scale.abs() * ci.half_width()) <= tolerance);
    }

    /// The order of the results changes the summary only by rounding, and
    /// the same order gives bit-identical results (determinism).
    #[test]
    fn order_changes_only_rounding(xs in results(), rotation in 0_usize..60) {
        let a = Summary::from_values(&xs).unwrap();
        let mut rotated = xs.clone();
        rotated.rotate_left(rotation % xs.len());
        let b = Summary::from_values(&rotated).unwrap();
        let spread = a.std_dev().unwrap();
        prop_assert!((a.mean().unwrap() - b.mean().unwrap()).abs() <= 1e-12 * (1e3 + spread));
        prop_assert!(relative_error(b.variance().unwrap(), a.variance().unwrap()) < 1e-9);
        prop_assert_eq!(Summary::from_values(&xs).unwrap(), a);
    }
}

/// A piecewise-constant signal: values at strictly increasing times from 0,
/// and a window end after the last change.
fn signal() -> impl Strategy<Value = (Vec<(SimTime, f64)>, SimTime)> {
    (
        vec((1_u64..10_000_000_000, -1e3..1e3_f64), 1..40),
        1_u64..10_000_000_000,
    )
        .prop_map(|(steps, tail)| {
            let mut time = 0_u64;
            let mut changes = Vec::new();
            for (i, (gap, value)) in steps.into_iter().enumerate() {
                if i > 0 {
                    time += gap;
                }
                changes.push((SimTime::from_nanos(time), value));
            }
            (changes, SimTime::from_nanos(time + tail))
        })
}

#[allow(clippy::unwrap_used)] // test helper
fn stats(changes: &[(SimTime, f64)], window: Window) -> TimeWeighted {
    time_weighted(changes, window).unwrap()
}

proptest! {
    /// A constant signal has its value as mean, and no spread, however its
    /// changes and window fall.
    #[test]
    #[allow(clippy::float_cmp)] // exact equality is the property
    fn constant_signal_has_its_value_as_mean(
        value in -1e6..1e6_f64,
        times in vec(0_u64..1_000_000_000_000, 1..20),
        end in 1_u64..2_000_000_000_000,
    ) {
        let mut times = times;
        times.sort_unstable();
        let changes: Vec<(SimTime, f64)> =
            std::iter::once(0).chain(times).map(|t| (SimTime::from_nanos(t), value)).collect();
        let s = stats(&changes, Window::new(SimTime::START, SimTime::from_nanos(end)).unwrap());
        prop_assert_eq!(s.mean(), value);
        prop_assert_eq!(s.variance(), 0.0);
        prop_assert_eq!((s.min(), s.max()), (value, value));
    }

    /// Splitting a segment into two of the same value changes nothing, bit
    /// for bit.
    #[test]
    fn splitting_a_segment_changes_nothing(
        (changes, end) in signal(),
        at in 0_u64..u64::MAX,
    ) {
        let window = Window::new(SimTime::START, end).unwrap();
        let split_time = SimTime::from_nanos(at % end.as_nanos());
        let position = changes.partition_point(|&(t, _)| t <= split_time);
        let current = changes[position - 1].1;
        let mut split = changes.clone();
        split.insert(position, (split_time, current));
        prop_assert_eq!(stats(&split, window), stats(&changes, window));
    }

    /// Splitting the window: the mean over [0, end] is the duration-weighted
    /// mean of the means over [0, m] and [m, end].
    #[test]
    fn mean_over_a_window_combines_its_parts((changes, end) in signal(), at in 1_u64..u64::MAX) {
        let m = SimTime::from_nanos(1 + at % (end.as_nanos() - 1));
        let whole = stats(&changes, Window::new(SimTime::START, end).unwrap());
        let first = stats(&changes, Window::new(SimTime::START, m).unwrap());
        let second = stats(&changes, Window::new(m, end).unwrap());
        let (t1, t2) = (m.as_secs_f64(), end.as_secs_f64() - m.as_secs_f64());
        let combined = (t1 * first.mean() + t2 * second.mean()) / (t1 + t2);
        prop_assert!((whole.mean() - combined).abs() <= 1e-9 * (1.0 + whole.max().abs().max(whole.min().abs())));
    }

    /// The statistics agree with the direct two-pass definition, and stay
    /// within the range of the signal.
    #[test]
    fn statistics_match_the_definition((changes, end) in signal()) {
        let s = stats(&changes, Window::new(SimTime::START, end).unwrap());
        let segments: Vec<(f64, f64)> = changes
            .iter()
            .enumerate()
            .map(|(i, &(t, v))| {
                let until = changes.get(i + 1).map_or(end, |&(next, _)| next);
                (until.as_secs_f64() - t.as_secs_f64(), v)
            })
            .collect();
        let total: f64 = segments.iter().map(|&(d, _)| d).sum();
        let mean = segments.iter().map(|&(d, v)| d * v).sum::<f64>() / total;
        let variance = segments.iter().map(|&(d, v)| d * (v - mean) * (v - mean)).sum::<f64>() / total;
        let scale = 1.0 + s.max().abs().max(s.min().abs());
        prop_assert!((s.mean() - mean).abs() <= 1e-9 * scale);
        prop_assert!((s.variance() - variance).abs() <= 1e-9 * scale * scale);
        prop_assert!(s.min() <= s.mean() + 1e-9 * scale && s.mean() <= s.max() + 1e-9 * scale);
        prop_assert!(s.variance() >= 0.0);
    }

    /// A warm-up excludes exactly its part of the window: the statistics
    /// equal those of the window that starts where the warm-up ends.
    #[test]
    fn warm_up_is_excluded((changes, end) in signal(), at in 0_u64..u64::MAX) {
        let warm_up_end = SimTime::from_nanos(at % end.as_nanos());
        let with_warm_up = Window::new(SimTime::START, end).unwrap().with_warm_up_end(warm_up_end).unwrap();
        let after = Window::new(warm_up_end, end).unwrap();
        let a = stats(&changes, with_warm_up);
        let b = stats(&changes, after);
        prop_assert_eq!((a.mean(), a.variance(), a.min(), a.max()), (b.mean(), b.variance(), b.min(), b.max()));
        prop_assert_eq!(a.window().warm_up(), warm_up_end.checked_duration_since(SimTime::START).unwrap());
    }
}

proptest! {
    /// Scaling the series by a power of two scales every intermediate value
    /// exactly, so the truncation point stays and the statistic scales by
    /// the square.
    #[test]
    #[allow(clippy::float_cmp)] // exact equality is the property
    fn mser_is_scale_invariant(xs in vec(-1e3..1e3_f64, 15..400), exponent in -20_i32..20) {
        let factor = libm::pow(2.0, f64::from(exponent));
        let scaled: Vec<f64> = xs.iter().map(|x| x * factor).collect();
        let a = mser5(&xs).unwrap();
        let b = mser5(&scaled).unwrap();
        prop_assert_eq!(a.deleted_batches(), b.deleted_batches());
        prop_assert_eq!(b.statistic(), a.statistic() * factor * factor);
    }

    /// The search covers the first half only, and observations after the
    /// last complete batch are ignored.
    #[test]
    fn mser_searches_the_first_half_of_complete_batches(
        xs in vec(-1e3..1e3_f64, 15..400),
        extra in vec(-1e6..1e6_f64, 0..5),
    ) {
        let complete = xs.len() - xs.len() % 5;
        let mut padded = xs[..complete].to_vec();
        padded.extend(&extra);
        let a = mser5(&xs[..complete]).unwrap();
        prop_assert_eq!(mser5(&padded).unwrap(), a);
        prop_assert_eq!(a.batches(), complete / 5);
        prop_assert!(a.deleted_batches() <= a.batches() / 2);
        prop_assert_eq!(a.deleted_observations(), 5 * a.deleted_batches());
    }

    /// A transient far above the noise is always deleted completely.
    #[test]
    fn mser_deletes_a_large_transient(
        noise in vec(0.0..1.0_f64, 1000..2000),
        biased_batches in 0_usize..100,
    ) {
        // At least 200 batches, so up to 99 biased ones lie in the first half.
        let series: Vec<f64> = noise
            .iter()
            .enumerate()
            .map(|(i, &x)| if i < 5 * biased_batches { x + 1e6 } else { x })
            .collect();
        prop_assert!(mser5(&series).unwrap().deleted_batches() >= biased_batches);
    }
}
