//! The `value` and `unit` columns: a finite number and the unit it is in.

use std::fmt;
use std::str::FromStr;

use taktsim_units::{Count, Rate, Ratio, SimDuration, UnitError};

/// The unit of an observed value: the units `taktsim-units` knows
/// (ADR-0012), plus `1` for dimensionless values such as state codes and
/// flags.
///
/// The text form is the unit as `taktsim-units` writes it; on input, the
/// singular `part` forms are accepted as well. Units of material quantities
/// (`kg`, `l`, `m`) arrive with taktsim-units in v0.2.
///
/// ```
/// use taktsim_adapters::observation::{Dimension, Unit};
///
/// let unit: Unit = "parts/h".parse()?;
/// assert_eq!(unit, Unit::PartsPerHour);
/// assert_eq!(unit.dimension(), Dimension::Rate);
/// assert_eq!(Unit::Minute.to_string(), "min");
/// assert_eq!("part".parse::<Unit>()?, Unit::Parts);
/// # Ok::<(), taktsim_adapters::observation::UnknownUnit>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unit {
    /// Milliseconds, `ms`.
    Millisecond,
    /// Seconds, `s`.
    Second,
    /// Minutes, `min`.
    Minute,
    /// Hours, `h`.
    Hour,
    /// Days, `d`.
    Day,
    /// Discrete units, `parts`.
    Parts,
    /// Parts per second, `parts/s`.
    PartsPerSecond,
    /// Parts per minute, `parts/min`.
    PartsPerMinute,
    /// Parts per hour, `parts/h`.
    PartsPerHour,
    /// Percent, `%`.
    Percent,
    /// A dimensionless number, `1` (as in the SI and UCUM), e.g. a state
    /// code or a 0/1 flag.
    One,
}

impl Unit {
    /// Every unit, in the order of [`UnknownUnit::ALLOWED`].
    ///
    /// ```
    /// use taktsim_adapters::observation::Unit;
    ///
    /// for unit in Unit::ALL {
    ///     assert_eq!(unit.as_str().parse::<Unit>(), Ok(unit));
    /// }
    /// ```
    pub const ALL: [Self; 11] = [
        Self::Millisecond,
        Self::Second,
        Self::Minute,
        Self::Hour,
        Self::Day,
        Self::Parts,
        Self::PartsPerSecond,
        Self::PartsPerMinute,
        Self::PartsPerHour,
        Self::Percent,
        Self::One,
    ];

    /// The unit's text, e.g. `parts/h`.
    ///
    /// ```
    /// use taktsim_adapters::observation::Unit;
    ///
    /// assert_eq!(Unit::PartsPerHour.as_str(), "parts/h");
    /// assert_eq!(Unit::One.as_str(), "1");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Millisecond => "ms",
            Self::Second => "s",
            Self::Minute => "min",
            Self::Hour => "h",
            Self::Day => "d",
            Self::Parts => "parts",
            Self::PartsPerSecond => "parts/s",
            Self::PartsPerMinute => "parts/min",
            Self::PartsPerHour => "parts/h",
            Self::Percent => "%",
            Self::One => "1",
        }
    }

    /// What kind of quantity the unit measures.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Dimension, Unit};
    ///
    /// assert_eq!(Unit::Millisecond.dimension(), Dimension::Duration);
    /// assert_eq!(Unit::One.dimension(), Dimension::Dimensionless);
    /// ```
    #[must_use]
    pub const fn dimension(self) -> Dimension {
        match self {
            Self::Millisecond | Self::Second | Self::Minute | Self::Hour | Self::Day => {
                Dimension::Duration
            }
            Self::Parts => Dimension::Count,
            Self::PartsPerSecond | Self::PartsPerMinute | Self::PartsPerHour => Dimension::Rate,
            Self::Percent => Dimension::Ratio,
            Self::One => Dimension::Dimensionless,
        }
    }
}

impl FromStr for Unit {
    type Err = UnknownUnit;

    /// The exact text of a unit, case-sensitive; `part`, `part/s`,
    /// `part/min` and `part/h` are accepted for the plural forms.
    fn from_str(text: &str) -> Result<Self, UnknownUnit> {
        match text {
            "ms" => Ok(Self::Millisecond),
            "s" => Ok(Self::Second),
            "min" => Ok(Self::Minute),
            "h" => Ok(Self::Hour),
            "d" => Ok(Self::Day),
            "parts" | "part" => Ok(Self::Parts),
            "parts/s" | "part/s" => Ok(Self::PartsPerSecond),
            "parts/min" | "part/min" => Ok(Self::PartsPerMinute),
            "parts/h" | "part/h" => Ok(Self::PartsPerHour),
            "%" => Ok(Self::Percent),
            "1" => Ok(Self::One),
            _ => Err(UnknownUnit),
        }
    }
}

