//! `Serialize` for every quantity, behind the `serde` feature.
//!
//! A quantity serializes as its [`Display`](std::fmt::Display) text, which
//! is canonical: `0.5h` and `30min` both become `"30min"`, so a model's
//! normalized form and its hash do not depend on how a quantity was
//! spelled (ADR-0002, ADR-0020). There is no `Deserialize`: the model
//! loader reads quantities with its own typed decoder, which keeps the
//! [`UnitError`](crate::UnitError) instead of turning it into text.

use serde::{Serialize, Serializer};

use crate::{Count, Rate, Ratio, SimDuration, SimTime};

macro_rules! serialize_as_display {
    ($($ty:ty),*) => {$(
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
    )*};
}

serialize_as_display!(SimDuration, SimTime, Count, Rate, Ratio);

#[cfg(test)]
mod tests {
    use crate::{Count, Rate, Ratio, SimDuration, SimTime};

    fn json<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    #[test]
    fn quantities_serialize_as_their_canonical_text() {
        let half_hour: SimDuration = "0.5h".parse().unwrap();
        assert_eq!(json(&half_hour), r#""30min""#);
        assert_eq!(json(&SimTime::from_nanos(1_500_000_000)), r#""1.5s""#);
        assert_eq!(json(&"20 part".parse::<Count>().unwrap()), r#""20 parts""#);
        assert_eq!(json(&"2%".parse::<Ratio>().unwrap()), r#""2%""#);
        assert_eq!(
            json(&"3 parts/s".parse::<Rate>().unwrap()),
            r#""3 parts/s""#
        );
    }
}
