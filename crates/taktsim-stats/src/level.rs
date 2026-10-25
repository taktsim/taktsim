//! Probabilities and confidence levels.

use std::fmt;

use taktsim_units::Ratio;

use crate::StatsError;

/// A probability strictly between 0 and 1, the argument of a quantile
/// function.
///
/// ```
/// use taktsim_stats::Probability;
///
/// let p = Probability::new(0.975)?;
/// assert_eq!(p.get(), 0.975);
/// assert!(Probability::new(1.0).is_err());
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability(f64);

impl Probability {
    /// The probability `p`.
    ///
    /// # Errors
    ///
    /// [`StatsError::NotAProbability`] unless `0 < p < 1`.
    ///
    /// ```
    /// use taktsim_stats::Probability;
    ///
    /// assert!(Probability::new(0.5).is_ok());
    /// assert!(Probability::new(0.0).is_err());
    /// assert!(Probability::new(f64::NAN).is_err());
    /// ```
    pub fn new(p: f64) -> Result<Self, StatsError> {
        if p > 0.0 && p < 1.0 {
            Ok(Self(p))
        } else {
            Err(StatsError::NotAProbability {
                parameter: "p",
                found: p,
                example: "0.975",
            })
        }
    }

    /// The probability as a number.
    ///
    /// ```
    /// use taktsim_stats::Probability;
    ///
    /// assert_eq!(Probability::new(0.25)?.get(), 0.25);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

/// The confidence level of an interval, e.g. 95 %.
///
/// Stored as the miss probability α = 1 − level, so a Bonferroni-adjusted
/// level such as 1 − 0.05/2000 keeps every digit of α.
///
/// ```
/// use taktsim_stats::ConfidenceLevel;
///
/// let level: ConfidenceLevel = "99%".parse::<taktsim_units::Ratio>()?.try_into()?;
/// assert_eq!(level.to_string(), "99%");
/// assert_eq!(ConfidenceLevel::DEFAULT.to_string(), "95%");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ConfidenceLevel {
    /// α = 1 − level, in (0, 1).
    alpha: f64,
}

impl ConfidenceLevel {
    /// 95 %, the level used when a model does not declare one (ADR-0014).
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// assert!((ConfidenceLevel::DEFAULT.level() - 0.95).abs() < 1e-15);
    /// assert_eq!(ConfidenceLevel::default(), ConfidenceLevel::DEFAULT);
    /// ```
    pub const DEFAULT: Self = Self { alpha: 0.05 };

    /// The level `level`, e.g. `0.95`.
    ///
    /// # Errors
    ///
    /// [`StatsError::NotAProbability`] unless `0 < level < 1`.
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// assert_eq!(ConfidenceLevel::new(0.9)?.to_string(), "90%");
    /// assert!(ConfidenceLevel::new(1.0).is_err());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn new(level: f64) -> Result<Self, StatsError> {
        if level > 0.0 && level < 1.0 {
            Ok(Self { alpha: 1.0 - level })
        } else {
            Err(StatsError::NotAProbability {
                parameter: "level",
                found: level,
                example: "0.95",
            })
        }
    }

    /// The level whose miss probability is `alpha`, e.g. `0.05` for 95 %.
    ///
    /// # Errors
    ///
    /// [`StatsError::NotAProbability`] unless `0 < alpha < 1`.
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// assert_eq!(ConfidenceLevel::from_alpha(0.05)?, ConfidenceLevel::DEFAULT);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn from_alpha(alpha: f64) -> Result<Self, StatsError> {
        if alpha > 0.0 && alpha < 1.0 {
            Ok(Self { alpha })
        } else {
            Err(StatsError::NotAProbability {
                parameter: "alpha",
                found: alpha,
                example: "0.05",
            })
        }
    }

    /// The level for each of `k` intervals that hold jointly at this level
    /// (Bonferroni): α/k. ADR-0007 uses it for the analytical test suite.
    ///
    /// # Errors
    ///
    /// [`StatsError::Zero`] if `k` is 0.
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// let each = ConfidenceLevel::DEFAULT.bonferroni(10)?;
    /// assert_eq!(each.alpha(), 0.005);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn bonferroni(self, k: u32) -> Result<Self, StatsError> {
        if k == 0 {
            return Err(StatsError::Zero {
                parameter: "k",
                example: "10",
            });
        }
        Ok(Self {
            alpha: self.alpha / f64::from(k),
        })
    }

    /// The miss probability α = 1 − level.
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// assert_eq!(ConfidenceLevel::DEFAULT.alpha(), 0.05);
    /// ```
    #[must_use]
    pub fn alpha(self) -> f64 {
        self.alpha
    }

    /// The level, 1 − α.
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// assert_eq!(ConfidenceLevel::new(0.99)?.level(), 0.99);
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    #[must_use]
    pub fn level(self) -> f64 {
        1.0 - self.alpha
    }
}

impl Default for ConfidenceLevel {
    /// [`ConfidenceLevel::DEFAULT`], 95 %.
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<Ratio> for ConfidenceLevel {
    type Error = StatsError;

    /// The level written as a percentage, e.g. `95%`.
    fn try_from(ratio: Ratio) -> Result<Self, StatsError> {
        let level = ratio.as_fraction();
        Self::new(level).map_err(|_| StatsError::NotAProbability {
            parameter: "level",
            found: level,
            example: "95%",
        })
    }
}

impl fmt::Display for ConfidenceLevel {
    /// The level as a percentage, e.g. `95%`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.level() * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_levels_outside_the_open_unit_interval() {
        for bad in [0.0, 1.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert!(Probability::new(bad).is_err());
            assert!(ConfidenceLevel::new(bad).is_err());
            assert!(ConfidenceLevel::from_alpha(bad).is_err());
        }
        let err = ConfidenceLevel::try_from(Ratio::from_percent(100.0).unwrap()).unwrap_err();
        assert_eq!(
            err.to_string(),
            "`level` must be a number strictly between 0 and 1, found 1; example: 95%"
        );
        assert!(ConfidenceLevel::DEFAULT.bonferroni(0).is_err());
    }

    #[test]
    fn keeps_alpha_exact() {
        let level = ConfidenceLevel::DEFAULT.bonferroni(2000).unwrap();
        assert_eq!(level.alpha(), 0.05 / 2000.0);
        // 1 - level would lose the low digits of alpha; alpha itself does not.
        assert_eq!(ConfidenceLevel::new(0.95).unwrap().alpha(), 1.0 - 0.95);
    }
}
