//! Acceptance criterion A2 at the statistics level (spec §6, ADR-0007): on
//! cases with a known true mean, 95 % confidence intervals must contain it
//! in 930 to 970 of 1,000 seeded experiments.
//!
//! Each experiment draws its replication results from its own stream
//! (seed = experiment number), so every count below is fixed: the test
//! passes or fails the same way on every run and platform. The expected
//! counts and their spread come from theory or from simulations in
//! numpy/scipy that do not use this crate.

use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
use taktsim_stats::{
    ConfidenceInterval, ConfidenceLevel, ReplicationControl, ReplicationPlan, ReplicationStatus,
};
use taktsim_units::SimDuration;

const EXPERIMENTS: u64 = 1000;

/// How many of the experiments' 95 % intervals contain `truth`; `results`
/// gives the replication results of one experiment from its stream.
#[allow(clippy::expect_used)] // test helper
fn covered(purpose: &str, truth: f64, mut results: impl FnMut(&mut Stream) -> Vec<f64>) -> u64 {
    (0..EXPERIMENTS)
        .filter(|&seed| {
            let mut stream = Stream::new(seed, StreamKey::new("ci-coverage", purpose));
            let values = results(&mut stream);
            ConfidenceInterval::from_values(&values, ConfidenceLevel::DEFAULT)
                .expect("finite results")
                .contains(truth)
        })
        .count()
        .try_into()
        .expect("at most 1000")
}

/// Normal replication results, 5 replications per experiment.
///
/// For normal data the t interval covers the mean with probability exactly
/// 95 % for every n ≥ 2, so the count is Binomial(1000, 0.95): mean 950,
/// standard deviation 6.9. The window [930, 970] is ±2.9 standard
/// deviations, which a correct implementation meets with probability
/// 99.7 %. n = 5 makes the test sharp: with the normal quantile 1.96 in
/// place of t(4, 0.975) = 2.776, coverage would be 87.8 % (scipy:
/// 2·t.cdf(1.96, 4) − 1), far outside the window.
#[test]
fn normal_results_give_95_percent_coverage() {
    let (mu, sigma) = (100.0, 15.0);
    let count = covered("normal", mu, |stream| {
        (0..5)
            .map(|_| mu + sigma * stream.next_standard_normal())
            .collect()
    });
    println!("normal, n = 5: {count} of 1000 intervals cover the mean");
    assert!((930..=970).contains(&count), "{count} of 1000");
}

/// Each replication result is the mean of 50 exponential durations with
/// mean 2 s (e.g. the mean cycle time of one run), 10 replications per
/// experiment.
///
/// A mean of 50 exponentials is Gamma(50) / 50, skewness 2/√50 = 0.28, so
/// it is only approximately normal and the t interval slightly
/// undercovers: 2,000,000 experiments in numpy (gamma(shape 50, scale
/// 1/50), t(9, 0.975) from scipy) give coverage 0.94874 ± 0.00016. The
/// count then has mean 948.7 and standard deviation 7.0; [930, 970] is
/// −2.7/+3.0 standard deviations, met with probability 99.5 %. Durations
/// are rounded to 1 ns by the sampler; at 2 s that moves the mean by far
/// less than 10⁻⁹ relative.
#[test]
fn means_of_exponentials_give_about_95_percent_coverage() {
    let exponential =
        DurationDistribution::exponential(SimDuration::from_secs(2).unwrap()).unwrap();
    let count = covered("exponential", 2.0, |stream| {
        (0..10)
            .map(|_| {
                let total: f64 = (0..50)
                    .map(|_| exponential.sample(stream).unwrap().as_secs_f64())
                    .sum();
                total / 50.0
            })
            .collect()
    });
    println!("means of 50 exponentials, n = 10: {count} of 1000 intervals cover the mean");
    assert!((930..=970).contains(&count), "{count} of 1000");
}

/// The sequential procedure on normal results (mean 10, sd 2) with target
/// precision 5 %, n₀ = 10 and a budget of 1000: its final intervals
/// cover the mean slightly less often than 95 %, because stopping depends
/// on the sample standard deviation (Law 2015, §9.4.1).
///
/// The procedure's coverage and stopping point, simulated with
/// numpy/scipy in 200,000 experiments with the same rule: coverage
/// 0.94349 ± 0.00052, mean number of replications 62.36 with standard
/// deviation 12.3 per experiment (theory: (1.96 · 2 / (0.05 · 10))² = 61.5).
/// Over 1000 experiments the count has mean 943.5 and standard deviation
/// 7.3, and the mean number of replications has standard deviation 0.39;
/// both windows below are ±4 standard deviations. This checks the
/// procedure itself; criterion A2 (930–970) applies to the fixed-n
/// intervals above.
#[test]
fn sequential_procedure_reaches_its_precision_with_near_nominal_coverage() {
    let (mu, sigma) = (10.0, 2.0);
    let plan = ReplicationPlan::new("5%".parse().unwrap(), 10, 1000).unwrap();
    let mut count = 0_u32;
    let mut replications = 0_u32;
    for seed in 0..EXPERIMENTS {
        let mut stream = Stream::new(seed, StreamKey::new("ci-coverage", "sequential"));
        let mut control = ReplicationControl::new(plan);
        while control
            .push(mu + sigma * stream.next_standard_normal())
            .unwrap()
            == ReplicationStatus::Continue
        {}
        let report = control.report().unwrap();
        assert!(report.precision_reached(), "seed {seed} ran out of budget");
        assert!(report.relative_half_width().unwrap() <= 0.05);
        count += u32::from(report.interval().contains(mu));
        replications += report.interval().replications();
    }
    let mean_replications = f64::from(replications) / 1000.0;
    println!(
        "sequential: {count} of 1000 cover the mean; {mean_replications} replications on average"
    );
    assert!((914..=973).contains(&count), "{count} of 1000");
    assert!(
        (60.8..=63.9).contains(&mean_replications),
        "{mean_replications}"
    );
}