impl fmt::Display for Unit {
    /// [`Unit::as_str`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a [`Unit`] measures; each maps to a `taktsim-units` type.
///
/// ```
/// use taktsim_adapters::observation::{Dimension, Unit};
///
/// assert_eq!(Unit::Percent.dimension(), Dimension::Ratio);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dimension {
    /// A length of time: [`SimDuration`].
    Duration,
    /// A number of discrete units: [`Count`].
    Count,
    /// Units per time: [`Rate`].
    Rate,
    /// A percentage: [`Ratio`].
    Ratio,
    /// A plain number.
    Dimensionless,
}

/// The text is not one of the [`Unit`]s.
///
/// ```
/// use taktsim_adapters::observation::{Unit, UnknownUnit};
///
/// assert_eq!("pcs".parse::<Unit>(), Err(UnknownUnit));
/// assert_eq!(UnknownUnit.to_string(), "unknown unit");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown unit")]
pub struct UnknownUnit;

impl UnknownUnit {
    /// The units allowed, as text.
    ///
    /// ```
    /// use taktsim_adapters::observation::UnknownUnit;
    ///
    /// assert!(UnknownUnit::ALLOWED.starts_with("ms, s, min, h, d, parts"));
    /// ```
    pub const ALLOWED: &'static str = "ms, s, min, h, d, parts, parts/s, parts/min, parts/h, %, 1 (dimensionless), case-sensitive";

    /// One valid unit.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Unit, UnknownUnit};
    ///
    /// assert_eq!(UnknownUnit::EXAMPLE.parse::<Unit>(), Ok(Unit::Parts));
    /// ```
    pub const EXAMPLE: &'static str = "parts";
}

/// An observed value with its unit: a finite number, never NaN or infinite.
///
/// Negative zero is stored as zero, so equal measurements have equal bits.
/// A value may be implausible for its unit (a negative count): the table
/// keeps it and [`Measurement::implausibility`] flags it, rather than
/// dropping it. [`Display`](fmt::Display) writes the value and the unit, the
/// value as the shortest text that reads back to it, with an exponent below
/// 10⁻⁵ and from 10¹⁶.
///
/// ```
/// use taktsim_adapters::observation::{Measurement, Unit, ValueError};
///
/// let m = Measurement::new(42.5, Unit::Second)?;
/// assert_eq!((m.value(), m.unit()), (42.5, Unit::Second));
/// assert_eq!(m.to_string(), "42.5 s");
/// assert_eq!(Measurement::new(-1e300, Unit::Parts)?.to_string(), "-1e300 parts");
/// assert_eq!(Measurement::new(f64::NAN, Unit::Second), Err(ValueError::NotFinite));
/// # Ok::<(), ValueError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measurement {
    value: f64,
    unit: Unit,
}

impl Measurement {
    /// `value` in `unit`.
    ///
    /// # Errors
    ///
    /// [`ValueError::NotFinite`] for NaN or infinity.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Measurement, Unit};
    ///
    /// let zero = Measurement::new(-0.0, Unit::Parts)?;
    /// assert_eq!(zero.value().to_bits(), 0.0_f64.to_bits());
    /// assert!(Measurement::new(f64::INFINITY, Unit::Parts).is_err());
    /// # Ok::<(), taktsim_adapters::observation::ValueError>(())
    /// ```
    pub fn new(value: f64, unit: Unit) -> Result<Self, ValueError> {
        if value.is_finite() {
            Ok(Self::from_finite(value, unit))
        } else {
            Err(ValueError::NotFinite)
        }
    }

    /// `value` must be finite.
    pub(crate) fn from_finite(value: f64, unit: Unit) -> Self {
        // -0.0 + 0.0 is +0.0; every other value is unchanged.
        Self {
            value: value + 0.0,
            unit,
        }
    }

