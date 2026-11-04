//! A JSON value: the normalized form of a model, its JSON Schema, and the
//! document as written (ADR-0020). The model crate has no JSON library
//! dependency; it reads JSON with its own parser and writes it with
//! [`jcs`](crate::jcs) (canonical) or [`Json::to_pretty`] (for people).

use std::collections::BTreeMap;
use std::fmt::Write;

/// A JSON value. Objects keep their keys sorted (by bytes), so iterating
/// and writing them is deterministic.
///
/// ```
/// use taktsim_model::Json;
///
/// let value = Json::object([("b", Json::from(1)), ("a", Json::from("x"))]);
/// assert_eq!(value["a"], "x");
/// assert_eq!(value.to_pretty(), "{\n  \"a\": \"x\",\n  \"b\": 1\n}");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer. Canonical JSON represents it exactly only within
    /// ±2^53 ([`jcs::canonicalize`](crate::jcs::canonicalize) rejects others).
    Integer(i128),
    /// A floating-point number; canonical JSON needs it finite.
    Float(f64),
    /// Text.
    String(String),
    /// A list.
    Array(Vec<Json>),
    /// An object.
    Object(BTreeMap<String, Json>),
}

/// The value `Index` returns for a missing key or index.
static NULL: Json = Json::Null;

impl Json {
    /// An object of `entries`; a later entry replaces an earlier one with
    /// the same key.
    ///
    /// ```
    /// use taktsim_model::Json;
    ///
    /// let value = Json::object([("type", Json::from("string"))]);
    /// assert_eq!(value["type"], "string");
    /// ```
    pub fn object<'k>(entries: impl IntoIterator<Item = (&'k str, Self)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    /// This object with `key` set to `value`'s normalized form. A value
    /// that is not an object is returned unchanged.
    ///
    /// ```
    /// use taktsim_model::Json;
    /// use taktsim_units::SimDuration;
    ///
    /// let cycle: SimDuration = "0.5h".parse()?;
    /// let value = Json::object([]).with("cycle_time", &cycle);
    /// assert_eq!(value["cycle_time"], "30min");
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn with(mut self, key: &str, value: &impl Encode) -> Self {
        if let Self::Object(map) = &mut self {
            map.insert(key.to_owned(), value.encode());
        }
        self
    }

    /// This object with `key` set to `value`'s normalized form if there is
    /// a value; an absent optional value is left out, so adding an
    /// optional parameter does not change existing hashes.
    ///
    /// ```
    /// use taktsim_model::Json;
    ///
    /// let value = Json::object([]).with_optional("seed", None::<&u64>);
    /// assert_eq!(value, Json::object([]));
    /// ```
    #[must_use]
    pub fn with_optional<T: Encode>(self, key: &str, value: Option<&T>) -> Self {
        match value {
            Some(value) => self.with(key, value),
            None => self,
        }
    }

    /// The text, if this is a string.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Json::from("a").as_str(), Some("a"));
    /// ```
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            Self::Null
            | Self::Bool(_)
            | Self::Integer(_)
            | Self::Float(_)
            | Self::Array(_)
            | Self::Object(_) => None,
        }
    }

    /// The items, if this is a list.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Json::Array(vec![]).as_array().map(<[_]>::len), Some(0));
    /// ```
    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            Self::Null
            | Self::Bool(_)
            | Self::Integer(_)
            | Self::Float(_)
            | Self::String(_)
            | Self::Object(_) => None,
        }
    }

    /// The entries, if this is an object.
    ///
    /// ```
    /// assert!(taktsim_model::Json::object([]).as_object().is_some());
    /// ```
    #[must_use]
    pub fn as_object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(map) => Some(map),
            Self::Null
            | Self::Bool(_)
            | Self::Integer(_)
            | Self::Float(_)
            | Self::String(_)
            | Self::Array(_) => None,
        }
    }

    /// The value as a double, if this is a number.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Json::from(2).as_f64(), Some(2.0));
    /// ```
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // A number's value as a double, as JSON readers see it.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(n) => Some(*n as f64),
            Self::Float(x) => Some(*x),
            Self::Null | Self::Bool(_) | Self::String(_) | Self::Array(_) | Self::Object(_) => None,
        }
    }

    /// The text, indented by two spaces per level, one entry per line,
    /// keys in sorted order: the layout of the committed schemas.
    ///
    /// ```
    /// use taktsim_model::Json;
    ///
    /// let value = Json::object([("a", Json::Array(vec![Json::from(1), Json::Null]))]);
    /// assert_eq!(value.to_pretty(), "{\n  \"a\": [\n    1,\n    null\n  ]\n}");
    /// ```
    #[must_use]
    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(0));
        out
    }

    /// Writes the text, indented at `depth` levels, or on one line without
    /// spaces if `depth` is `None`.
    fn write(&self, out: &mut String, depth: Option<usize>) {
        let newline = |out: &mut String, depth: Option<usize>| {
            if let Some(depth) = depth {
                out.push('\n');
                out.extend(std::iter::repeat_n("  ", depth));
            }
        };
        let inner = depth.map(|d| d + 1);
        match self {
            Self::Array(items) if !items.is_empty() => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, inner);
                    item.write(out, inner);
                }
                newline(out, depth);
                out.push(']');
            }
            Self::Object(map) if !map.is_empty() => {
                out.push('{');
                for (i, (key, value)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    newline(out, inner);
                    crate::jcs::write_string(out, key);
                    out.push_str(if depth.is_some() { ": " } else { ":" });
                    value.write(out, inner);
                }
                newline(out, depth);
                out.push('}');
            }
            Self::Array(_) => out.push_str("[]"),
            Self::Object(_) => out.push_str("{}"),
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Integer(n) => {
                let _ = write!(out, "{n}");
            }
            Self::Float(x) => out.push_str(&crate::jcs::number_to_string(*x)),
            Self::String(s) => crate::jcs::write_string(out, s),
        }
    }
}

