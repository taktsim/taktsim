//! Sequential replication control to a relative precision (spec §6;
//! ADR-0014).
//!
//! The procedure is the relative-precision sequential procedure of Law,
//! *Simulation Modeling and Analysis* (5th ed., 2015, §9.4.1), after Law,
//! Kelton & Koenig (1981): run n₀ replications; after each further one,
//! stop as soon as the half-width of the confidence interval is within the
//! target fraction γ of the magnitude of the mean, or when the budget is
//! used up.
//!
//! The stop rule is the spec's: half-width ≤ γ·|mean| (the YAML example
//! "replications until the CI half-width is within 2 %"). Law states the
//! rule with γ/(1 + γ) in place of γ, which bounds the error relative to
//! the true mean instead; ADR-0014 records the choice.

use taktsim_units::Ratio;

use crate::{ConfidenceInterval, ConfidenceLevel, StatsError, Summary};

/// What replication control is asked to do: the target relative precision,
/// the confidence level, and the minimum and maximum number of
/// replications.
///
/// ```
/// use taktsim_stats::{ConfidenceLevel, ReplicationPlan};
///
/// let n0 = ReplicationPlan::DEFAULT_MIN_REPLICATIONS;
/// let plan = ReplicationPlan::new("2%".parse()?, n0, 200)?;
/// assert_eq!(plan.min_replications(), 10);
/// assert_eq!(plan.max_replications(), 200);
/// assert_eq!(plan.level(), ConfidenceLevel::DEFAULT);
/// let plan = plan.with_level(ConfidenceLevel::new(0.99)?);
/// assert_eq!(plan.level().to_string(), "99%");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplicationPlan {
    precision: Ratio,
    level: ConfidenceLevel,
    min_replications: u32,
    max_replications: u32,
}

impl ReplicationPlan {
    /// The default n₀, 10: below it the sample standard deviation is too
    /// unreliable for the stop rule; Law (2015, §9.4.1) recommends
    /// n₀ ≥ 10 (with γ ≤ 0.15) after the coverage study of Law, Kelton &
    /// Koenig (1981). See ADR-0014.
    ///
    /// ```
    /// assert_eq!(taktsim_stats::ReplicationPlan::DEFAULT_MIN_REPLICATIONS, 10);
    /// ```
    pub const DEFAULT_MIN_REPLICATIONS: u32 = 10;

    /// Replications until the half-width at 95 % is within `precision` of
    /// the mean: at least n₀ = `min_replications`, at most
    /// `max_replications` (the budget).
    ///
    /// # Errors
    ///
    /// [`StatsError::PrecisionNotPositive`] for a precision of 0 %,
    /// [`StatsError::MinReplicationsOutOfRange`] unless
    /// 2 ≤ `min_replications` ≤ `max_replications`.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationPlan, StatsError};
    ///
    /// assert!(matches!(
    ///     ReplicationPlan::new("0%".parse()?, 10, 100),
    ///     Err(StatsError::PrecisionNotPositive { .. })
    /// ));
    /// assert!(matches!(
    ///     ReplicationPlan::new("2%".parse()?, 10, 5),
    ///     Err(StatsError::MinReplicationsOutOfRange { found: 10, max: 5 })
    /// ));
    /// assert!(ReplicationPlan::new("2%".parse()?, 1, 5).is_err());
    /// assert!(ReplicationPlan::new("2%".parse()?, 5, 5).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        precision: Ratio,
        min_replications: u32,
        max_replications: u32,
    ) -> Result<Self, StatsError> {
        if precision.as_fraction() <= 0.0 {
            return Err(StatsError::PrecisionNotPositive { found: precision });
        }
        if min_replications < 2 || min_replications > max_replications {
            return Err(StatsError::MinReplicationsOutOfRange {
                found: min_replications,
                max: max_replications,
            });
        }
        Ok(Self {
            precision,
            level: ConfidenceLevel::DEFAULT,
            min_replications,
            max_replications,
        })
    }

    /// The same plan at confidence level `level`.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceLevel, ReplicationPlan};
    ///
    /// let level = ConfidenceLevel::new(0.9)?;
    /// let plan = ReplicationPlan::new("5%".parse()?, 10, 50)?.with_level(level);
    /// assert_eq!(plan.level(), level);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn with_level(self, level: ConfidenceLevel) -> Self {
        Self { level, ..self }
    }

    /// The target precision γ: the largest half-width, as a share of the
    /// magnitude of the mean.
    ///
    /// ```
    /// use taktsim_stats::ReplicationPlan;
    ///
    /// let plan = ReplicationPlan::new("2%".parse()?, 10, 100)?;
    /// assert_eq!(plan.precision().as_fraction(), 0.02);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn precision(&self) -> Ratio {
        self.precision
    }

    /// The confidence level of the intervals.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceLevel, ReplicationPlan};
    ///
    /// assert_eq!(ReplicationPlan::new("2%".parse()?, 10, 100)?.level(), ConfidenceLevel::DEFAULT);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn level(&self) -> ConfidenceLevel {
        self.level
    }

    /// n₀: replications before the stop rule is first checked.
    ///
    /// ```
    /// use taktsim_stats::ReplicationPlan;
    ///
    /// assert_eq!(ReplicationPlan::new("2%".parse()?, 10, 100)?.min_replications(), 10);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn min_replications(&self) -> u32 {
        self.min_replications
    }

    /// The budget: the most replications that will be run.
    ///
    /// ```
    /// use taktsim_stats::ReplicationPlan;
    ///
    /// assert_eq!(ReplicationPlan::new("2%".parse()?, 10, 100)?.max_replications(), 100);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn max_replications(&self) -> u32 {
        self.max_replications
    }
}

