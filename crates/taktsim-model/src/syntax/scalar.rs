//! Resolving plain YAML scalars and JSON numbers to values (ADR-0002,
//! ADR-0020).
//!
//! The YAML 1.2 core schema, restricted so that a model reads the same in
//! every YAML parser and in its JSON form:
//!
//! - `true` and `false` are booleans; `null`, `Null`, `NULL`, `~` and the
//!   empty scalar are null.
//! - Numbers follow the JSON grammar. An integer must be within
//!   ±(2^53 − 1), so canonical JSON (RFC 8785) represents it exactly; a
//!   number must be finite.
//! - Every other plain scalar that a YAML 1.1 or 1.2 parser reads as a
//!   boolean, number or date (`yes`, `True`, `012`, `0x1F`, `+1`, `.5`,
//!   `1_000`, `.inf`, `2026-10-26`) is an error that says to quote it. That
//!   is the "no → false" family of surprises; rejecting them now can be
//!   relaxed later without breaking a model.
//! - Everything else is text: `42s`, `20 parts`, `line-a`.

use crate::error::{Ambiguity, LoadErrorKind, MAX_INTEGER};
use crate::syntax::Value;

/// The value of the plain (unquoted) YAML scalar `text`.
pub(super) fn plain(text: &str) -> Result<Value, LoadErrorKind> {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Ok(Value::Null),
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if is_json_number(text) {
        return number(text);
    }
    let reading = if is_yaml_bool(text) {
        Some(Ambiguity::Boolean)
    } else if is_yaml_number(text) {
        Some(Ambiguity::Number)
    } else if is_yaml_timestamp(text) {
        Some(Ambiguity::Timestamp)
    } else {
        None
    };
    match reading {
        Some(reading) => Err(LoadErrorKind::Ambiguous {
            text: text.to_owned(),
            reading,
        }),
        None => Ok(Value::Str(text.to_owned())),
    }
}

/// The value of `text`, which follows the JSON number grammar: an integer
/// without a fraction or exponent, otherwise a float.
pub(crate) fn number(text: &str) -> Result<Value, LoadErrorKind> {
    if !text.contains(['.', 'e', 'E']) {
        // Digits only (after an optional minus), so the only parse failure
        // is a value too large for i64, which is out of range anyway.
        return match text.parse::<i64>() {
            Ok(n) if n.abs() <= MAX_INTEGER => Ok(Value::Int(n)),
            Ok(_) | Err(_) => Err(LoadErrorKind::IntegerRange {
                text: text.to_owned(),
            }),
        };
    }
    // Rust's float parsing is correctly rounded on every platform.
    match text.parse::<f64>() {
        Ok(x) if x.is_finite() => Ok(Value::Float(x)),
        Ok(_) | Err(_) => Err(LoadErrorKind::NotFinite {
            text: text.to_owned(),
        }),
    }
}

