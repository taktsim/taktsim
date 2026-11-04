//! Component entries and the catalog of component types (ADR-0020).
//!
//! A component entry in a model is one flat mapping: `id`, `type`, and the
//! parameters of that type. The loader reads `id` and `type` itself; the
//! parameters belong to the component type, which reads them with its own
//! Rust type through [`ComponentParams`] (ADR-0018, deviation 5). Component
//! types are registered in a [`Catalog`] that the caller gives the loader,
//! so this crate knows no component type: Source, Machine, Buffer and Sink
//! arrive with M5, without a change to the document format or the hash.
//!
//! Some keys of an entry belong to the entry itself, not to its type
//! ([`is_reserved_key`]): the envelope that every domain shares. No
//! component type may use them as parameter names.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use crate::decode::{Decode, Decoder, Fields};
use crate::error::{FieldPath, LoadErrorKind, Location, NormalizeProblem, Place};
use crate::expr::{ComponentId, Signature, TypeEnv};
use crate::json::{Encode, Json};
use crate::syntax::{Node, Value};

/// The keys of a component entry that belong to the entry itself, the
/// envelope every domain shares, and not to its component type (owner's
/// decision, 2026-11-04; ADR-0020 §6). The loader reads `id` and `type`;
/// `description` is reserved so the envelope can add it later without
/// breaking a component package that would otherwise have used the name.
///
/// ```
/// assert_eq!(taktsim_model::RESERVED_KEYS, ["id", "type", "description"]);
/// ```
pub const RESERVED_KEYS: [&str; 3] = ["id", "type", "description"];

/// The prefix of the keys of a component entry reserved for annotations by
/// tools (a viewer's layout, an agent's note), such as `x-position`. No
/// component type may use such a key; the loader does not accept them yet.
///
/// ```
/// assert_eq!(taktsim_model::RESERVED_PREFIX, "x-");
/// ```
pub const RESERVED_PREFIX: &str = "x-";

/// Whether `key` is reserved for the component entry itself: one of
/// [`RESERVED_KEYS`], or a key starting with [`RESERVED_PREFIX`].
///
/// ```
/// use taktsim_model::is_reserved_key;
///
/// assert!(is_reserved_key("id"));
/// assert!(is_reserved_key("description"));
/// assert!(is_reserved_key("x-position"));
/// assert!(!is_reserved_key("cycle_time"));
/// ```
#[must_use]
pub fn is_reserved_key(key: &str) -> bool {
    RESERVED_KEYS.contains(&key) || key.starts_with(RESERVED_PREFIX)
}

/// A reference to a component by its id, e.g. `rework_to: asm`. The loader
/// checks that the component exists.
///
/// Two references are equal if they name the same id, wherever they were
/// written. The normalized form is the id.
///
/// ```
/// use taktsim_model::{ComponentRef, Format, decode_str};
///
/// let r: ComponentRef = decode_str("weld-1", Format::Yaml)?;
/// assert_eq!(r.id(), "weld-1");
/// assert!(decode_str::<ComponentRef>("weld 1", Format::Yaml).is_err());
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
#[derive(Debug, Clone)]
pub struct ComponentRef {
    id: String,
    place: Place,
    path: FieldPath,
}

impl ComponentRef {
    /// The id referred to.
    ///
    /// ```
    /// use taktsim_model::{ComponentRef, Format, decode_str};
    ///
    /// let r: ComponentRef = decode_str("buf1", Format::Yaml)?;
    /// assert_eq!(r.id(), "buf1");
    /// # Ok::<(), taktsim_model::LoadErrors>(())
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Where the reference is written.
    ///
    /// ```
    /// use taktsim_model::{ComponentRef, Format, decode_str};
    ///
    /// let r: ComponentRef = decode_str("  buf1", Format::Yaml)?;
    /// assert_eq!(r.place().location().column(), 3);
    /// # Ok::<(), taktsim_model::LoadErrors>(())
    /// ```
    #[must_use]
    pub const fn place(&self) -> &Place {
        &self.place
    }

