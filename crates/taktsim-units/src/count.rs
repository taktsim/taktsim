//! Counts of discrete units and rates of them.

use std::fmt;
use std::str::FromStr;

use crate::decimal::{self, Scaled, Spec};
use crate::{SimDuration, UnitError};

/// The largest count, for error messages.
const COUNT_LIMIT: &str = "18446744073709551615 parts";

const COUNT: Spec = Spec {
    quantity: "count",
    allowed: &["parts", "part"],
    example: "20 parts",
};

/// A whole number of discrete units (`parts`), e.g. a buffer capacity or a
/// batch size.
///
/// ```
/// use taktsim_units::Count;
///
/// let capacity: Count = "20 parts".parse()?;
/// assert_eq!(capacity.get(), 20);
/// assert_eq!(capacity.to_string(), "20 parts");
/// assert!("2.5 parts".parse::<Count>().is_err());
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Count(u64);

impl Count {
    /// The count zero.
    ///
    /// ```
    /// assert_eq!(taktsim_units::Count::ZERO.get(), 0);
    /// ```
    pub const ZERO: Self = Self(0);

    /// `n` units.
    ///
    /// ```
    /// assert_eq!(taktsim_units::Count::new(3).to_string(), "3 parts");
    /// ```
    #[must_use]
    pub const fn new(n: u64) -> Self {
        Self(n)
    }

    /// The number of units.
    ///
    /// ```
    /// assert_eq!(taktsim_units::Count::new(3).get(), 3);
    /// ```
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// `self + other`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] beyond `u64::MAX` units.
    ///
    /// ```
    /// use taktsim_units::Count;
    ///
    /// assert_eq!(Count::new(2).checked_add(Count::new(3))?, Count::new(5));
    /// assert!(Count::new(u64::MAX).checked_add(Count::new(1)).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_add(self, other: Self) -> Result<Self, UnitError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or_else(|| UnitError::Overflow {
                what: format!("count {self} + {other}"),
                limit: COUNT_LIMIT,
            })
    }

    /// `self - other`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Underflow`] if `other` is larger than `self`.
    ///
    /// ```
    /// use taktsim_units::Count;
    ///
    /// assert_eq!(Count::new(5).checked_sub(Count::new(3))?, Count::new(2));
    /// assert!(Count::new(1).checked_sub(Count::new(2)).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_sub(self, other: Self) -> Result<Self, UnitError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or_else(|| UnitError::Underflow {
                what: format!("count {self} - {other}"),
            })
    }

    /// The rate of `self` units per `d`, e.g. throughput.
    ///
    /// # Errors
    ///
    /// [`UnitError::ZeroDuration`] if `d` is zero.
    ///
    /// ```
    /// use taktsim_units::Count;
    ///
    /// let rate = Count::new(120).per("1h".parse()?)?;
    /// assert_eq!(rate.as_per_hour(), 120.0);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn per(self, d: SimDuration) -> Result<Rate, UnitError> {
        if d == SimDuration::ZERO {
            return Err(UnitError::ZeroDuration);
        }
        // Rates are statistics in f64; counts beyond 2^53 lose precision.
        #[allow(clippy::cast_precision_loss)]
        let n = self.0 as f64;
        Ok(Rate(n / d.as_secs_f64()))
    }
}

impl FromStr for Count {
    type Err = UnitError;

    /// Parses `<whole number> parts` (or `part`), with an optional single
    /// space, e.g. `20 parts`, `1 part`.
    fn from_str(input: &str) -> Result<Self, UnitError> {
        let (number, unit) = decimal::split(input, &COUNT)?;
        if !COUNT.allowed.contains(&unit) {
            return Err(decimal::unknown_unit(input, unit, &COUNT));
        }
        // Scaling by 1 is inexact exactly when there is a fractional part.
        match number.scale(1) {
            Scaled::Exact(n) => Ok(Self(n)),
            Scaled::Inexact => Err(UnitError::NotWhole {
                input: input.to_owned(),
                unit: unit.to_owned(),
            }),
            Scaled::Overflow => Err(UnitError::Overflow {
                what: format!("count `{input}`"),
                limit: COUNT_LIMIT,
            }),
        }
    }
}

impl fmt::Display for Count {
    /// `<n> parts`, also for one unit, as in the spec's examples.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} parts", self.0)
    }
}

const RATE: Spec = Spec {
    quantity: "rate",
    allowed: &[
        "parts/s",
        "parts/min",
        "parts/h",
        "part/s",
        "part/min",
        "part/h",
    ],
    example: "120 parts/h",
};

/// Units per time, e.g. throughput, stored as units per second.
///
/// A rate is a statistic, so it is a floating-point number; it is displayed
/// in `parts/s` with the shortest text that parses back to the same value.
///
/// ```
/// use taktsim_units::Rate;
///
/// let r: Rate = "120 parts/h".parse()?;
/// assert_eq!(r.as_per_hour(), 120.0);
/// assert_eq!(r.to_string().parse::<Rate>()?, r);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Rate(f64);

