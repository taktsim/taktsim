//! Every way a statistic can be refused: a typed error whose message says
//! what was found, what is allowed and how to fix it (spec §5). Nothing
//! panics.

use taktsim_stats::{
    ConfidenceInterval, ConfidenceLevel, Probability, ReplicationControl, ReplicationPlan,
    StatsError, StudentT, Summary, Window, mser5, mser5_timed, time_weighted,
};
use taktsim_units::{Ratio, SimTime};

#[allow(clippy::expect_used)] // test helper
fn t(text: &str) -> SimTime {
    text.parse().expect("valid time")
}

#[test]
fn probabilities_and_levels() {
    let err = Probability::new(1.5).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`p` must be a number strictly between 0 and 1, found 1.5; example: 0.975"
    );
    let err = ConfidenceLevel::new(95.0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`level` must be a number strictly between 0 and 1, found 95; example: 0.95"
    );
    let err = ConfidenceLevel::from_alpha(0.0).unwrap_err();
    assert!(err.to_string().contains("`alpha`"));
    let err = ConfidenceLevel::DEFAULT.bonferroni(0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`k` must be a whole number >= 1, found 0; example: 10"
    );
    let err = StudentT::new(0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`degrees_of_freedom` must be a whole number >= 1, found 0; example: 9"
    );
    let level = ConfidenceLevel::try_from("99.5%".parse::<Ratio>().unwrap()).unwrap();
    assert_eq!(level.to_string(), "99.5%");
    let err = ConfidenceLevel::try_from("100%".parse::<Ratio>().unwrap()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`level` must be a percentage strictly between 0% and 100%, found 100%; example: 95%"
    );
    let err = ConfidenceLevel::from_alpha(1e-310).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`alpha` = 1e-310 is below the smallest supported value 4.450147717014403e-308; \
         example: 0.05"
    );
    let err = Probability::new(1e-310).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`p` = 1e-310 is below the smallest supported value 2.2250738585072014e-308; \
         example: 0.025"
    );
}

#[test]
fn too_few_or_bad_replications() {
    for values in [&[][..], &[1.0][..]] {
        assert!(matches!(
            ConfidenceInterval::from_values(values, ConfidenceLevel::DEFAULT),
            Err(StatsError::TooFewReplications { required: 2, .. })
        ));
    }
    let err = Summary::from_values(&[1.0, f64::NAN]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "replication results: value 1 is NaN; only finite numbers are allowed"
    );
    let err = Summary::from_values(&[f64::MAX, -f64::MAX]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the sum of squared deviations of the replication results overflows f64 at inputs \
         of magnitude 1.7976931348623157e308; allowed: results whose spread is below about \
         1e154; example: express the KPI in a larger unit, e.g. hours instead of nanoseconds"
    );
    // The spread fits, but the half-width at an extreme level does not;
    // the magnitude is s = √(2e300) as f64 (Python: math.sqrt(2e300)).
    let summary = Summary::from_values(&[1e150, -1e150]).unwrap();
    let extreme = ConfidenceLevel::from_alpha(1e-300).unwrap();
    let err = summary.confidence_interval(extreme).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the confidence interval half-width overflows f64 at inputs of magnitude \
         1.414213562373095e150; allowed: a half-width t(n - 1, 1 - alpha/2) * s / sqrt(n) \
         below 1.8e308; example: rescale the results, or use a level such as 95%"
    );
}

#[test]
fn replication_plans_and_budgets() {
    let err = ReplicationPlan::new("0%".parse().unwrap(), 10, 100).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`precision` must be above 0%, found 0%; example: 2%"
    );
    for (min, max) in [(1, 100), (0, 100), (11, 10)] {
        let err = ReplicationPlan::new("2%".parse().unwrap(), min, max).unwrap_err();
        assert_eq!(
            err,
            StatsError::MinReplicationsOutOfRange { found: min, max }
        );
    }
    let err = ReplicationPlan::new("2%".parse().unwrap(), 11, 10).unwrap_err();
    assert_eq!(
        err.to_string(),
        "`min_replications` must be between 2 and the budget `max_replications` = 10, \
         found 11; example: 10 with max_replications 100"
    );
    let plan = ReplicationPlan::new("2%".parse().unwrap(), 2, 2).unwrap();
    let mut control = ReplicationControl::new(plan);
    control.push(1.0).unwrap();
    control.push(5.0).unwrap();
    let err = control.push(3.0).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the budget of 2 replications is used up; raise `max_replications` to run more, e.g. 4"
    );
    let err = ReplicationControl::new(plan).report().unwrap_err();
    assert!(matches!(
        err,
        StatsError::TooFewReplications { found: 0, .. }
    ));
}

#[test]
fn windows_and_signals() {
    let err = Window::new(t("1h"), t("1h")).unwrap_err();
    assert!(matches!(err, StatsError::EmptyWindow { .. }));
    let window = Window::new(t("1h"), t("2h")).unwrap();
    let err = window.with_warm_up_end(t("2h")).unwrap_err();
    assert_eq!(
        err.to_string(),
        "the warm-up must end in [1h, 2h), the window without its end, found 2h; example: 1h"
    );
    let err = window.with_warm_up("1h".parse().unwrap()).unwrap_err();
    assert!(matches!(err, StatsError::WarmUpOutsideWindow { .. }));
    let err = time_weighted(&[(t("1h"), f64::INFINITY)], window).unwrap_err();
    assert_eq!(
        err.to_string(),
        "signal changes: value 0 is inf; only finite numbers are allowed"
    );
    let err = time_weighted(&[(t("90min"), 1.0)], window).unwrap_err();
    assert_eq!(
        err,
        StatsError::NoValueAtStart {
            start: t("1h"),
            first: Some(t("90min"))
        }
    );
}

#[test]
fn warm_up_series() {
    let err = mser5(&[1.0; 10]).unwrap_err();
    assert_eq!(
        err,
        StatsError::SeriesTooShort {
            what: "MSER-5",
            found: 10,
            required: 15
        }
    );
    let mut series: Vec<(SimTime, f64)> = (0..30).map(|i| (SimTime::from_nanos(i), 1.0)).collect();
    series[20].0 = SimTime::from_nanos(3);
    let err = mser5_timed(&series).unwrap_err();
    assert_eq!(
        err.to_string(),
        "MSER-5 observations: times must not decrease, but value 20 at 0.000000003s \
         follows one at 0.000000019s"
    );
    series[20].0 = SimTime::from_nanos(20);
    series[5].1 = f64::NAN;
    assert!(matches!(
        mser5_timed(&series),
        Err(StatsError::NotFinite { index: 5, .. })
    ));
    series[5].1 = 1.0;
    let cut = mser5_timed(&series).unwrap();
    assert_eq!(cut.truncation().deleted_observations(), 0);
    assert_eq!(cut.warm_up_end(), None);
}

#[test]
fn intervals_display_their_uncertainty() {
    let ci =
        ConfidenceInterval::from_values(&[9.0, 11.0], ConfidenceLevel::new(0.9).unwrap()).unwrap();
    // t(1, 0.95) = 6.3138 (mpmath), s = √2, n = 2: half-width 6.3138.
    assert_eq!(
        format!("{ci:.3}"),
        "10.000 ± 6.314 (90% CI, 2 replications)"
    );
    assert!(ci.to_string().starts_with("10 ± 6.31375151467"));
}
