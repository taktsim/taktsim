//! Types and the typing environment a caller supplies per context.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use super::ast::{ComponentId, Ident, Path};

/// The dimension of a quantity: the quantities `taktsim-units` has.
///
/// ```
/// use taktsim_model::expr::{Dimension, Type};
///
/// assert_eq!(Type::Quantity(Dimension::Time).to_string(), "duration");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dimension {
    /// A duration (`SimDuration`, integer nanoseconds).
    Time,
    /// A whole number of parts (`Count`).
    Count,
    /// Parts per time (`Rate`).
    Rate,
    /// A percentage (`Ratio`).
    Ratio,
}

impl fmt::Display for Dimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Time => "duration",
            Self::Count => "count",
            Self::Rate => "rate",
            Self::Ratio => "ratio",
        })
    }
}

/// The type of an expression (ADR-0009).
///
/// ```
/// use taktsim_model::expr::{Expected, Type, TypeEnv, check, parse};
///
/// let checked = check(&parse("2 parts / 1h")?, &TypeEnv::new(), Expected::Any)?;
/// assert_eq!(checked.ty().to_string(), "rate");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Type {
    /// `true` or `false`.
    Bool,
    /// An exact integer.
    Int,
    /// A finite floating-point number without a unit.
    Number,
    /// A quantity of one dimension.
    Quantity(Dimension),
    /// Text.
    String,
    /// One of the values of the named enumeration in the environment, e.g.
    /// the variants of the model.
    Enum(String),
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool => f.write_str("bool"),
            Self::Int => f.write_str("int"),
            Self::Number => f.write_str("number"),
            Self::Quantity(d) => d.fmt(f),
            Self::String => f.write_str("string"),
            Self::Enum(name) => write!(f, "enum `{name}`"),
        }
    }
}

/// The type an expression must have in its context.
///
/// ```
/// use taktsim_model::expr::{Dimension, Expected, Type};
///
/// assert!(Expected::Bool.accepts(&Type::Bool));
/// assert!(Expected::Numeric.accepts(&Type::Quantity(Dimension::Rate)));
/// assert!(!Expected::Numeric.accepts(&Type::Bool));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Expected {
    /// A condition or constraint: `bool`.
    Bool,
    /// A KPI or observable: `int`, `number` or a quantity.
    Numeric,
    /// Any type.
    Any,
}

impl Expected {
    /// Whether `ty` is allowed.
    ///
    /// ```
    /// use taktsim_model::expr::{Expected, Type};
    ///
    /// assert!(Expected::Any.accepts(&Type::String));
    /// assert!(!Expected::Bool.accepts(&Type::Int));
    /// ```
    #[must_use]
    pub fn accepts(self, ty: &Type) -> bool {
        match self {
            Self::Bool => *ty == Type::Bool,
            Self::Numeric => matches!(ty, Type::Int | Type::Number | Type::Quantity(_)),
            Self::Any => true,
        }
    }
}

impl fmt::Display for Expected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bool => "a bool, e.g. `variant == 'premium'`",
            Self::Numeric => "an int, a number or a quantity, e.g. `throughput(sink)`",
            Self::Any => "any type",
        })
    }
}

/// How many component ids a built-in function takes.
///
/// ```
/// use taktsim_model::expr::Arity;
///
/// assert!(Arity::AtLeast(1).allows(3));
/// assert!(!Arity::Exactly(1).allows(2));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arity {
    /// Exactly this many.
    Exactly(usize),
    /// This many or more.
    AtLeast(usize),
}

impl Arity {
    /// Whether `n` arguments are allowed.
    ///
    /// ```
    /// use taktsim_model::expr::Arity;
    ///
    /// assert!(Arity::Exactly(0).allows(0));
    /// ```
    #[must_use]
    pub const fn allows(self, n: usize) -> bool {
        match self {
            Self::Exactly(k) => n == k,
            Self::AtLeast(k) => n >= k,
        }
    }
}

