//! The type checker (ADR-0009 type rules, details in ADR-0015).
//!
//! Checking lowers the syntax tree to the typed form in [`super::ir`], in
//! which an ill-typed expression cannot be represented. Exact arithmetic
//! (int, duration, count) is bounded here: every intermediate result is
//! proven to stay below 2^127 in magnitude for any input of its type, or
//! the expression is rejected, so evaluation in `i128` cannot overflow.
//!
//! Errors are found in reading order: an operand's own errors, then whether
//! it fits its operator, left to right (ADR-0015, "Errors").

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;
use std::sync::Arc;

use super::ast::{BinOp, Call, CmpOp, Expr, ExprKind, List, Path, Quantity};
use super::error::{ErrorKind, ExprError, Operator, suggest};
use super::eval::{Condition, Input};
use super::ir::{
    BoolIr, CountD, EqOp, ExactIr, FloatIr, IntD, NumberD, OrdOp, RateD, RatioD, TextIr, TimeD,
};
use super::parser::parse;
use super::span::Span;
use super::types::{Arity, Dimension, Enumeration, Expected, Type, TypeEnv};

/// The largest magnitude an exact intermediate result may have:
/// `i128::MAX`.
const EXACT_LIMIT: u128 = u128::MAX >> 1;
/// The largest magnitude of an `int` input: `|i64::MIN|`.
const INT_INPUT: u128 = 1 << 63;
/// The largest duration (in ns) or count input: `u64::MAX`.
const U64_INPUT: u128 = (1 << 64) - 1;

/// A type-checked expression of any type, e.g. an observable reference or
/// a KPI. Conditions, which can be evaluated, are [`Condition`]s.
///
/// ```
/// use taktsim_model::expr::{Arity, Dimension, Expected, Signature, Type, TypeEnv, check, parse};
///
/// let env = TypeEnv::new()
///     .with_function("throughput", Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate)))?
///     .with_component("sink", "Sink")?;
/// let checked = check(&parse("throughput(sink)")?, &env, Expected::Numeric)?;
/// assert_eq!(checked.ty(), &Type::Quantity(Dimension::Rate));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    expr: Expr,
    ty: Type,
    inputs: BTreeMap<Input, Type>,
}

impl Checked {
    /// The syntax tree that was checked.
    ///
    /// ```
    /// use taktsim_model::expr::{Expected, TypeEnv, check, parse};
    ///
    /// let checked = check(&parse("2 * 30s")?, &TypeEnv::new(), Expected::Numeric)?;
    /// assert_eq!(checked.expr().to_string(), "2 * 30s");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn expr(&self) -> &Expr {
        &self.expr
    }

    /// The type of the expression.
    ///
    /// ```
    /// use taktsim_model::expr::{Dimension, Expected, Type, TypeEnv, check, parse};
    ///
    /// let checked = check(&parse("2 * 30s")?, &TypeEnv::new(), Expected::Numeric)?;
    /// assert_eq!(checked.ty(), &Type::Quantity(Dimension::Time));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn ty(&self) -> &Type {
        &self.ty
    }

    /// Every input the expression reads, with its declared type.
    ///
    /// ```
    /// use taktsim_model::expr::{Expected, Type, TypeEnv, check, parse};
    ///
    /// let env = TypeEnv::new().with_path("unit.priority", Type::Int)?;
    /// let checked = check(&parse("unit.priority + 1")?, &env, Expected::Numeric)?;
    /// assert_eq!(checked.inputs().values().next(), Some(&Type::Int));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn inputs(&self) -> &BTreeMap<Input, Type> {
        &self.inputs
    }
}

/// Type-checks `expr` against `env` for a context that needs `expected`.
///
/// # Errors
///
/// An [`ExprError`] with the span of the first ill-typed subexpression.
///
/// ```
/// use taktsim_model::expr::{ErrorKind, Expected, TypeEnv, check, parse};
///
/// let err = check(&parse("30s + 2 parts")?, &TypeEnv::new(), Expected::Any).unwrap_err();
/// assert!(matches!(err.kind(), ErrorKind::Mismatch { .. }));
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
pub fn check(expr: &Expr, env: &TypeEnv, expected: Expected) -> Result<Checked, ExprError> {
    let (lowered, inputs) = lower(expr, env)?;
    let ty = lowered.ty();
    if !expected.accepts(&ty) {
        return Err(wrong_result(expected, ty, expr));
    }
    Ok(Checked {
        expr: expr.clone(),
        ty,
        inputs,
    })
}

