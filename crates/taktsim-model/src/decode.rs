//! Typed decoding of a [`Node`] tree (ADR-0002, step 4 of the pipeline;
//! ADR-0020), and the JSON Schema of what it reads.
//!
//! Decoding is strict: unknown keys are errors, every value has the type
//! its field declares, and every error has a field path and a location. It
//! does not stop at the first error: a [`Decoder`] collects them all, so
//! `validate` can report every problem in one pass (spec §5).
//!
//! The schema comes from the same code. Every [`Decode`] type states its
//! schema next to its decoder, and a mapping read through [`Fields`] (a
//! [`Record`], or a component type's parameters) gets its schema by running
//! the same reading code in a describing mode, where each key it reads
//! becomes a property. So decoder and schema cannot drift (ADR-0020 §11).
//!
//! Component types (M5) read their parameters with the same API: a
//! [`Fields`] for the entry's keys and [`Decode`] for each value.

use std::collections::BTreeMap;

use taktsim_units::{Count, Rate, Ratio, SimDuration, UnitError};

use crate::error::{FieldPath, LoadError, LoadErrorKind, Location, MAX_INTEGER, Segment, suggest};
use crate::json::Json;
use crate::syntax::{Entry, Node, Value};

/// A value that can be read from a model file, and described in its JSON
/// Schema.
///
/// `EXPECTED` and `EXAMPLE` describe the value in error messages: "expected
/// `EXPECTED`, found …" and "the required key `x` is missing; it takes
/// `EXPECTED`". `schema` states, next to the decoder, the JSON Schema of
/// what `decode` accepts; a type with a `SCHEMA_NAME` is written once in
/// the schema's `$defs` and referred to. A mapping is better written as a
/// [`Record`], which gets both from one function.
///
/// ```
/// use taktsim_model::{Decode, Decoder, Json, Node, Schemas};
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
///
///     fn schema(_: &mut Schemas) -> Json {
///         Json::object([
///             ("type", Json::from("integer")),
///             ("minimum", Json::from(1)),
///             ("maximum", Json::from(1000)),
///         ])
///     }
/// }
/// ```
pub trait Decode: Sized {
    /// What the value is, for error messages, e.g. "a duration such as
    /// `42s`".
    const EXPECTED: &'static str;
    /// One valid value as written in YAML, e.g. `42s`.
    const EXAMPLE: &'static str;
    /// The name of the schema in `$defs`, if it is shared there; `None`:
    /// written where it is used.
    const SCHEMA_NAME: Option<&'static str> = None;

    /// Reads the value from `node`, or records why it cannot in `cx` and
    /// returns `None`.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self>;

    /// The JSON Schema (draft 2020-12) of what `decode` accepts. Use
    /// [`Schemas::of`] for the schemas of other types.
    fn schema(schemas: &mut Schemas) -> Json;
}

/// A mapping with known keys, read with [`Fields`]: one `read` function
/// gives both its decoder and its schema (it is [`Decode`] through a
/// blanket implementation).
///
/// `read` must ask for every key before it uses `?`: when a value is
/// invalid the other keys must still be read (or they are reported as
/// unknown), and the schema lists exactly the keys asked for.
///
/// ```
/// use taktsim_model::{Fields, Format, Record, decode_str};
/// use taktsim_units::SimDuration;
///
/// struct Failure {
///     mtbf: SimDuration,
///     mttr: Option<SimDuration>,
/// }
///
/// impl Record for Failure {
///     const EXPECTED: &'static str = "a failure model such as `{mtbf: 6h, mttr: 12min}`";
///     const EXAMPLE: &'static str = "{mtbf: 6h, mttr: 12min}";
///
///     fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
///         let mtbf = fields.required("mtbf");
///         let mttr = fields.optional("mttr");
///         Some(Self { mtbf: mtbf?, mttr: mttr? })
///     }
/// }
///
/// let f: Failure = decode_str("{mtbf: 6h}", Format::Yaml)?;
/// assert_eq!(f.mtbf.to_string(), "6h");
/// assert!(decode_str::<Failure>("{mtbf: 6h, mtr: 1min}", Format::Yaml).is_err());
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
pub trait Record: Sized {
    /// What the mapping is, for error messages.
    const EXPECTED: &'static str;
    /// One valid mapping in YAML flow style.
    const EXAMPLE: &'static str;
    /// The name of the schema in `$defs`, if it is shared there.
    const SCHEMA_NAME: Option<&'static str> = None;
    /// The schema's description, if any.
    const DESCRIPTION: Option<&'static str> = None;

