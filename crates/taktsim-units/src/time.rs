//! Simulation time: integer nanoseconds (ADR-0001).

use std::fmt;
use std::str::FromStr;

use crate::UnitError;
use crate::decimal::{self, Scaled, Spec};

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// The longest duration and the latest instant, for error messages.
const DURATION_LIMIT: &str = "18446744073.709551615s (about 584 years)";

/// Duration units accepted by the parser, with their length in nanoseconds.
const UNITS: [(&str, u64); 5] = [
    ("d", 86_400 * NANOS_PER_SEC),
    ("h", 3_600 * NANOS_PER_SEC),
    ("min", 60 * NANOS_PER_SEC),
    ("s", NANOS_PER_SEC),
    ("ms", 1_000_000),
];

/// Units used for display, largest first. Milliseconds are parsed but not
/// displayed: `0.25s`, not `250ms`.
const DISPLAY_UNITS: [(&str, u64); 4] = [
    ("d", 86_400 * NANOS_PER_SEC),
    ("h", 3_600 * NANOS_PER_SEC),
    ("min", 60 * NANOS_PER_SEC),
    ("s", NANOS_PER_SEC),
];

const SPEC: Spec = Spec {
    quantity: "duration",
    allowed: &["d", "h", "min", "s", "ms"],
    example: "42s",
};

/// A length of simulated time, in whole nanoseconds.
///
/// Parsed exactly from text such as `42s`, `12min`, `0.5h` or `2d`, so the
/// same text gives the same duration on every platform. Arithmetic is
/// checked: there are no `+` or `-` operators that could overflow.
///
/// ```
/// use taktsim_units::SimDuration;
///
/// let cycle: SimDuration = "42s".parse()?;
/// let setup: SimDuration = "4min".parse()?;
/// assert_eq!(cycle.checked_add(setup)?.to_string(), "282s");
/// assert_eq!("0.1h".parse::<SimDuration>()?, "6min".parse()?);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
///
/// A duration and a rate are different types, so they cannot be mixed:
///
/// ```compile_fail,E0308
/// use taktsim_units::{Count, SimDuration};
///
/// let d: SimDuration = "42s".parse().unwrap();
/// let r = Count::new(20).per("1h".parse().unwrap()).unwrap();
/// let _ = d.checked_add(r);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SimDuration(u64);

impl SimDuration {
    /// The zero duration.
    ///
    /// ```
    /// assert_eq!(taktsim_units::SimDuration::ZERO.as_nanos(), 0);
    /// ```
    pub const ZERO: Self = Self(0);

    /// The longest representable duration, about 584 years.
    ///
    /// ```
    /// assert_eq!(taktsim_units::SimDuration::MAX.as_nanos(), u64::MAX);
    /// ```
    pub const MAX: Self = Self(u64::MAX);