/// Parses and type-checks the condition `source`, e.g. an edge's `when`.
///
/// # Errors
///
/// An [`ExprError`] if `source` does not parse, does not type-check, or is
/// not a `bool`.
///
/// ```
/// use taktsim_model::expr::{ErrorKind, TypeEnv, condition};
///
/// assert!(condition("2 parts / 1h >= 1 parts/h", &TypeEnv::new()).is_ok());
/// let err = condition("1 + 2", &TypeEnv::new()).unwrap_err();
/// assert!(matches!(err.kind(), ErrorKind::WrongResultType { .. }));
/// ```
pub fn condition(source: &str, env: &TypeEnv) -> Result<Condition, ExprError> {
    Condition::from_expr(parse(source)?, env)
}

impl Condition {
    /// Type-checks `expr` against `env` as a condition.
    ///
    /// # Errors
    ///
    /// An [`ExprError`] if `expr` does not type-check or is not a `bool`.
    ///
    /// ```
    /// use taktsim_model::expr::{Condition, TypeEnv, parse};
    ///
    /// let cond = Condition::new(&parse("not (1 > 2)")?, &TypeEnv::new())?;
    /// assert!(cond.eval(&std::collections::BTreeMap::new())?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(expr: &Expr, env: &TypeEnv) -> Result<Self, ExprError> {
        Self::from_expr(expr.clone(), env)
    }

    fn from_expr(expr: Expr, env: &TypeEnv) -> Result<Self, ExprError> {
        let (lowered, inputs) = lower(&expr, env)?;
        if let Lowered::Bool(ir) = lowered {
            Ok(Self { expr, inputs, ir })
        } else {
            Err(wrong_result(Expected::Bool, lowered.ty(), &expr))
        }
    }
}

fn wrong_result(expected: Expected, found: Type, expr: &Expr) -> ExprError {
    ExprError::new(ErrorKind::WrongResultType { expected, found }, expr.span())
}

fn lower(expr: &Expr, env: &TypeEnv) -> Result<(Lowered, BTreeMap<Input, Type>), ExprError> {
    let mut checker = Checker {
        env,
        inputs: BTreeMap::new(),
    };
    let lowered = checker.lower(expr)?;
    Ok((lowered, checker.inputs))
}

/// An exact subexpression and a bound on the magnitude of its value.
struct Ex<D> {
    ir: ExactIr<D>,
    bound: u128,
}

fn constant<D>(n: u64) -> Ex<D> {
    Ex {
        ir: ExactIr::Const(n.into()),
        bound: n.into(),
    }
}

/// The type of a typed string-valued subexpression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TextTy {
    String,
    Enum(Arc<Enumeration>),
}

impl TextTy {
    fn ty(&self) -> Type {
        match self {
            Self::String => Type::String,
            Self::Enum(e) => Type::Enum(e.name.clone()),
        }
    }
}

/// A string-valued subexpression. A string literal has no type of its own:
/// it takes the type of what it is compared with, and must then be one of
/// that enum's values.
enum TextSide {
    Typed(TextIr, TextTy),
    Literal(String),
}

impl TextSide {
    fn into_ir(self) -> TextIr {
        match self {
            Self::Typed(ir, _) => ir,
            Self::Literal(s) => TextIr::Const(s),
        }
    }
}

/// A subexpression lowered to its typed form; the variant is its type.
enum Lowered {
    Bool(BoolIr),
    Int(Ex<IntD>),
    Time(Ex<TimeD>),
    Count(Ex<CountD>),
    Number(FloatIr<NumberD>),
    Rate(FloatIr<RateD>),
    Ratio(FloatIr<RatioD>),
    Text(TextSide),
}

impl Lowered {
    fn ty(&self) -> Type {
        match self {
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Number(_) => Type::Number,
            Self::Time(_) => Type::Quantity(Dimension::Time),
            Self::Count(_) => Type::Quantity(Dimension::Count),
            Self::Rate(_) => Type::Quantity(Dimension::Rate),
            Self::Ratio(_) => Type::Quantity(Dimension::Ratio),
            Self::Text(TextSide::Typed(_, ty)) => ty.ty(),
            Self::Text(TextSide::Literal(_)) => Type::String,
        }
    }

    const fn dimension(&self) -> Option<Dimension> {
        match self {
            Self::Time(_) => Some(Dimension::Time),
            Self::Count(_) => Some(Dimension::Count),
            Self::Rate(_) => Some(Dimension::Rate),
            Self::Ratio(_) => Some(Dimension::Ratio),
            Self::Bool(_) | Self::Int(_) | Self::Number(_) | Self::Text(_) => None,
        }
    }

    /// Whether arithmetic and ordering apply to the type at all.
    const fn is_numeric(&self) -> bool {
        match self {
            Self::Int(_)
            | Self::Time(_)
            | Self::Count(_)
            | Self::Number(_)
            | Self::Rate(_)
            | Self::Ratio(_) => true,
            Self::Bool(_) | Self::Text(_) => false,
        }
    }

    fn into_bool(self) -> Result<BoolIr, Self> {
        if let Self::Bool(b) = self {
            Ok(b)
        } else {
            Err(self)
        }
    }