/// What to do after the latest replication.
///
/// ```
/// use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus};
///
/// let plan = ReplicationPlan::new("1%".parse()?, 2, 3)?;
/// let mut control = ReplicationControl::new(plan);
/// assert_eq!(control.push(10.0)?, ReplicationStatus::Continue);
/// assert_eq!(control.push(20.0)?, ReplicationStatus::Continue);
/// assert_eq!(control.push(30.0)?, ReplicationStatus::BudgetExhausted);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicationStatus {
    /// Run another replication.
    Continue,
    /// The half-width is within the target precision: stop.
    PrecisionReached,
    /// The budget is used up without reaching the precision: stop and
    /// report that the precision was not reached.
    BudgetExhausted,
}

/// Sequential replication control: takes replication results one at a time
/// and says after each whether to continue.
///
/// Push results in replication order (by seed index), not in the order in
/// which parallel replications finish, so that the decision and the
/// statistics are deterministic. With several target KPIs, keep one
/// control per KPI and stop when none says
/// [`Continue`](ReplicationStatus::Continue); a KPI that has reached its
/// precision may take further results.
///
/// ```
/// use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus};
///
/// let plan = ReplicationPlan::new("10%".parse()?, 3, 20)?;
/// let mut control = ReplicationControl::new(plan);
/// let mut results = [50.0, 52.0, 49.0, 51.0].into_iter();
/// while let Some(x) = results.next() {
///     if control.push(x)? != ReplicationStatus::Continue {
///         break;
///     }
/// }
/// let report = control.report()?;
/// assert!(report.precision_reached());
/// assert_eq!(report.interval().replications(), 3);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ReplicationControl {
    plan: ReplicationPlan,
    summary: Summary,
}

