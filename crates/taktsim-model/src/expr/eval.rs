//! Evaluation of type-checked conditions (ADR-0015, "Evaluation").
//!
//! Evaluation is total: once a condition has type-checked, evaluating it
//! cannot fail for any input values of the declared types. Exact types
//! (int, duration, count) are computed in `i128`, and the checker has
//! proven at load time that no intermediate result leaves that range, so
//! there is no run-time overflow. Number, rate and ratio arithmetic is IEEE
//! 754 binary64 (identical on every platform): it cannot fail either; a
//! result too large becomes infinite, and a comparison involving NaN is
//! false. The only errors are values that do not match their declared
//! types.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use taktsim_units::{Count, Rate, Ratio, SimDuration};

use super::ast::{ComponentId, Expr, Ident, Path};
use super::ir::{
    BoolIr, CountD, EqOp, ExactIr, FloatIr, IntD, NumberD, OrdOp, RateD, RatioD, TextIr, TimeD,
};
use super::types::{Dimension, Type};

/// Something an expression reads from its environment: a path or a call.
///
/// Its [`Display`](fmt::Display) text is its canonical source text, e.g.
/// `unit.priority` or `throughput(sink)`.
///
/// ```
/// use taktsim_model::expr::{Input, Path};
///
/// let input = Input::Path(Path::parse("unit.priority").unwrap());
/// assert_eq!(input.to_string(), "unit.priority");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Input {
    /// A path, e.g. `variant`.
    Path(Path),
    /// A built-in call, e.g. `throughput(sink)`.
    Call {
        /// The function.
        function: Ident,
        /// Its component ids.
        args: Vec<ComponentId>,
    },
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(p) => p.fmt(f),
            Self::Call { function, args } => {
                write!(f, "{function}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    arg.fmt(f)?;
                }
                f.write_str(")")
            }
        }
    }
}

/// The value of an input.
///
/// ```
/// use taktsim_model::expr::Value;
///
/// let v = Value::Time("90min".parse()?);
/// assert_eq!(v.kind(), "duration");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    /// For type `bool`.
    Bool(bool),
    /// For type `int`.
    Int(i64),
    /// For type `number`; must be finite.
    Number(f64),
    /// For a duration.
    Time(SimDuration),
    /// For a count.
    Count(Count),
    /// For a rate.
    Rate(Rate),
    /// For a ratio.
    Ratio(Ratio),
    /// For a string or an enum; for an enum, one of its values.
    Text(&'a str),
}

impl Value<'_> {
    /// The name of the value's kind, for error messages.
    ///
    /// ```
    /// use taktsim_model::expr::Value;
    ///
    /// assert_eq!(Value::Text("premium").kind(), "text");
    /// ```
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Number(_) => "number",
            Self::Time(_) => "duration",
            Self::Count(_) => "count",
            Self::Rate(_) => "rate",
            Self::Ratio(_) => "ratio",
            Self::Text(_) => "text",
        }
    }
}

/// Supplies the values of inputs during evaluation.
///
/// A map from each input's canonical text (`variant`, `wip(buf1)`) to its
/// value implements it.
///
/// ```
/// use std::collections::BTreeMap;
/// use taktsim_model::expr::{Input, Path, Value, Values};
///
/// let values = BTreeMap::from([("variant".to_owned(), Value::Text("premium"))]);
/// let input = Input::Path(Path::parse("variant").unwrap());
/// assert_eq!(values.value(&input), Some(Value::Text("premium")));
/// ```
pub trait Values {
    /// The value of `input`, or `None` if there is none.
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use taktsim_model::expr::{Input, Path, Value, Values};
    ///
    /// let empty: BTreeMap<String, Value<'_>> = BTreeMap::new();
    /// assert_eq!(empty.value(&Input::Path(Path::parse("x").unwrap())), None);
    /// ```
    fn value(&self, input: &Input) -> Option<Value<'_>>;
}

impl Values for BTreeMap<String, Value<'_>> {
    fn value(&self, input: &Input) -> Option<Value<'_>> {
        self.get(&input.to_string()).copied()
    }
}