    fn into_int(self) -> Result<ExactIr<IntD>, Self> {
        if let Self::Int(x) = self {
            Ok(x.ir)
        } else {
            Err(self)
        }
    }

    fn into_time(self) -> Result<ExactIr<TimeD>, Self> {
        if let Self::Time(x) = self {
            Ok(x.ir)
        } else {
            Err(self)
        }
    }

    fn into_count(self) -> Result<ExactIr<CountD>, Self> {
        if let Self::Count(x) = self {
            Ok(x.ir)
        } else {
            Err(self)
        }
    }
}

/// Whether `==` applies to values of type `ty`: only exact ones
/// (ADR-0009; counts are exact integers, ADR-0015).
const fn compares_exactly(ty: &Type) -> bool {
    match ty {
        Type::Bool
        | Type::Int
        | Type::Quantity(Dimension::Time | Dimension::Count)
        | Type::String
        | Type::Enum(_) => true,
        Type::Number | Type::Quantity(Dimension::Rate | Dimension::Ratio) => false,
    }
}

/// An operand: its lowered form and the syntax it came from.
type Operand<'a> = (Lowered, &'a Expr);

/// What an error message needs to know about an operand.
struct Side<'a> {
    ty: Type,
    dimension: Option<Dimension>,
    expr: &'a Expr,
}

impl<'a> Side<'a> {
    fn of(x: &Lowered, expr: &'a Expr) -> Self {
        Self {
            ty: x.ty(),
            dimension: x.dimension(),
            expr,
        }
    }
}

/// The literal text of a bare `int` or `number` literal.
fn bare_number(e: &Expr) -> Option<String> {
    if let ExprKind::Int(n) = e.kind() {
        Some(n.to_string())
    } else if let ExprKind::Number(x) = e.kind() {
        Some(x.to_string())
    } else {
        None
    }
}

/// Operands of different types for an arithmetic or ordering operator. With
/// `unit_hint`, a bare number next to a quantity gets the more helpful
/// [`ErrorKind::MissingUnit`], whose fix then type-checks.
fn mismatch(op: Operator, l: &Side<'_>, r: &Side<'_>, span: Span, unit_hint: bool) -> ExprError {
    if unit_hint {
        for (quantity, other) in [(l, r), (r, l)] {
            if let (Some(dimension), Some(literal)) = (quantity.dimension, bare_number(other.expr))
            {
                return ExprError::new(
                    ErrorKind::MissingUnit { literal, dimension },
                    other.expr.span(),
                );
            }
        }
    }
    ExprError::new(
        ErrorKind::Mismatch {
            op,
            left: l.ty.clone(),
            right: r.ty.clone(),
        },
        span,
    )
}

/// Operands of different types for `==`, `!=` or `in`. A unit is suggested
/// for a bare number only where `==` then applies (durations and counts);
/// next to a value that never compares exactly, the error says so instead.
fn equality_mismatch(op: Operator, l: &Side<'_>, r: &Side<'_>, span: Span) -> ExprError {
    for (value, other) in [(l, r), (r, l)] {
        let Some(literal) = bare_number(other.expr) else {
            continue;
        };
        if let Some(dimension @ (Dimension::Time | Dimension::Count)) = value.dimension {
            return ExprError::new(
                ErrorKind::MissingUnit { literal, dimension },
                other.expr.span(),
            );
        }
        if !compares_exactly(&value.ty) {
            return ExprError::new(
                ErrorKind::EqualityNotAllowed {
                    op,
                    ty: value.ty.clone(),
                },
                span,
            );
        }
    }
    ExprError::new(
        ErrorKind::Mismatch {
            op,
            left: l.ty.clone(),
            right: r.ty.clone(),
        },
        span,
    )
}

fn not_defined(op: Operator, x: &Lowered, e: &Expr) -> ExprError {
    ExprError::new(ErrorKind::NotDefinedFor { op, found: x.ty() }, e.span())
}

fn boolean(x: Lowered, e: &Expr, op: Operator) -> Result<BoolIr, ExprError> {
    x.into_bool().map_err(|x| not_defined(op, &x, e))
}

/// Rejects an operand that arithmetic and ordering never apply to.
fn numeric(op: Operator, x: &Lowered, e: &Expr) -> Result<(), ExprError> {
    if x.is_numeric() {
        Ok(())
    } else {
        Err(not_defined(op, x, e))
    }
}

/// Rejects a left operand of `==`, `!=` or `in` that never compares
/// exactly, before the right side is read.
fn compared_exactly(op: Operator, x: &Lowered, span: Span) -> Result<(), ExprError> {
    let ty = x.ty();
    if compares_exactly(&ty) {
        Ok(())
    } else {
        Err(ExprError::new(
            ErrorKind::EqualityNotAllowed { op, ty },
            span,
        ))
    }
}

