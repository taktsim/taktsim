//! Probabilities and confidence levels.

use std::cmp::Ordering;
use std::fmt;

use taktsim_units::Ratio;

use crate::StatsError;

/// The smallest tail probability a quantile is computed for: the smallest
/// normal `f64`. Below it, quantiles of the t distribution with 1 degree of
/// freedom overflow, and halving a subnormal α can give 0.
const SMALLEST_TAIL: f64 = f64::MIN_POSITIVE;

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
    /// [`StatsError::NotAProbability`] unless `0 < p < 1`;
    /// [`StatsError::TooSmall`] below the smallest normal `f64`
    /// (about 2.2e-308), where quantiles would overflow.
    ///
    /// ```
    /// use taktsim_stats::Probability;
    ///
    /// assert!(Probability::new(0.5).is_ok());
    /// assert!(Probability::new(0.0).is_err());
    /// assert!(Probability::new(f64::NAN).is_err());
    /// assert!(Probability::new(1e-310).is_err());
    /// ```
    pub fn new(p: f64) -> Result<Self, StatsError> {
        if !(p > 0.0 && p < 1.0) {
            return Err(StatsError::NotAProbability {
                parameter: "p",
                found: p,
                example: "0.975",
            });
        }
        if p < SMALLEST_TAIL {
            return Err(StatsError::TooSmall {
                parameter: "p",
                found: p,
                smallest: SMALLEST_TAIL,
                example: "0.025",
            });
        }
        Ok(Self(p))
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
/// level such as 1 − 0.05/2000 keeps every digit of α. Levels compare by
/// level: 99 % > 95 %.
///
/// ```
/// use taktsim_stats::ConfidenceLevel;
///
/// let level: ConfidenceLevel = "99%".parse::<taktsim_units::Ratio>()?.try_into()?;
/// assert_eq!(level.to_string(), "99%");
/// assert!(level > ConfidenceLevel::DEFAULT);
/// assert_eq!(ConfidenceLevel::DEFAULT.to_string(), "95%");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfidenceLevel {
    /// α = 1 − level, in (0, 1), with α/2 a normal `f64`.
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

    /// The level `level`, e.g. `0.9`.
    ///
    /// α is 1 − `level`, exact for the `f64` given. That `f64` is often not
    /// the decimal written: `new(0.95)` has α = 0.050000000000000044, not
    /// 0.05. For exactly 95 % use [`DEFAULT`](Self::DEFAULT),
    /// [`from_alpha(0.05)`](Self::from_alpha) or the percentage `95%`.
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
            // α ≥ 2⁻⁵³ here, far above the smallest supported α.
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
    /// [`StatsError::NotAProbability`] unless `0 < alpha < 1`;
    /// [`StatsError::TooSmall`] if α/2, the tail probability of a two-sided
    /// interval, is below the smallest normal `f64` (α below about 4.5e-308).
    ///
    /// ```
    /// use taktsim_stats::ConfidenceLevel;
    ///
    /// assert_eq!(ConfidenceLevel::from_alpha(0.05)?, ConfidenceLevel::DEFAULT);
    /// assert!(ConfidenceLevel::from_alpha(1e-310).is_err());
    /// # Ok::<(), taktsim_stats::StatsError>(())
    /// ```
    pub fn from_alpha(alpha: f64) -> Result<Self, StatsError> {
        if !(alpha > 0.0 && alpha < 1.0) {
            return Err(StatsError::NotAProbability {
                parameter: "alpha",
                found: alpha,
                example: "0.05",
            });
        }
        Self::checked(alpha, "alpha", "0.05")
    }

    /// The level for each of `k` intervals that hold jointly at this level
    /// (Bonferroni): α/k. ADR-0007 uses it for the analytical test suite.
    ///
    /// # Errors
    ///
    /// [`StatsError::Zero`] if `k` is 0; [`StatsError::TooSmall`] if α/k is
    /// below the smallest supported α (see [`from_alpha`](Self::from_alpha)).
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
        Self::checked(
            self.alpha / f64::from(k),
            "alpha / k",
            "k = 10 with alpha = 0.05",
        )
    }

    /// α in (0, 1), checked against the smallest supported value.
    fn checked(
        alpha: f64,
        parameter: &'static str,
        example: &'static str,
    ) -> Result<Self, StatsError> {
        let smallest = 2.0 * SMALLEST_TAIL;
        if alpha < smallest {
            return Err(StatsError::TooSmall {
                parameter,
                found: alpha,
                smallest,
                example,
            });
        }
        Ok(Self { alpha })
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

impl PartialOrd for ConfidenceLevel {
    /// By level: a smaller α is a higher level.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        other.alpha.partial_cmp(&self.alpha)
    }
}

