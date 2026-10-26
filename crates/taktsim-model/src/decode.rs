//! Typed decoding of a [`Node`] tree (ADR-0002, step 4 of the pipeline;
//! ADR-0020).
//!
//! Decoding is strict: unknown keys are errors, every value has the type
//! its field declares, and every error has a field path and a location. It
//! does not stop at the first error: a [`Decoder`] collects them all, so
//! `validate` can report every problem in one pass (spec §5).
//!
//! Component types (M5) read their parameters with the same API: a
//! [`Fields`] for the entry's keys and [`Decode`] for each value.

use taktsim_units::{Count, Rate, Ratio, SimDuration, UnitError};

use crate::error::{FieldPath, LoadError, LoadErrorKind, Location, Segment, suggest};
use crate::syntax::{Entry, Node, Value};

/// A value that can be read from a model file.
///
/// `EXPECTED` and `EXAMPLE` describe the value in error messages: "expected
/// `EXPECTED`, found …" and "the required key `x` is missing; it takes
/// `EXPECTED`".
///
/// ```
/// use taktsim_model::{Decode, Decoder, Node, Value};
///
/// /// A batch size: a whole number from 1 to 1000.
/// struct BatchSize(u32);
///
/// impl Decode for BatchSize {
///     const EXPECTED: &'static str = "a whole number from 1 to 1000";
///     const EXAMPLE: &'static str = "10";
///
///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
///         let n = u32::decode(node, cx)?;
///         if !(1..=1000).contains(&n) {
///             cx.not_allowed(node, n.to_string(), Self::EXPECTED, Self::EXAMPLE);
///             return None;
///         }
///         Some(Self(n))
///     }
/// }
/// ```
pub trait Decode: Sized {
    /// What the value is, for error messages, e.g. "a duration such as
    /// `42s`".
    const EXPECTED: &'static str;
    /// One valid value as written in YAML, e.g. `42s`.
    const EXAMPLE: &'static str;

    /// Reads the value from `node`, or records why it cannot in `cx` and
    /// returns `None`.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self>;
}

/// Collects the errors of one file while its values are decoded, with the
/// field path of the value being read.
///
/// ```
/// use taktsim_model::{Decode, Decoder, Node};
///
/// fn read_positive(node: &Node, cx: &mut Decoder) -> Option<f64> {
///     let x = f64::decode(node, cx)?;
///     if x <= 0.0 {
///         cx.not_allowed(node, x.to_string(), "a number > 0", "0.5");
///         return None;
///     }
///     Some(x)
/// }
/// ```
#[derive(Debug)]
pub struct Decoder {
    file: Option<String>,
    path: FieldPath,
    errors: Vec<LoadError>,
}

impl Decoder {
    pub(crate) fn new(file: Option<String>) -> Self {
        Self {
            file,
            path: FieldPath::new(),
            errors: Vec::new(),
        }
    }

    pub(crate) fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub(crate) fn into_errors(self) -> Vec<LoadError> {
        self.errors
    }

    /// Whether any error has been recorded.
    #[cfg(test)]
    pub(crate) fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// The number of errors recorded so far.
    pub(crate) fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// The path of the value being read.
    ///
    /// ```
    /// use taktsim_model::{Decoder, Node};
    ///
    /// fn where_am_i(cx: &Decoder) -> String {
    ///     cx.path().to_string()
    /// }
    /// ```
    #[must_use]
    pub const fn path(&self) -> &FieldPath {
        &self.path
    }

    /// Records `kind` at `at`, for the value being read.
    ///
    /// ```
    /// use taktsim_model::{Decoder, LoadErrorKind, Node};
    ///
    /// fn reject(node: &Node, cx: &mut Decoder) {
    ///     cx.error(node.start(), LoadErrorKind::SteadyStateUnsupported);
    /// }
    /// ```
    pub fn error(&mut self, at: Location, kind: LoadErrorKind) {
        let error = LoadError::new(self.file.clone(), Some(at), self.path.clone(), kind);
        self.errors.push(error);
    }

    /// Records that `node` has the wrong kind of value for `T`.
    ///
    /// ```
    /// use taktsim_model::{Decoder, Node};
    /// use taktsim_units::SimDuration;
    ///
    /// fn reject(node: &Node, cx: &mut Decoder) {
    ///     cx.wrong_type::<SimDuration>(node);
    /// }
    /// ```
    pub fn wrong_type<T: Decode>(&mut self, node: &Node) {
        let kind = LoadErrorKind::WrongType {
            expected: T::EXPECTED,
            example: T::EXAMPLE,
            found: node.found(),
        };
        self.error(node.start(), kind);
    }