fn non_empty(list: &List) -> Result<(), ExprError> {
    if list.items.is_empty() {
        Err(ExprError::new(ErrorKind::EmptyList, list.span))
    } else {
        Ok(())
    }
}

fn bounded(bound: Option<u128>, span: Span) -> Result<u128, ExprError> {
    bound
        .filter(|&b| b <= EXACT_LIMIT)
        .ok_or_else(|| ExprError::new(ErrorKind::MayOverflow, span))
}

fn sum<D>(sub: bool, a: Ex<D>, b: Ex<D>, span: Span) -> Result<Ex<D>, ExprError> {
    let bound = bounded(a.bound.checked_add(b.bound), span)?;
    let (a, b) = (Box::new(a.ir), Box::new(b.ir));
    let ir = if sub {
        ExactIr::Sub(a, b)
    } else {
        ExactIr::Add(a, b)
    };
    Ok(Ex { ir, bound })
}

fn scale<D>(a: Ex<D>, k: Ex<IntD>, span: Span) -> Result<Ex<D>, ExprError> {
    let bound = bounded(a.bound.checked_mul(k.bound), span)?;
    Ok(Ex {
        ir: ExactIr::Mul(Box::new(a.ir), Box::new(k.ir)),
        bound,
    })
}

fn float_sum<D>(sub: bool, a: FloatIr<D>, b: FloatIr<D>) -> FloatIr<D> {
    let (a, b) = (Box::new(a), Box::new(b));
    if sub {
        FloatIr::Sub(a, b)
    } else {
        FloatIr::Add(a, b)
    }
}

fn float_scale<D>(a: FloatIr<D>, k: FloatIr<NumberD>) -> FloatIr<D> {
    FloatIr::Mul(Box::new(a), Box::new(k))
}

/// `+` and `-` on two numeric operands.
fn additive(sub: bool, l: Operand<'_>, r: Operand<'_>, span: Span) -> Result<Lowered, ExprError> {
    let ((lx, le), (rx, re)) = (l, r);
    Ok(match (lx, rx) {
        (Lowered::Int(a), Lowered::Int(b)) => Lowered::Int(sum(sub, a, b, span)?),
        (Lowered::Time(a), Lowered::Time(b)) => Lowered::Time(sum(sub, a, b, span)?),
        (Lowered::Count(a), Lowered::Count(b)) => Lowered::Count(sum(sub, a, b, span)?),
        (Lowered::Number(a), Lowered::Number(b)) => Lowered::Number(float_sum(sub, a, b)),
        (Lowered::Rate(a), Lowered::Rate(b)) => Lowered::Rate(float_sum(sub, a, b)),
        (Lowered::Ratio(a), Lowered::Ratio(b)) => Lowered::Ratio(float_sum(sub, a, b)),
        (lx, rx) => {
            let op = if sub { Operator::Sub } else { Operator::Add };
            let (l, r) = (Side::of(&lx, le), Side::of(&rx, re));
            return Err(mismatch(op, &l, &r, span, true));
        }
    })
}

/// `*` on two numeric operands: the same scalar type on both sides, or a
/// quantity scaled by its natural factor (an int for exact quantities, a
/// number for the others).
fn multiply(l: Operand<'_>, r: Operand<'_>, span: Span) -> Result<Lowered, ExprError> {
    let ((lx, le), (rx, re)) = (l, r);
    Ok(match (lx, rx) {
        (Lowered::Int(a), Lowered::Int(k)) => Lowered::Int(scale(a, k, span)?),
        (Lowered::Time(a), Lowered::Int(k)) | (Lowered::Int(k), Lowered::Time(a)) => {
            Lowered::Time(scale(a, k, span)?)
        }
        (Lowered::Count(a), Lowered::Int(k)) | (Lowered::Int(k), Lowered::Count(a)) => {
            Lowered::Count(scale(a, k, span)?)
        }
        (Lowered::Number(a), Lowered::Number(k)) => Lowered::Number(float_scale(a, k)),
        (Lowered::Rate(a), Lowered::Number(k)) | (Lowered::Number(k), Lowered::Rate(a)) => {
            Lowered::Rate(float_scale(a, k))
        }
        (Lowered::Ratio(a), Lowered::Number(k)) | (Lowered::Number(k), Lowered::Ratio(a)) => {
            Lowered::Ratio(float_scale(a, k))
        }
        (lx, rx) => {
            let (l, r) = (Side::of(&lx, le), Side::of(&rx, re));
            return Err(mismatch(Operator::Mul, &l, &r, span, false));
        }
    })
}

/// A literal divisor.
enum Divisor {
    Int(u64),
    Number(f64),
    Quantity(Quantity),
}