    pub(crate) const fn path(&self) -> &FieldPath {
        &self.path
    }
}

impl PartialEq for ComponentRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ComponentRef {}

impl Encode for ComponentRef {
    /// The id.
    fn encode(&self) -> Json {
        self.id.as_str().into()
    }
}

impl fmt::Display for ComponentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)
    }
}

/// Reads an id of the form of ADR-0015 (`[A-Za-z_][A-Za-z0-9_]*`, then
/// `-` and more such characters), so every id can be named in an
/// expression.
pub(crate) fn read_id(node: &Node, cx: &mut Decoder, what: &'static str) -> Option<String> {
    let text = String::decode(node, cx)?;
    if ComponentId::new(&text).is_none() {
        cx.error(node.start(), LoadErrorKind::InvalidId { what, found: text });
        return None;
    }
    Some(text)
}

impl Decode for ComponentRef {
    const EXPECTED: &'static str = "a component id";
    const EXAMPLE: &'static str = "weld-1";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let id = read_id(node, cx, "component id")?;
        Some(Self {
            id,
            place: Place::new(cx.file().map(str::to_owned), node.start()),
            path: cx.path().clone(),
        })
    }
}

/// The parameters of one component type: its typed Rust struct (M5).
///
/// A component type reads its parameters with [`Fields`]: the loader has
/// already read `id` and `type`, and reports every key the type did not
/// read as unknown. A parameter may not have a name the entry reserves
/// ([`is_reserved_key`]); the catalog refuses such a type. Its normalized form is its [`Encode`] output, which
/// must be an object that leaves out absent optional parameters (so adding
/// an optional parameter later does not change the hash of any existing
/// model) and writes each value with its own `Encode` (quantities and
/// distributions in their canonical forms).
///
/// ```
/// use taktsim_model::{Catalog, ComponentParams, Distribution, Encode, Fields, Json};
///
/// #[derive(Debug)]
/// struct Delay {
///     time: Distribution,
/// }
///
/// impl Encode for Delay {
///     fn encode(&self) -> Json {
///         Json::object([]).with("time", &self.time)
///     }
/// }
///
/// impl ComponentParams for Delay {
///     const TYPE: &'static str = "Delay";
///     const EXAMPLE: &'static str = "{id: wait, type: Delay, time: 5min}";
///
///     fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self { time: fields.required("time")? })
///     }
/// }
///
/// let catalog = Catalog::new().with::<Delay>()?;
/// assert_eq!(catalog.types().collect::<Vec<_>>(), ["Delay"]);
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
pub trait ComponentParams: Encode + fmt::Debug + Send + Sync + Sized + 'static {
    /// The `type` value that selects this component type, e.g. `Machine`.
    const TYPE: &'static str;
    /// One valid entry in YAML flow style, for error messages.
    const EXAMPLE: &'static str;

    /// Reads the parameters; returns `None` if any is invalid, with the
    /// errors recorded through `fields`.
    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self>;

    /// The components the parameters refer to, which the loader checks
    /// exist.
    ///
    /// ```
    /// use taktsim_model::{ComponentParams, ComponentRef, Encode, Fields, Json};
    ///
    /// #[derive(Debug)]
    /// struct Inspection {
    ///     rework_to: ComponentRef,
    /// }
    ///
    /// impl Encode for Inspection {
    ///     fn encode(&self) -> Json {
    ///         Json::object([]).with("rework_to", &self.rework_to)
    ///     }
    /// }
    ///
    /// impl ComponentParams for Inspection {
    ///     const TYPE: &'static str = "Inspection";
    ///     const EXAMPLE: &'static str = "{id: qc, type: Inspection, rework_to: asm}";
    ///
    ///     fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
    ///         Some(Self { rework_to: fields.required("rework_to")? })
    ///     }
    ///
    ///     fn references(&self) -> Vec<&ComponentRef> {
    ///         vec![&self.rework_to]
    ///     }
    /// }
    /// ```
    fn references(&self) -> Vec<&ComponentRef> {
        Vec::new()
    }
}