impl ReplicationControl {
    /// Control for `plan`, with no results yet.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus};
    ///
    /// let control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 100)?);
    /// assert_eq!(control.status(), ReplicationStatus::Continue);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(plan: ReplicationPlan) -> Self {
        Self {
            plan,
            summary: Summary::new(),
        }
    }

    /// Adds the result of the next replication and returns the new status.
    ///
    /// # Errors
    ///
    /// [`StatsError::BudgetExhausted`] once the budget is used up; as
    /// [`Summary::push`] for a non-finite value. On error nothing changes.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan, StatsError};
    ///
    /// let plan = ReplicationPlan::new("2%".parse()?, 2, 2)?;
    /// let mut control = ReplicationControl::new(plan);
    /// control.push(1.0)?;
    /// control.push(2.0)?;
    /// assert_eq!(control.push(3.0), Err(StatsError::BudgetExhausted { max: 2 }));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn push(&mut self, value: f64) -> Result<ReplicationStatus, StatsError> {
        if self.summary.count() >= self.plan.max_replications {
            return Err(StatsError::BudgetExhausted {
                max: self.plan.max_replications,
            });
        }
        self.summary.push(value)?;
        Ok(self.status())
    }

    /// The status after the results so far.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus};
    ///
    /// let plan = ReplicationPlan::new("2%".parse()?, 2, 10)?;
    /// let mut control = ReplicationControl::new(plan);
    /// control.push(100.0)?;
    /// control.push(100.5)?;
    /// assert_eq!(control.status(), ReplicationStatus::Continue);
    /// control.push(100.2)?;
    /// assert_eq!(control.status(), ReplicationStatus::PrecisionReached);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn status(&self) -> ReplicationStatus {
        let n = self.summary.count();
        if n < self.plan.min_replications {
            return ReplicationStatus::Continue;
        }
        let reached = self
            .summary
            .confidence_interval(self.plan.level)
            .is_ok_and(|ci| self.reached(&ci));
        if reached {
            ReplicationStatus::PrecisionReached
        } else if n >= self.plan.max_replications {
            ReplicationStatus::BudgetExhausted
        } else {
            ReplicationStatus::Continue
        }
    }

    /// The report for the results so far: the confidence interval, the
    /// target and whether it was reached. Available from 2 results on, so
    /// a run stopped early by another budget (e.g. wall-clock time) still
    /// reports its precision.
    ///
    /// # Errors
    ///
    /// As [`Summary::confidence_interval`]: below 2 results, or overflow.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let mut control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 100)?);
    /// control.push(10.0)?;
    /// assert!(control.report().is_err());
    /// control.push(14.0)?;
    /// let report = control.report()?;
    /// assert!(!report.precision_reached());
    /// assert_eq!(report.interval().mean(), 12.0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn report(&self) -> Result<PrecisionReport, StatsError> {
        let interval = self.summary.confidence_interval(self.plan.level)?;
        Ok(PrecisionReport {
            interval,
            precision: self.plan.precision,
            max_replications: self.plan.max_replications,
            reached: self.reached(&interval),
        })
    }

    /// The results so far.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let mut control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 100)?);
    /// control.push(3.0)?;
    /// assert_eq!(control.summary().count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// The plan.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let plan = ReplicationPlan::new("2%".parse()?, 10, 100)?;
    /// assert_eq!(ReplicationControl::new(plan).plan(), &plan);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn plan(&self) -> &ReplicationPlan {
        &self.plan
    }

    /// The stop rule: at least n₀ results and half-width ≤ γ·|mean|, written
    /// without a division so that a mean of 0 with a half-width of 0 counts
    /// as reached.
    fn reached(&self, ci: &ConfidenceInterval) -> bool {
        ci.replications() >= self.plan.min_replications
            && ci.half_width() <= self.plan.precision.as_fraction() * ci.mean().abs()
    }
}

/// The outcome of replication control: the estimate with its interval, the
/// target precision and whether it was reached (spec §6: "The report states
/// the achieved confidence interval half-width and whether the precision
/// was reached").
///
/// ```
/// use taktsim_stats::{ReplicationControl, ReplicationPlan};
///
/// let plan = ReplicationPlan::new("1%".parse()?, 3, 3)?;
/// let mut control = ReplicationControl::new(plan);
/// for x in [90.0, 100.0, 110.0] {
///     control.push(x)?;
/// }
/// let report = control.report()?;
/// assert!(!report.precision_reached());
/// assert_eq!(report.max_replications(), 3);
/// assert!(report.relative_half_width().unwrap() > 0.2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrecisionReport {
    interval: ConfidenceInterval,
    precision: Ratio,
    max_replications: u32,
    reached: bool,
}