impl Divisor {
    fn of(e: &Expr) -> Option<Self> {
        match e.kind() {
            ExprKind::Int(n) => Some(Self::Int(*n)),
            ExprKind::Number(x) => Some(Self::Number(x.get())),
            ExprKind::Quantity(q) => Some(Self::Quantity(*q)),
            ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Path(_)
            | ExprKind::Call(_)
            | ExprKind::Neg(_)
            | ExprKind::Not(_)
            | ExprKind::Binary { .. }
            | ExprKind::Compare { .. }
            | ExprKind::In { .. } => None,
        }
    }

    fn ty(&self) -> Type {
        match self {
            Self::Int(_) => Type::Int,
            Self::Number(_) => Type::Number,
            Self::Quantity(q) => Type::Quantity(q.dimension()),
        }
    }

    fn is_zero(&self) -> bool {
        // Literals are never negative, so `<= 0.0` means zero.
        match self {
            Self::Int(n) => *n == 0,
            Self::Number(x) => *x <= 0.0,
            Self::Quantity(Quantity::Time(d)) => d.as_nanos() == 0,
            Self::Quantity(Quantity::Count(c)) => c.get() == 0,
            Self::Quantity(Quantity::Rate(r)) => r.as_per_second() <= 0.0,
            Self::Quantity(Quantity::Ratio(r)) => r.as_percent() <= 0.0,
        }
    }
}

/// `/`: by a non-zero literal only (ADR-0009, v0.1). The divisor is a
/// literal, so it is never lowered.
fn divide((lx, le): Operand<'_>, re: &Expr, span: Span) -> Result<Lowered, ExprError> {
    numeric(Operator::Div, &lx, le)?;
    let divisor =
        Divisor::of(re).ok_or_else(|| ExprError::new(ErrorKind::DivisorNotLiteral, re.span()))?;
    if divisor.is_zero() {
        return Err(ExprError::new(ErrorKind::DivisionByZero, re.span()));
    }
    Ok(match (lx, divisor) {
        (Lowered::Number(a), Divisor::Number(k)) => Lowered::Number(FloatIr::Div(Box::new(a), k)),
        (Lowered::Rate(a), Divisor::Number(k)) => Lowered::Rate(FloatIr::Div(Box::new(a), k)),
        (Lowered::Ratio(a), Divisor::Number(k)) => Lowered::Ratio(FloatIr::Div(Box::new(a), k)),
        (Lowered::Count(n), Divisor::Quantity(Quantity::Time(d))) => {
            Lowered::Rate(FloatIr::PerTime(Box::new(n.ir), d))
        }
        (lx, divisor) => {
            return Err(ExprError::new(
                ErrorKind::DivisionNotAllowed {
                    left: lx.ty(),
                    right: divisor.ty(),
                },
                span,
            ));
        }
    })
}

/// `<`, `<=`, `>`, `>=` on two numeric values of the same type.
fn ordering(
    op: OrdOp,
    operator: Operator,
    l: Operand<'_>,
    r: Operand<'_>,
    span: Span,
) -> Result<BoolIr, ExprError> {
    let ((lx, le), (rx, re)) = (l, r);
    Ok(match (lx, rx) {
        (Lowered::Int(a), Lowered::Int(b)) => BoolIr::OrdInt(op, Box::new(a.ir), Box::new(b.ir)),
        (Lowered::Time(a), Lowered::Time(b)) => BoolIr::OrdTime(op, Box::new(a.ir), Box::new(b.ir)),
        (Lowered::Count(a), Lowered::Count(b)) => {
            BoolIr::OrdCount(op, Box::new(a.ir), Box::new(b.ir))
        }
        (Lowered::Number(a), Lowered::Number(b)) => BoolIr::OrdNumber(op, Box::new(a), Box::new(b)),
        (Lowered::Rate(a), Lowered::Rate(b)) => BoolIr::OrdRate(op, Box::new(a), Box::new(b)),
        (Lowered::Ratio(a), Lowered::Ratio(b)) => BoolIr::OrdRatio(op, Box::new(a), Box::new(b)),
        (lx, rx) => {
            let (l, r) = (Side::of(&lx, le), Side::of(&rx, re));
            return Err(mismatch(operator, &l, &r, span, true));
        }
    })
}

struct Checker<'e> {
    env: &'e TypeEnv,
    inputs: BTreeMap<Input, Type>,
}