/// A component type with its parameter type erased.
trait Kind: Send + Sync {
    fn example(&self) -> &'static str;
    fn decode(&self, fields: &mut Fields<'_, '_>) -> Option<Box<dyn Params>>;
}

struct KindOf<P>(PhantomData<fn() -> P>);

impl<P: ComponentParams> Kind for KindOf<P> {
    fn example(&self) -> &'static str {
        P::EXAMPLE
    }

    fn decode(&self, fields: &mut Fields<'_, '_>) -> Option<Box<dyn Params>> {
        P::decode(fields).map(|p| Box::new(p) as Box<dyn Params>)
    }
}

/// Parameters with their type erased.
trait Params: fmt::Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn encode(&self) -> Json;
    fn references(&self) -> Vec<&ComponentRef>;
}

impl<P: ComponentParams> Params for P {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn encode(&self) -> Json {
        Encode::encode(self)
    }

    fn references(&self) -> Vec<&ComponentRef> {
        ComponentParams::references(self)
    }
}

/// Why a component type or observable cannot be added to a catalog.
///
/// ```
/// use taktsim_model::{Catalog, CatalogError, ComponentParams, Fields};
///
/// #[derive(Debug)]
/// struct Sink {}
///
/// impl taktsim_model::Encode for Sink {
///     fn encode(&self) -> taktsim_model::Json {
///         taktsim_model::Json::object([])
///     }
/// }
///
/// impl ComponentParams for Sink {
///     const TYPE: &'static str = "Sink";
///     const EXAMPLE: &'static str = "{id: sink, type: Sink}";
///     fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self {})
///     }
/// }
///
/// let err = Catalog::new().with::<Sink>()?.with::<Sink>().unwrap_err();
/// assert_eq!(err, CatalogError::Duplicate { name: "Sink".into() });
/// # Ok::<(), CatalogError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// The name is registered already.
    #[error("`{name}` is registered twice")]
    Duplicate {
        /// The name.
        name: String,
    },
    /// A component type name that is not an identifier
    /// (`[A-Za-z_][A-Za-z0-9_]*`).
    #[error("`{name}` is not a valid component type name; use letters, digits and `_`")]
    InvalidTypeName {
        /// The name.
        name: String,
    },
    /// An observable that the expression environment rejects.
    #[error("observable `{name}`: {error}")]
    Observable {
        /// The name.
        name: String,
        /// Why.
        error: crate::expr::EnvError,
    },
    /// A component type that reads a parameter whose name is reserved for
    /// the component entry itself ([`is_reserved_key`]).
    #[error(
        "component type `{type_name}` reads the parameter `{key}`, but `id`, `type`, \
         `description` and every key starting with `x-` belong to the component entry \
         itself; rename the parameter, e.g. `{key}` → `{type_name_lower}_{key_suffix}`",
        type_name_lower = type_name.to_lowercase(),
        key_suffix = key.trim_start_matches(RESERVED_PREFIX)
    )]
    ReservedParameter {
        /// The component type.
        type_name: String,
        /// The reserved key it reads.
        key: String,
    },
}

/// The component types and observables a model may use.
///
/// The loader is given a catalog; this crate registers nothing. The
/// component package (M5) registers its types, and the statistics (M8)
/// declare the observables, the built-in functions of `observe` and
/// `experiment.precision` with their argument and result types.
///
/// ```
/// use taktsim_model::Catalog;
/// use taktsim_model::expr::{Arity, Dimension, Signature, Type};
///
/// let catalog = Catalog::new().with_observable(
///     "throughput",
///     Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate)).accepting(["Sink"]),
/// )?;
/// assert_eq!(catalog.observables().collect::<Vec<_>>(), ["throughput"]);
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
#[derive(Default)]
pub struct Catalog {
    kinds: BTreeMap<&'static str, Box<dyn Kind>>,
    observables: BTreeMap<String, Signature>,
    /// The observables as an expression environment, built as they are
    /// added, so the loader never meets an environment error.
    env: TypeEnv,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog")
            .field("types", &self.kinds.keys().collect::<Vec<_>>())
            .field("observables", &self.observables)
            .finish_non_exhaustive()
    }
}

