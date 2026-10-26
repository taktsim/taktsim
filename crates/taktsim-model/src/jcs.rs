//! JSON Canonicalization Scheme (RFC 8785), the canonical form a model is
//! hashed in (ADR-0002, ADR-0020).
//!
//! - Objects: keys sorted by their UTF-16 code units, no whitespace.
//! - Strings: as ECMAScript's `JSON.stringify` writes them: `"` and `\`
//!   escaped, U+0000 to U+001F as `\b \t \n \f \r` or `\u00xx` (lowercase
//!   hex), everything else as UTF-8.
//! - Numbers: IEEE 754 doubles, written as ECMAScript's
//!   `Number.prototype.toString` writes them: the shortest digits that
//!   read back to the same double, in plain notation from 1e-6 up to
//!   below 1e21 and in exponent notation (`1e+21`, `1e-7`) outside it.
//!
//! The shortest digits come from Rust's float formatting (exact, and the
//! same on every platform); only their layout follows ECMAScript.

use std::fmt::Write;

use serde_json::{Number, Value};

/// Why a value has no canonical form.
///
/// ```
/// use serde_json::json;
/// use taktsim_model::jcs::{CanonicalError, canonicalize};
///
/// let err = canonicalize(&json!(9007199254740993_u64)).unwrap_err();
/// assert_eq!(err, CanonicalError::InexactInteger("9007199254740993".into()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// An integer that a double cannot hold exactly (beyond ±2^53), so
    /// readers of the canonical form would see another number.
    #[error("the integer {0} cannot be represented exactly as a double")]
    InexactInteger(String),
}

/// The canonical JSON text of `value` (RFC 8785).
///
/// # Errors
///
/// [`CanonicalError::InexactInteger`] for an integer beyond ±2^53, which
/// RFC 8785 cannot represent exactly. A loaded model never has one: its
/// integers are within ±(2^53 − 1).
///
/// ```
/// use serde_json::json;
/// use taktsim_model::jcs::canonicalize;
///
/// let value = json!({"b": [1.0, 1e21, 1e-7, "€"], "a": null});
/// assert_eq!(canonicalize(&value)?, r#"{"a":null,"b":[1,1e+21,1e-7,"€"]}"#);
/// # Ok::<(), taktsim_model::jcs::CanonicalError>(())
/// ```
pub fn canonicalize(value: &Value) -> Result<String, CanonicalError> {
    let mut out = String::new();
    write_value(&mut out, value)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(out, n)?,
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, item)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// The largest integer every double holds exactly, with its neighbours.
const EXACT: u64 = 1 << 53;

fn write_number(out: &mut String, n: &Number) -> Result<(), CanonicalError> {
    if let Some(u) = n.as_u64() {
        if u > EXACT {
            return Err(CanonicalError::InexactInteger(u.to_string()));
        }
        // Below 1e21, so ECMAScript writes the integer's digits.
        out.push_str(&u.to_string());
    } else if let Some(i) = n.as_i64() {
        if i.unsigned_abs() > EXACT {
            return Err(CanonicalError::InexactInteger(i.to_string()));
        }
        out.push_str(&i.to_string());
    } else if let Some(x) = n.as_f64() {
        out.push_str(&number_to_string(x));
    }
    Ok(())
}

/// ECMAScript's `Number.prototype.toString(x)` for a finite `x`
/// (ECMA-262, `Number::toString`). `serde_json` cannot hold NaN or
/// infinities, which have no JSON form.
///
/// ```
/// use taktsim_model::jcs::number_to_string;
///
/// assert_eq!(number_to_string(1e21), "1e+21");
/// assert_eq!(number_to_string(0.000001), "0.000001");
/// // An exact tie between two shortest candidates goes to the even digit.
/// assert_eq!(number_to_string(1424953923781206.25), "1424953923781206.2");
/// ```
#[must_use]
pub fn number_to_string(x: f64) -> String {
    if x == 0.0 {
        // Both zeros are written "0".
        return "0".to_owned();
    }
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    let (digits, exponent) = shortest_digits(x.abs());
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    // ECMAScript's n: the value is 0.d1d2…dk × 10^n.
    let n = exponent + 1;
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n(
            '0',
            usize::try_from(n - k).unwrap_or(0),
        ));
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(0);
        out.push_str(&digits[..split]);
        out.push('.');
        out.push_str(&digits[split..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', usize::try_from(-n).unwrap_or(0)));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = n - 1;
        let _ = write!(
            out,
            "e{}{}",
            if e < 0 { '-' } else { '+' },
            e.unsigned_abs()
        );
    }
    out
}