impl fmt::Display for Arity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (prefix, n) = match self {
            Self::Exactly(n) => ("", n),
            Self::AtLeast(n) => ("at least ", n),
        };
        let s = if *n == 1 { "" } else { "s" };
        write!(f, "{prefix}{n} component id{s}")
    }
}

/// The signature of a built-in function: how many component ids it takes,
/// which component types it accepts and the type of its result.
///
/// ```
/// use taktsim_model::expr::{Arity, Dimension, Signature, Type};
///
/// let wip = Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Count))
///     .accepting(["Buffer"]);
/// assert_eq!(wip.result(), &Type::Quantity(Dimension::Count));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    arity: Arity,
    accepts: Option<BTreeSet<String>>,
    result: Type,
}

impl Signature {
    /// A function taking `arity` ids of any component type.
    ///
    /// ```
    /// use taktsim_model::expr::{Arity, Dimension, Signature, Type};
    ///
    /// let s = Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate));
    /// assert!(s.accepts().is_none());
    /// ```
    #[must_use]
    pub const fn new(arity: Arity, result: Type) -> Self {
        Self {
            arity,
            accepts: None,
            result,
        }
    }

    /// The same function, restricted to components of the given types.
    ///
    /// ```
    /// use taktsim_model::expr::{Arity, Signature, Type};
    ///
    /// let s = Signature::new(Arity::Exactly(1), Type::Int).accepting(["Machine"]);
    /// assert!(s.accepts().unwrap().contains("Machine"));
    /// ```
    #[must_use]
    pub fn accepting<I, S>(mut self, kinds: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.accepts = Some(kinds.into_iter().map(Into::into).collect());
        self
    }

    /// How many ids the function takes.
    ///
    /// ```
    /// use taktsim_model::expr::{Arity, Signature, Type};
    ///
    /// assert_eq!(Signature::new(Arity::AtLeast(1), Type::Int).arity(), Arity::AtLeast(1));
    /// ```
    #[must_use]
    pub const fn arity(&self) -> Arity {
        self.arity
    }

    /// The component types accepted, or `None` for any.
    ///
    /// ```
    /// use taktsim_model::expr::{Arity, Signature, Type};
    ///
    /// assert!(Signature::new(Arity::Exactly(1), Type::Int).accepts().is_none());
    /// ```
    #[must_use]
    pub const fn accepts(&self) -> Option<&BTreeSet<String>> {
        self.accepts.as_ref()
    }

    /// The type of the result.
    ///
    /// ```
    /// use taktsim_model::expr::{Arity, Signature, Type};
    ///
    /// assert_eq!(Signature::new(Arity::Exactly(1), Type::Int).result(), &Type::Int);
    /// ```
    #[must_use]
    pub const fn result(&self) -> &Type {
        &self.result
    }
}

/// Why a typing environment could not be built. These are mistakes of the
/// program that builds the environment, not of the model author.
///
/// ```
/// use taktsim_model::expr::{EnvError, Type, TypeEnv};
///
/// let err = TypeEnv::new().with_path("unit-age", Type::Int).unwrap_err();
/// assert!(matches!(err, EnvError::InvalidName { .. }));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvError {
    /// A path or function name that the language cannot write.
    #[error(
        "`{name}` is not a valid name; names are ASCII letters, digits and `_`, \
         do not start with a digit and are not keywords; example: unit.priority"
    )]
    InvalidName {
        /// The name given.
        name: String,
    },
    /// A component id that a call argument cannot write.
    #[error("`{id}` is not a valid component id; example: weld-1")]
    InvalidComponentId {
        /// The id given.
        id: String,
    },
    /// An enum value that a string literal cannot write.
    #[error(
        "enum `{enumeration}` value `{value}` contains `'`, which a string literal cannot \
         write; example: premium"
    )]
    InvalidEnumValue {
        /// The enumeration.
        enumeration: String,
        /// The value given.
        value: String,
    },
    /// An enumeration without values.
    #[error("enum `{name}` has no values; give at least one, e.g. [\"basic\", \"premium\"]")]
    EmptyEnum {
        /// The enumeration.
        name: String,
    },
    /// A type that names an enumeration not defined first.
    #[error("enum `{name}` is not defined; define it with `with_enum` first")]
    UnknownEnum {
        /// The enumeration named.
        name: String,
    },
    /// A name defined twice.
    #[error(
        "`{name}` is defined twice; define each enum, path, function and component once, \
         e.g. `unit.priority` once and `unit.rank` once"
    )]
    Duplicate {
        /// The name.
        name: String,
    },
}