    /// Reads the keys; returns `None` if any is missing or invalid, with
    /// the errors recorded through `fields`.
    fn read(fields: &mut Fields<'_, '_>) -> Option<Self>;
}

impl<T: Record> Decode for T {
    const EXPECTED: &'static str = T::EXPECTED;
    const EXAMPLE: &'static str = T::EXAMPLE;
    const SCHEMA_NAME: Option<&'static str> = T::SCHEMA_NAME;

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let mut fields = cx.fields::<Self>(node)?;
        let value = T::read(&mut fields);
        fields.finish();
        value
    }

    fn schema(schemas: &mut Schemas) -> Json {
        let schema = schemas.describe(|fields| {
            let _ = T::read(fields);
        });
        match T::DESCRIPTION {
            Some(description) => schema.with("description", &description),
            None => schema,
        }
    }
}

/// The schemas of a model document while it is generated: the shared
/// definitions (`$defs`), and what the catalog contributes.
///
/// ```
/// use taktsim_model::{Catalog, model_schema};
///
/// // Schemas are generated through `model_schema`.
/// let schema = model_schema(&Catalog::new());
/// assert_eq!(schema["$defs"]["Duration"]["type"], "string");
/// ```
#[derive(Debug, Default)]
pub struct Schemas {
    defs: BTreeMap<String, Json>,
    observables: Vec<String>,
    components: Vec<Json>,
}

impl Schemas {
    pub(crate) fn new(observables: Vec<String>) -> Self {
        Self {
            observables,
            ..Self::default()
        }
    }

    /// The schema of `T`: a reference into `$defs` if `T` has a
    /// `SCHEMA_NAME` (added there on first use), otherwise `T`'s schema.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Json, Node, Schemas};
    /// use taktsim_units::SimDuration;
    ///
    /// /// A list of durations, at least one.
    /// struct Times(Vec<SimDuration>);
    ///
    /// impl Decode for Times {
    ///     const EXPECTED: &'static str = "a list of durations";
    ///     const EXAMPLE: &'static str = "[1min, 2min]";
    ///     fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
    ///         Vec::decode(node, cx).map(Self)
    ///     }
    ///     fn schema(schemas: &mut Schemas) -> Json {
    ///         Json::object([("type", Json::from("array")), ("items", schemas.of::<SimDuration>())])
    ///     }
    /// }
    /// ```
    pub fn of<T: Decode>(&mut self) -> Json {
        let Some(name) = T::SCHEMA_NAME else {
            return T::schema(self);
        };
        if !self.defs.contains_key(name) {
            // A placeholder first, so a type that refers to itself ends.
            self.defs.insert(name.to_owned(), Json::Null);
            let schema = T::schema(self);
            self.defs.insert(name.to_owned(), schema);
        }
        Json::object([("$ref", Json::from(format!("#/$defs/{name}")))])
    }

    /// The object schema of the keys `read` asks for: each a property,
    /// required or not, and no other key.
    pub(crate) fn describe(&mut self, read: impl FnOnce(&mut Fields<'_, '_>)) -> Json {
        let mut scratch = Decoder::new(None);
        let mut fields = Fields {
            cx: &mut scratch,
            mode: Mode::Describe {
                schemas: self,
                properties: BTreeMap::new(),
                required: Vec::new(),
            },
        };
        read(&mut fields);
        let Mode::Describe {
            properties,
            required,
            ..
        } = fields.mode
        else {
            return Json::Null;
        };
        let schema = Json::object([
            ("type", Json::from("object")),
            ("properties", Json::Object(properties)),
            ("additionalProperties", Json::from(false)),
        ]);
        if required.is_empty() {
            return schema;
        }
        schema.with(
            "required",
            &Json::Array(required.into_iter().map(Json::from).collect()),
        )
    }