impl std::fmt::Display for Json {
    /// The text on one line, keys in sorted order.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        self.write(&mut out, None);
        f.write_str(&out)
    }
}

impl std::ops::Index<&str> for Json {
    type Output = Self;

    /// The value of `key`; `null` if this is not an object or has no such
    /// key.
    fn index(&self, key: &str) -> &Self {
        self.as_object()
            .and_then(|map| map.get(key))
            .unwrap_or(&NULL)
    }
}

impl std::ops::Index<usize> for Json {
    type Output = Self;

    /// The item at `index`; `null` if this is not a list or too short.
    fn index(&self, index: usize) -> &Self {
        self.as_array()
            .and_then(|items| items.get(index))
            .unwrap_or(&NULL)
    }
}

impl PartialEq<&str> for Json {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<bool> for Json {
    fn eq(&self, other: &bool) -> bool {
        *self == Self::Bool(*other)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<i64> for Json {
    fn from(n: i64) -> Self {
        Self::Integer(n.into())
    }
}

impl From<i32> for Json {
    fn from(n: i32) -> Self {
        Self::Integer(n.into())
    }
}

impl From<u32> for Json {
    fn from(n: u32) -> Self {
        Self::Integer(n.into())
    }
}

impl From<u64> for Json {
    fn from(n: u64) -> Self {
        Self::Integer(n.into())
    }
}

impl From<f64> for Json {
    fn from(x: f64) -> Self {
        Self::Float(x)
    }
}

impl From<Vec<Self>> for Json {
    fn from(items: Vec<Self>) -> Self {
        Self::Array(items)
    }
}

/// A value's normalized form (ADR-0002, ADR-0020): what the model hash is
/// computed over. Quantities are written in their canonical text (`0.5h`
/// as `"30min"`), distributions as a bare duration or a `dist` object.
///
/// A component type's parameters implement it by listing their fields,
/// leaving out absent optional ones:
///
/// ```
/// use taktsim_model::{Distribution, Encode, Json};
///
/// struct Machine {
///     cycle_time: Distribution,
///     mtbf: Option<Distribution>,
/// }
///
/// impl Encode for Machine {
///     fn encode(&self) -> Json {
///         Json::object([])
///             .with("cycle_time", &self.cycle_time)
///             .with_optional("mtbf", self.mtbf.as_ref())
///     }
/// }
///
/// let m = Machine { cycle_time: Distribution::Constant("45s".parse()?), mtbf: None };
/// assert_eq!(m.encode(), Json::object([("cycle_time", Json::from("45s"))]));
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
pub trait Encode {
    /// The normalized form.
    fn encode(&self) -> Json;
}

impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self) -> Json {
        (**self).encode()
    }
}

impl Encode for Json {
    fn encode(&self) -> Json {
        self.clone()
    }
}

impl Encode for str {
    fn encode(&self) -> Json {
        self.into()
    }
}

impl Encode for String {
    fn encode(&self) -> Json {
        self.as_str().into()
    }
}

impl Encode for bool {
    fn encode(&self) -> Json {
        (*self).into()
    }
}

impl Encode for u32 {
    fn encode(&self) -> Json {
        (*self).into()
    }
}

impl Encode for u64 {
    fn encode(&self) -> Json {
        (*self).into()
    }
}

impl Encode for f64 {
    fn encode(&self) -> Json {
        (*self).into()
    }
}

impl<T: Encode> Encode for [T] {
    fn encode(&self) -> Json {
        Json::Array(self.iter().map(Encode::encode).collect())
    }
}

impl<T: Encode> Encode for Vec<T> {
    fn encode(&self) -> Json {
        self.as_slice().encode()
    }
}

macro_rules! encode_as_text {
    ($($ty:ty),*) => {$(
        impl Encode for $ty {
            /// The canonical text (`Display`).
            fn encode(&self) -> Json {
                Json::String(self.to_string())
            }
        }
    )*};
}

encode_as_text!(
    taktsim_units::SimDuration,
    taktsim_units::Count,
    taktsim_units::Rate,
    taktsim_units::Ratio
);

#[cfg(test)]
mod tests {
    use super::*;
    use taktsim_units::{Count, Rate, Ratio, SimDuration};

