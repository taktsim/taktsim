//! Means and confidence intervals from independent replications.

use std::fmt;

use crate::{ConfidenceLevel, StatsError, StudentT};

/// Count, mean and spread of replication results, updated one result at a
/// time (Welford's algorithm).
///
/// Welford's update is numerically stable: it never subtracts two large
/// sums. The result depends on the order of the values, so push them in a
/// fixed order, e.g. by replication index, never in the order in which
/// parallel replications happen to finish.
///
/// ```
/// use taktsim_stats::Summary;
///
/// let mut s = Summary::new();
/// for x in [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0] {
///     s.push(x)?;
/// }
/// assert_eq!(s.count(), 8);
/// assert_eq!(s.mean(), Some(5.0));
/// assert_eq!(s.variance(), Some(32.0 / 7.0));
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Summary {
    count: u32,
    mean: f64,
    /// Sum of squared deviations from the mean.
    m2: f64,
}

impl Summary {
    /// An empty summary.
    ///
    /// ```
    /// use taktsim_stats::Summary;
    ///
    /// assert_eq!(Summary::new().count(), 0);
    /// assert_eq!(Summary::new().mean(), None);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The summary of `values`, pushed in order.
    ///
    /// # Errors
    ///
    /// As [`push`](Self::push), with the index of the offending value.
    ///
    /// ```
    /// use taktsim_stats::{StatsError, Summary};
    ///
    /// assert_eq!(Summary::from_values(&[1.0, 3.0])?.mean(), Some(2.0));
    /// assert!(matches!(
    ///     Summary::from_values(&[1.0, f64::NAN]),
    ///     Err(StatsError::NotFinite { index: 1, .. })
    /// ));
    /// # Ok::<(), StatsError>(())
    /// ```
    pub fn from_values(values: &[f64]) -> Result<Self, StatsError> {
        let mut summary = Self::new();
        for (index, &value) in values.iter().enumerate() {
            summary.add(value, index)?;
        }
        Ok(summary)
    }

    /// Adds one result. On error the summary is unchanged.
    ///
    /// # Errors
    ///
    /// [`StatsError::NotFinite`] for NaN or infinity (reported at index =
    /// the current count), [`StatsError::Overflow`] if the spread of the
    /// values exceeds `f64`, [`StatsError::TooManyReplications`] after
    /// `u32::MAX` values.
    ///
    /// ```
    /// use taktsim_stats::Summary;
    ///
    /// let mut s = Summary::new();
    /// s.push(1.5)?;
    /// assert!(s.push(f64::INFINITY).is_err());
    /// assert_eq!(s.count(), 1);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn push(&mut self, value: f64) -> Result<(), StatsError> {
        self.add(value, usize::try_from(self.count).unwrap_or(usize::MAX))
    }

    /// [`push`](Self::push), reporting a non-finite value at `index`.
    fn add(&mut self, value: f64, index: usize) -> Result<(), StatsError> {
        if !value.is_finite() {
            return Err(StatsError::NotFinite {
                what: "replication results",
                index,
                found: value,
            });
        }
        let count = self
            .count
            .checked_add(1)
            .ok_or(StatsError::TooManyReplications { max: u32::MAX })?;
        let delta = value - self.mean;
        let mean = self.mean + delta / f64::from(count);
        let m2 = self.m2 + delta * (value - mean);
        if !delta.is_finite() || !mean.is_finite() || !m2.is_finite() {
            return Err(StatsError::Overflow {
                what: "sum of squared deviations",
            });
        }
        *self = Self { count, mean, m2 };
        Ok(())
    }

    /// The number of results.
    ///
    /// ```
    /// use taktsim_stats::Summary;
    ///
    /// assert_eq!(Summary::from_values(&[1.0, 2.0, 3.0])?.count(), 3);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The sample mean; `None` without results.
    ///
    /// ```
    /// use taktsim_stats::Summary;
    ///
    /// assert_eq!(Summary::from_values(&[1.0, 2.0])?.mean(), Some(1.5));
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then_some(self.mean)
    }

    /// The sample variance with divisor n − 1; `None` below 2 results.
    ///
    /// ```
    /// use taktsim_stats::Summary;
    ///
    /// assert_eq!(Summary::from_values(&[1.0, 3.0])?.variance(), Some(2.0));
    /// assert_eq!(Summary::from_values(&[1.0])?.variance(), None);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn variance(&self) -> Option<f64> {
        (self.count > 1).then(|| self.m2 / f64::from(self.count - 1))
    }

    /// The sample standard deviation; `None` below 2 results.
    ///
    /// ```
    /// use taktsim_stats::Summary;
    ///
    /// assert_eq!(Summary::from_values(&[1.0, 3.0])?.std_dev(), Some(2f64.sqrt()));
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn std_dev(&self) -> Option<f64> {
        self.variance().map(f64::sqrt)
    }

    /// The t confidence interval for the mean at `level`:
    /// mean ± t₍ₙ₋₁, 1−α/2₎ · s/√n. Exact for normally distributed results,
    /// and approximately right for means of many observations (central
    /// limit theorem).
    ///
    /// # Errors
    ///
    /// [`StatsError::TooFewReplications`] below 2 results,
    /// [`StatsError::Overflow`] if the half-width exceeds `f64`.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceLevel, Summary};
    ///
    /// let ci = Summary::from_values(&[9.0, 11.0])?.confidence_interval(ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.mean(), 10.0);
    /// // t(1, 0.975) = 12.706…, s = √2, n = 2.
    /// assert!((ci.half_width() - 12.706_204_736_174_7).abs() < 1e-12);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn confidence_interval(
        &self,
        level: ConfidenceLevel,
    ) -> Result<ConfidenceInterval, StatsError> {
        let (Some(std_dev), Ok(t)) = (self.std_dev(), StudentT::new(self.count.saturating_sub(1)))
        else {
            return Err(StatsError::TooFewReplications {
                found: self.count,
                required: 2,
            });
        };
        let half_width = t.critical_value(level) * std_dev / f64::from(self.count).sqrt();
        if !half_width.is_finite() {
            return Err(StatsError::Overflow {
                what: "confidence interval half-width",
            });
        }
        Ok(ConfidenceInterval {
            mean: self.mean,
            half_width,
            std_dev,
            replications: self.count,
            level,
        })
    }
}