    /// Records that the value of `node`, `found`, breaks a rule of its
    /// field: `allowed` says what the field takes, `example` is one valid
    /// value.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Node};
    /// use taktsim_units::Count;
    ///
    /// fn capacity(node: &Node, cx: &mut Decoder) -> Option<Count> {
    ///     let c = Count::decode(node, cx)?;
    ///     if c.get() == 0 {
    ///         cx.not_allowed(node, c.to_string(), "at least 1 parts", "20 parts");
    ///         return None;
    ///     }
    ///     Some(c)
    /// }
    /// ```
    pub fn not_allowed(
        &mut self,
        node: &Node,
        found: impl Into<String>,
        allowed: impl Into<String>,
        example: impl Into<String>,
    ) {
        let kind = LoadErrorKind::NotAllowed {
            found: found.into(),
            allowed: allowed.into(),
            example: example.into(),
        };
        self.error(node.start(), kind);
    }

    /// Reads the mapping `node` as fields with known keys; records an
    /// error and returns `None` if it is not a mapping. `T` names what the
    /// mapping is, for that error.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Node};
    ///
    /// struct Setup {
    ///     per_variant: taktsim_units::SimDuration,
    /// }
    ///
    /// impl Decode for Setup {
    ///     const EXPECTED: &'static str = "a setup such as `{per_variant: 4min}`";
    ///     const EXAMPLE: &'static str = "{per_variant: 4min}";
    ///
    ///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
    ///         let mut fields = cx.fields::<Self>(node)?;
    ///         let per_variant = fields.required("per_variant");
    ///         fields.finish();
    ///         Some(Self { per_variant: per_variant? })
    ///     }
    /// }
    /// ```
    pub fn fields<'n, T: Decode>(&mut self, node: &'n Node) -> Option<Fields<'n, '_>> {
        let Value::Map(entries) = node.value() else {
            self.wrong_type::<T>(node);
            return None;
        };
        Some(Fields {
            used: vec![false; entries.len()],
            entries,
            node,
            known: Vec::new(),
            cx: self,
        })
    }

    /// Runs `read` with `segment` added to the path.
    pub(crate) fn within<R>(&mut self, segment: Segment, read: impl FnOnce(&mut Self) -> R) -> R {
        self.path.push(segment);
        let result = read(self);
        self.path.pop();
        result
    }

    /// Decodes `node` as a list, each item with its index in the path.
    pub(crate) fn list<'n, R>(
        &mut self,
        node: &'n Node,
        mut read: impl FnMut(&'n Node, &mut Self) -> Option<R>,
    ) -> Option<Vec<R>> {
        let Value::List(items) = node.value() else {
            return None;
        };
        let mut out = Vec::with_capacity(items.len());
        let mut ok = true;
        for (i, item) in items.iter().enumerate() {
            match self.within(Segment::Index(i), |cx| read(item, cx)) {
                Some(value) => out.push(value),
                None => ok = false,
            }
        }
        ok.then_some(out)
    }
}

/// The keys of one mapping, read one at a time. Each key may be read once;
/// [`finish`](Self::finish) reports every key that was not read as
/// unknown, with the keys that were as the allowed ones.
///
/// ```
/// use taktsim_model::{Decode, Decoder, Node};
/// use taktsim_units::SimDuration;
///
/// struct Failure {
///     mtbf: SimDuration,
///     mttr: Option<SimDuration>,
/// }
///
/// impl Decode for Failure {
///     const EXPECTED: &'static str = "a failure model such as `{mtbf: 6h, mttr: 12min}`";
///     const EXAMPLE: &'static str = "{mtbf: 6h, mttr: 12min}";
///
///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
///         let mut fields = cx.fields::<Self>(node)?;
///         let mtbf = fields.required("mtbf");
///         let mttr = fields.optional("mttr");
///         fields.finish();
///         Some(Self { mtbf: mtbf?, mttr: mttr? })
///     }
/// }
/// ```
#[derive(Debug)]
pub struct Fields<'n, 'd> {
    cx: &'d mut Decoder,
    node: &'n Node,
    entries: &'n [Entry],
    used: Vec<bool>,
    known: Vec<&'static str>,
}