/// Input values that do not match the types the condition was checked
/// against. A valid environment never produces one.
///
/// ```
/// use std::collections::BTreeMap;
/// use taktsim_model::expr::{EvalError, Type, TypeEnv, Value, condition};
///
/// let env = TypeEnv::new().with_path("unit.priority", Type::Int)?;
/// let cond = condition("unit.priority > 2", &env)?;
/// let values = BTreeMap::from([("unit.priority".to_owned(), Value::Number(3.0))]);
/// let err = cond.eval(&values).unwrap_err();
/// assert!(matches!(err, EvalError::WrongType { .. }));
/// assert_eq!(err.to_string(), "`unit.priority` is a number, but the condition needs an int");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EvalError {
    /// No value was supplied.
    #[error("no value for `{input}`; supply a value of type {expected}")]
    Missing {
        /// The input.
        input: Input,
        /// Its declared type.
        expected: Type,
    },
    /// A value of another type than declared.
    #[error(
        "`{input}` is {}, but the condition needs {}",
        Article(found),
        Article(expected)
    )]
    WrongType {
        /// The input.
        input: Input,
        /// Its declared type.
        expected: Type,
        /// The kind of value supplied.
        found: &'static str,
    },
    /// A `number` that is NaN or infinite.
    #[error("`{input}` is {value}, which is not a finite number")]
    NotFinite {
        /// The input.
        input: Input,
        /// The value supplied.
        value: f64,
    },
    /// An enum value outside its enumeration.
    #[error("`{input}` is '{value}', which is not one of its values: {}", allowed.join(", "))]
    NotAllowed {
        /// The input.
        input: Input,
        /// The value supplied.
        value: String,
        /// The allowed values.
        allowed: Vec<String>,
    },
}

/// `a <word>` or `an <word>`.
struct Article<T>(T);

impl<T: fmt::Display> fmt::Display for Article<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.0.to_string();
        let vowel = text.starts_with(['a', 'e', 'i', 'o', 'u']);
        write!(f, "{} {text}", if vowel { "an" } else { "a" })
    }
}

/// A type-checked expression of type `bool`, ready to evaluate: an edge
/// condition or, later, a constraint.
///
/// ```
/// use std::collections::BTreeMap;
/// use taktsim_model::expr::{Type, TypeEnv, Value, condition};
///
/// let env = TypeEnv::new()
///     .with_enum("variant", ["basic", "premium"])?
///     .with_path("variant", Type::Enum("variant".into()))?;
/// let cond = condition("variant == 'premium'", &env)?;
/// let premium = BTreeMap::from([("variant".to_owned(), Value::Text("premium"))]);
/// let basic = BTreeMap::from([("variant".to_owned(), Value::Text("basic"))]);
/// assert!(cond.eval(&premium)?);
/// assert!(!cond.eval(&basic)?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Condition {
    pub(crate) expr: Expr,
    pub(crate) inputs: BTreeMap<Input, Type>,
    pub(crate) ir: BoolIr,
}

impl Condition {
    /// The syntax tree the condition was checked from.
    ///
    /// ```
    /// use taktsim_model::expr::{TypeEnv, condition};
    ///
    /// let cond = condition("1 < 2", &TypeEnv::new())?;
    /// assert_eq!(cond.expr().to_string(), "1 < 2");
    /// # Ok::<(), taktsim_model::expr::ExprError>(())
    /// ```
    #[must_use]
    pub const fn expr(&self) -> &Expr {
        &self.expr
    }

    /// Every input the condition reads, with its declared type.
    ///
    /// ```
    /// use taktsim_model::expr::{Type, TypeEnv, condition};
    ///
    /// let env = TypeEnv::new().with_path("unit.priority", Type::Int)?;
    /// let cond = condition("unit.priority > 2 or unit.priority < 0", &env)?;
    /// assert_eq!(cond.inputs().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn inputs(&self) -> &BTreeMap<Input, Type> {
        &self.inputs
    }

    /// Evaluates the condition. Every input is read, whatever the result,
    /// so a missing or ill-typed value is always reported.
    ///
    /// # Errors
    ///
    /// An [`EvalError`] if `values` lacks an input or gives one a value of
    /// another type; never for a valid environment.
    ///
    /// ```
    /// use std::collections::BTreeMap;
    /// use taktsim_model::expr::{Dimension, Type, TypeEnv, Value, condition};
    ///
    /// let env = TypeEnv::new()
    ///     .with_path("unit.waited", Type::Quantity(Dimension::Time))?;
    /// let cond = condition("unit.waited - 1h < 10min", &env)?;
    /// // 30min - 1h is negative, which is fine in an intermediate result.
    /// let values = BTreeMap::from([("unit.waited".to_owned(), Value::Time("30min".parse()?))]);
    /// assert!(cond.eval(&values)?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn eval<V: Values + ?Sized>(&self, values: &V) -> Result<bool, EvalError> {
        Eval { values }.bool(&self.ir)
    }
}

/// An exact value of dimension `D`.
struct Exact<D>(i128, PhantomData<D>);

impl<D> Exact<D> {
    const fn new(value: i128) -> Self {
        Self(value, PhantomData)
    }
}

/// A floating-point value of dimension `D`.
struct Float<D>(f64, PhantomData<D>);

impl<D> Float<D> {
    const fn new(value: f64) -> Self {
        Self(value, PhantomData)
    }
}