impl Rate {
    /// A rate of `per_second` units per second.
    ///
    /// # Errors
    ///
    /// [`UnitError::NotFinite`] for NaN or infinity,
    /// [`UnitError::Negative`] below zero.
    ///
    /// ```
    /// use taktsim_units::Rate;
    ///
    /// assert_eq!(Rate::per_second(0.5)?.as_per_hour(), 1800.0);
    /// assert!(Rate::per_second(-1.0).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn per_second(per_second: f64) -> Result<Self, UnitError> {
        if !per_second.is_finite() {
            return Err(UnitError::NotFinite {
                what: format!("rate {per_second} parts/s"),
            });
        }
        if per_second < 0.0 {
            return Err(UnitError::Negative {
                input: format!("{per_second} parts/s"),
                quantity: RATE.quantity,
                example: RATE.example,
            });
        }
        // -0.0 is not negative but would display as "-0 parts/s".
        Ok(Self(per_second.abs()))
    }

    /// Units per second.
    ///
    /// ```
    /// use taktsim_units::Rate;
    ///
    /// assert_eq!("3 parts/min".parse::<Rate>()?.as_per_second(), 0.05);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn as_per_second(self) -> f64 {
        self.0
    }

    /// Units per hour.
    ///
    /// ```
    /// use taktsim_units::Rate;
    ///
    /// assert_eq!(Rate::per_second(1.0)?.as_per_hour(), 3600.0);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn as_per_hour(self) -> f64 {
        self.0 * 3600.0
    }
}

impl FromStr for Rate {
    type Err = UnitError;

    /// Parses `<number> parts/s`, `parts/min` or `parts/h`; `part/…` is
    /// accepted too, as for [`Count`].
    fn from_str(input: &str) -> Result<Self, UnitError> {
        let (number, unit) = decimal::split(input, &RATE)?;
        let secs = match unit {
            "parts/s" | "part/s" => 1.0,
            "parts/min" | "part/min" => 60.0,
            "parts/h" | "part/h" => 3600.0,
            _ => return Err(decimal::unknown_unit(input, unit, &RATE)),
        };
        // Cannot fail: the text is digits with at most one dot. A value too
        // large for f64 parses as infinity and is rejected as not finite.
        let value: f64 = number.text.parse().unwrap_or(f64::NAN);
        Self::per_second(value / secs)
    }
}

impl fmt::Display for Rate {
    /// `<units per second> parts/s`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} parts/s", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_parse_whole_parts_only() {
        assert_eq!("1 part".parse::<Count>().unwrap(), Count::new(1));
        assert_eq!("20parts".parse::<Count>().unwrap(), Count::new(20));
        assert_eq!("20.0 parts".parse::<Count>().unwrap(), Count::new(20));
        let err = |s: &str| s.parse::<Count>().unwrap_err();
        assert!(matches!(err("2.5 parts"), UnitError::NotWhole { .. }));
        assert!(matches!(err("20 kg"), UnitError::UnknownUnit { .. }));
        assert!(matches!(
            err("99999999999999999999 parts"),
            UnitError::Overflow { .. }
        ));
        assert_eq!(
            err("2.5 parts").to_string(),
            "`2.5 parts` is not a whole number of parts; example: 20 parts"
        );
    }

    #[test]
    fn rates_reject_zero_durations_and_bad_values() {
        assert_eq!(
            Count::new(1).per(SimDuration::ZERO),
            Err(UnitError::ZeroDuration)
        );
        assert!(matches!(
            Rate::per_second(f64::NAN),
            Err(UnitError::NotFinite { .. })
        ));
        assert!(matches!(
            "12 parts/day".parse::<Rate>(),
            Err(UnitError::UnknownUnit { .. })
        ));
        assert_eq!(Rate::per_second(-0.0).unwrap().to_string(), "0 parts/s");
    }

    #[test]
    fn rates_over_huge_numbers_still_parse() {
        let digits = "9".repeat(400);
        assert!(matches!(
            format!("{digits} parts/s").parse::<Rate>(),
            Err(UnitError::NotFinite { .. })
        ));
    }

    #[test]
    fn count_errors_name_the_operation() {
        let e = Count::new(1).checked_sub(Count::new(2)).unwrap_err();
        assert_eq!(e.to_string(), "count 1 parts - 2 parts would be negative");
    }

    #[test]
    fn rates_from_counts() {
        let r = Count::new(120).per("1h".parse().unwrap()).unwrap();
        assert!((r.as_per_hour() - 120.0).abs() < 1e-9);
        assert!((r.as_per_second() - 120.0 / 3600.0).abs() < f64::EPSILON);
        assert_eq!(Rate::per_second(0.5).unwrap().to_string(), "0.5 parts/s");
        let r: Rate = "3 parts/min".parse().unwrap();
        assert!((r.as_per_second() - 0.05).abs() < f64::EPSILON);
        assert_eq!("2 parts/s".parse::<Rate>().unwrap().as_per_second(), 2.0);
        assert_eq!(
            "1 part/min".parse::<Rate>().unwrap(),
            "1 parts/min".parse().unwrap()
        );
        let r: Rate = "120 parts/h".parse().unwrap();
        assert!((r.as_per_hour() - 120.0).abs() < 1e-9);
    }

    #[test]
    fn negative_rates_are_rejected_with_an_example() {
        assert_eq!(
            Rate::per_second(-1.0).unwrap_err().to_string(),
            "negative rate `-1 parts/s` is not allowed; example: 120 parts/h"
        );
        assert!(matches!(
            "-5 parts/h".parse::<Rate>(),
            Err(UnitError::Negative { .. })
        ));
    }

    #[test]
    fn counts_basics() {
        assert_eq!(Count::ZERO.get(), 0);
        assert_eq!(Count::new(1).to_string(), "1 parts");
        assert_eq!(
            Count::new(u64::MAX)
                .checked_add(Count::new(1))
                .unwrap_err()
                .to_string(),
            "count 18446744073709551615 parts + 1 parts overflows; \
             the largest value is 18446744073709551615 parts"
        );
    }
}