    /// The number, in [`Measurement::unit`].
    ///
    /// ```
    /// use taktsim_adapters::observation::{Measurement, Unit};
    ///
    /// assert_eq!(Measurement::new(3.0, Unit::Minute)?.value(), 3.0);
    /// # Ok::<(), taktsim_adapters::observation::ValueError>(())
    /// ```
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    /// The unit of [`Measurement::value`].
    ///
    /// ```
    /// use taktsim_adapters::observation::{Measurement, Unit};
    ///
    /// assert_eq!(Measurement::new(3.0, Unit::Minute)?.unit(), Unit::Minute);
    /// # Ok::<(), taktsim_adapters::observation::ValueError>(())
    /// ```
    #[must_use]
    pub const fn unit(self) -> Unit {
        self.unit
    }

    /// Why the value is implausible for its unit, if it is: exactly when
    /// [`Measurement::to_quantity`] fails.
    ///
    /// Checked from the unit alone: a negative duration, count, rate or
    /// percentage, a count that is not whole, and a duration or count too
    /// large for `taktsim-units`. What needs the meaning of the signal (a
    /// cumulative counter that decreases, gaps, stuck values) is left to the
    /// data-quality report in v0.3 (ADR-0017).
    ///
    /// ```
    /// use taktsim_adapters::observation::{Implausibility, Measurement, Unit};
    ///
    /// let check = |v, u| Measurement::new(v, u).map(Measurement::implausibility);
    /// assert_eq!(check(-3.0, Unit::Parts)?, Some(Implausibility::Negative));
    /// assert_eq!(check(2.5, Unit::Parts)?, Some(Implausibility::NotWhole));
    /// assert_eq!(check(1e20, Unit::Parts)?, Some(Implausibility::TooLarge));
    /// assert_eq!(check(-1.0, Unit::One)?, None);
    /// assert_eq!(check(2.5, Unit::Second)?, None);
    /// # Ok::<(), taktsim_adapters::observation::ValueError>(())
    /// ```
    #[must_use]
    pub fn implausibility(self) -> Option<Implausibility> {
        self.convert().err()
    }

    /// The value as the `taktsim-units` quantity of its unit.
    ///
    /// Durations are rounded once, half to even, to whole nanoseconds from
    /// the exact binary value; rates are converted to parts per second as
    /// `taktsim-units` parses them.
    ///
    /// # Errors
    ///
    /// Exactly when [`Measurement::implausibility`] says why:
    /// [`UnitError::Negative`] for a negative duration, count, rate or
    /// percentage, [`UnitError::NotWhole`] for a fractional count, and
    /// [`UnitError::Overflow`] for a duration beyond about 584 years or a
    /// count beyond `u64::MAX`.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Measurement, Quantity, Unit};
    /// use taktsim_units::SimDuration;
    ///
    /// let m = Measurement::new(1.5, Unit::Minute)?;
    /// assert_eq!(m.to_quantity()?, Quantity::Duration("90s".parse::<SimDuration>()?));
    /// assert!(Measurement::new(-1.0, Unit::Second)?.to_quantity().is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn to_quantity(self) -> Result<Quantity, UnitError> {
        self.convert().map_err(|why| self.unit_error(why))
    }

    /// The quantity, or why there is none: the one place both
    /// [`Measurement::implausibility`] and [`Measurement::to_quantity`]
    /// decide.
    fn convert(self) -> Result<Quantity, Implausibility> {
        let (value, unit) = (self.value, self.unit);
        let dimension = unit.dimension();
        if value < 0.0 && dimension != Dimension::Dimensionless {
            return Err(Implausibility::Negative);
        }
        match dimension {
            Dimension::Duration => duration_nanos(value, nanos_per_unit(unit))
                .map(|nanos| Quantity::Duration(SimDuration::from_nanos(nanos)))
                .ok_or(Implausibility::TooLarge),
            Dimension::Count => count(value).map(Quantity::Count),
            // Cannot fail: the value is finite and not negative.
            Dimension::Rate => Rate::per_second(value / rate_seconds(unit))
                .map(Quantity::Rate)
                .map_err(|_| Implausibility::Negative),
            Dimension::Ratio => Ratio::from_percent(value)
                .map(Quantity::Ratio)
                .map_err(|_| Implausibility::Negative),
            Dimension::Dimensionless => Ok(Quantity::Number(value)),
        }
    }

    /// The `taktsim-units` error for `why`.
    fn unit_error(self, why: Implausibility) -> UnitError {
        let (quantity, example, limit) = match self.unit.dimension() {
            Dimension::Duration => (
                "duration",
                "42s",
                "18446744073.709551615s (about 584 years)",
            ),
            Dimension::Count => ("count", "20 parts", "18446744073709551615 parts"),
            Dimension::Rate => ("rate", "120 parts/h", ""),
            Dimension::Ratio => ("percentage", "2%", ""),
            Dimension::Dimensionless => ("number", "1", ""),
        };
        match why {
            Implausibility::Negative => UnitError::Negative {
                input: self.to_string(),
                quantity,
                example,
            },
            Implausibility::NotWhole => UnitError::NotWhole {
                input: self.to_string(),
                unit: self.unit.as_str().to_owned(),
            },
            Implausibility::TooLarge => UnitError::Overflow {
                what: format!("{quantity} `{self}`"),
                limit,
            },
        }
    }
}