impl Checker<'_> {
    fn lower(&mut self, expr: &Expr) -> Result<Lowered, ExprError> {
        Ok(match expr.kind() {
            ExprKind::Bool(b) => Lowered::Bool(BoolIr::Const(*b)),
            ExprKind::Int(n) => Lowered::Int(constant(*n)),
            ExprKind::Number(x) => Lowered::Number(FloatIr::Const(x.get())),
            ExprKind::Quantity(Quantity::Time(d)) => Lowered::Time(constant(d.as_nanos())),
            ExprKind::Quantity(Quantity::Count(c)) => Lowered::Count(constant(c.get())),
            ExprKind::Quantity(Quantity::Rate(r)) => {
                Lowered::Rate(FloatIr::Const(r.as_per_second()))
            }
            ExprKind::Quantity(Quantity::Ratio(r)) => {
                Lowered::Ratio(FloatIr::Const(r.as_percent()))
            }
            ExprKind::Str(t) => Lowered::Text(TextSide::Literal(t.as_str().to_owned())),
            ExprKind::Path(p) => self.path(p, expr.span())?,
            ExprKind::Call(c) => self.call(c, expr.span())?,
            ExprKind::Neg(x) => match self.lower(x)? {
                Lowered::Int(a) => Lowered::Int(Ex {
                    bound: a.bound,
                    ir: ExactIr::Neg(Box::new(a.ir)),
                }),
                Lowered::Number(a) => Lowered::Number(FloatIr::Neg(Box::new(a))),
                other @ (Lowered::Bool(_)
                | Lowered::Time(_)
                | Lowered::Count(_)
                | Lowered::Rate(_)
                | Lowered::Ratio(_)
                | Lowered::Text(_)) => return Err(not_defined(Operator::Neg, &other, x)),
            },
            ExprKind::Not(x) => {
                let operand = boolean(self.lower(x)?, x, Operator::Not)?;
                Lowered::Bool(BoolIr::Not(Box::new(operand)))
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, expr.span())?,
            ExprKind::Compare { op, lhs, rhs } => {
                Lowered::Bool(self.compare(*op, lhs, rhs, expr.span())?)
            }
            ExprKind::In { lhs, list } => Lowered::Bool(self.membership(lhs, list, expr.span())?),
        })
    }

    /// A binary operator. The left operand is checked against the operator
    /// before the right one is read.
    fn binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> Result<Lowered, ExprError> {
        let operator = Operator::from(op);
        let l = self.lower(lhs)?;
        match op {
            BinOp::Or | BinOp::And => {
                let a = Box::new(boolean(l, lhs, operator)?);
                let b = Box::new(boolean(self.lower(rhs)?, rhs, operator)?);
                Ok(Lowered::Bool(if op == BinOp::Or {
                    BoolIr::Or(a, b)
                } else {
                    BoolIr::And(a, b)
                }))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                numeric(operator, &l, lhs)?;
                let r = self.lower(rhs)?;
                numeric(operator, &r, rhs)?;
                if op == BinOp::Mul {
                    multiply((l, lhs), (r, rhs), span)
                } else {
                    additive(op == BinOp::Sub, (l, lhs), (r, rhs), span)
                }
            }
            BinOp::Div => divide((l, lhs), rhs, span),
        }
    }

    /// A comparison. The left operand is checked against the operator
    /// before the right one is read.
    fn compare(
        &mut self,
        op: CmpOp,
        lhs: &Expr,
        rhs: &Expr,
        span: Span,
    ) -> Result<BoolIr, ExprError> {
        let operator = Operator::from(op);
        let ord = match op {
            CmpOp::Eq => return self.equality(EqOp::Eq, operator, lhs, rhs, span),
            CmpOp::Ne => return self.equality(EqOp::Ne, operator, lhs, rhs, span),
            CmpOp::Lt => OrdOp::Lt,
            CmpOp::Le => OrdOp::Le,
            CmpOp::Gt => OrdOp::Gt,
            CmpOp::Ge => OrdOp::Ge,
        };
        let l = self.lower(lhs)?;
        numeric(operator, &l, lhs)?;
        let r = self.lower(rhs)?;
        numeric(operator, &r, rhs)?;
        ordering(ord, operator, (l, lhs), (r, rhs), span)
    }

    /// `==` and `!=`, only on values that compare exactly (ADR-0009).
    fn equality(
        &mut self,
        op: EqOp,
        operator: Operator,
        le: &Expr,
        re: &Expr,
        span: Span,
    ) -> Result<BoolIr, ExprError> {
        let lx = self.lower(le)?;
        compared_exactly(operator, &lx, span)?;
        let rx = self.lower(re)?;
        Ok(match (lx, rx) {
            (Lowered::Bool(a), Lowered::Bool(b)) => BoolIr::EqBool(op, Box::new(a), Box::new(b)),
            (Lowered::Int(a), Lowered::Int(b)) => BoolIr::EqInt(op, Box::new(a.ir), Box::new(b.ir)),
            (Lowered::Time(a), Lowered::Time(b)) => {
                BoolIr::EqTime(op, Box::new(a.ir), Box::new(b.ir))
            }
            (Lowered::Count(a), Lowered::Count(b)) => {
                BoolIr::EqCount(op, Box::new(a.ir), Box::new(b.ir))
            }
            (Lowered::Text(a), Lowered::Text(b)) => {
                let b = Self::text_element(operator, (&a, le), (b, re), span)?;
                BoolIr::EqText(op, Box::new(a.into_ir()), Box::new(b))
            }
            (lx, rx) => {
                let (l, r) = (Side::of(&lx, le), Side::of(&rx, re));
                return Err(equality_mismatch(operator, &l, &r, span));
            }
        })
    }

    /// `x in [a, b, ...]`: `x == a or x == b or ...`, with the rules of `==`
    /// for every element, checked element by element.
    fn membership(&mut self, lhs: &Expr, list: &List, span: Span) -> Result<BoolIr, ExprError> {
        let l = self.lower(lhs)?;
        let left = Side::of(&l, lhs);
        Ok(match l {
            Lowered::Bool(a) => {
                BoolIr::InBool(Box::new(a), self.elements(&left, list, Lowered::into_bool)?)
            }
            Lowered::Int(a) => BoolIr::InInt(
                Box::new(a.ir),
                self.elements(&left, list, Lowered::into_int)?,
            ),
            Lowered::Time(a) => BoolIr::InTime(
                Box::new(a.ir),
                self.elements(&left, list, Lowered::into_time)?,
            ),
            Lowered::Count(a) => BoolIr::InCount(
                Box::new(a.ir),
                self.elements(&left, list, Lowered::into_count)?,
            ),
            Lowered::Text(a) => {
                non_empty(list)?;
                let mut texts = Vec::with_capacity(list.items.len());
                for item in &list.items {
                    let x = self.lower(item)?;
                    let Lowered::Text(text) = x else {
                        let element = Side::of(&x, item);
                        return Err(equality_mismatch(
                            Operator::In,
                            &left,
                            &element,
                            item.span(),
                        ));
                    };
                    texts.push(Self::text_element(
                        Operator::In,
                        (&a, lhs),
                        (text, item),
                        item.span(),
                    )?);
                }
                BoolIr::InText(Box::new(a.into_ir()), texts)
            }
            Lowered::Number(_) | Lowered::Rate(_) | Lowered::Ratio(_) => {
                return Err(ExprError::new(
                    ErrorKind::EqualityNotAllowed {
                        op: Operator::In,
                        ty: left.ty,
                    },
                    span,
                ));
            }
        })
    }

    /// The elements of a list, each lowered and accepted by `take` before
    /// the next one is read.
    fn elements<T>(
        &mut self,
        left: &Side<'_>,
        list: &List,
        take: fn(Lowered) -> Result<T, Lowered>,
    ) -> Result<Vec<T>, ExprError> {
        non_empty(list)?;
        let mut out = Vec::with_capacity(list.items.len());
        for item in &list.items {
            let x = self.lower(item)?;
            let element = take(x).map_err(|x| {
                equality_mismatch(Operator::In, left, &Side::of(&x, item), item.span())
            })?;
            out.push(element);
        }
        Ok(out)
    }

    /// The right side `item` of a string comparison with `left`, checked
    /// against it. Typed sides must have the same type; a literal must be a
    /// value of the other side's enum.
    fn text_element(
        op: Operator,
        (left, left_expr): (&TextSide, &Expr),
        (item, item_expr): (TextSide, &Expr),
        span: Span,
    ) -> Result<TextIr, ExprError> {
        match (left, item) {
            (TextSide::Typed(_, lt), TextSide::Typed(ir, it)) => {
                if *lt == it {
                    Ok(ir)
                } else {
                    Err(ExprError::new(
                        ErrorKind::Mismatch {
                            op,
                            left: lt.ty(),
                            right: it.ty(),
                        },
                        span,
                    ))
                }
            }
            (TextSide::Typed(_, lt), TextSide::Literal(s)) => {
                fits(lt, &s, item_expr.span())?;
                Ok(TextIr::Const(s))
            }
            (TextSide::Literal(s), TextSide::Typed(ir, it)) => {
                fits(&it, s, left_expr.span())?;
                Ok(ir)
            }
            (TextSide::Literal(_), TextSide::Literal(s)) => Ok(TextIr::Const(s)),
        }
    }

    /// Records `input` and lowers it according to its declared type.
    fn input(&mut self, input: Input, ty: &Type) -> Lowered {
        self.inputs.insert(input.clone(), ty.clone());
        match ty {
            Type::Bool => Lowered::Bool(BoolIr::Input(input)),
            Type::Int => Lowered::Int(Ex {
                ir: ExactIr::Input(input, PhantomData),
                bound: INT_INPUT,
            }),
            Type::Number => Lowered::Number(FloatIr::Input(input, PhantomData)),
            Type::Quantity(Dimension::Time) => Lowered::Time(Ex {
                ir: ExactIr::Input(input, PhantomData),
                bound: U64_INPUT,
            }),
            Type::Quantity(Dimension::Count) => Lowered::Count(Ex {
                ir: ExactIr::Input(input, PhantomData),
                bound: U64_INPUT,
            }),
            Type::Quantity(Dimension::Rate) => Lowered::Rate(FloatIr::Input(input, PhantomData)),
            Type::Quantity(Dimension::Ratio) => Lowered::Ratio(FloatIr::Input(input, PhantomData)),
            Type::String => {
                Lowered::Text(TextSide::Typed(TextIr::Input(input, None), TextTy::String))
            }
            Type::Enum(name) => {
                // The environment defines every enum its types name.
                let enumeration = self.env.enumeration(name).cloned();
                let ty = enumeration.clone().map_or(TextTy::String, TextTy::Enum);
                Lowered::Text(TextSide::Typed(TextIr::Input(input, enumeration), ty))
            }
        }
    }

    fn path(&mut self, path: &Path, span: Span) -> Result<Lowered, ExprError> {
        let env = self.env;
        let Some(ty) = env.path_type(path) else {
            let name = path.to_string();
            let known = env.path_names();
            let suggestion = suggest(&name, known.iter().map(String::as_str)).map(str::to_owned);
            return Err(ExprError::new(
                ErrorKind::UnknownName {
                    name,
                    suggestion,
                    known,
                },
                span,
            ));
        };
        Ok(self.input(Input::path(path.clone()), ty))
    }

    fn call(&mut self, call: &Call, span: Span) -> Result<Lowered, ExprError> {
        let env = self.env;
        let function = &call.name.value;
        let name = || function.as_str().to_owned();
        let Some(signature) = env.function(function) else {
            let known = env.function_names();
            let suggestion =
                suggest(function.as_str(), known.iter().map(String::as_str)).map(str::to_owned);
            return Err(ExprError::new(
                ErrorKind::UnknownFunction {
                    name: name(),
                    suggestion,
                    known,
                },
                call.name.span,
            ));
        };
        let accepted = env.components_for(signature);
        if !signature.arity().allows(call.args.len()) {
            return Err(ExprError::new(
                ErrorKind::ArgumentCount {
                    function: name(),
                    arity: signature.arity(),
                    found: call.args.len(),
                    example: example_call(function.as_str(), signature.arity(), &accepted),
                },
                span,
            ));
        }
        let mut seen = BTreeSet::new();
        for arg in &call.args {
            let id = &arg.value;
            let error = |kind| Err(ExprError::new(kind, arg.span));
            let Some(kind) = env.component_kind(id) else {
                let suggestion =
                    suggest(id.as_str(), accepted.iter().map(String::as_str)).map(str::to_owned);
                return error(ErrorKind::UnknownComponent {
                    function: name(),
                    id: id.as_str().to_owned(),
                    suggestion,
                    known: accepted,
                });
            };
            if let Some(kinds) = signature.accepts()
                && !kinds.contains(kind)
            {
                return error(ErrorKind::ComponentNotAccepted {
                    function: name(),
                    id: id.as_str().to_owned(),
                    kind: kind.to_owned(),
                    accepted: kinds.iter().cloned().collect(),
                });
            }
            if !seen.insert(id) {
                return error(ErrorKind::DuplicateArgument {
                    function: name(),
                    id: id.as_str().to_owned(),
                });
            }
        }
        let args = call.args.iter().map(|a| a.value.clone()).collect();
        Ok(self.input(Input::call(function.clone(), args), signature.result()))
    }
}