impl<'n> Fields<'n, '_> {
    /// The value of `key`, marking it read.
    pub(crate) fn take(&mut self, key: &'static str) -> Option<&'n Node> {
        if !self.known.contains(&key) {
            self.known.push(key);
        }
        let i = self.entries.iter().position(|e| e.key() == key)?;
        self.used[i] = true;
        Some(self.entries[i].value())
    }

    /// Where the value of `key` was read from, if it is present.
    pub(crate) fn entry(&self, key: &str) -> Option<&'n Entry> {
        self.entries.iter().find(|e| e.key() == key)
    }

    /// The mapping itself.
    pub(crate) const fn node(&self) -> &'n Node {
        self.node
    }

    /// The decoder, to read values with custom rules.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Fields};
    ///
    /// fn path_of(fields: &mut Fields<'_, '_>) -> String {
    ///     fields.decoder().path().to_string()
    /// }
    /// ```
    pub fn decoder(&mut self) -> &mut Decoder {
        self.cx
    }

    /// The value of the required key `key`, decoded as `T`; `None` if it
    /// is missing or invalid, with the error recorded.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Node};
    ///
    /// struct Named(String);
    ///
    /// impl Decode for Named {
    ///     const EXPECTED: &'static str = "a mapping with a name";
    ///     const EXAMPLE: &'static str = "{name: a}";
    ///
    ///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
    ///         let mut fields = cx.fields::<Self>(node)?;
    ///         let name = fields.required("name");
    ///         fields.finish();
    ///         Some(Self(name?))
    ///     }
    /// }
    /// ```
    pub fn required<T: Decode>(&mut self, key: &'static str) -> Option<T> {
        let Some(node) = self.take(key) else {
            let at = self.node.start();
            let kind = LoadErrorKind::MissingField {
                field: key,
                expected: T::EXPECTED,
                example: T::EXAMPLE,
            };
            self.cx
                .within(Segment::Key(key.to_owned()), |cx| cx.error(at, kind));
            return None;
        };
        self.cx
            .within(Segment::Key(key.to_owned()), |cx| T::decode(node, cx))
    }

    /// The value of the optional key `key`: `Some(None)` if it is absent,
    /// `Some(Some(value))` if it is valid, `None` if it is invalid (with
    /// the error recorded).
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Node};
    ///
    /// struct Seeded(Option<u64>);
    ///
    /// impl Decode for Seeded {
    ///     const EXPECTED: &'static str = "a mapping with an optional seed";
    ///     const EXAMPLE: &'static str = "{seed: 42}";
    ///
    ///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
    ///         let mut fields = cx.fields::<Self>(node)?;
    ///         let seed = fields.optional("seed");
    ///         fields.finish();
    ///         Some(Self(seed?))
    ///     }
    /// }
    /// ```
    pub fn optional<T: Decode>(&mut self, key: &'static str) -> Option<Option<T>> {
        let Some(node) = self.take(key) else {
            return Some(None);
        };
        self.cx
            .within(Segment::Key(key.to_owned()), |cx| T::decode(node, cx))
            .map(Some)
    }

    /// Reports every key that was not read, with a suggestion where one is
    /// close to an allowed key.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Node};
    ///
    /// struct Empty;
    ///
    /// impl Decode for Empty {
    ///     const EXPECTED: &'static str = "an empty mapping";
    ///     const EXAMPLE: &'static str = "{}";
    ///
    ///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
    ///         let fields = cx.fields::<Self>(node)?;
    ///         fields.finish();
    ///         Some(Self)
    ///     }
    /// }
    /// ```
    pub fn finish(self) {
        let allowed: Vec<String> = self.known.iter().map(|&k| k.to_owned()).collect();
        for (entry, used) in self.entries.iter().zip(&self.used) {
            if *used {
                continue;
            }
            let kind = LoadErrorKind::UnknownField {
                field: entry.key().to_owned(),
                allowed: allowed.clone(),
                suggestion: suggest(entry.key(), self.known.iter().copied()),
            };
            self.cx.within(Segment::Key(entry.key().to_owned()), |cx| {
                cx.error(entry.key_start(), kind);
            });
        }
    }

    /// Marks every key read, so `finish` reports none: for a mapping whose
    /// other errors make unknown-key errors noise.
    pub(crate) fn ignore_rest(mut self) {
        self.used.fill(true);
        self.finish();
    }
}

impl Decode for String {
    const EXPECTED: &'static str = "text";
    const EXAMPLE: &'static str = "line-a";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if let Value::Str(s) = node.value() {
            return Some(s.clone());
        }
        cx.wrong_type::<Self>(node);
        None
    }
}

impl Decode for bool {
    const EXPECTED: &'static str = "true or false";
    const EXAMPLE: &'static str = "true";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if let Value::Bool(b) = node.value() {
            return Some(*b);
        }
        cx.wrong_type::<Self>(node);
        None
    }
}

