//! Dimensionless shares written as percentages.

use std::fmt;
use std::str::FromStr;

use crate::UnitError;
use crate::decimal::{self, Spec};

const SPEC: Spec = Spec {
    quantity: "percentage",
    allowed: &["%"],
    example: "2%",
};

/// A non-negative share written as a percentage, e.g. a target precision
/// (`2%`).
///
/// Stored as the percentage itself, so text and value round-trip exactly.
///
/// ```
/// use taktsim_units::Ratio;
///
/// let precision: Ratio = "2%".parse()?;
/// assert_eq!(precision.as_percent(), 2.0);
/// assert_eq!(precision.as_fraction(), 0.02);
/// assert_eq!(precision.to_string(), "2%");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Ratio(f64);

impl Ratio {
    /// The share `percent` / 100.
    ///
    /// # Errors
    ///
    /// [`UnitError::NotFinite`] for NaN or infinity,
    /// [`UnitError::Negative`] below zero.
    ///
    /// ```
    /// use taktsim_units::Ratio;
    ///
    /// assert_eq!(Ratio::from_percent(12.5)?.as_fraction(), 0.125);
    /// assert!(Ratio::from_percent(-1.0).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn from_percent(percent: f64) -> Result<Self, UnitError> {
        if !percent.is_finite() {
            return Err(UnitError::NotFinite {
                what: format!("percentage {percent}%"),
            });
        }
        if percent < 0.0 {
            return Err(UnitError::Negative {
                input: format!("{percent}%"),
                quantity: SPEC.quantity,
                example: SPEC.example,
            });
        }
        // -0.0 is not negative but would display as "-0%".
        Ok(Self(percent.abs()))
    }

    /// The percentage, e.g. `2.0` for `2%`.
    ///
    /// ```
    /// use taktsim_units::Ratio;
    ///
    /// assert_eq!("0.5%".parse::<Ratio>()?.as_percent(), 0.5);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn as_percent(self) -> f64 {
        self.0
    }

    /// The share as a fraction, e.g. `0.02` for `2%`.
    ///
    /// ```
    /// use taktsim_units::Ratio;
    ///
    /// assert_eq!("50%".parse::<Ratio>()?.as_fraction(), 0.5);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn as_fraction(self) -> f64 {
        self.0 / 100.0
    }
}

impl FromStr for Ratio {
    type Err = UnitError;

    /// Parses `<number>%`, with an optional single space.
    fn from_str(input: &str) -> Result<Self, UnitError> {
        let (number, unit) = decimal::split(input, &SPEC)?;
        if unit != "%" {
            return Err(decimal::unknown_unit(input, unit, &SPEC));
        }
        // Cannot fail: the text is digits with at most one dot. A value too
        // large for f64 parses as infinity and is rejected as not finite.
        let percent: f64 = number.text.parse().unwrap_or(f64::NAN);
        Self::from_percent(percent)
    }
}

impl fmt::Display for Ratio {
    /// `<percentage>%`, the shortest text that parses back to the same value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_other_units_and_bad_values() {
        assert!(matches!(
            "2 percent".parse::<Ratio>(),
            Err(UnitError::UnknownUnit { .. })
        ));
        assert!(matches!(
            "-2%".parse::<Ratio>(),
            Err(UnitError::Negative { .. })
        ));
        assert!(matches!(
            Ratio::from_percent(f64::INFINITY),
            Err(UnitError::NotFinite { .. })
        ));
        assert_eq!(Ratio::from_percent(-0.0).unwrap().to_string(), "0%");
        assert_eq!("2 %".parse::<Ratio>().unwrap().to_string(), "2%");
    }

    #[test]
    fn percent_and_fraction() {
        let r = Ratio::from_percent(12.5).unwrap();
        assert_eq!(r.as_percent(), 12.5);
        assert_eq!(r.as_fraction(), 0.125);
        assert_eq!("50%".parse::<Ratio>().unwrap().as_fraction(), 0.5);
        assert_eq!(
            Ratio::from_percent(-1.0).unwrap_err().to_string(),
            "negative percentage `-1%` is not allowed; example: 2%"
        );
        assert!(matches!(
            "2".parse::<Ratio>(),
            Err(UnitError::UnknownUnit { .. })
        ));
    }
}