    #[test]
    fn quantities_encode_as_their_canonical_text() {
        let half_hour: SimDuration = "0.5h".parse().unwrap();
        assert_eq!(half_hour.encode(), "30min");
        assert_eq!("20 part".parse::<Count>().unwrap().encode(), "20 parts");
        assert_eq!("2%".parse::<Ratio>().unwrap().encode(), "2%");
        assert_eq!("3 parts/s".parse::<Rate>().unwrap().encode(), "3 parts/s");
    }

    #[test]
    fn primitives_and_lists() {
        assert_eq!(true.encode(), true);
        assert_eq!(3_u32.encode(), Json::Integer(3));
        assert_eq!(42_u64.encode(), Json::Integer(42));
        assert_eq!(0.5_f64.encode(), Json::Float(0.5));
        assert_eq!("a".encode(), "a");
        assert_eq!(String::from("b").encode(), "b");
        assert_eq!(
            vec![1_u32, 2].encode(),
            Json::Array(vec![Json::from(1), Json::from(2)])
        );
        assert_eq!(Json::Null.encode(), Json::Null);
        assert_eq!(Json::from(-1_i64), Json::Integer(-1));
        // `with` on a value that is not an object leaves it unchanged.
        assert_eq!(Json::Null.with("a", &1_u32), Json::Null);
    }

    #[test]
    fn indexing_missing_keys_gives_null() {
        let value = Json::object([("a", Json::Array(vec![Json::from(1)]))]);
        assert_eq!(value["a"][0], Json::Integer(1));
        assert_eq!(value["a"][1], Json::Null);
        assert_eq!(value["b"], Json::Null);
        assert_eq!(value["a"]["x"], Json::Null);
        assert_eq!(Json::Null.as_f64(), None);
        assert_eq!(Json::Float(1.5).as_f64(), Some(1.5));
        assert_eq!(value.as_str(), None);
        assert!(Json::Null.as_object().is_none() && Json::Null.as_array().is_none());
    }

    #[test]
    fn pretty_text_matches_the_schema_layout() {
        let value = Json::object([
            ("e", Json::Array(vec![])),
            ("o", Json::object([])),
            ("s", Json::from("a\"\\\u{1}§")),
            ("f", Json::Float(0.5)),
            ("b", Json::from(false)),
            ("n", Json::Null),
            ("i", Json::from(-3)),
            ("x", Json::object([("y", Json::from(true))])),
        ]);
        assert_eq!(
            value.to_pretty(),
            "{\n  \"b\": false,\n  \"e\": [],\n  \"f\": 0.5,\n  \"i\": -3,\n  \"n\": null,\n  \"o\": {},\n  \
             \"s\": \"a\\\"\\\\\\u0001§\",\n  \"x\": {\n    \"y\": true\n  }\n}"
        );
        assert_eq!(
            value.to_string(),
            "{\"b\":false,\"e\":[],\"f\":0.5,\"i\":-3,\"n\":null,\"o\":{},\
             \"s\":\"a\\\"\\\\\\u0001§\",\"x\":{\"y\":true}}"
        );
    }
}