impl fmt::Display for Measurement {
    /// `<value> <unit>`, e.g. `42.5 s`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", Shortest(self.value), self.unit)
    }
}

/// An `f64` as the shortest text that reads back to the same bits: plain
/// decimal from 10⁻⁵ to below 10¹⁶ and for zero, otherwise with an
/// exponent (`1e-7`, `1e16`), so no number takes hundreds of digits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shortest(pub(crate) f64);

impl fmt::Display for Shortest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let x = self.0;
        let magnitude = x.abs();
        if (magnitude > 0.0 && magnitude < 1e-5) || magnitude >= 1e16 {
            write!(f, "{x:e}")
        } else {
            write!(f, "{x}")
        }
    }
}

/// Seconds in the time base of a rate unit; 1 for other units.
fn rate_seconds(unit: Unit) -> f64 {
    match unit {
        Unit::PartsPerMinute => 60.0,
        Unit::PartsPerHour => 3_600.0,
        Unit::PartsPerSecond
        | Unit::Millisecond
        | Unit::Second
        | Unit::Minute
        | Unit::Hour
        | Unit::Day
        | Unit::Parts
        | Unit::Percent
        | Unit::One => 1.0,
    }
}

/// Nanoseconds in one duration unit; 1 for other units.
fn nanos_per_unit(unit: Unit) -> u64 {
    match unit {
        Unit::Millisecond => 1_000_000,
        Unit::Second => 1_000_000_000,
        Unit::Minute => 60_000_000_000,
        Unit::Hour => 3_600_000_000_000,
        Unit::Day => 86_400_000_000_000,
        Unit::Parts
        | Unit::PartsPerSecond
        | Unit::PartsPerMinute
        | Unit::PartsPerHour
        | Unit::Percent
        | Unit::One => 1,
    }
}

/// `value` (finite, ≥ 0) units of `unit_nanos` nanoseconds as whole
/// nanoseconds, rounded once, half to even, from the exact product of the
/// binary value and the unit; `None` beyond `u64::MAX`.
fn duration_nanos(value: f64, unit_nanos: u64) -> Option<u64> {
    // value = mantissa × 2^(exponent - 1075), exactly (IEEE 754 binary64).
    let bits = value.to_bits();
    let biased = (bits >> 52) & 0x7ff;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, biased) = if biased == 0 {
        (fraction, 1)
    } else {
        (fraction | (1 << 52), biased)
    };
    // mantissa < 2^53 and nanoseconds per unit < 2^47, so the product < 2^100.
    let product = u128::from(mantissa) * u128::from(unit_nanos);
    let nanos = if biased >= 1075 {
        let shift = u32::try_from(biased - 1075).ok()?;
        // Any shift past the leading zeros of the product overflows.
        if product != 0 && shift >= product.leading_zeros() {
            return None;
        }
        product << shift
    } else {
        let shift = u32::try_from(1075 - biased).ok()?;
        if shift >= 101 {
            // product < 2^100 ≤ half of 2^shift: rounds to zero.
            0
        } else {
            let quotient = product >> shift;
            let remainder = product & ((1 << shift) - 1);
            let half = 1 << (shift - 1);
            let round_up = remainder > half || (remainder == half && quotient % 2 == 1);
            quotient + u128::from(round_up)
        }
    };
    u64::try_from(nanos).ok()
}

/// `value` (finite, ≥ 0) as a whole count.
fn count(value: f64) -> Result<Count, Implausibility> {
    if value.fract() != 0.0 {
        return Err(Implausibility::NotWhole);
    }
    // 2^64 is the first whole f64 beyond u64::MAX.
    if value >= 18_446_744_073_709_551_616.0 {
        return Err(Implausibility::TooLarge);
    }
    // Whole, non-negative and below 2^64: the cast is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(Count::new(value as u64))
}