/// Checks that the string literal `value` is allowed where `ty` is.
fn fits(ty: &TextTy, value: &str, span: Span) -> Result<(), ExprError> {
    let TextTy::Enum(enumeration) = ty else {
        return Ok(());
    };
    let values = &enumeration.values;
    if values.contains(value) {
        return Ok(());
    }
    let suggestion = suggest(value, values.iter().map(String::as_str)).map(str::to_owned);
    Err(ExprError::new(
        ErrorKind::UnknownEnumValue {
            enumeration: enumeration.name.clone(),
            value: value.to_owned(),
            allowed: values.iter().cloned().collect(),
            suggestion,
        },
        span,
    ))
}

/// A valid call of `function`: as many of the model's `components` as it
/// takes, then placeholders if the model has too few.
fn example_call(function: &str, arity: Arity, components: &[String]) -> String {
    let n = match arity {
        Arity::Exactly(n) | Arity::AtLeast(n) => n,
    };
    let args: Vec<String> = (0..n)
        .map(|i| {
            components
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("c{}", i + 1))
        })
        .collect();
    format!("{function}({})", args.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerations_are_shared_not_copied() {
        let env = TypeEnv::new()
            .with_enum("variant", ["basic", "premium"])
            .unwrap()
            .with_path("variant", Type::Enum("variant".into()))
            .unwrap();
        let holders = || env.enumeration("variant").map(Arc::strong_count);
        assert_eq!(holders(), Some(1));
        let cond = condition("variant == 'basic' or variant in ['premium']", &env).unwrap();
        // One more holder per reference in the condition, no copy of the set.
        assert_eq!(holders(), Some(3));
        drop(cond);
        assert_eq!(holders(), Some(1));
    }
}