/// How an exact dimension reads its inputs.
trait ExactDim: Sized {
    fn ty() -> Type;
    fn read(value: Value<'_>) -> Option<Exact<Self>>;
}

impl ExactDim for IntD {
    fn ty() -> Type {
        Type::Int
    }
    fn read(value: Value<'_>) -> Option<Exact<Self>> {
        if let Value::Int(i) = value {
            Some(Exact::new(i.into()))
        } else {
            None
        }
    }
}

impl ExactDim for TimeD {
    fn ty() -> Type {
        Type::Quantity(Dimension::Time)
    }
    fn read(value: Value<'_>) -> Option<Exact<Self>> {
        if let Value::Time(d) = value {
            Some(Exact::new(d.as_nanos().into()))
        } else {
            None
        }
    }
}

impl ExactDim for CountD {
    fn ty() -> Type {
        Type::Quantity(Dimension::Count)
    }
    fn read(value: Value<'_>) -> Option<Exact<Self>> {
        if let Value::Count(c) = value {
            Some(Exact::new(c.get().into()))
        } else {
            None
        }
    }
}

/// How a floating-point dimension reads its inputs.
trait FloatDim: Sized {
    fn ty() -> Type;
    fn read(value: Value<'_>) -> Option<Float<Self>>;
}

impl FloatDim for NumberD {
    fn ty() -> Type {
        Type::Number
    }
    fn read(value: Value<'_>) -> Option<Float<Self>> {
        if let Value::Number(x) = value {
            Some(Float::new(x))
        } else {
            None
        }
    }
}

impl FloatDim for RateD {
    fn ty() -> Type {
        Type::Quantity(Dimension::Rate)
    }
    fn read(value: Value<'_>) -> Option<Float<Self>> {
        if let Value::Rate(r) = value {
            Some(Float::new(r.as_per_second()))
        } else {
            None
        }
    }
}

impl FloatDim for RatioD {
    fn ty() -> Type {
        Type::Quantity(Dimension::Ratio)
    }
    fn read(value: Value<'_>) -> Option<Float<Self>> {
        if let Value::Ratio(r) = value {
            Some(Float::new(r.as_percent()))
        } else {
            None
        }
    }
}

fn ord<T: PartialOrd>(op: OrdOp, a: &T, b: &T) -> bool {
    match op {
        OrdOp::Lt => a < b,
        OrdOp::Le => a <= b,
        OrdOp::Gt => a > b,
        OrdOp::Ge => a >= b,
    }
}

fn eq<T: PartialEq + ?Sized>(op: EqOp, a: &T, b: &T) -> bool {
    match op {
        EqOp::Eq => a == b,
        EqOp::Ne => a != b,
    }
}

struct Eval<'v, V: ?Sized> {
    values: &'v V,
}

impl<'v, V: Values + ?Sized> Eval<'v, V> {
    fn lookup(
        &self,
        input: &Input,
        expected: impl FnOnce() -> Type,
    ) -> Result<Value<'v>, EvalError> {
        self.values.value(input).ok_or_else(|| EvalError::Missing {
            input: input.clone(),
            expected: expected(),
        })
    }

    fn wrong_type(input: &Input, expected: Type, found: Value<'_>) -> EvalError {
        EvalError::WrongType {
            input: input.clone(),
            expected,
            found: found.kind(),
        }
    }

    fn bool(&self, e: &BoolIr) -> Result<bool, EvalError> {
        Ok(match e {
            BoolIr::Const(b) => *b,
            BoolIr::Input(input) => match self.lookup(input, || Type::Bool)? {
                Value::Bool(b) => b,
                other @ (Value::Int(_)
                | Value::Number(_)
                | Value::Time(_)
                | Value::Count(_)
                | Value::Rate(_)
                | Value::Ratio(_)
                | Value::Text(_)) => return Err(Self::wrong_type(input, Type::Bool, other)),
            },
            BoolIr::Not(a) => !self.bool(a)?,
            // Both sides are evaluated, so every input is checked.
            BoolIr::And(a, b) => {
                let (a, b) = (self.bool(a)?, self.bool(b)?);
                a && b
            }
            BoolIr::Or(a, b) => {
                let (a, b) = (self.bool(a)?, self.bool(b)?);
                a || b
            }
            BoolIr::EqBool(op, a, b) => eq(*op, &self.bool(a)?, &self.bool(b)?),
            BoolIr::EqText(op, a, b) => eq(*op, self.text(a)?, self.text(b)?),
            BoolIr::EqInt(op, a, b) => eq(*op, &self.exact(a)?.0, &self.exact(b)?.0),
            BoolIr::EqTime(op, a, b) => eq(*op, &self.exact(a)?.0, &self.exact(b)?.0),
            BoolIr::OrdInt(op, a, b) => ord(*op, &self.exact(a)?.0, &self.exact(b)?.0),
            BoolIr::OrdTime(op, a, b) => ord(*op, &self.exact(a)?.0, &self.exact(b)?.0),
            BoolIr::OrdCount(op, a, b) => ord(*op, &self.exact(a)?.0, &self.exact(b)?.0),
            BoolIr::OrdNumber(op, a, b) => ord(*op, &self.float(a)?.0, &self.float(b)?.0),
            BoolIr::OrdRate(op, a, b) => ord(*op, &self.float(a)?.0, &self.float(b)?.0),
            BoolIr::OrdRatio(op, a, b) => ord(*op, &self.float(a)?.0, &self.float(b)?.0),
            BoolIr::InBool(a, items) => {
                let a = self.bool(a)?;
                let mut found = false;
                for item in items {
                    found |= self.bool(item)? == a;
                }
                found
            }
            BoolIr::InText(a, items) => {
                let a = self.text(a)?;
                let mut found = false;
                for item in items {
                    found |= self.text(item)? == a;
                }
                found
            }
            BoolIr::InInt(a, items) => self.exact_in(a, items)?,
            BoolIr::InTime(a, items) => self.exact_in(a, items)?,
        })
    }