    /// A duration of `nanos` nanoseconds.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// assert_eq!(SimDuration::from_nanos(1_500_000_000).to_string(), "1.5s");
    /// ```
    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// A duration of `secs` whole seconds.
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] beyond about 584 years.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// assert_eq!(SimDuration::from_secs(3_600)?.to_string(), "1h");
    /// assert!(SimDuration::from_secs(u64::MAX).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn from_secs(secs: u64) -> Result<Self, UnitError> {
        secs.checked_mul(NANOS_PER_SEC)
            .map(Self)
            .ok_or_else(|| UnitError::Overflow {
                what: format!("duration of {secs} s"),
                limit: DURATION_LIMIT,
            })
    }

    /// The duration nearest to `secs` seconds, for sampled values.
    ///
    /// The exact value of `secs` (an `f64` is a binary fraction) times 10⁹
    /// is rounded once, half to even, to whole nanoseconds, using integer
    /// arithmetic only, so the result is the same on every platform.
    /// Negative values become zero, so a sample from a distribution that can
    /// go below zero (e.g. a normal) is truncated at zero (ADR-0001,
    /// ADR-0003).
    ///
    /// # Errors
    ///
    /// [`UnitError::NotFinite`] for NaN or infinity, [`UnitError::Overflow`]
    /// beyond about 584 years.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// assert_eq!(SimDuration::from_secs_f64(42.0)?.to_string(), "42s");
    /// // 1/1024 s is exactly 976562.5 ns: a tie, rounded to the even tick.
    /// assert_eq!(SimDuration::from_secs_f64(1.0 / 1024.0)?.as_nanos(), 976_562);
    /// assert_eq!(SimDuration::from_secs_f64(-1.0)?, SimDuration::ZERO);
    /// assert!(SimDuration::from_secs_f64(f64::NAN).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn from_secs_f64(secs: f64) -> Result<Self, UnitError> {
        if !secs.is_finite() {
            return Err(UnitError::NotFinite {
                what: format!("duration of {secs} s"),
            });
        }
        if secs <= 0.0 {
            return Ok(Self::ZERO);
        }
        // secs = mantissa × 2^(exponent - 1075), exactly (IEEE 754 binary64).
        let bits = secs.to_bits();
        let exponent = (bits >> 52) & 0x7ff;
        let fraction = bits & ((1 << 52) - 1);
        let (mantissa, exponent) = if exponent == 0 {
            (fraction, 1) // subnormal
        } else {
            (fraction | (1 << 52), exponent)
        };
        // Below 2^53 × 10^9 < 2^83.
        let product = u128::from(mantissa) * u128::from(NANOS_PER_SEC);
        let nanos = if exponent >= 1075 {
            // A whole number of nanoseconds; beyond 2^44 it overflows anyway.
            let shift = exponent - 1075;
            if shift > 44 {
                None
            } else {
                Some(product << shift)
            }
        } else {
            let shift = 1075 - exponent;
            if shift > 84 {
                // Below 2^83 / 2^85: less than half a nanosecond.
                Some(0)
            } else {
                let quotient = product >> shift;
                let remainder = product & ((1 << shift) - 1);
                let half = 1 << (shift - 1);
                let round_up = remainder > half || (remainder == half && quotient % 2 == 1);
                Some(quotient + u128::from(round_up))
            }
        };
        nanos
            .and_then(|n| u64::try_from(n).ok())
            .map(Self)
            .ok_or_else(|| UnitError::Overflow {
                what: format!("duration of {secs} s"),
                limit: DURATION_LIMIT,
            })
    }

    /// The duration in nanoseconds.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// assert_eq!("2ms".parse::<SimDuration>()?.as_nanos(), 2_000_000);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// The duration in seconds, for statistics. Exact up to about 104 days;
    /// beyond that it is rounded to the nearest representable `f64`.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// assert_eq!("1.5min".parse::<SimDuration>()?.as_secs_f64(), 90.0);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn as_secs_f64(self) -> f64 {
        // Statistics work in f64; rounding beyond 2^53 ns is intended.
        #[allow(clippy::cast_precision_loss)]
        let nanos = self.0 as f64;
        nanos / 1e9
    }

    /// `self + other`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] beyond about 584 years.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// let a: SimDuration = "1h".parse()?;
    /// assert_eq!(a.checked_add("30min".parse()?)?.to_string(), "90min");
    /// assert!(SimDuration::MAX.checked_add(a).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_add(self, other: Self) -> Result<Self, UnitError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or_else(|| UnitError::Overflow {
                what: format!("duration {self} + {other}"),
                limit: DURATION_LIMIT,
            })
    }

    /// `self - other`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Underflow`] if `other` is longer than `self`.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// let a: SimDuration = "1h".parse()?;
    /// assert_eq!(a.checked_sub("15min".parse()?)?.to_string(), "45min");
    /// assert!(SimDuration::ZERO.checked_sub(a).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_sub(self, other: Self) -> Result<Self, UnitError> {
        self.0
            .checked_sub(other.0)
            .map(Self)
            .ok_or_else(|| UnitError::Underflow {
                what: format!("duration {self} - {other}"),
            })
    }

    /// `self × n`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] beyond about 584 years.
    ///
    /// ```
    /// use taktsim_units::SimDuration;
    ///
    /// let shift: SimDuration = "8h".parse()?;
    /// assert_eq!(shift.checked_mul(3)?.to_string(), "1d");
    /// assert!(shift.checked_mul(u64::MAX).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_mul(self, n: u64) -> Result<Self, UnitError> {
        self.0
            .checked_mul(n)
            .map(Self)
            .ok_or_else(|| UnitError::Overflow {
                what: format!("duration {self} × {n}"),
                limit: DURATION_LIMIT,
            })
    }
}