/// The digits and decimal exponent of `text`, Rust's `{:e}` form
/// `d.ddde±x`, without trailing zeros: `("15", -7)` for `1.5e-7`.
fn scientific(text: &str) -> (String, i64) {
    let (mantissa, exponent) = text.split_once('e').unwrap_or((text, "0"));
    let mut digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    while digits.len() > 1 && digits.ends_with('0') {
        digits.pop();
    }
    (digits, exponent.parse().unwrap_or(0))
}

/// The digits `d1…dk` and exponent `e` of `x = d1.d2…dk × 10^e` (`x > 0`)
/// that ECMAScript chooses: the fewest digits that read back as `x`; among
/// those, the closest to `x`; on a tie, the even one (ECMA-262,
/// `Number::toString`, step 5 and its note).
///
/// Rust's `{:e}` gives the fewest digits, closest to `x`, but breaks an
/// exact tie upwards: for `1424953923781206.25` it writes
/// `1.4249539237812063e15` where ECMAScript writes `…206.2`
/// (RFC 8785, Appendix B). So when its last digit is odd, check whether `x`
/// lies exactly halfway to a neighbour that also reads back as `x`, using
/// the exact decimal expansion of `x` (at most 767 significant digits).
fn shortest_digits(x: f64) -> (String, i64) {
    let (digits, exponent) = scientific(&format!("{x:e}"));
    let Some(&last) = digits.as_bytes().last() else {
        return (digits, exponent);
    };
    if last % 2 == 0 {
        return (digits, exponent);
    }
    let exact = scientific(&format!("{x:.800e}"));
    let head = &digits[..digits.len() - 1];
    for neighbour in [last - 1, last + 1] {
        if neighbour > b'9' {
            continue;
        }
        // These strings mean d1.d2…dk × 10^exponent, as in `scientific`.
        let low = char::from(last.min(neighbour));
        if scientific(&format!("{head}{low}5e{exponent}")) != exact {
            continue;
        }
        let candidate = format!("{head}{}", char::from(neighbour));
        let text = format!("{}.{}e{exponent}", &candidate[..1], &candidate[1..]);
        if text.parse::<f64>() == Ok(x) {
            return scientific(&format!("{candidate}e{exponent}"));
        }
    }
    (digits, exponent)
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// RFC 8785, Appendix B: IEEE 754 bit patterns and their canonical
    /// text.
    #[test]
    fn rfc_8785_appendix_b_numbers() {
        let cases: [(u64, &str); 24] = [
            (0x0000_0000_0000_0000, "0"),
            (0x8000_0000_0000_0000, "0"),
            (0x0000_0000_0000_0001, "5e-324"),
            (0x8000_0000_0000_0001, "-5e-324"),
            (0x7fef_ffff_ffff_ffff, "1.7976931348623157e+308"),
            (0xffef_ffff_ffff_ffff, "-1.7976931348623157e+308"),
            (0x4340_0000_0000_0000, "9007199254740992"),
            (0xc340_0000_0000_0000, "-9007199254740992"),
            (0x4430_0000_0000_0000, "295147905179352830000"),
            (0x44b5_2d02_c7e1_4af5, "9.999999999999997e+22"),
            (0x44b5_2d02_c7e1_4af6, "1e+23"),
            (0x44b5_2d02_c7e1_4af7, "1.0000000000000001e+23"),
            (0x444b_1ae4_d6e2_ef4e, "999999999999999700000"),
            (0x444b_1ae4_d6e2_ef4f, "999999999999999900000"),
            (0x444b_1ae4_d6e2_ef50, "1e+21"),
            (0x3eb0_c6f7_a0b5_ed8c, "9.999999999999997e-7"),
            (0x3eb0_c6f7_a0b5_ed8d, "0.000001"),
            (0x41b3_de43_5555_5553, "333333333.3333332"),
            (0x41b3_de43_5555_5554, "333333333.33333325"),
            (0x41b3_de43_5555_5555, "333333333.3333333"),
            (0x41b3_de43_5555_5556, "333333333.3333334"),
            (0x41b3_de43_5555_5557, "333333333.33333343"),
            (0xbecb_f647_612f_3696, "-0.0000033333333333333333"),
            (0x4314_3ff3_c1cb_0959, "1424953923781206.2"),
        ];
        for (bits, expected) in cases {
            let x = f64::from_bits(bits);
            assert_eq!(number_to_string(x), expected, "{bits:016x}");
            let value = Value::Number(Number::from_f64(x).unwrap());
            assert_eq!(canonicalize(&value).unwrap(), expected, "{bits:016x}");
        }
    }

    /// RFC 8785 §3.2.2: the example's values, canonicalized. (The input
    /// text's number literals are parsed here by Rust, which rounds
    /// correctly.)
    #[test]
    fn rfc_8785_section_3_2_2_example() {
        let value = json!({
            "numbers": [
                "333333333.33333329".parse::<f64>().unwrap(),
                1e30,
                4.50,
                2e-3,
                "0.000000000000000000000000001".parse::<f64>().unwrap(),
            ],
            "string": "\u{20ac}$\u{000F}\u{000a}A'\u{0042}\u{0022}\u{005c}\\\"/",
            "literals": [null, true, false],
        });
        assert_eq!(
            canonicalize(&value).unwrap(),
            "{\"literals\":[null,true,false],\"numbers\":[333333333.3333333,1e+30,4.5,0.002,1e-27],\
             \"string\":\"€$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
        );
    }

    /// RFC 8785 §3.2.3: keys sorted by UTF-16 code units, so U+1F600
    /// (surrogates D83D DE00) sorts before U+FB33.
    #[test]
    fn rfc_8785_section_3_2_3_sorting() {
        let value = json!({
            "\u{20ac}": "Euro Sign",
            "\r": "Carriage Return",
            "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
            "1": "One",
            "\u{1f600}": "Emoji: Grinning Face",
            "\u{0080}": "Control",
            "\u{00f6}": "Latin Small Letter O With Diaeresis",
        });
        assert_eq!(
            canonicalize(&value).unwrap(),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{0080}\":\"Control\",\
             \"ö\":\"Latin Small Letter O With Diaeresis\",\"€\":\"Euro Sign\",\
             \"😀\":\"Emoji: Grinning Face\",\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
    }

    #[test]
    fn strings_escape_only_what_ecmascript_escapes() {
        let s = "\u{0}\u{1}\u{8}\t\n\u{b}\u{c}\r\u{1f} \u{7f}\u{2028}/<>";
        assert_eq!(
            canonicalize(&json!(s)).unwrap(),
            "\"\\u0000\\u0001\\b\\t\\n\\u000b\\f\\r\\u001f \u{7f}\u{2028}/<>\""
        );
    }

    #[test]
    fn integers_are_exact_or_rejected() {
        assert_eq!(
            canonicalize(&json!(9_007_199_254_740_992_u64)).unwrap(),
            "9007199254740992"
        );
        assert_eq!(
            canonicalize(&json!(-9_007_199_254_740_992_i64)).unwrap(),
            "-9007199254740992"
        );
        assert_eq!(canonicalize(&json!(-42)).unwrap(), "-42");
        assert!(canonicalize(&json!(-9_007_199_254_740_993_i64)).is_err());
        assert!(canonicalize(&json!([u64::MAX])).is_err());
        assert!(canonicalize(&json!({"a": u64::MAX})).is_err());
    }

    #[test]
    fn plain_and_exponent_notation_switch_where_ecmascript_switches() {
        let cases = [
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123e18, "123000000000000000000"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (0.1, "0.1"),
            (-0.5, "-0.5"),
            (2.0, "2"),
            (1.25e300, "1.25e+300"),
            (12.5, "12.5"),
        ];
        for (x, expected) in cases {
            assert_eq!(number_to_string(x), expected, "{x}");
        }
        assert_eq!(number_to_string(-0.0), "0");
    }
}