    fn exact_in<D: ExactDim>(
        &self,
        a: &ExactIr<D>,
        items: &[ExactIr<D>],
    ) -> Result<bool, EvalError> {
        let a = self.exact(a)?.0;
        let mut found = false;
        for item in items {
            found |= self.exact(item)?.0 == a;
        }
        Ok(found)
    }

    /// Exact arithmetic. The checker bounds every intermediate result below
    /// 2^127 in magnitude, so the wrapping operations never wrap.
    fn exact<D: ExactDim>(&self, e: &ExactIr<D>) -> Result<Exact<D>, EvalError> {
        Ok(Exact::new(match e {
            ExactIr::Const(c) => *c,
            ExactIr::Input(input, PhantomData) => {
                let value = self.lookup(input, D::ty)?;
                D::read(value)
                    .ok_or_else(|| Self::wrong_type(input, D::ty(), value))?
                    .0
            }
            ExactIr::Neg(a) => self.exact(a)?.0.wrapping_neg(),
            ExactIr::Add(a, b) => self.exact(a)?.0.wrapping_add(self.exact(b)?.0),
            ExactIr::Sub(a, b) => self.exact(a)?.0.wrapping_sub(self.exact(b)?.0),
            ExactIr::Mul(a, k) => self.exact(a)?.0.wrapping_mul(self.exact(k)?.0),
        }))
    }

    /// IEEE 754 arithmetic, the same on every platform.
    fn float<D: FloatDim>(&self, e: &FloatIr<D>) -> Result<Float<D>, EvalError> {
        Ok(Float::new(match e {
            FloatIr::Const(c) => *c,
            FloatIr::Input(input, PhantomData) => {
                let value = self.lookup(input, D::ty)?;
                let x = D::read(value)
                    .ok_or_else(|| Self::wrong_type(input, D::ty(), value))?
                    .0;
                if !x.is_finite() {
                    return Err(EvalError::NotFinite {
                        input: input.clone(),
                        value: x,
                    });
                }
                x
            }
            FloatIr::Neg(a) => -self.float(a)?.0,
            FloatIr::Add(a, b) => self.float(a)?.0 + self.float(b)?.0,
            FloatIr::Sub(a, b) => self.float(a)?.0 - self.float(b)?.0,
            FloatIr::Mul(a, k) => self.float(a)?.0 * self.float(k)?.0,
            FloatIr::Div(a, k) => self.float(a)?.0 / k,
            FloatIr::PerTime(n, d) => {
                // As `Count::per`: parts / seconds. Rounding to the nearest
                // f64 is intended; rates are statistics.
                #[allow(clippy::cast_precision_loss)]
                let parts = self.exact(n)?.0 as f64;
                parts / d.as_secs_f64()
            }
        }))
    }

    fn text<'a>(&'a self, e: &'a TextIr) -> Result<&'a str, EvalError> {
        match e {
            TextIr::Const(s) => Ok(s),
            TextIr::Input(input, enumeration) => {
                let ty = || {
                    enumeration
                        .as_ref()
                        .map_or(Type::String, |e| Type::Enum(e.name.clone()))
                };
                let value = self.lookup(input, ty)?;
                let Value::Text(text) = value else {
                    return Err(Self::wrong_type(input, ty(), value));
                };
                if let Some(e) = enumeration
                    && !e.values.contains(text)
                {
                    return Err(not_allowed(input, text, &e.values));
                }
                Ok(text)
            }
        }
    }
}

fn not_allowed(input: &Input, text: &str, allowed: &BTreeSet<String>) -> EvalError {
    EvalError::NotAllowed {
        input: input.clone(),
        value: text.to_owned(),
        allowed: allowed.iter().cloned().collect(),
    }
}