impl Catalog {
    /// A catalog without component types or observables.
    ///
    /// ```
    /// assert_eq!(taktsim_model::Catalog::new().types().count(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the component type `P`.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Duplicate`] if a type of that name is registered,
    /// [`CatalogError::InvalidTypeName`] if `P::TYPE` is not an identifier,
    /// [`CatalogError::ReservedParameter`] if `P` reads a parameter whose
    /// name is reserved for the entry itself ([`is_reserved_key`]). The
    /// check runs `P`'s reader on an empty entry, so it sees every key the
    /// reader asks for before it gives up; the loader checks again on every
    /// entry it reads.
    ///
    /// ```
    /// use taktsim_model::{Catalog, ComponentParams, Fields};
    ///
    /// #[derive(Debug)]
    /// struct Sink {}
    ///
    /// impl taktsim_model::Encode for Sink {
    ///     fn encode(&self) -> taktsim_model::Json {
    ///         taktsim_model::Json::object([])
    ///     }
    /// }
    ///
    /// impl ComponentParams for Sink {
    ///     const TYPE: &'static str = "Sink";
    ///     const EXAMPLE: &'static str = "{id: sink, type: Sink}";
    ///     fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
    ///         Some(Self {})
    ///     }
    /// }
    ///
    /// assert!(Catalog::new().with::<Sink>().is_ok());
    /// ```
    pub fn with<P: ComponentParams>(mut self) -> Result<Self, CatalogError> {
        let name = P::TYPE;
        let mut bytes = name.bytes();
        let valid = bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid {
            return Err(CatalogError::InvalidTypeName {
                name: name.to_owned(),
            });
        }
        if self.kinds.contains_key(name) {
            return Err(CatalogError::Duplicate {
                name: name.to_owned(),
            });
        }
        if let Some(key) = asked_keys::<P>().into_iter().find(|k| is_reserved_key(k)) {
            return Err(CatalogError::ReservedParameter {
                type_name: name.to_owned(),
                key: key.to_owned(),
            });
        }
        self.kinds.insert(name, Box::new(KindOf::<P>(PhantomData)));
        Ok(self)
    }

    /// Adds the observable `name`: a built-in function of component ids
    /// that `observe` and `experiment.precision` may name (ADR-0015).
    ///
    /// # Errors
    ///
    /// [`CatalogError::Duplicate`] if it is registered,
    /// [`CatalogError::Observable`] if the expression language rejects it
    /// (an invalid name, or a result type it does not know).
    ///
    /// ```
    /// use taktsim_model::Catalog;
    /// use taktsim_model::expr::{Arity, Dimension, Signature, Type};
    ///
    /// let wip = Signature::new(Arity::Exactly(1), Type::Number);
    /// assert!(Catalog::new().with_observable("wip", wip.clone()).is_ok());
    /// assert!(Catalog::new().with_observable("not a name", wip).is_err());
    /// ```
    pub fn with_observable(
        mut self,
        name: &str,
        signature: Signature,
    ) -> Result<Self, CatalogError> {
        if self.observables.contains_key(name) {
            return Err(CatalogError::Duplicate {
                name: name.to_owned(),
            });
        }
        self.env = self
            .env
            .with_function(name, signature.clone())
            .map_err(|error| CatalogError::Observable {
                name: name.to_owned(),
                error,
            })?;
        self.observables.insert(name.to_owned(), signature);
        Ok(self)
    }

    /// The registered component type names, in sorted order.
    ///
    /// ```
    /// assert!(taktsim_model::Catalog::new().types().next().is_none());
    /// ```
    pub fn types(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.kinds.keys().copied()
    }

    /// The registered observable names, in sorted order.
    ///
    /// ```
    /// assert!(taktsim_model::Catalog::new().observables().next().is_none());
    /// ```
    pub fn observables(&self) -> impl Iterator<Item = &str> {
        self.observables.keys().map(String::as_str)
    }