/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?` (RFC 8259 §6).
pub(crate) fn is_json_number(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => i += digits(&b[i..]),
        _ => return false,
    }
    if b.get(i) == Some(&b'.') {
        let n = digits(&b[i + 1..]);
        if n == 0 {
            return false;
        }
        i += 1 + n;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let n = digits(&b[i..]);
        if n == 0 {
            return false;
        }
        i += n;
    }
    i == b.len()
}

/// The number of leading ASCII digits.
fn digits(b: &[u8]) -> usize {
    b.iter().take_while(|c| c.is_ascii_digit()).count()
}

/// A boolean in YAML 1.1 (`y`, `Yes`, `ON`, …) or 1.2 (`True`, `FALSE`),
/// other than `true` and `false`.
fn is_yaml_bool(text: &str) -> bool {
    matches!(
        text,
        "y" | "Y"
            | "yes"
            | "Yes"
            | "YES"
            | "n"
            | "N"
            | "no"
            | "No"
            | "NO"
            | "True"
            | "TRUE"
            | "False"
            | "FALSE"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    )
}

/// A number in the YAML 1.2 core schema or YAML 1.1 that is not a JSON
/// number: a sign `+`, leading zeros or octal, `0x`/`0o`/`0b`, a leading or
/// trailing dot, `_` separators, base-60 (`1:30`), infinities and NaN.
fn is_yaml_number(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if matches!(unsigned, ".inf" | ".Inf" | ".INF") || matches!(text, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    if let Some(rest) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0o"))
        .or_else(|| unsigned.strip_prefix("0b"))
    {
        return !rest.is_empty() && rest.bytes().all(|c| c.is_ascii_hexdigit() || c == b'_');
    }
    // Decimal forms: digits, `_`, at most one `.`, `:` groups, and an
    // optional exponent; at least one digit before the exponent.
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(i) => (&unsigned[..i], Some(&unsigned[i + 1..])),
        None => (unsigned, None),
    };
    let mantissa_ok = mantissa.bytes().any(|c| c.is_ascii_digit())
        && mantissa
            .bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'_' | b'.' | b':'))
        && mantissa.bytes().filter(|&c| c == b'.').count() <= 1
        && mantissa
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_digit() || *c == b'.');
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && e.bytes().all(|c| c.is_ascii_digit())
    });
    mantissa_ok && exponent_ok
}

/// A YAML 1.1 timestamp: it starts with `YYYY-M-D`.
fn is_yaml_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    let year = digits(b);
    if year != 4 || b.get(4) != Some(&b'-') {
        return false;
    }
    let month = digits(&b[5..]);
    if !(1..=2).contains(&month) || b.get(5 + month) != Some(&b'-') {
        return false;
    }
    let day = digits(&b[6 + month..]);
    (1..=2).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ambiguity(text: &str) -> Option<Ambiguity> {
        match plain(text) {
            Err(LoadErrorKind::Ambiguous { reading, .. }) => Some(reading),
            Ok(_) | Err(_) => None,
        }
    }

    #[test]
    fn nulls_booleans_and_text() {
        for text in ["", "~", "null", "Null", "NULL"] {
            assert_eq!(plain(text), Ok(Value::Null), "{text:?}");
        }
        assert_eq!(plain("true"), Ok(Value::Bool(true)));
        assert_eq!(plain("false"), Ok(Value::Bool(false)));
        for text in [
            "42s", "20 parts", "2%", "line-a", "1d", "0.5h", "1e", "weld-1", "-", ".",
        ] {
            assert_eq!(plain(text), Ok(Value::Str(text.to_owned())), "{text:?}");
        }
    }

    #[test]
    fn json_numbers() {
        assert_eq!(plain("0"), Ok(Value::Int(0)));
        assert_eq!(plain("-0"), Ok(Value::Int(0)));
        assert_eq!(plain("42"), Ok(Value::Int(42)));
        assert_eq!(plain("9007199254740991"), Ok(Value::Int(MAX_INTEGER)));
        assert_eq!(plain("-9007199254740991"), Ok(Value::Int(-MAX_INTEGER)));
        assert_eq!(plain("0.08"), Ok(Value::Float(0.08)));
        assert_eq!(plain("1e3"), Ok(Value::Float(1000.0)));
        assert_eq!(plain("-2.5E-3"), Ok(Value::Float(-0.0025)));
        assert_eq!(plain("1.0"), Ok(Value::Float(1.0)));
    }

    #[test]
    fn integers_beyond_two_to_the_53_are_rejected() {
        for text in [
            "9007199254740992",
            "-9007199254740992",
            "99999999999999999999999",
        ] {
            assert!(
                matches!(plain(text), Err(LoadErrorKind::IntegerRange { .. })),
                "{text}"
            );
        }
    }

    #[test]
    fn infinite_numbers_are_rejected() {
        assert!(matches!(
            plain("1e400"),
            Err(LoadErrorKind::NotFinite { .. })
        ));
        assert!(matches!(
            plain("-1e400"),
            Err(LoadErrorKind::NotFinite { .. })
        ));
        // Underflow to zero is a finite number.
        assert_eq!(plain("1e-400"), Ok(Value::Float(0.0)));
    }

    #[test]
    fn yaml_booleans_other_than_true_and_false_are_ambiguous() {
        for text in ["yes", "No", "NO", "y", "N", "on", "OFF", "True", "FALSE"] {
            assert_eq!(ambiguity(text), Some(Ambiguity::Boolean), "{text}");
        }
    }

    #[test]
    fn yaml_numbers_that_are_not_json_numbers_are_ambiguous() {
        for text in [
            "012",
            "+1",
            ".5",
            "1.",
            "1_000",
            "0x1F",
            "0o17",
            "0b101",
            "-0x1",
            "1:30",
            "190:20:30",
            ".inf",
            "-.Inf",
            "+.INF",
            ".nan",
            ".NaN",
            "1.5e+",
            "00",
            "1e5_0",
            "+1e3",
        ] {
            let expected = if text == "1.5e+" || text == "1e5_0" {
                None
            } else {
                Some(Ambiguity::Number)
            };
            assert_eq!(ambiguity(text), expected, "{text}");
        }
    }

    #[test]
    fn yaml_dates_are_ambiguous() {
        for text in ["2026-10-26", "2026-1-2", "2001-12-14t21:59:43.10-05:00"] {
            assert_eq!(ambiguity(text), Some(Ambiguity::Timestamp), "{text}");
        }
        for text in ["2026-10", "202-10-26", "2026-100-1", "2026-10-"] {
            assert_eq!(ambiguity(text), None, "{text}");
        }
    }

    #[test]
    fn the_json_grammar_is_exact() {
        for good in [
            "0", "-0", "1", "10", "1.5", "0.0", "1e5", "1E+5", "1e-5", "-0.5e10",
        ] {
            assert!(is_json_number(good), "{good}");
        }
        for bad in [
            "", "-", "01", "1.", ".1", "+1", "1e", "1e+", "--1", "1.2.3", "0x1", "1 ",
        ] {
            assert!(!is_json_number(bad), "{bad}");
        }
    }
}
