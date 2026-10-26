//! `JsonSchema` for every quantity, behind the `schemars` feature.
//!
//! A quantity is a string: a non-negative decimal number, at most one
//! space, and a unit. The pattern is the grammar the parser accepts; rules
//! the pattern cannot state (a duration finer than 1 ns, a fractional
//! count, a value too large) are left to the parser, as for every other
//! value (ADR-0020).

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};

use crate::{Count, Rate, Ratio, SimDuration, SimTime};

/// The pattern of a duration, also of a time since the start.
const DURATION_PATTERN: &str = r"^[0-9]+(\.[0-9]+)? ?(d|h|min|s|ms)$";
/// The pattern of a count: whole parts.
const COUNT_PATTERN: &str = r"^[0-9]+(\.0+)? ?parts?$";
/// The pattern of a rate: parts per second, minute or hour.
const RATE_PATTERN: &str = r"^[0-9]+(\.[0-9]+)? ?parts?/(s|min|h)$";
/// The pattern of a ratio: a percentage.
const RATIO_PATTERN: &str = r"^[0-9]+(\.[0-9]+)? ?%$";

macro_rules! quantity_schema {
    ($ty:ty, $name:literal, $pattern:expr, $description:literal, [$($example:literal),*]) => {
        impl JsonSchema for $ty {
            fn schema_name() -> Cow<'static, str> {
                $name.into()
            }

            fn schema_id() -> Cow<'static, str> {
                concat!("taktsim_units::", $name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "pattern": $pattern,
                    "description": $description,
                    "examples": [$($example),*],
                })
            }
        }
    };
}

quantity_schema!(
    SimDuration,
    "Duration",
    DURATION_PATTERN,
    "A duration: a number and one of the units d, h, min, s, ms, with at most one space; \
     exact to the nanosecond.",
    ["42s", "12min", "0.5h", "8h"]
);
quantity_schema!(
    SimTime,
    "Time",
    DURATION_PATTERN,
    "A point in simulated time, written as the duration since the start.",
    ["8h"]
);
quantity_schema!(
    Count,
    "Count",
    COUNT_PATTERN,
    "A whole number of parts.",
    ["20 parts", "1 part"]
);
quantity_schema!(
    Rate,
    "Rate",
    RATE_PATTERN,
    "Parts per second, minute or hour.",
    ["120 parts/h", "2 parts/min"]
);
quantity_schema!(
    Ratio,
    "Ratio",
    RATIO_PATTERN,
    "A percentage.",
    ["2%", "0.5%"]
);

#[cfg(test)]
mod tests {
    use super::*;

    fn schema<T: JsonSchema>() -> serde_json::Value {
        T::json_schema(&mut SchemaGenerator::default()).to_value()
    }

    #[test]
    fn quantities_are_strings_with_their_unit_pattern_and_examples_that_parse() {
        let duration = schema::<SimDuration>();
        assert_eq!(duration["type"], "string");
        assert_eq!(duration["pattern"], DURATION_PATTERN);
        for example in duration["examples"].as_array().unwrap() {
            assert!(example.as_str().unwrap().parse::<SimDuration>().is_ok());
        }
        for example in schema::<SimTime>()["examples"].as_array().unwrap() {
            assert!(example.as_str().unwrap().parse::<SimTime>().is_ok());
        }
        for example in schema::<Count>()["examples"].as_array().unwrap() {
            assert!(example.as_str().unwrap().parse::<Count>().is_ok());
        }
        for example in schema::<Rate>()["examples"].as_array().unwrap() {
            assert!(example.as_str().unwrap().parse::<Rate>().is_ok());
        }
        for example in schema::<Ratio>()["examples"].as_array().unwrap() {
            assert!(example.as_str().unwrap().parse::<Ratio>().is_ok());
        }
        assert_eq!(SimDuration::schema_name(), "Duration");
        assert_eq!(Count::schema_id(), "taktsim_units::Count");
    }

    /// Whether the parser's error is about the text's form, which the
    /// pattern states (a count's pattern also states that it is whole),
    /// rather than its value (precision, size), which it leaves to the
    /// parser.
    fn syntax_error(e: &crate::UnitError) -> bool {
        matches!(
            e,
            crate::UnitError::InvalidNumber { .. }
                | crate::UnitError::UnknownUnit { .. }
                | crate::UnitError::Negative { .. }
                | crate::UnitError::NotWhole { .. }
        )
    }

    fn agrees<T: std::str::FromStr<Err = crate::UnitError>>(pattern: &str, text: &str) -> bool {
        let matches = regex::Regex::new(pattern).unwrap().is_match(text);
        let parses = match text.parse::<T>() {
            Ok(_) => true,
            Err(e) => !syntax_error(&e),
        };
        matches == parses
    }

    proptest::proptest! {
        /// Each pattern accepts exactly the text whose form the parser
        /// accepts.
        #[test]
        fn patterns_accept_what_the_parser_reads(
            text in "[0-9.]{0,6}[ ]{0,2}(d|h|min|s|ms|sec|part|parts|parts/s|part/min|parts/h|parts/d|%|x|)",
        ) {
            proptest::prop_assert!(agrees::<SimDuration>(DURATION_PATTERN, &text), "duration {text:?}");
            proptest::prop_assert!(agrees::<Count>(COUNT_PATTERN, &text), "count {text:?}");
            proptest::prop_assert!(agrees::<Rate>(RATE_PATTERN, &text), "rate {text:?}");
            proptest::prop_assert!(agrees::<Ratio>(RATIO_PATTERN, &text), "ratio {text:?}");
        }
    }
}