    /// The observables as an expression environment.
    pub(crate) const fn environment(&self) -> &TypeEnv {
        &self.env
    }

    fn kind(&self, name: &str) -> Option<(&'static str, &dyn Kind)> {
        self.kinds
            .get_key_value(name)
            .map(|(name, kind)| (*name, kind.as_ref()))
    }
}

/// One component of a model: its id, its type and its typed parameters.
///
/// ```
/// use taktsim_model::{Catalog, ComponentParams, Encode, Fields, Format, Json, Loader};
/// use taktsim_units::Count;
///
/// #[derive(Debug)]
/// struct Buffer {
///     capacity: Count,
/// }
///
/// impl Encode for Buffer {
///     fn encode(&self) -> Json {
///         Json::object([]).with("capacity", &self.capacity)
///     }
/// }
///
/// impl ComponentParams for Buffer {
///     const TYPE: &'static str = "Buffer";
///     const EXAMPLE: &'static str = "{id: buf1, type: Buffer, capacity: 20 parts}";
///     fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self { capacity: fields.required("capacity")? })
///     }
/// }
///
/// let catalog = Catalog::new().with::<Buffer>()?;
/// let model = Loader::new(&catalog).load_str(
///     r#"
/// format: "1.0"
/// model: {name: line-a}
/// experiment: {type: terminating, horizon: 8h}
/// components:
///   - {id: a, type: Buffer, capacity: 5 parts}
///   - {id: b, type: Buffer, capacity: 20 parts}
/// flow: [[a, b]]
/// "#,
///     Format::Yaml,
/// )?;
/// let b = &model.document().components()[1];
/// assert_eq!((b.id(), b.type_name()), ("b", "Buffer"));
/// assert_eq!(b.params::<Buffer>().map(|p| p.capacity.get()), Some(20));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Component {
    id: String,
    type_name: &'static str,
    params: Box<dyn Params>,
    place: Place,
    path: FieldPath,
    id_place: Place,
}

impl Component {
    /// The id.
    ///
    /// ```
    /// fn id(c: &taktsim_model::Component) -> &str {
    ///     c.id()
    /// }
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The component type, e.g. `Machine`.
    ///
    /// ```
    /// fn is_machine(c: &taktsim_model::Component) -> bool {
    ///     c.type_name() == "Machine"
    /// }
    /// ```
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The parameters, if they are of type `P`.
    ///
    /// ```
    /// use taktsim_model::{Component, ComponentParams};
    ///
    /// fn params<P: ComponentParams>(c: &Component) -> Option<&P> {
    ///     c.params::<P>()
    /// }
    /// ```
    #[must_use]
    pub fn params<P: ComponentParams>(&self) -> Option<&P> {
        self.params.as_any().downcast_ref()
    }

    /// Where the entry starts.
    ///
    /// ```
    /// fn line(c: &taktsim_model::Component) -> usize {
    ///     c.place().location().line()
    /// }
    /// ```
    #[must_use]
    pub const fn place(&self) -> &Place {
        &self.place
    }

    pub(crate) const fn path(&self) -> &FieldPath {
        &self.path
    }

    /// Where the id is written, and its path.
    pub(crate) fn id_place(&self) -> (&Place, FieldPath) {
        (
            &self.id_place,
            self.path.child(crate::error::Segment::Key("id".to_owned())),
        )
    }

    pub(crate) fn references(&self) -> Vec<&ComponentRef> {
        self.params.references()
    }

    /// The normalized form: `{id, type, ...parameters}`.
    pub(crate) fn to_json(&self) -> Result<Json, LoadErrorKind> {
        let problem = |problem| LoadErrorKind::Normalize {
            type_name: self.type_name.to_owned(),
            problem,
        };
        let Json::Object(params) = self.params.encode() else {
            return Err(problem(crate::error::NormalizeProblem::NotAMapping));
        };
        let mut entry = Json::object([
            ("id", Json::from(self.id.as_str())),
            ("type", Json::from(self.type_name)),
        ]);
        for (key, value) in params {
            if is_reserved_key(&key) {
                return Err(problem(NormalizeProblem::ReservedKey(key)));
            }
            entry = entry.with(&key, &value);
        }
        Ok(entry)
    }
}