    /// The observables the catalog declares.
    pub(crate) fn observables(&self) -> &[String] {
        &self.observables
    }

    /// The component entries the catalog's types allow.
    pub(crate) fn components(&self) -> &[Json] {
        &self.components
    }

    pub(crate) fn set_components(&mut self, entries: Vec<Json>) {
        self.components = entries;
    }

    /// Adds `schema` to `$defs` as `name`.
    pub(crate) fn define(&mut self, name: &str, schema: Json) {
        self.defs.insert(name.to_owned(), schema);
    }

    pub(crate) fn into_defs(self) -> BTreeMap<String, Json> {
        self.defs
    }
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
    /// mapping is, for that error. A [`Record`] does this for you.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Json, Node, Schemas};
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
    ///
    ///     fn schema(_: &mut Schemas) -> Json {
    ///         Json::object([("type", Json::from("object")), ("additionalProperties", Json::from(false))])
    ///     }
    /// }
    /// ```
    pub fn fields<'n, T: Decode>(&mut self, node: &'n Node) -> Option<Fields<'n, '_>> {
        let Value::Map(entries) = node.value() else {
            self.wrong_type::<T>(node);
            return None;
        };
        Some(Fields {
            mode: Mode::Read {
                used: vec![false; entries.len()],
                entries,
                node,
                known: Vec::new(),
            },
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

/// What a [`Fields`] does: read a mapping, or describe its keys.
#[derive(Debug)]
enum Mode<'n, 'd> {
    Read {
        node: &'n Node,
        entries: &'n [Entry],
        used: Vec<bool>,
        known: Vec<&'static str>,
    },
    Describe {
        schemas: &'d mut Schemas,
        properties: BTreeMap<String, Json>,
        required: Vec<String>,
    },
}

/// The keys of one mapping, read one at a time. Each key may be read once;
/// [`finish`](Self::finish) reports every key that was not read as
/// unknown, with the keys that were as the allowed ones.
///
/// When the schema is generated, the same reading code runs with a
/// describing `Fields`: every key asked for becomes a property, and every
/// read returns `None`.
///
/// ```
/// use taktsim_model::{Fields, Record};
///
/// struct Named(String);
///
/// impl Record for Named {
///     const EXPECTED: &'static str = "a mapping with a name";
///     const EXAMPLE: &'static str = "{name: a}";
///
///     fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self(fields.required("name")?))
///     }
/// }
/// ```
#[derive(Debug)]
pub struct Fields<'n, 'd> {
    cx: &'d mut Decoder,
    mode: Mode<'n, 'd>,
}

impl<'n> Fields<'n, '_> {
    /// The value of `key`, marking it read; `None` when describing.
    pub(crate) fn take(&mut self, key: &'static str) -> Option<&'n Node> {
        let Mode::Read {
            entries,
            used,
            known,
            ..
        } = &mut self.mode
        else {
            return None;
        };
        if !known.contains(&key) {
            known.push(key);
        }
        let i = entries.iter().position(|e| e.key() == key)?;
        used[i] = true;
        Some(entries[i].value())
    }

    /// Where the value of `key` was read from, if it is present; `None`
    /// when describing.
    pub(crate) fn entry(&self, key: &str) -> Option<&'n Entry> {
        match &self.mode {
            Mode::Read { entries, .. } => entries.iter().find(|e| e.key() == key),
            Mode::Describe { .. } => None,
        }
    }

    /// The mapping itself; `None` when describing.
    pub(crate) const fn node(&self) -> Option<&'n Node> {
        match &self.mode {
            Mode::Read { node, .. } => Some(node),
            Mode::Describe { .. } => None,
        }
    }

    /// Records `key` as a property with `T`'s schema, when describing.
    /// Returns whether it was describing.
    fn describe<T: Decode>(&mut self, key: &'static str, is_required: bool) -> bool {
        let Mode::Describe {
            schemas,
            properties,
            required,
        } = &mut self.mode
        else {
            return false;
        };
        let schema = schemas.of::<T>();
        properties.insert(key.to_owned(), schema);
        if is_required {
            required.push(key.to_owned());
        }
        true
    }

    /// The decoder, to read values with custom rules.
    ///
    /// ```
    /// use taktsim_model::Fields;
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
    /// use taktsim_model::{Fields, Record};
    ///
    /// struct Named(String);
    ///
    /// impl Record for Named {
    ///     const EXPECTED: &'static str = "a mapping with a name";
    ///     const EXAMPLE: &'static str = "{name: a}";
    ///
    ///     fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
    ///         Some(Self(fields.required("name")?))
    ///     }
    /// }
    /// ```
    pub fn required<T: Decode>(&mut self, key: &'static str) -> Option<T> {
        if self.describe::<T>(key, true) {
            return None;
        }
        let Some(node) = self.take(key) else {
            let at = self.node().map_or(Location::new(1, 1), Node::start);
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
    /// use taktsim_model::{Fields, Record};
    ///
    /// struct Seeded(Option<u64>);
    ///
    /// impl Record for Seeded {
    ///     const EXPECTED: &'static str = "a mapping with an optional seed";
    ///     const EXAMPLE: &'static str = "{seed: 42}";
    ///
    ///     fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
    ///         Some(Self(fields.optional("seed")?))
    ///     }
    /// }
    /// ```
    pub fn optional<T: Decode>(&mut self, key: &'static str) -> Option<Option<T>> {
        self.optional_with::<T, T>(key, T::decode)
    }

    /// Like [`optional`](Self::optional), but the value is read by `read`
    /// (for example with context the [`Decode`] type does not have), while
    /// `T` gives the messages and the schema.
    ///
    /// ```
    /// use taktsim_model::{Decode, Fields, Record};
    ///
    /// struct Limits(Option<u32>);
    ///
    /// impl Record for Limits {
    ///     const EXPECTED: &'static str = "limits";
    ///     const EXAMPLE: &'static str = "{max: 3}";
    ///
    ///     fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
    ///         let max = fields.optional_with::<u32, _>("max", |node, cx| u32::decode(node, cx).map(|n| n.min(10)));
    ///         Some(Self(max?))
    ///     }
    /// }
    /// ```
    pub fn optional_with<T: Decode, R>(
        &mut self,
        key: &'static str,
        read: impl FnOnce(&'n Node, &mut Decoder) -> Option<R>,
    ) -> Option<Option<R>> {
        if self.describe::<T>(key, false) {
            return None;
        }
        let Some(node) = self.take(key) else {
            return Some(None);
        };
        self.cx
            .within(Segment::Key(key.to_owned()), |cx| read(node, cx))
            .map(Some)
    }

    /// Reports every key that was not read, with a suggestion where one is
    /// close to an allowed key. Does nothing when describing.
    ///
    /// ```
    /// use taktsim_model::{Decode, Decoder, Json, Node, Schemas};
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
    ///
    ///     fn schema(_: &mut Schemas) -> Json {
    ///         Json::object([("type", Json::from("object")), ("additionalProperties", Json::from(false))])
    ///     }
    /// }
    /// ```
    pub fn finish(self) {
        let Mode::Read {
            entries,
            used,
            known,
            ..
        } = self.mode
        else {
            return;
        };
        let allowed: Vec<String> = known.iter().map(|&k| k.to_owned()).collect();
        for (entry, used) in entries.iter().zip(&used) {
            if *used {
                continue;
            }
            let kind = LoadErrorKind::UnknownField {
                field: entry.key().to_owned(),
                allowed: allowed.clone(),
                suggestion: suggest(entry.key(), known.iter().copied()),
            };
            self.cx.within(Segment::Key(entry.key().to_owned()), |cx| {
                cx.error(entry.key_start(), kind);
            });
        }
    }

    /// Marks every key read, so `finish` reports none: for a mapping whose
    /// other errors make unknown-key errors noise.
    pub(crate) fn ignore_rest(mut self) {
        if let Mode::Read { used, .. } = &mut self.mode {
            used.fill(true);
        }
        self.finish();
    }
}

/// `{"type": name}` plus `extra`.
fn typed<'k>(name: &str, extra: impl IntoIterator<Item = (&'k str, Json)>) -> Json {
    let mut schema = Json::object(extra);
    if let Json::Object(map) = &mut schema {
        map.insert("type".to_owned(), Json::from(name));
    }
    schema
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

    fn schema(_: &mut Schemas) -> Json {
        typed("string", [])
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

    fn schema(_: &mut Schemas) -> Json {
        typed("boolean", [])
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

    fn schema(_: &mut Schemas) -> Json {
        typed("number", [])
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

    fn schema(_: &mut Schemas) -> Json {
        typed(
            "integer",
            [
                ("minimum", Json::from(0)),
                ("maximum", Json::from(u32::MAX)),
            ],
        )
    }
}

impl Decode for u64 {
    const EXPECTED: &'static str = "a whole number";
    const EXAMPLE: &'static str = "42";

    /// A whole number up to 2^53 − 1, the integer range of a model.
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        whole(node, cx, "9007199254740991")
    }

    fn schema(_: &mut Schemas) -> Json {
        typed(
            "integer",
            [
                ("minimum", Json::from(0)),
                ("maximum", Json::from(MAX_INTEGER)),
            ],
        )
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

    fn schema(schemas: &mut Schemas) -> Json {
        typed("array", [("items", schemas.of::<T>())])
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

/// The pattern of a duration: the grammar `taktsim-units` parses (a
/// number, at most one space, a unit).
pub(crate) const DURATION_PATTERN: &str = r"^[0-9]+(\.[0-9]+)? ?(d|h|min|s|ms)$";
/// The pattern of a count: whole parts.
pub(crate) const COUNT_PATTERN: &str = r"^[0-9]+(\.0+)? ?parts?$";
/// The pattern of a rate: parts per second, minute or hour.
pub(crate) const RATE_PATTERN: &str = r"^[0-9]+(\.[0-9]+)? ?parts?/(s|min|h)$";
/// The pattern of a ratio: a percentage.
pub(crate) const RATIO_PATTERN: &str = r"^[0-9]+(\.[0-9]+)? ?%$";

/// The schema of a quantity: a string with its pattern. Rules a pattern
/// cannot state (precision finer than 1 ns, size) are the parser's.
fn quantity_schema(pattern: &str, description: &str, examples: &[&str]) -> Json {
    typed(
        "string",
        [
            ("pattern", Json::from(pattern)),
            ("description", Json::from(description)),
            (
                "examples",
                Json::Array(examples.iter().map(|&e| Json::from(e)).collect()),
            ),
        ],
    )
}

impl Decode for SimDuration {
    const EXPECTED: &'static str = "a duration with a unit, such as `42s` (d, h, min, s, ms)";
    const EXAMPLE: &'static str = "42s";
    const SCHEMA_NAME: Option<&'static str> = Some("Duration");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }

    fn schema(_: &mut Schemas) -> Json {
        quantity_schema(
            DURATION_PATTERN,
            "A duration: a number and one of the units d, h, min, s, ms, with at most one space; \
             exact to the nanosecond.",
            &["42s", "12min", "0.5h", "8h"],
        )
    }
}

impl Decode for Count {
    const EXPECTED: &'static str = "a count with a unit, such as `20 parts`";
    const EXAMPLE: &'static str = "20 parts";
    const SCHEMA_NAME: Option<&'static str> = Some("Count");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }

    fn schema(_: &mut Schemas) -> Json {
        quantity_schema(
            COUNT_PATTERN,
            "A whole number of parts.",
            &["20 parts", "1 part"],
        )
    }
}

impl Decode for Rate {
    const EXPECTED: &'static str = "a rate with a unit, such as `120 parts/h`";
    const EXAMPLE: &'static str = "120 parts/h";
    const SCHEMA_NAME: Option<&'static str> = Some("Rate");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        quantity(node, cx)
    }

    fn schema(_: &mut Schemas) -> Json {
        quantity_schema(
            RATE_PATTERN,
            "Parts per second, minute or hour.",
            &["120 parts/h", "2 parts/min"],
        )
    }
}

impl Decode for Ratio {
    const EXPECTED: &'static str = "a percentage, such as `2%`";
    const EXAMPLE: &'static str = "2%";
    const SCHEMA_NAME: Option<&'static str> = Some("Ratio");

    /// A percentage with its unit. A bare number (`0.02`) is an error
    /// whose fix is the same share with its unit (`2%`); the owner decided
    /// that shares are written with a unit (ADR-0020, answer 10).
    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let number = match node.value() {
            Value::Int(n) => n.to_string(),
            Value::Float(x) => x.to_string(),
            Value::Null | Value::Bool(_) | Value::Str(_) | Value::List(_) | Value::Map(_) => {
                return quantity(node, cx);
            }
        };
        let kind = LoadErrorKind::BareShare {
            fix: percent(&number),
            found: number,
        };
        cx.error(node.start(), kind);
        None
    }

    fn schema(_: &mut Schemas) -> Json {
        quantity_schema(RATIO_PATTERN, "A percentage.", &["2%", "0.5%"])
    }
}

/// The share `number` (plain decimal text, e.g. `0.025`) as a percentage
/// (`2.5%`), by moving the decimal point: exact, no floating point.
fn percent(number: &str) -> String {
    let (sign, digits) = number
        .strip_prefix('-')
        .map_or(("", number), |rest| ("-", rest));
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let frac = format!("{frac:0<2}");
    let (moved, rest) = frac.split_at(2);
    let whole = format!("{int}{moved}");
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let rest = rest.trim_end_matches('0');
    if rest.is_empty() {
        format!("{sign}{whole}%")
    } else {
        format!("{sign}{whole}.{rest}%")
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

    #[test]
    fn a_bare_share_offers_the_percentage() {
        let fix = |yaml| {
            let errors = decode::<Ratio>(yaml).unwrap_err();
            let LoadErrorKind::BareShare { fix, .. } = &errors[0].1 else {
                panic!("{errors:?}")
            };
            fix.clone()
        };
        assert_eq!(fix("0.02"), "2%");
        assert_eq!(fix("0.05"), "5%");
        assert_eq!(fix("0.025"), "2.5%");
        assert_eq!(fix("0.07"), "7%");
        assert_eq!(fix("0.5"), "50%");
        assert_eq!(fix("1"), "100%");
        assert_eq!(fix("0"), "0%");
        assert_eq!(fix("1e-7"), "0.00001%");
        assert_eq!(fix("12.3456"), "1234.56%");
        assert_eq!(percent("-0.02"), "-2%");
        // Text is still parsed as a percentage.
        assert_eq!(decode::<Ratio>("2%"), Ok("2%".parse().unwrap()));
    }

    #[derive(Debug)]
    struct Pair {
        a: u32,
        b: Option<u32>,
    }

    impl Record for Pair {
        const EXPECTED: &'static str = "a pair";
        const EXAMPLE: &'static str = "{a: 1, b: 2}";
        const DESCRIPTION: Option<&'static str> = Some("Two numbers.");

        fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
            let a = fields.required("a");
            let b = fields.optional("b");
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

    #[test]
    fn a_record_describes_exactly_the_keys_it_reads() {
        let mut schemas = Schemas::default();
        let schema = schemas.of::<Pair>();
        let number = Json::object([
            ("type", Json::from("integer")),
            ("minimum", Json::from(0)),
            ("maximum", Json::from(u32::MAX)),
        ]);
        assert_eq!(
            schema,
            Json::object([
                ("type", Json::from("object")),
                ("description", Json::from("Two numbers.")),
                (
                    "properties",
                    Json::object([("a", number.clone()), ("b", number)])
                ),
                ("required", Json::Array(vec![Json::from("a")])),
                ("additionalProperties", Json::from(false)),
            ])
        );
    }

    #[test]
    fn named_schemas_are_shared_in_defs() {
        let mut schemas = Schemas::default();
        let list = schemas.of::<Vec<SimDuration>>();
        assert_eq!(list["items"]["$ref"], "#/$defs/Duration");
        assert_eq!(schemas.of::<SimDuration>()["$ref"], "#/$defs/Duration");
        let defs = schemas.into_defs();
        assert_eq!(defs.keys().collect::<Vec<_>>(), ["Duration"]);
        assert_eq!(defs["Duration"]["pattern"], DURATION_PATTERN);
        for t in [String::schema, bool::schema, f64::schema, u64::schema] {
            assert!(t(&mut Schemas::default())["type"].as_str().is_some());
        }
    }
}