impl FromStr for SimDuration {
    type Err = UnitError;

    /// Parses `<number><unit>`, with an optional single space, e.g. `42s`,
    /// `12 min`, `0.5h`. Units: `d`, `h`, `min`, `s`, `ms`.
    fn from_str(input: &str) -> Result<Self, UnitError> {
        let (number, unit) = decimal::split(input, &SPEC)?;
        let Some(&(_, factor)) = UNITS.iter().find(|(name, _)| *name == unit) else {
            return Err(decimal::unknown_unit(input, unit, &SPEC));
        };
        match number.scale(factor) {
            Scaled::Exact(nanos) => Ok(Self(nanos)),
            Scaled::Inexact => Err(UnitError::TooPrecise {
                input: input.to_owned(),
            }),
            Scaled::Overflow => Err(UnitError::Overflow {
                what: format!("duration `{input}`"),
                limit: DURATION_LIMIT,
            }),
        }
    }
}

impl fmt::Display for SimDuration {
    /// The largest of `d`, `h`, `min`, `s` that represents the duration as a
    /// whole number (`1d`, `90min`, `42s`), otherwise seconds with a decimal
    /// fraction (`1.5s`, `0.25s`, `0.000000001s`). Parsing the output gives
    /// the same duration.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            return f.write_str("0s");
        }
        if let Some((unit, factor)) = DISPLAY_UNITS
            .iter()
            .find(|(_, factor)| self.0.is_multiple_of(*factor))
        {
            return write!(f, "{}{unit}", self.0 / factor);
        }
        // Not whole seconds, so the fraction is non-zero; drop its trailing
        // zeros without allocating.
        let secs = self.0 / NANOS_PER_SEC;
        let mut frac = self.0 % NANOS_PER_SEC;
        let mut width = 9;
        while frac.is_multiple_of(10) {
            frac /= 10;
            width -= 1;
        }
        write!(f, "{secs}.{frac:0width$}s")
    }
}

/// A point in simulated time, in nanoseconds since the start of the run.
///
/// Instants are moved by durations, and the gap between two instants is a
/// duration; two instants cannot be added.
///
/// ```
/// use taktsim_units::{SimDuration, SimTime};
///
/// let start = SimTime::START;
/// let end = start.checked_add("8h".parse()?)?;
/// assert_eq!(end.checked_duration_since(start)?.to_string(), "8h");
/// assert_eq!(end.to_string(), "8h");
/// assert_eq!("8h".parse::<SimTime>()?, end);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
///
/// ```compile_fail,E0308
/// use taktsim_units::SimTime;
///
/// let _ = SimTime::START.checked_add(SimTime::START);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SimTime(u64);

impl SimTime {
    /// The start of a run.
    ///
    /// ```
    /// assert_eq!(taktsim_units::SimTime::START.as_nanos(), 0);
    /// ```
    pub const START: Self = Self(0);

    /// The instant `nanos` nanoseconds after the start.
    ///
    /// ```
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(SimTime::from_nanos(60_000_000_000).to_string(), "1min");
    /// ```
    #[must_use]
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Nanoseconds since the start.
    ///
    /// ```
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(SimTime::from_nanos(7).as_nanos(), 7);
    /// ```
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Seconds since the start, for statistics (see
    /// [`SimDuration::as_secs_f64`]).
    ///
    /// ```
    /// use taktsim_units::SimTime;
    ///
    /// assert_eq!(SimTime::from_nanos(1_500_000_000).as_secs_f64(), 1.5);
    /// ```
    #[must_use]
    pub fn as_secs_f64(self) -> f64 {
        SimDuration(self.0).as_secs_f64()
    }

    /// The instant `d` after `self`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Overflow`] beyond about 584 years after the start.
    ///
    /// ```
    /// use taktsim_units::{SimDuration, SimTime};
    ///
    /// let t = SimTime::START.checked_add("42s".parse()?)?;
    /// assert_eq!(t.as_nanos(), 42_000_000_000);
    /// assert!(t.checked_add(SimDuration::MAX).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_add(self, d: SimDuration) -> Result<Self, UnitError> {
        self.0
            .checked_add(d.0)
            .map(Self)
            .ok_or_else(|| UnitError::Overflow {
                what: format!("time {self} + {d}"),
                limit: DURATION_LIMIT,
            })
    }