/// A point estimate with its confidence interval: mean ± half-width at a
/// confidence level, from a number of independent replications.
///
/// There is no way to get the mean without the interval, so a reported
/// number always carries its uncertainty (spec §5).
///
/// ```
/// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
///
/// let ci = ConfidenceInterval::from_values(&[98.0, 102.0, 100.0], ConfidenceLevel::DEFAULT)?;
/// assert_eq!(ci.mean(), 100.0);
/// assert_eq!(ci.replications(), 3);
/// assert!(ci.contains(101.0));
/// // s = 2 and t(2, 0.975) = 4.3027, so the half-width is 4.3027 · 2/√3.
/// assert_eq!(format!("{ci:.2}"), "100.00 ± 4.97 (95% CI, 3 replications)");
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfidenceInterval {
    mean: f64,
    half_width: f64,
    std_dev: f64,
    replications: u32,
    level: ConfidenceLevel,
}

impl ConfidenceInterval {
    /// The interval for the mean of `values` at `level`; see
    /// [`Summary::confidence_interval`].
    ///
    /// # Errors
    ///
    /// As [`Summary::from_values`] and [`Summary::confidence_interval`].
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// assert!(ConfidenceInterval::from_values(&[1.0], ConfidenceLevel::DEFAULT).is_err());
    /// ```
    pub fn from_values(values: &[f64], level: ConfidenceLevel) -> Result<Self, StatsError> {
        Summary::from_values(values)?.confidence_interval(level)
    }