impl Decode for f64 {
    const EXPECTED: &'static str = "a number";
    const EXAMPLE: &'static str = "0.5";

    /// An integer or a number; integers are within ±(2^53 − 1), so the
    /// conversion is exact.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        match node.value() {
            Value::Float(x) => Some(*x),
            #[allow(clippy::cast_precision_loss)] // |n| < 2^53: exact.
            Value::Int(n) => Some(*n as f64),
            Value::Null | Value::Bool(_) | Value::Str(_) | Value::List(_) | Value::Map(_) => {
                cx.wrong_type::<Self>(node);
                None
            }
        }
    }
}

/// Reads a non-negative integer of at most `max`.
fn whole<T: Decode + TryFrom<i64>>(node: &Node, cx: &mut Decoder, max: &str) -> Option<T> {
    let Value::Int(n) = node.value() else {
        cx.wrong_type::<T>(node);
        return None;
    };
    if let (true, Ok(value)) = (*n >= 0, T::try_from(*n)) {
        return Some(value);
    }
    cx.not_allowed(
        node,
        n.to_string(),
        format!("a whole number from 0 to {max}"),
        T::EXAMPLE,
    );
    None
}

impl Decode for u32 {
    const EXPECTED: &'static str = "a whole number";
    const EXAMPLE: &'static str = "3";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        whole(node, cx, "4294967295")
    }
}

impl Decode for u64 {
    const EXPECTED: &'static str = "a whole number";
    const EXAMPLE: &'static str = "42";

    /// A whole number up to 2^53 − 1, the integer range of a model.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        whole(node, cx, "9007199254740991")
    }
}

impl<T: Decode> Decode for Vec<T> {
    const EXPECTED: &'static str = "a list";
    const EXAMPLE: &'static str = "[a, b]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if !matches!(node.value(), Value::List(_)) {
            cx.wrong_type::<Self>(node);
            return None;
        }
        cx.list(node, T::decode)
    }
}

/// Reads a quantity from text with its `FromStr`, keeping the typed unit
/// error.
fn quantity<T: Decode + std::str::FromStr<Err = UnitError>>(
    node: &Node,
    cx: &mut Decoder,
) -> Option<T> {
    let Value::Str(text) = node.value() else {
        cx.wrong_type::<T>(node);
        return None;
    };
    match text.parse() {
        Ok(value) => Some(value),
        Err(error) => {
            cx.error(node.start(), LoadErrorKind::Unit(error));
            None
        }
    }
}

impl Decode for SimDuration {
    const EXPECTED: &'static str = "a duration with a unit, such as `42s` (d, h, min, s, ms)";
    const EXAMPLE: &'static str = "42s";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }
}

impl Decode for Count {
    const EXPECTED: &'static str = "a count with a unit, such as `20 parts`";
    const EXAMPLE: &'static str = "20 parts";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }
}

impl Decode for Rate {
    const EXPECTED: &'static str = "a rate with a unit, such as `120 parts/h`";
    const EXAMPLE: &'static str = "120 parts/h";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }
}

impl Decode for Ratio {
    const EXPECTED: &'static str = "a percentage, such as `2%`";
    const EXAMPLE: &'static str = "2%";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Found;
    use crate::syntax::{Format, parse};

    fn node(yaml: &str) -> Node {
        parse(yaml, Format::Yaml).unwrap()
    }

    fn decode<T: Decode>(yaml: &str) -> Result<T, Vec<(String, LoadErrorKind)>> {
        let mut cx = Decoder::new(None);
        let node = node(yaml);
        match T::decode(&node, &mut cx) {
            Some(value) if !cx.has_errors() => Ok(value),
            Some(_) | None => Err(cx
                .into_errors()
                .into_iter()
                .map(|e| (e.path().to_string(), e.kind().clone()))
                .collect()),
        }
    }

    #[test]
    fn scalars() {
        assert_eq!(decode::<String>("abc"), Ok("abc".to_owned()));
        assert_eq!(decode::<bool>("true"), Ok(true));
        assert_eq!(decode::<f64>("0.5"), Ok(0.5));
        assert_eq!(decode::<f64>("2"), Ok(2.0));
        assert_eq!(decode::<u32>("3"), Ok(3));
        assert_eq!(decode::<u64>("9007199254740991"), Ok(9_007_199_254_740_991));
        assert_eq!(decode::<Vec<u32>>("[1, 2]"), Ok(vec![1, 2]));
    }