/// The keys a component type asks for when it reads an empty entry.
fn asked_keys<P: ComponentParams>() -> Vec<&'static str> {
    let start = Location::new(1, 1);
    let node = Node::new(Value::Map(Vec::new()), start, start);
    let mut cx = Decoder::new(None);
    let Some(mut fields) = cx.fields::<TypeName>(&node) else {
        return Vec::new();
    };
    let _ = P::decode(&mut fields);
    fields.asked().to_vec()
}

/// The `type` value of an entry.
struct TypeName(String);

impl Decode for TypeName {
    const EXPECTED: &'static str = "a component type such as `Machine`";
    const EXAMPLE: &'static str = "Machine";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        String::decode(node, cx).map(Self)
    }
}

/// An id read by `Fields::required`.
struct Id(String);

impl Decode for Id {
    const EXPECTED: &'static str = "a component id such as `weld-1`";
    const EXAMPLE: &'static str = "weld-1";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        read_id(node, cx, "component id").map(Self)
    }
}

/// Reads one component entry with the types of `catalog`.
pub(crate) fn read_component(
    node: &Node,
    cx: &mut Decoder,
    catalog: &Catalog,
) -> Option<Component> {
    /// For the error when the entry is not a mapping.
    struct Entry;
    impl Decode for Entry {
        const EXPECTED: &'static str =
            "a component: a mapping with `id`, `type` and its parameters";
        const EXAMPLE: &'static str = "{id: cut, type: Machine, cycle_time: 42s}";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
        }
    }

    let mut fields = cx.fields::<Entry>(node)?;
    let id = fields.required::<Id>("id");
    let type_name = fields.required::<TypeName>("type");
    let Some(TypeName(type_name)) = type_name else {
        fields.ignore_rest();
        return None;
    };
    let Some((type_name, kind)) = catalog.kind(&type_name) else {
        let at = fields
            .entry("type")
            .map_or(node.start(), |e| e.value().start());
        let registered: Vec<&str> = catalog.types().collect();
        let error = LoadErrorKind::UnknownValue {
            what: "component type",
            suggestion: crate::error::suggest(&type_name, registered.iter().copied()),
            found: type_name,
            allowed: registered.iter().map(|&t| t.to_owned()).collect(),
        };
        fields
            .decoder()
            .within(crate::error::Segment::Key("type".to_owned()), |cx| {
                cx.error(at, error);
            });
        fields.ignore_rest();
        return None;
    };
    let before = fields.decoder().error_count();
    let params = kind.decode(&mut fields);
    // The catalog refused types that ask for a reserved key on an empty
    // entry; one that asks only after other keys is caught here. The loader
    // itself asked for `id` and `type`.
    let reserved = fields
        .asked()
        .iter()
        .find(|&&k| !matches!(k, "id" | "type") && is_reserved_key(k))
        .copied();
    if let Some(key) = reserved {
        let kind = LoadErrorKind::Normalize {
            type_name: type_name.to_owned(),
            problem: NormalizeProblem::ReservedKey(key.to_owned()),
        };
        fields.decoder().error(node.start(), kind);
    }
    if params.is_none() && fields.decoder().error_count() == before {
        // A component type must record why it rejects its parameters.
        let found = fields.node().found().to_string();
        let example = kind.example();
        let cx = fields.decoder();
        cx.not_allowed(
            node,
            found,
            format!("the parameters of {type_name}"),
            example,
        );
    }
    let id_at = fields
        .entry("id")
        .map_or(node.start(), |e| e.value().start());
    fields.finish();
    let file = cx.file().map(str::to_owned);
    Some(Component {
        id: id?.0,
        type_name,
        params: params?,
        place: Place::new(file.clone(), node.start()),
        path: cx.path().clone(),
        id_place: Place::new(file, id_at),
    })
}