    /// The point estimate, the sample mean.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[1.0, 2.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.mean(), 1.5);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn mean(&self) -> f64 {
        self.mean
    }

    /// The half-width: the interval is mean ± half-width.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[5.0, 5.0, 5.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.half_width(), 0.0);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn half_width(&self) -> f64 {
        self.half_width
    }

    /// The lower end, mean − half-width.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[1.0, 2.0, 3.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.lower(), ci.mean() - ci.half_width());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn lower(&self) -> f64 {
        self.mean - self.half_width
    }

    /// The upper end, mean + half-width.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[1.0, 2.0, 3.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.upper(), ci.mean() + ci.half_width());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn upper(&self) -> f64 {
        self.mean + self.half_width
    }

    /// Whether `value` lies in [lower, upper].
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[1.0, 2.0, 3.0], ConfidenceLevel::DEFAULT)?;
    /// assert!(ci.contains(2.0) && !ci.contains(10.0));
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn contains(&self, value: f64) -> bool {
        self.lower() <= value && value <= self.upper()
    }

    /// The sample standard deviation of the replication results.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[1.0, 3.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.std_dev(), 2f64.sqrt());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn std_dev(&self) -> f64 {
        self.std_dev
    }

    /// The number of replications behind the estimate.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[1.0, 2.0, 4.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(ci.replications(), 3);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn replications(&self) -> u32 {
        self.replications
    }

    /// The confidence level.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let level = ConfidenceLevel::new(0.9)?;
    /// assert_eq!(ConfidenceInterval::from_values(&[1.0, 2.0], level)?.level(), level);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn level(&self) -> ConfidenceLevel {
        self.level
    }

    /// The half-width relative to the magnitude of the mean, the precision
    /// that replication control targets (spec §6). `Some(0.0)` when the
    /// half-width is 0; `None` when the mean is 0 (or so close to 0 that the
    /// ratio overflows) while the half-width is not.
    ///
    /// ```
    /// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
    ///
    /// let ci = ConfidenceInterval::from_values(&[98.0, 102.0, 100.0], ConfidenceLevel::DEFAULT)?;
    /// assert!((ci.relative_half_width().unwrap() - 0.0497).abs() < 1e-4);
    /// let around_zero = ConfidenceInterval::from_values(&[-1.0, 1.0], ConfidenceLevel::DEFAULT)?;
    /// assert_eq!(around_zero.relative_half_width(), None);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn relative_half_width(&self) -> Option<f64> {
        if self.half_width == 0.0 {
            return Some(0.0);
        }
        let ratio = self.half_width / self.mean.abs();
        ratio.is_finite().then_some(ratio)
    }
}

impl fmt::Display for ConfidenceInterval {
    /// `<mean> ± <half-width> (<level> CI, <n> replications)`; a precision
    /// such as `{:.2}` applies to the mean and the half-width.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (mean, half_width) = match f.precision() {
            Some(digits) => (
                format!("{:.*}", digits, self.mean),
                format!("{:.*}", digits, self.half_width),
            ),
            None => (self.mean.to_string(), self.half_width.to_string()),
        };
        write!(
            f,
            "{mean} ± {half_width} ({} CI, {} replications)",
            self.level, self.replications
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_two_replications() {
        for values in [&[][..], &[3.0][..]] {
            let err =
                ConfidenceInterval::from_values(values, ConfidenceLevel::DEFAULT).unwrap_err();
            assert!(matches!(
                err,
                StatsError::TooFewReplications { required: 2, .. }
            ));
        }
    }

    #[test]
    fn rejects_non_finite_values_and_overflow() {
        let err = Summary::from_values(&[1.0, 2.0, f64::NEG_INFINITY]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "replication results: value 2 is -inf; only finite numbers are allowed"
        );
        let err = Summary::from_values(&[f64::MAX, -f64::MAX]).unwrap_err();
        assert!(matches!(err, StatsError::Overflow { .. }));
        // Spread 2e300 overflows: the variance of ±1e300 is 2e600.
        let err = Summary::from_values(&[1e300, -1e300]).unwrap_err();
        assert!(matches!(err, StatsError::Overflow { .. }));
        // The half-width overflows even though the spread fits:
        // s = √2·1e150, and t(1) at α = 1e-300 is about 6e299.
        let s = Summary::from_values(&[1e150, -1e150]).unwrap();
        let tiny = ConfidenceLevel::from_alpha(1e-300).unwrap();
        assert!(matches!(
            s.confidence_interval(tiny),
            Err(StatsError::Overflow { .. })
        ));
    }

    #[test]
    fn counts_saturate_with_an_error() {
        let mut s = Summary {
            count: u32::MAX,
            mean: 1.0,
            m2: 0.0,
        };
        assert_eq!(
            s.push(1.0),
            Err(StatsError::TooManyReplications { max: u32::MAX })
        );
    }

    #[test]
    fn is_accurate_for_values_with_a_large_offset() {
        // A naive sum of squares loses every digit here; Welford does not.
        let offset = 1e9;
        let s = Summary::from_values(&[offset + 4.0, offset + 7.0, offset + 13.0, offset + 16.0])
            .unwrap();
        assert_eq!(s.mean(), Some(offset + 10.0));
        assert_eq!(s.variance(), Some(30.0));
    }

    #[test]
    fn relative_half_width_of_a_tiny_mean_is_undefined() {
        let ci = ConfidenceInterval {
            mean: 1e-300,
            half_width: 1e10,
            std_dev: 1.0,
            replications: 2,
            level: ConfidenceLevel::DEFAULT,
        };
        assert_eq!(ci.relative_half_width(), None);
    }
}
