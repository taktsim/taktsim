//! RFC 8785 numbers against ECMAScript itself: 3,500 doubles and the text
//! Node.js's `Number.prototype.toString` writes for them
//! (`data/es-numbers.mjs` generates `data/es-numbers.txt`), including 1,000
//! exact ties between two 17-digit candidates and values around the
//! plain/exponent boundaries.

use serde_json::{Number, Value};
use taktsim_model::jcs::{canonicalize, number_to_string};

const VECTORS: &str = include_str!("data/es-numbers.txt");

#[test]
fn jcs_numbers_match_ecmascript() {
    let mut checked = 0;
    for line in VECTORS.lines() {
        let (hex, expected) = line.split_once(' ').unwrap();
        let x = f64::from_bits(u64::from_str_radix(hex, 16).unwrap());
        assert_eq!(number_to_string(x), expected, "{hex}");
        let value = Value::Number(Number::from_f64(x).unwrap());
        assert_eq!(canonicalize(&value).unwrap(), expected, "{hex}");
        // The text reads back as the same double.
        assert_eq!(
            expected.parse::<f64>().unwrap().to_bits(),
            x.to_bits() & !(u64::from(x == 0.0) << 63),
            "{hex}"
        );
        checked += 1;
    }
    assert_eq!(checked, 3500);
}