/// Why a value is implausible for its unit; see
/// [`Measurement::implausibility`].
///
/// ```
/// use taktsim_adapters::observation::{Implausibility, Measurement, Unit};
///
/// let m = Measurement::new(-5.0, Unit::Second)?;
/// assert_eq!(m.implausibility(), Some(Implausibility::Negative));
/// assert_eq!(Implausibility::Negative.to_string(), "negative, but the unit measures a quantity that cannot be");
/// # Ok::<(), taktsim_adapters::observation::ValueError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum Implausibility {
    /// A negative duration, count, rate or percentage.
    #[error("negative, but the unit measures a quantity that cannot be")]
    Negative,
    /// A count (`parts`) that is not a whole number.
    #[error("a count that is not a whole number")]
    NotWhole,
    /// A duration beyond about 584 years, or a count beyond
    /// 18446744073709551615 parts: more than `taktsim-units` can hold.
    #[error("too large: durations end at about 584 years, counts at 18446744073709551615 parts")]
    TooLarge,
}

/// A [`Measurement`] as the `taktsim-units` type of its unit; see
/// [`Measurement::to_quantity`].
///
/// ```
/// use taktsim_adapters::observation::{Measurement, Quantity, Unit};
/// use taktsim_units::Count;
///
/// let q = Measurement::new(1520.0, Unit::Parts)?.to_quantity()?;
/// assert_eq!(q, Quantity::Count(Count::new(1520)));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quantity {
    /// From `ms`, `s`, `min`, `h` or `d`.
    Duration(SimDuration),
    /// From `parts`.
    Count(Count),
    /// From `parts/s`, `parts/min` or `parts/h`.
    Rate(Rate),
    /// From `%`.
    Ratio(Ratio),
    /// From `1`.
    Number(f64),
}

/// Why a field is not a valid value.
///
/// ```
/// use taktsim_adapters::observation::ValueError;
///
/// assert_eq!(ValueError::DecimalComma.to_string(), "the number uses `,` as decimal separator; use `.`");
/// assert_eq!(ValueError::EXAMPLE, "1520");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ValueError {
    /// The text is not a decimal number.
    #[error("not a decimal number")]
    NotANumber,
    /// The text is a number with a decimal comma, e.g. `12,5`.
    #[error("the number uses `,` as decimal separator; use `.`")]
    DecimalComma,
    /// NaN, infinity, or a number beyond the range of an `f64` (±1.8e308).
    #[error("the number is not finite")]
    NotFinite,
}

impl ValueError {
    /// What a value may be.
    ///
    /// ```
    /// use taktsim_adapters::observation::ValueError;
    ///
    /// assert!(ValueError::ALLOWED.starts_with("a finite decimal number"));
    /// ```
    pub const ALLOWED: &'static str = "a finite decimal number with `.` as decimal separator and an \
         optional exponent, or an empty field for a missing value";

    /// One valid value.
    ///
    /// ```
    /// use taktsim_adapters::observation::ValueError;
    ///
    /// assert_eq!(ValueError::EXAMPLE.parse::<f64>(), Ok(1520.0));
    /// ```
    pub const EXAMPLE: &'static str = "1520";
}