/// An enumeration and its values, shared by every expression that uses
/// it rather than copied.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Enumeration {
    pub(crate) name: String,
    pub(crate) values: BTreeSet<String>,
}

/// What an expression may refer to in one context: paths and their types,
/// enumerations, built-in functions and component ids.
///
/// Edge conditions see unit attributes; KPIs and objectives see
/// observables. Each context builds its own environment.
///
/// ```
/// use taktsim_model::expr::{Arity, Dimension, Signature, Type, TypeEnv};
///
/// let env = TypeEnv::new()
///     .with_enum("variant", ["basic", "premium"])?
///     .with_path("variant", Type::Enum("variant".into()))?
///     .with_function(
///         "throughput",
///         Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate)),
///     )?
///     .with_component("sink", "Sink")?;
/// # Ok::<(), taktsim_model::expr::EnvError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TypeEnv {
    enums: BTreeMap<String, Arc<Enumeration>>,
    paths: BTreeMap<Path, Type>,
    functions: BTreeMap<Ident, Signature>,
    components: BTreeMap<ComponentId, String>,
}

impl TypeEnv {
    /// An empty environment: only literals type-check.
    ///
    /// ```
    /// use taktsim_model::expr::{Expected, TypeEnv, check, parse};
    ///
    /// assert!(check(&parse("1 < 2")?, &TypeEnv::new(), Expected::Bool).is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the enumeration `name` with its allowed `values`.
    ///
    /// # Errors
    ///
    /// [`EnvError::Duplicate`], [`EnvError::EmptyEnum`], or
    /// [`EnvError::InvalidEnumValue`] for a value containing `'`.
    ///
    /// ```
    /// use taktsim_model::expr::TypeEnv;
    ///
    /// let env = TypeEnv::new().with_enum("quality", ["ok", "rework", "scrap"])?;
    /// assert!(env.with_enum("empty", Vec::<String>::new()).is_err());
    /// # Ok::<(), taktsim_model::expr::EnvError>(())
    /// ```
    pub fn with_enum<I, S>(mut self, name: &str, values: I) -> Result<Self, EnvError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let values: BTreeSet<String> = values.into_iter().map(Into::into).collect();
        if self.enums.contains_key(name) {
            return Err(EnvError::Duplicate {
                name: name.to_owned(),
            });
        }
        if values.is_empty() {
            return Err(EnvError::EmptyEnum {
                name: name.to_owned(),
            });
        }
        if let Some(value) = values.iter().find(|v| v.contains('\'')) {
            return Err(EnvError::InvalidEnumValue {
                enumeration: name.to_owned(),
                value: value.clone(),
            });
        }
        let enumeration = Enumeration {
            name: name.to_owned(),
            values,
        };
        self.enums.insert(name.to_owned(), Arc::new(enumeration));
        Ok(self)
    }

    /// Adds the path `path` (dotted, e.g. `unit.priority`) of type `ty`.
    ///
    /// # Errors
    ///
    /// [`EnvError::InvalidName`], [`EnvError::UnknownEnum`] or
    /// [`EnvError::Duplicate`].
    ///
    /// ```
    /// use taktsim_model::expr::{Type, TypeEnv};
    ///
    /// let env = TypeEnv::new().with_path("unit.priority", Type::Int)?;
    /// assert!(env.with_path("product", Type::Enum("product".into())).is_err());
    /// # Ok::<(), taktsim_model::expr::EnvError>(())
    /// ```
    pub fn with_path(mut self, path: &str, ty: Type) -> Result<Self, EnvError> {
        let parsed = Path::parse(path).ok_or_else(|| EnvError::InvalidName {
            name: path.to_owned(),
        })?;
        self.known_type(&ty)?;
        if self.paths.insert(parsed, ty).is_some() {
            return Err(EnvError::Duplicate {
                name: path.to_owned(),
            });
        }
        Ok(self)
    }

    /// Adds the built-in function `name`.
    ///
    /// # Errors
    ///
    /// [`EnvError::InvalidName`], [`EnvError::UnknownEnum`] or
    /// [`EnvError::Duplicate`].
    ///
    /// ```
    /// use taktsim_model::expr::{Arity, Dimension, Signature, Type, TypeEnv};
    ///
    /// let lead_time = Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Time));
    /// let env = TypeEnv::new().with_function("lead_time", lead_time)?;
    /// # Ok::<(), taktsim_model::expr::EnvError>(())
    /// ```
    pub fn with_function(mut self, name: &str, signature: Signature) -> Result<Self, EnvError> {
        let ident = Ident::new(name).ok_or_else(|| EnvError::InvalidName {
            name: name.to_owned(),
        })?;
        self.known_type(&signature.result)?;
        if self.functions.insert(ident, signature).is_some() {
            return Err(EnvError::Duplicate {
                name: name.to_owned(),
            });
        }
        Ok(self)
    }

    /// Adds the component `id` of type `kind` (e.g. `Buffer`), which calls
    /// may name.
    ///
    /// # Errors
    ///
    /// [`EnvError::InvalidComponentId`] or [`EnvError::Duplicate`].
    ///
    /// ```
    /// use taktsim_model::expr::TypeEnv;
    ///
    /// let env = TypeEnv::new().with_component("weld-1", "Machine")?;
    /// assert!(env.with_component("weld 2", "Machine").is_err());
    /// # Ok::<(), taktsim_model::expr::EnvError>(())
    /// ```
    pub fn with_component(mut self, id: &str, kind: &str) -> Result<Self, EnvError> {
        let parsed = ComponentId::new(id)
            .ok_or_else(|| EnvError::InvalidComponentId { id: id.to_owned() })?;
        if self.components.insert(parsed, kind.to_owned()).is_some() {
            return Err(EnvError::Duplicate {
                name: id.to_owned(),
            });
        }
        Ok(self)
    }

    fn known_type(&self, ty: &Type) -> Result<(), EnvError> {
        if let Type::Enum(name) = ty
            && !self.enums.contains_key(name)
        {
            return Err(EnvError::UnknownEnum { name: name.clone() });
        }
        Ok(())
    }

    pub(crate) fn path_type(&self, path: &Path) -> Option<&Type> {
        self.paths.get(path)
    }

    pub(crate) fn path_names(&self) -> Vec<String> {
        self.paths.keys().map(ToString::to_string).collect()
    }

    pub(crate) fn function(&self, name: &Ident) -> Option<&Signature> {
        self.functions.get(name)
    }

    pub(crate) fn function_names(&self) -> Vec<String> {
        self.functions
            .keys()
            .map(|i| i.as_str().to_owned())
            .collect()
    }

    pub(crate) fn component_kind(&self, id: &ComponentId) -> Option<&str> {
        self.components.get(id).map(String::as_str)
    }

    /// The ids of the components that `signature` takes, in order.
    pub(crate) fn components_for(&self, signature: &Signature) -> Vec<String> {
        self.components
            .iter()
            .filter(|(_, kind)| signature.accepts().is_none_or(|a| a.contains(*kind)))
            .map(|(id, _)| id.as_str().to_owned())
            .collect()
    }

    pub(crate) fn enumeration(&self, name: &str) -> Option<&Arc<Enumeration>> {
        self.enums.get(name)
    }
}