impl PrecisionReport {
    /// The estimate and its confidence interval, with the number of
    /// replications.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let mut control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 100)?);
    /// control.push(1.0)?;
    /// control.push(3.0)?;
    /// assert_eq!(control.report()?.interval().replications(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn interval(&self) -> &ConfidenceInterval {
        &self.interval
    }

    /// The achieved precision: half-width / |mean| (see
    /// [`ConfidenceInterval::relative_half_width`]).
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let mut control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 100)?);
    /// control.push(4.0)?;
    /// control.push(4.0)?;
    /// assert_eq!(control.report()?.relative_half_width(), Some(0.0));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn relative_half_width(&self) -> Option<f64> {
        self.interval.relative_half_width()
    }

    /// The target precision γ.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let mut control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 100)?);
    /// control.push(4.0)?;
    /// control.push(5.0)?;
    /// assert_eq!(control.report()?.precision().to_string(), "2%");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn precision(&self) -> Ratio {
        self.precision
    }

    /// The budget of the plan.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let mut control = ReplicationControl::new(ReplicationPlan::new("2%".parse()?, 10, 40)?);
    /// control.push(4.0)?;
    /// control.push(5.0)?;
    /// assert_eq!(control.report()?.max_replications(), 40);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn max_replications(&self) -> u32 {
        self.max_replications
    }

    /// Whether the precision was reached: at least n₀ replications, and the
    /// half-width within the target precision.
    ///
    /// ```
    /// use taktsim_stats::{ReplicationControl, ReplicationPlan};
    ///
    /// let plan = ReplicationPlan::new("50%".parse()?, 3, 40)?;
    /// let mut control = ReplicationControl::new(plan);
    /// control.push(10.0)?;
    /// control.push(10.1)?;
    /// // Within 50 %, but n₀ = 3 replications are not yet done.
    /// assert!(!control.report()?.precision_reached());
    /// control.push(10.2)?;
    /// assert!(control.report()?.precision_reached());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn precision_reached(&self) -> bool {
        self.reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(precision: &str, max: u32, min: u32) -> ReplicationPlan {
        ReplicationPlan::new(precision.parse().unwrap(), min, max).unwrap()
    }

    #[test]
    fn waits_for_min_replications_even_when_precise() {
        let mut control = ReplicationControl::new(plan("50%", 100, 4));
        for _ in 0..3 {
            assert_eq!(control.push(7.0).unwrap(), ReplicationStatus::Continue);
        }
        assert_eq!(
            control.push(7.0).unwrap(),
            ReplicationStatus::PrecisionReached
        );
    }

    #[test]
    fn precision_at_the_last_replication_wins_over_the_budget() {
        let mut control = ReplicationControl::new(plan("50%", 3, 3));
        control.push(10.0).unwrap();
        control.push(10.0).unwrap();
        assert_eq!(
            control.push(10.0).unwrap(),
            ReplicationStatus::PrecisionReached
        );
        assert!(control.report().unwrap().precision_reached());
        assert!(matches!(
            control.push(10.0),
            Err(StatsError::BudgetExhausted { max: 3 })
        ));
    }

    #[test]
    fn a_precise_kpi_can_lose_its_precision_again() {
        let mut control = ReplicationControl::new(plan("5%", 100, 2));
        control.push(100.0).unwrap();
        assert_eq!(
            control.push(100.0).unwrap(),
            ReplicationStatus::PrecisionReached
        );
        assert_eq!(control.push(150.0).unwrap(), ReplicationStatus::Continue);
    }

    #[test]
    fn a_zero_mean_with_spread_never_reaches_precision() {
        let mut control = ReplicationControl::new(plan("50%", 4, 2));
        for x in [-1.0, 1.0, -1.0] {
            assert_eq!(control.push(x).unwrap(), ReplicationStatus::Continue);
        }
        assert_eq!(
            control.push(1.0).unwrap(),
            ReplicationStatus::BudgetExhausted
        );
        let report = control.report().unwrap();
        assert_eq!(report.relative_half_width(), None);
        assert!(!report.precision_reached());
    }

    #[test]
    fn a_non_finite_result_changes_nothing() {
        let mut control = ReplicationControl::new(plan("5%", 10, 2));
        control.push(1.0).unwrap();
        assert!(control.push(f64::NAN).is_err());
        assert_eq!(control.summary().count(), 1);
    }

    #[test]
    fn stops_where_the_half_width_crosses_the_target() {
        // Results 9, 11, 9, 11, … with γ = 10 %. Relative half-width
        // t(n−1, 0.975)·s/(√n·|mean|), by hand with scipy's t quantiles:
        // n = 6: 2.5706·1.0954/√6/10 = 11.5 %; n = 7: mean 9.857,
        // s = 1.0690, 2.4469·1.0690/√7/9.857 = 10.03 %;
        // n = 8: 2.3646·1.0690/√8/10 = 8.9 %. So it stops at n = 8.
        let mut control = ReplicationControl::new(plan("10%", 100, 2));
        let mut n = 0;
        loop {
            n += 1;
            let x = if n % 2 == 1 { 9.0 } else { 11.0 };
            if control.push(x).unwrap() != ReplicationStatus::Continue {
                break;
            }
        }
        assert_eq!(n, 8);
        let report = control.report().unwrap();
        assert!(report.precision_reached());
        let achieved = report.relative_half_width().unwrap();
        assert!((achieved - 0.0894).abs() < 1e-4, "{achieved}");
    }
}