    #[test]
    fn wrong_types_name_what_was_expected_and_found() {
        fn wrong<T>(
            expected: &'static str,
            example: &'static str,
            found: Found,
        ) -> Result<T, Vec<(String, LoadErrorKind)>> {
            Err(vec![(
                "(document)".to_owned(),
                LoadErrorKind::WrongType {
                    expected,
                    example,
                    found,
                },
            )])
        }
        assert_eq!(
            decode::<String>("1"),
            wrong("text", "line-a", Found::Int(1))
        );
        assert_eq!(
            decode::<bool>("x"),
            wrong("true or false", "true", Found::Str("x".into()))
        );
        assert_eq!(decode::<f64>("[]"), wrong("a number", "0.5", Found::List));
        assert_eq!(
            decode::<u32>("1.5"),
            wrong("a whole number", "3", Found::Float(1.5))
        );
        assert_eq!(
            decode::<Vec<u32>>("{}"),
            wrong("a list", "[a, b]", Found::Map)
        );
        assert_eq!(
            decode::<SimDuration>("42"),
            wrong(SimDuration::EXPECTED, "42s", Found::Int(42))
        );
    }

    #[test]
    fn whole_numbers_are_range_checked() {
        let err = decode::<u32>("4294967296").unwrap_err();
        assert!(
            matches!(&err[0].1, LoadErrorKind::NotAllowed { found, .. } if found == "4294967296")
        );
        assert!(matches!(
            &decode::<u64>("-1").unwrap_err()[0].1,
            LoadErrorKind::NotAllowed { .. }
        ));
    }

    #[test]
    fn list_errors_carry_the_index() {
        let err = decode::<Vec<u32>>("[1, x, 3, z]").unwrap_err();
        let paths: Vec<_> = err.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["[1]", "[3]"]);
    }

    #[test]
    fn quantities_keep_their_unit_errors() {
        assert_eq!(decode::<SimDuration>("0.5h"), Ok("30min".parse().unwrap()));
        assert_eq!(decode::<Count>("20 parts"), Ok(Count::new(20)));
        assert_eq!(
            decode::<Rate>("120 parts/h"),
            Ok("120 parts/h".parse().unwrap())
        );
        assert_eq!(decode::<Ratio>("2%"), Ok("2%".parse().unwrap()));
        let err = decode::<SimDuration>("42sec").unwrap_err();
        assert!(matches!(
            &err[0].1,
            LoadErrorKind::Unit(UnitError::UnknownUnit { .. })
        ));
        for yaml in ["1", "[]"] {
            assert!(decode::<Count>(yaml).is_err());
            assert!(decode::<Rate>(yaml).is_err());
            assert!(decode::<Ratio>(yaml).is_err());
        }
    }

    #[derive(Debug)]
    struct Pair {
        a: u32,
        b: Option<u32>,
    }

    impl Decode for Pair {
        const EXPECTED: &'static str = "a pair";
        const EXAMPLE: &'static str = "{a: 1, b: 2}";

        fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
            let mut fields = cx.fields::<Self>(node)?;
            let a = fields.required("a");
            let b = fields.optional("b");
            fields.finish();
            Some(Self { a: a?, b: b? })
        }
    }

    #[test]
    fn fields_report_missing_unknown_and_invalid_keys_together() {
        let pair = decode::<Pair>("{a: 1}").unwrap();
        assert_eq!((pair.a, pair.b), (1, None));
        let pair = decode::<Pair>("{a: 1, b: 2}").unwrap();
        assert_eq!(pair.b, Some(2));

        let err = decode::<Pair>("{b: x, c: 1, aa: 2}").unwrap_err();
        assert_eq!(
            err,
            [
                (
                    "a".to_owned(),
                    LoadErrorKind::MissingField {
                        field: "a",
                        expected: "a whole number",
                        example: "3"
                    }
                ),
                (
                    "b".to_owned(),
                    LoadErrorKind::WrongType {
                        expected: "a whole number",
                        example: "3",
                        found: Found::Str("x".into())
                    }
                ),
                (
                    "c".to_owned(),
                    LoadErrorKind::UnknownField {
                        field: "c".into(),
                        allowed: vec!["a".into(), "b".into()],
                        suggestion: Some("a".into())
                    }
                ),
                (
                    "aa".to_owned(),
                    LoadErrorKind::UnknownField {
                        field: "aa".into(),
                        allowed: vec!["a".into(), "b".into()],
                        suggestion: Some("a".into())
                    }
                ),
            ]
        );
        assert!(decode::<Pair>("[1]").is_err());
    }
}
