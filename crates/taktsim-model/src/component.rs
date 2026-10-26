//! Component entries and the catalog of component types (ADR-0020).
//!
//! A component entry in a model is one flat mapping: `id`, `type`, and the
//! parameters of that type. The loader reads `id` and `type` itself; the
//! parameters belong to the component type, which reads them with its own
//! Rust type through [`ComponentParams`] (ADR-0018, deviation 5). Component
//! types are registered in a [`Catalog`] that the caller gives the loader,
//! so this crate knows no component type: Source, Machine, Buffer and Sink
//! arrive with M5, without a change to the document format or the hash.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use serde::Serialize;

use crate::decode::{Decode, Decoder, Fields};
use crate::error::{FieldPath, LoadErrorKind, Place};
use crate::expr::{ComponentId, Signature, TypeEnv};
use crate::syntax::Node;

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

impl Serialize for ComponentRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.id)
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
/// read as unknown. Its normalized form is its `Serialize` output, which
/// must be a mapping that leaves out absent optional parameters (so adding
/// an optional parameter later does not change the hash of any existing
/// model) and writes quantities and distributions in their canonical forms
/// (they serialize that way). Its `JsonSchema` (derived) describes the
/// parameters in the model's schema ([`model_schema`](crate::model_schema));
/// a nested parameter struct needs `#[schemars(deny_unknown_fields)]`, as
/// its decoder rejects unknown keys.
///
/// ```
/// use serde::Serialize;
/// use taktsim_model::{Catalog, ComponentParams, Distribution, Fields};
///
/// #[derive(Debug, Serialize, schemars::JsonSchema)]
/// struct Delay {
///     time: Distribution,
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
pub trait ComponentParams:
    Serialize + schemars::JsonSchema + fmt::Debug + Send + Sync + Sized + 'static
{
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
    /// use serde::Serialize;
    /// use taktsim_model::{ComponentParams, ComponentRef, Fields};
    ///
    /// #[derive(Debug, Serialize, schemars::JsonSchema)]
    /// struct Inspection {
    ///     rework_to: ComponentRef,
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
    fn schema(&self, generator: &mut schemars::SchemaGenerator) -> serde_json::Value;
}

struct KindOf<P>(PhantomData<fn() -> P>);

impl<P: ComponentParams> Kind for KindOf<P> {
    fn example(&self) -> &'static str {
        P::EXAMPLE
    }

    fn decode(&self, fields: &mut Fields<'_, '_>) -> Option<Box<dyn Params>> {
        P::decode(fields).map(|p| Box::new(p) as Box<dyn Params>)
    }

    fn schema(&self, generator: &mut schemars::SchemaGenerator) -> serde_json::Value {
        let params = P::json_schema(generator);
        crate::schema::entry_schema(P::TYPE, params, generator)
    }
}

/// Parameters with their type erased.
trait Params: fmt::Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn to_json(&self) -> Result<serde_json::Value, serde_json::Error>;
    fn references(&self) -> Vec<&ComponentRef>;
}

impl<P: ComponentParams> Params for P {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn to_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
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
/// #[derive(Debug, serde::Serialize, schemars::JsonSchema)]
/// struct Sink {}
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
    /// [`CatalogError::InvalidTypeName`] if `P::TYPE` is not an identifier.
    ///
    /// ```
    /// use taktsim_model::{Catalog, ComponentParams, Fields};
    ///
    /// #[derive(Debug, serde::Serialize, schemars::JsonSchema)]
    /// struct Sink {}
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

    /// The schema of an entry of each component type, in sorted order.
    pub(crate) fn entry_schemas(
        &self,
        generator: &mut schemars::SchemaGenerator,
    ) -> Vec<serde_json::Value> {
        self.kinds
            .values()
            .map(|kind| kind.schema(generator))
            .collect()
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
/// use serde::Serialize;
/// use taktsim_model::{Catalog, ComponentParams, Fields, Format, Loader};
/// use taktsim_units::Count;
///
/// #[derive(Debug, Serialize, schemars::JsonSchema)]
/// struct Buffer {
///     capacity: Count,
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
    pub(crate) fn to_json(&self) -> Result<serde_json::Value, LoadErrorKind> {
        let problem = |problem| LoadErrorKind::Normalize {
            type_name: self.type_name.to_owned(),
            problem,
        };
        let params = self
            .params
            .to_json()
            .map_err(|e| problem(crate::error::NormalizeProblem::Serialize(e.to_string())))?;
        let serde_json::Value::Object(params) = params else {
            return Err(problem(crate::error::NormalizeProblem::NotAMapping));
        };
        let mut entry = serde_json::Map::new();
        entry.insert("id".to_owned(), self.id.clone().into());
        entry.insert("type".to_owned(), self.type_name.into());
        for (key, value) in params {
            if entry.contains_key(&key) {
                return Err(problem(crate::error::NormalizeProblem::ReservedKey(key)));
            }
            entry.insert(key, value);
        }
        Ok(serde_json::Value::Object(entry))
    }
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
