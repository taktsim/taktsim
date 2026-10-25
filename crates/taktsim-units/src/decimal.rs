//! Exact parsing of `<decimal number><optional space><unit>` text.
//!
//! Durations and counts are parsed without floating point, so `0.1h` is
//! exactly 360 s on every platform.

use crate::UnitError;

/// What kind of quantity is being parsed, for error messages.
pub(crate) struct Spec {
    pub(crate) quantity: &'static str,
    pub(crate) allowed: &'static [&'static str],
    pub(crate) example: &'static str,
}

/// A non-negative decimal number, split into its digits.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Decimal<'a> {
    /// The number exactly as written, e.g. `0.50`.
    pub(crate) text: &'a str,
    int: &'a str,
    /// Fractional digits with trailing zeros removed.
    frac: &'a str,
}

impl Decimal<'_> {
    /// `self × factor`: an exact integer, not an integer, or too large for a
    /// `u64`.
    pub(crate) fn scale(&self, factor: u64) -> Scaled {
        // Up to 20 integer digits fit u64; more always overflows.
        let Ok(int) = self.int.parse::<u64>() else {
            return Scaled::Overflow;
        };
        let Some(int_part) = int.checked_mul(factor) else {
            return Scaled::Overflow;
        };
        // With d significant fractional digits (the last one non-zero), the
        // result is an integer only if `factor` holds d factors of 2 or of 5.
        // Every factor in this crate divides 864 × 10^11 (one day in ns), so
        // more than 19 digits is never exact.
        let Ok(digits @ 0..=19) = u32::try_from(self.frac.len()) else {
            return Scaled::Inexact;
        };
        // At most 19 ASCII digits, so neither this nor `frac × factor`
        // (below 10^19 × 2^64 < 2^128) can overflow.
        let frac = self
            .frac
            .bytes()
            .fold(0_u128, |acc, b| acc * 10 + u128::from(b - b'0'));
        let product = frac * u128::from(factor);
        let divisor = 10_u128.pow(digits);
        if !product.is_multiple_of(divisor) {
            return Scaled::Inexact;
        }
        match u64::try_from(u128::from(int_part) + product / divisor) {
            Ok(total) => Scaled::Exact(total),
            Err(_) => Scaled::Overflow,
        }
    }
}

/// The result of [`Decimal::scale`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scaled {
    Exact(u64),
    Inexact,
    Overflow,
}

/// Splits `input` into a decimal number and a unit, allowing at most one
/// space between them.
pub(crate) fn split<'a>(input: &'a str, spec: &Spec) -> Result<(Decimal<'a>, &'a str), UnitError> {
    if input.starts_with('-') {
        return Err(UnitError::Negative {
            input: input.to_owned(),
            quantity: spec.quantity,
            example: spec.example,
        });
    }
    let end = input
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(input.len());
    let (text, rest) = input.split_at(end);
    let invalid = || UnitError::InvalidNumber {
        input: input.to_owned(),
        quantity: spec.quantity,
        example: spec.example,
    };
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    let all_digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !all_digits(int) || (text.contains('.') && !all_digits(frac)) {
        return Err(invalid());
    }
    let unit = rest.strip_prefix(' ').unwrap_or(rest);
    Ok((
        Decimal {
            text,
            int,
            frac: frac.trim_end_matches('0'),
        },
        unit,
    ))
}

/// The error for a unit that is not in `spec.allowed`.
pub(crate) fn unknown_unit(input: &str, unit: &str, spec: &Spec) -> UnitError {
    UnitError::UnknownUnit {
        input: input.to_owned(),
        unit: unit.to_owned(),
        quantity: spec.quantity,
        allowed: spec.allowed,
        example: spec.example,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: Spec = Spec {
        quantity: "duration",
        allowed: &["s"],
        example: "42s",
    };

    fn dec(s: &str) -> Decimal<'_> {
        split(s, &SPEC).unwrap().0
    }

    #[test]
    fn splits_number_and_unit() {
        let (d, unit) = split("12.50 min", &SPEC).unwrap();
        assert_eq!((d.text, d.int, d.frac, unit), ("12.50", "12", "5", "min"));
        assert_eq!(split("42s", &SPEC).unwrap().1, "s");
        assert_eq!(split("42", &SPEC).unwrap().1, "");
        assert_eq!(split("42  s", &SPEC).unwrap().1, " s");
    }

    #[test]
    fn rejects_malformed_numbers() {
        for bad in ["", "s", ".5s", "5.s", "1.2.3s", " 5s", "+5s"] {
            assert!(
                matches!(split(bad, &SPEC), Err(UnitError::InvalidNumber { .. })),
                "{bad:?}"
            );
        }
        assert!(matches!(
            split("-5s", &SPEC),
            Err(UnitError::Negative { .. })
        ));
    }

    #[test]
    fn scales_exactly() {
        assert_eq!(dec("42").scale(1_000), Scaled::Exact(42_000));
        assert_eq!(dec("0.1").scale(3_600), Scaled::Exact(360));
        assert_eq!(dec("2.500").scale(4), Scaled::Exact(10));
        assert_eq!(dec("2.000").scale(1), Scaled::Exact(2));
        assert_eq!(dec("2.500").scale(1), Scaled::Inexact);
    }

    #[test]
    fn detects_inexact_results() {
        assert_eq!(dec("0.1").scale(1), Scaled::Inexact);
        assert_eq!(
            dec("0.000000000000000000001").scale(1_000_000_000),
            Scaled::Inexact
        );
        assert_eq!(dec("0.99999999999999999999").scale(10), Scaled::Inexact);
    }

    #[test]
    fn detects_overflow() {
        assert_eq!(dec("18446744073709551616").scale(1), Scaled::Overflow);
        assert_eq!(dec("18446744073709551615").scale(2), Scaled::Overflow);
        assert_eq!(dec("18446744073709551615.5").scale(2), Scaled::Overflow);
        assert_eq!(
            dec("18446744073709551615").scale(1),
            Scaled::Exact(u64::MAX)
        );
    }

    #[test]
    fn detects_overflow_when_adding_the_fraction() {
        // The integer part fits; adding the fraction overflows.
        assert_eq!(
            dec("1844674407370955161.5").scale(10),
            Scaled::Exact(u64::MAX)
        );
        assert_eq!(dec("1844674407370955161.6").scale(10), Scaled::Overflow);
    }
}