impl TryFrom<Ratio> for ConfidenceLevel {
    type Error = StatsError;

    /// The level written as a percentage, e.g. `95%`. α is computed as
    /// (100 − percent)/100, so `95%` gives exactly
    /// [`ConfidenceLevel::DEFAULT`].
    fn try_from(ratio: Ratio) -> Result<Self, StatsError> {
        let percent = ratio.as_percent();
        if percent > 0.0 && percent < 100.0 {
            // α ≥ 1.4e-16 here, far above the smallest supported α.
            Ok(Self {
                alpha: (100.0 - percent) / 100.0,
            })
        } else {
            Err(StatsError::LevelOutOfRange { found: ratio })
        }
    }
}

impl fmt::Display for ConfidenceLevel {
    /// The level as a percentage, e.g. `95%`, computed as 100 − 100·α so
    /// that Bonferroni levels print without noise (`99.9975%`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", 100.0 - 100.0 * self.alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn percent(text: &str) -> Result<ConfidenceLevel, StatsError> {
        ConfidenceLevel::try_from(text.parse::<Ratio>().unwrap())
    }

    #[test]
    fn rejects_levels_outside_the_open_unit_interval() {
        for bad in [0.0, 1.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
            assert!(Probability::new(bad).is_err());
            assert!(ConfidenceLevel::new(bad).is_err());
            assert!(ConfidenceLevel::from_alpha(bad).is_err());
        }
        assert!(ConfidenceLevel::DEFAULT.bonferroni(0).is_err());
    }

    #[test]
    fn percentage_errors_speak_in_percent() {
        for bad in ["0%", "100%", "150%"] {
            let err = percent(bad).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!(
                    "`level` must be a percentage strictly between 0% and 100%, \
                     found {bad}; example: 95%"
                )
            );
        }
    }

    #[test]
    fn percentages_give_exact_alpha() {
        assert_eq!(percent("95%").unwrap(), ConfidenceLevel::DEFAULT);
        assert_eq!(percent("99%").unwrap().alpha(), 0.01);
        assert_eq!(percent("99.5%").unwrap().alpha(), 0.005);
    }

    #[test]
    fn levels_order_by_level() {
        let l99 = ConfidenceLevel::new(0.99).unwrap();
        let l90 = ConfidenceLevel::new(0.9).unwrap();
        assert!(l99 > ConfidenceLevel::DEFAULT);
        assert!(l90 < ConfidenceLevel::DEFAULT);
        assert!(ConfidenceLevel::DEFAULT.bonferroni(10).unwrap() > ConfidenceLevel::DEFAULT);
        assert_eq!(
            ConfidenceLevel::DEFAULT.partial_cmp(&ConfidenceLevel::DEFAULT),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn display_has_no_rounding_noise() {
        let each = ConfidenceLevel::DEFAULT.bonferroni(2000).unwrap();
        assert_eq!(each.to_string(), "99.9975%");
        assert_eq!(
            ConfidenceLevel::DEFAULT.bonferroni(3).unwrap().to_string(),
            "98.33333333333333%"
        );
        assert_eq!(percent("99.5%").unwrap().to_string(), "99.5%");
    }

    #[test]
    fn rejects_tail_probabilities_below_the_normal_range() {
        // A subnormal α would halve to 0 or to a tail whose quantile overflows.
        for alpha in [5e-324, 1e-310, f64::MIN_POSITIVE] {
            let err = ConfidenceLevel::from_alpha(alpha).unwrap_err();
            assert!(
                matches!(
                    err,
                    StatsError::TooSmall {
                        parameter: "alpha",
                        ..
                    }
                ),
                "{err}"
            );
        }
        assert!(ConfidenceLevel::from_alpha(2.0 * f64::MIN_POSITIVE).is_ok());
        let tiny = ConfidenceLevel::from_alpha(1e-300).unwrap();
        let err = tiny.bonferroni(1_000_000_000).unwrap_err();
        assert_eq!(
            err.to_string(),
            "`alpha / k` = 1e-309 is below the smallest supported value 4.450147717014403e-308; \
             example: k = 10 with alpha = 0.05"
        );
        assert!(matches!(
            Probability::new(1e-310),
            Err(StatsError::TooSmall { parameter: "p", .. })
        ));
        assert!(Probability::new(f64::MIN_POSITIVE).is_ok());
    }

    #[test]
    fn keeps_alpha_exact() {
        let level = ConfidenceLevel::DEFAULT.bonferroni(2000).unwrap();
        assert_eq!(level.alpha(), 0.05 / 2000.0);
        // 1 - level would lose the low digits of alpha; alpha itself does not.
        assert_eq!(ConfidenceLevel::new(0.95).unwrap().alpha(), 1.0 - 0.95);
    }
}