    /// The duration from `earlier` to `self`.
    ///
    /// # Errors
    ///
    /// [`UnitError::Underflow`] if `earlier` is after `self`.
    ///
    /// ```
    /// use taktsim_units::SimTime;
    ///
    /// let t = SimTime::START.checked_add("1h".parse()?)?;
    /// assert_eq!(t.checked_duration_since(SimTime::START)?.to_string(), "1h");
    /// assert!(SimTime::START.checked_duration_since(t).is_err());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn checked_duration_since(self, earlier: Self) -> Result<SimDuration, UnitError> {
        self.0
            .checked_sub(earlier.0)
            .map(SimDuration)
            .ok_or_else(|| UnitError::Underflow {
                what: format!("time {self} - {earlier}"),
            })
    }
}

impl FromStr for SimTime {
    type Err = UnitError;

    /// Parses the time since the start, written like a [`SimDuration`]
    /// (`8h`, `90min`, `1.5s`).
    fn from_str(input: &str) -> Result<Self, UnitError> {
        input.parse::<SimDuration>().map(|d| Self(d.0))
    }
}

impl fmt::Display for SimTime {
    /// The time since the start, formatted like a [`SimDuration`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        SimDuration(self.0).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> SimDuration {
        s.parse().unwrap()
    }

    #[test]
    fn parses_every_unit() {
        assert_eq!(d("2d").as_nanos(), 2 * 86_400 * NANOS_PER_SEC);
        assert_eq!(d("6h").as_nanos(), 6 * 3_600 * NANOS_PER_SEC);
        assert_eq!(d("12min").as_nanos(), 720 * NANOS_PER_SEC);
        assert_eq!(d("42s").as_nanos(), 42 * NANOS_PER_SEC);
        assert_eq!(d("250ms").as_nanos(), 250_000_000);
        assert_eq!(d("12 min"), d("12min"));
        assert_eq!(d("0.000000001s").as_nanos(), 1);
    }

    #[test]
    fn rejects_bad_text() {
        let err = |s: &str| s.parse::<SimDuration>().unwrap_err();
        assert!(matches!(err("42"), UnitError::UnknownUnit { unit, .. } if unit.is_empty()));
        assert!(matches!(err("42sec"), UnitError::UnknownUnit { unit, .. } if unit == "sec"));
        assert!(matches!(err("42  s"), UnitError::UnknownUnit { .. }));
        assert!(matches!(err("42S"), UnitError::UnknownUnit { .. }));
        assert!(matches!(err("-42s"), UnitError::Negative { .. }));
        assert!(matches!(err("0.0000000001s"), UnitError::TooPrecise { .. }));
        assert_eq!(
            err("0.0000000001s").to_string(),
            "`0.0000000001s` is finer than the 1 ns resolution of simulation time; \
             use at most 9 decimal places of seconds, e.g. 0.000000001s"
        );
        assert!(matches!(err("600000000d"), UnitError::Overflow { .. }));
        assert_eq!(
            err("42").to_string(),
            "missing duration unit `` in `42`; allowed: d, h, min, s, ms; example: 42s"
        );
    }

    #[test]
    fn displays_in_the_largest_whole_unit() {
        assert_eq!(SimDuration::ZERO.to_string(), "0s");
        assert_eq!(d("24h").to_string(), "1d");
        assert_eq!(d("90min").to_string(), "90min");
        assert_eq!(d("120min").to_string(), "2h");
        assert_eq!(d("1500ms").to_string(), "1.5s");
        assert_eq!(d("250ms").to_string(), "0.25s");
        assert_eq!(SimDuration::from_nanos(1).to_string(), "0.000000001s");
        assert_eq!(
            SimDuration::from_nanos(1_000_000_123).to_string(),
            "1.000000123s"
        );
    }

    #[test]
    fn sampled_durations_round_once_to_the_nearest_tick() {
        // Expected values computed exactly from each f64's binary value.
        let ns = |s: f64| SimDuration::from_secs_f64(s).unwrap().as_nanos();
        assert_eq!(ns(1.5e-9), 1); // the f64 is just below 1.5 ns
        assert_eq!(ns(2.5e-9), 3); // the f64 is just above 2.5 ns
        assert_eq!(ns(2.6e-9), 3);
        assert_eq!(ns(12.345_678_901_5), 12_345_678_901);
        assert_eq!(ns(0.1 + 0.2), 300_000_000);
        assert_eq!(ns(1e10), 10_000_000_000_000_000_000);
        // Exact ties go to the even tick: 976562.5 ns and 2929687.5 ns.
        assert_eq!(ns(1.0 / 1024.0), 976_562);
        assert_eq!(ns(3.0 / 1024.0), 2_929_688);
        // Subnormal and tiny values round to zero; negatives are truncated.
        assert_eq!(ns(5e-324), 0);
        assert_eq!(ns(2.225_073_858_507_201_4e-308), 0);
        assert_eq!(ns(-0.0), 0);
        assert_eq!(ns(-1.0), 0);
    }

    #[test]
    fn sampled_durations_reject_non_finite_and_huge_values() {
        assert!(matches!(
            SimDuration::from_secs_f64(f64::INFINITY),
            Err(UnitError::NotFinite { .. })
        ));
        // Just above 2^64 ns; then values whose binary exponent is >= 52
        // (whole seconds), small and huge.
        for secs in [18_446_744_073.709_553, 1e16, 1e300] {
            assert!(
                matches!(
                    SimDuration::from_secs_f64(secs),
                    Err(UnitError::Overflow { .. })
                ),
                "{secs}"
            );
        }
        assert_eq!(
            SimDuration::from_secs_f64(1e16).unwrap_err().to_string(),
            "duration of 10000000000000000 s overflows; \
             the largest value is 18446744073.709551615s (about 584 years)"
        );
    }

    #[test]
    fn errors_name_the_operation() {
        let e = SimDuration::MAX.checked_mul(2).unwrap_err();
        assert_eq!(
            e.to_string(),
            "duration 18446744073.709551615s × 2 overflows; the largest value is 18446744073.709551615s (about 584 years)"
        );
        let e = SimTime::START
            .checked_duration_since(SimTime::from_nanos(1))
            .unwrap_err();
        assert_eq!(e.to_string(), "time 0s - 0.000000001s would be negative");
    }

    #[test]
    fn converts_seconds() {
        assert_eq!(SimDuration::from_secs(90).unwrap(), d("90s"));
        assert!(matches!(
            SimDuration::from_secs(u64::MAX),
            Err(UnitError::Overflow { .. })
        ));
        assert!((d("1.5min").as_secs_f64() - 90.0).abs() < f64::EPSILON);
        assert!((SimTime::from_nanos(2_500_000_000).as_secs_f64() - 2.5).abs() < f64::EPSILON);
        assert_eq!(SimTime::START.as_nanos(), 0);
        assert_eq!(SimDuration::MAX.as_nanos(), u64::MAX);
    }

    #[test]
    fn time_arithmetic_is_checked() {
        let t = SimTime::START.checked_add(d("8h")).unwrap();
        assert_eq!(t.checked_duration_since(SimTime::START).unwrap(), d("8h"));
        assert_eq!(
            t.checked_add(SimDuration::MAX).unwrap_err().to_string(),
            "time 8h + 18446744073.709551615s overflows; the largest value is 18446744073.709551615s (about 584 years)"
        );
        assert_eq!(
            d("1h").checked_sub(d("2h")).unwrap_err().to_string(),
            "duration 1h - 2h would be negative"
        );
        assert_eq!(
            SimDuration::MAX
                .checked_add(d("1s"))
                .unwrap_err()
                .to_string(),
            "duration 18446744073.709551615s + 1s overflows; the largest value is 18446744073.709551615s (about 584 years)"
        );
    }
}