/// Parses a non-empty value field: Rust's `f64` parser, which rounds
/// decimal text correctly and identically on every platform.
pub(crate) fn parse_value(text: &str) -> Result<f64, ValueError> {
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        Ok(_) => Err(ValueError::NotFinite),
        Err(_) if text.matches(',').count() == 1 && !text.contains('.') => {
            match text.replacen(',', ".", 1).parse::<f64>() {
                Ok(_) => Err(ValueError::DecimalComma),
                Err(_) => Err(ValueError::NotANumber),
            }
        }
        Err(_) => Err(ValueError::NotANumber),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_match_taktsim_units() {
        // Every unit but `1` is one that taktsim-units parses, with the
        // dimension its type says.
        for unit in Unit::ALL {
            let text = format!("1 {unit}");
            let ok = match unit.dimension() {
                Dimension::Duration => text.replace(' ', "").parse::<SimDuration>().is_ok(),
                Dimension::Count => text.parse::<Count>().is_ok(),
                Dimension::Rate => text.parse::<Rate>().is_ok(),
                Dimension::Ratio => text.replace(' ', "").parse::<Ratio>().is_ok(),
                Dimension::Dimensionless => unit == Unit::One,
            };
            assert!(ok, "{unit}");
            assert_eq!(unit.as_str().parse::<Unit>(), Ok(unit));
        }
        for alias in ["part", "part/s", "part/min", "part/h"] {
            assert!(alias.parse::<Unit>().is_ok());
        }
        for unknown in ["", "S", "sec", "pcs", "parts/d", "percent", "kg", " s"] {
            assert_eq!(unknown.parse::<Unit>(), Err(UnknownUnit), "{unknown:?}");
        }
    }

    /// The units each taktsim-units parser accepts, as its error lists them.
    fn taktsim_units_allowed(err: &UnitError) -> &'static [&'static str] {
        let &UnitError::UnknownUnit { allowed, .. } = err else {
            panic!("{err:?}");
        };
        allowed
    }

    #[test]
    fn every_taktsim_units_unit_is_known_and_converts_alike() {
        // The other direction of `units_match_taktsim_units`: a unit added to
        // taktsim-units but not here fails this test.
        let lists = [
            (
                Dimension::Duration,
                taktsim_units_allowed(&"1xyz".parse::<SimDuration>().unwrap_err()),
            ),
            (
                Dimension::Count,
                taktsim_units_allowed(&"1 xyz".parse::<Count>().unwrap_err()),
            ),
            (
                Dimension::Rate,
                taktsim_units_allowed(&"1 xyz".parse::<Rate>().unwrap_err()),
            ),
            (
                Dimension::Ratio,
                taktsim_units_allowed(&"1xyz".parse::<Ratio>().unwrap_err()),
            ),
        ];
        let mut seen = 0;
        for (dimension, allowed) in lists {
            for &text in allowed {
                let unit: Unit = text.parse().unwrap();
                assert_eq!(unit.dimension(), dimension, "{text}");
                seen += 1;
                // Thousandths below 64 convert exactly both ways, for every
                // duration unit up to days; whole numbers in any range.
                let thousandths = (0..64_000).step_by(37).map(|n| (n / 1000, n % 1000));
                let whole = [1, 7, 60, 1000, 100_000, 213_503].map(|n| (n, 0));
                for (int, frac) in thousandths.chain(whole) {
                    let number = format!("{int}.{frac:03}");
                    let ours = Measurement::new(number.parse().unwrap(), unit)
                        .unwrap()
                        .to_quantity();
                    let theirs = match dimension {
                        Dimension::Duration => {
                            format!("{number}{text}").parse().map(Quantity::Duration)
                        }
                        Dimension::Count if frac == 0 => {
                            format!("{int} {text}").parse().map(Quantity::Count)
                        }
                        Dimension::Count => continue,
                        Dimension::Rate => format!("{number} {text}").parse().map(Quantity::Rate),
                        Dimension::Ratio => format!("{number}{text}").parse().map(Quantity::Ratio),
                        Dimension::Dimensionless => unreachable!(),
                    };
                    assert_eq!(ours, theirs, "{number} {text}");
                }
            }
        }
        // Every taktsim-units unit, and the adapter's own: the ten canonical
        // units, four singular forms; `1` is the adapter's addition.
        assert_eq!(seen, 14);
        assert_eq!(Unit::ALL.len(), 11);
    }

    #[test]
    fn values() {
        assert_eq!(parse_value("1520"), Ok(1520.0));
        assert_eq!(parse_value("-0.25"), Ok(-0.25));
        assert_eq!(parse_value("+1e3"), Ok(1000.0));
        assert_eq!(parse_value(".5"), Ok(0.5));
        assert_eq!(parse_value("1.7976931348623157e308"), Ok(f64::MAX));
        assert_eq!(parse_value("5e-324"), Ok(5e-324));
        assert_eq!(parse_value("NaN"), Err(ValueError::NotFinite));
        assert_eq!(parse_value("inf"), Err(ValueError::NotFinite));
        assert_eq!(parse_value("-infinity"), Err(ValueError::NotFinite));
        assert_eq!(parse_value("1e309"), Err(ValueError::NotFinite));
        assert_eq!(parse_value("12,5"), Err(ValueError::DecimalComma));
        assert_eq!(parse_value("1,234.5"), Err(ValueError::NotANumber));
        assert_eq!(parse_value("1,2,3"), Err(ValueError::NotANumber));
        assert_eq!(parse_value("a,b"), Err(ValueError::NotANumber));
        assert_eq!(parse_value("12 parts"), Err(ValueError::NotANumber));
        assert_eq!(parse_value("0x10"), Err(ValueError::NotANumber));
        assert_eq!(ValueError::NotANumber.to_string(), "not a decimal number");
        assert_eq!(
            ValueError::NotFinite.to_string(),
            "the number is not finite"
        );
    }

    #[test]
    fn negative_zero_is_zero() {
        let m = Measurement::new(-0.0, Unit::Second).unwrap();
        assert_eq!(m.value().to_bits(), 0);
        assert_eq!(m.implausibility(), None);
        assert_eq!(
            Measurement::new(f64::NAN, Unit::One),
            Err(ValueError::NotFinite)
        );
        assert_eq!(
            Measurement::new(f64::NEG_INFINITY, Unit::One),
            Err(ValueError::NotFinite)
        );
    }

    proptest::proptest! {
        /// A value is flagged implausible exactly when it has no
        /// taktsim-units quantity.
        #[test]
        fn implausible_exactly_when_not_convertible(
            bits in proptest::prelude::any::<u64>(),
            unit in proptest::sample::select(Unit::ALL.to_vec()),
        ) {
            let x = f64::from_bits(bits);
            proptest::prop_assume!(x.is_finite());
            let m = Measurement::new(x, unit).unwrap();
            proptest::prop_assert_eq!(m.implausibility().is_some(), m.to_quantity().is_err());
            // The message never spells out hundreds of digits.
            proptest::prop_assert!(m.to_string().len() < 40, "{}", m);
        }
    }

    #[test]
    fn display_is_short() {
        let show = |v, u| Measurement::new(v, u).unwrap().to_string();
        assert_eq!(show(1e300, Unit::Parts), "1e300 parts");
        assert_eq!(show(-2.5e-9, Unit::Second), "-2.5e-9 s");
        assert_eq!(show(1520.0, Unit::Parts), "1520 parts");
        assert_eq!(show(0.0, Unit::One), "0 1");
        let err = Measurement::new(1e300, Unit::Parts)
            .unwrap()
            .to_quantity()
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "count `1e300 parts` overflows; the largest value is 18446744073709551615 parts"
        );
        let err = Measurement::new(1e300, Unit::Day)
            .unwrap()
            .to_quantity()
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "duration `1e300 d` overflows; the largest value is 18446744073.709551615s (about 584 years)"
        );
        assert_eq!(
            Implausibility::TooLarge.to_string(),
            "too large: durations end at about 584 years, counts at 18446744073709551615 parts"
        );
    }

    #[test]
    fn durations_from_2_to_the_52_overflow() {
        // 2^52 ms is about 143,000 years: the exact product is formed, then
        // found too large.
        let m = Measurement::new(4_503_599_627_370_496.0, Unit::Millisecond).unwrap();
        assert!(matches!(m.to_quantity(), Err(UnitError::Overflow { .. })));
    }

    #[test]
    fn implausible_values() {
        let check = |v, u| Measurement::new(v, u).unwrap().implausibility();
        for unit in Unit::ALL {
            let expected = (unit != Unit::One).then_some(Implausibility::Negative);
            assert_eq!(check(-1.0, unit), expected, "{unit}");
            assert_eq!(check(-1e-300, unit), expected, "{unit}");
            assert_eq!(check(0.0, unit), None, "{unit}");
        }
        assert_eq!(check(-2.5, Unit::Parts), Some(Implausibility::Negative));
        assert_eq!(check(0.5, Unit::Parts), Some(Implausibility::NotWhole));
        assert_eq!(check(0.5, Unit::Percent), None);
        // Beyond what taktsim-units holds, so to_quantity would fail too.
        assert_eq!(check(1e300, Unit::Parts), Some(Implausibility::TooLarge));
        assert_eq!(
            check(18_446_744_073_709_551_616.0, Unit::Parts),
            Some(Implausibility::TooLarge)
        );
        assert_eq!(check(18_446_744_073_709_549_568.0, Unit::Parts), None);
        assert_eq!(check(1e12, Unit::Hour), Some(Implausibility::TooLarge));
        assert_eq!(
            check(18_446_744_074.0, Unit::Second),
            Some(Implausibility::TooLarge)
        );
        assert_eq!(check(18_446_744_073.0, Unit::Second), None);
        assert_eq!(check(1e300, Unit::PartsPerHour), None);
        assert_eq!(check(1e300, Unit::Percent), None);
        assert_eq!(check(-1e300, Unit::One), None);
        assert_eq!(
            Implausibility::NotWhole.to_string(),
            "a count that is not a whole number"
        );
    }

    #[test]
    fn quantities() {
        let q = |v, u| Measurement::new(v, u).unwrap().to_quantity();
        let d = |text: &str| Ok(Quantity::Duration(text.parse::<SimDuration>().unwrap()));
        assert_eq!(q(1.5, Unit::Millisecond), d("1.5ms"));
        assert_eq!(q(42.0, Unit::Second), d("42s"));
        assert_eq!(q(0.1, Unit::Hour), d("6min"));
        assert_eq!(q(2.0, Unit::Day), d("48h"));
        assert_eq!(q(0.0, Unit::Minute), d("0s"));
        assert_eq!(q(20.0, Unit::Parts), Ok(Quantity::Count(Count::new(20))));
        assert_eq!(
            q(120.0, Unit::PartsPerHour),
            Ok(Quantity::Rate("120 parts/h".parse().unwrap()))
        );
        assert_eq!(
            q(3.0, Unit::PartsPerMinute),
            Ok(Quantity::Rate("3 parts/min".parse().unwrap()))
        );
        assert_eq!(
            q(2.0, Unit::PartsPerSecond),
            Ok(Quantity::Rate("2 parts/s".parse().unwrap()))
        );
        assert_eq!(
            q(2.5, Unit::Percent),
            Ok(Quantity::Ratio("2.5%".parse().unwrap()))
        );
        assert_eq!(q(-7.0, Unit::One), Ok(Quantity::Number(-7.0)));
    }

    #[test]
    fn duration_rounding_is_exact() {
        let nanos = |v, u| match Measurement::new(v, u).unwrap().to_quantity() {
            Ok(Quantity::Duration(d)) => d.as_nanos(),
            other => panic!("{other:?}"),
        };
        // 2^-21 ms = 0.476837158203125 ns exactly: rounds down.
        assert_eq!(nanos(1.0 / 2_097_152.0, Unit::Millisecond), 0);
        // 2^-11 s = 488281.25 ns: down; 2^-10 s = 976562.5 ns: tie, to even.
        assert_eq!(nanos(1.0 / 2048.0, Unit::Second), 488_281);
        assert_eq!(nanos(1.0 / 1024.0, Unit::Second), 976_562);
        // 3 × 2^-10 s = 2929687.5 ns: tie, up to the even 2929688.
        assert_eq!(nanos(3.0 / 1024.0, Unit::Second), 2_929_688);
        // Subnormal and tiny values round to zero.
        assert_eq!(nanos(5e-324, Unit::Day), 0);
        assert_eq!(nanos(1e-200, Unit::Second), 0);
        // The largest representable duration, and just beyond it.
        assert_eq!(
            nanos(18_446_744_073.0, Unit::Second),
            18_446_744_073_000_000_000
        );
        let beyond = Measurement::new(18_446_744_074.0, Unit::Second).unwrap();
        assert!(matches!(
            beyond.to_quantity(),
            Err(UnitError::Overflow { .. })
        ));
        let huge = Measurement::new(1e300, Unit::Day).unwrap();
        assert!(matches!(
            huge.to_quantity(),
            Err(UnitError::Overflow { .. })
        ));
        // Whole numbers of a large unit are exact.
        assert_eq!(nanos(213_503.0, Unit::Day), 213_503 * 86_400_000_000_000);
    }

    #[test]
    fn quantity_errors() {
        let q = |v, u| Measurement::new(v, u).unwrap().to_quantity();
        assert_eq!(
            q(-3.0, Unit::Parts).unwrap_err().to_string(),
            "negative count `-3 parts` is not allowed; example: 20 parts"
        );
        assert!(matches!(
            q(-1.0, Unit::Minute),
            Err(UnitError::Negative {
                quantity: "duration",
                ..
            })
        ));
        assert!(matches!(
            q(-1.0, Unit::PartsPerHour),
            Err(UnitError::Negative {
                quantity: "rate",
                ..
            })
        ));
        assert!(matches!(
            q(-1.0, Unit::Percent),
            Err(UnitError::Negative {
                quantity: "percentage",
                ..
            })
        ));
        assert_eq!(
            q(2.5, Unit::Parts).unwrap_err().to_string(),
            "`2.5 parts` is not a whole number of parts; example: 20 parts"
        );
        assert!(matches!(
            q(18_446_744_073_709_551_616.0, Unit::Parts),
            Err(UnitError::Overflow { .. })
        ));
        assert_eq!(
            q(18_446_744_073_709_549_568.0, Unit::Parts),
            Ok(Quantity::Count(Count::new(18_446_744_073_709_549_568)))
        );
    }
}
