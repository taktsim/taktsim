//! The type checker (ADR-0009 type rules, details in ADR-0015).
//!
//! Checking lowers the syntax tree to the typed form in [`super::ir`], in
//! which an ill-typed expression cannot be represented. Exact arithmetic
//! (int, duration, count) is bounded here: every intermediate result is
//! proven to stay below 2^127 in magnitude for any input of its type, or
//! the expression is rejected, so evaluation in `i128` cannot overflow.

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use taktsim_units::SimDuration;

use super::ast::{BinOp, Call, CmpOp, Expr, ExprKind, List, Path, Quantity};
use super::error::{ErrorKind, ExprError, Operator, suggest};
use super::eval::{Condition, Input};
use super::ir::{
    BoolIr, CountD, EnumValues, EqOp, ExactIr, FloatIr, IntD, NumberD, OrdOp, RateD, RatioD,
    TextIr, TimeD,
};
use super::parser::parse;
use super::span::Span;
use super::types::{Dimension, Expected, Type, TypeEnv};

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
        return Err(ExprError::new(
            ErrorKind::WrongResultType {
                expected,
                found: ty,
            },
            expr.span,
        ));
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
    Condition::new(&parse(source)?, env)
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
        let (lowered, inputs) = lower(expr, env)?;
        if let Lowered::Bool(ir) = lowered {
            Ok(Self {
                expr: expr.clone(),
                inputs,
                ir,
            })
        } else {
            Err(ExprError::new(
                ErrorKind::WrongResultType {
                    expected: Expected::Bool,
                    found: lowered.ty(),
                },
                expr.span,
            ))
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum TextTy {
    String,
    Enum(String),
}

impl TextTy {
    fn ty(&self) -> Type {
        match self {
            Self::String => Type::String,
            Self::Enum(name) => Type::Enum(name.clone()),
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
    if let ExprKind::Int(n) = e.kind {
        Some(n.to_string())
    } else if let ExprKind::Number(x) = e.kind {
        Some(x.to_string())
    } else {
        None
    }
}

/// Operands of different types. With `unit_hint`, a bare number next to a
/// quantity gets the more helpful [`ErrorKind::MissingUnit`].
fn mismatch(op: Operator, l: &Side<'_>, r: &Side<'_>, span: Span, unit_hint: bool) -> ExprError {
    if unit_hint {
        for (quantity, literal) in [(l, r), (r, l)] {
            if let (Some(dimension), Some(text)) = (quantity.dimension, bare_number(literal.expr)) {
                return ExprError::new(
                    ErrorKind::MissingUnit {
                        literal: text,
                        dimension,
                    },
                    literal.expr.span,
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

fn not_defined(op: Operator, x: &Lowered, e: &Expr) -> ExprError {
    ExprError::new(ErrorKind::NotDefinedFor { op, found: x.ty() }, e.span)
}

fn boolean(x: Lowered, e: &Expr, op: Operator) -> Result<BoolIr, ExprError> {
    if let Lowered::Bool(b) = x {
        Ok(b)
    } else {
        Err(not_defined(op, &x, e))
    }
}

/// Rejects operands that arithmetic and ordering never apply to.
fn numeric(op: Operator, l: &Operand<'_>, r: &Operand<'_>) -> Result<(), ExprError> {
    for (x, e) in [l, r] {
        if !x.is_numeric() {
            return Err(not_defined(op, x, e));
        }
    }
    Ok(())
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

fn additive(op: BinOp, l: Operand<'_>, r: Operand<'_>, span: Span) -> Result<Lowered, ExprError> {
    let operator = Operator::from(op);
    numeric(operator, &l, &r)?;
    let sub = op == BinOp::Sub;
    let ((lx, le), (rx, re)) = (l, r);
    Ok(match (lx, rx) {
        (Lowered::Int(a), Lowered::Int(b)) => Lowered::Int(sum(sub, a, b, span)?),
        (Lowered::Time(a), Lowered::Time(b)) => Lowered::Time(sum(sub, a, b, span)?),
        (Lowered::Count(a), Lowered::Count(b)) => Lowered::Count(sum(sub, a, b, span)?),
        (Lowered::Number(a), Lowered::Number(b)) => Lowered::Number(float_sum(sub, a, b)),
        (Lowered::Rate(a), Lowered::Rate(b)) => Lowered::Rate(float_sum(sub, a, b)),
        (Lowered::Ratio(a), Lowered::Ratio(b)) => Lowered::Ratio(float_sum(sub, a, b)),
        (lx, rx) => {
            let (l, r) = (Side::of(&lx, le), Side::of(&rx, re));
            return Err(mismatch(operator, &l, &r, span, true));
        }
    })
}

/// `*`: the same scalar type on both sides, or a quantity scaled by its
/// natural factor (an int for exact quantities, a number for the others).
fn multiply(l: Operand<'_>, r: Operand<'_>, span: Span) -> Result<Lowered, ExprError> {
    numeric(Operator::Mul, &l, &r)?;
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
        match &e.kind {
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

    fn is_zero(&self) -> bool {
        // Literals are never negative, so `<= 0.0` means zero.
        match self {
            Self::Int(n) => *n == 0,
            Self::Number(x) => *x <= 0.0,
            Self::Quantity(Quantity::Time(d)) => *d == SimDuration::ZERO,
            Self::Quantity(Quantity::Count(c)) => c.get() == 0,
            Self::Quantity(Quantity::Rate(r)) => r.as_per_second() <= 0.0,
            Self::Quantity(Quantity::Ratio(r)) => r.as_percent() <= 0.0,
        }
    }
}

/// `/`: by a non-zero literal only (ADR-0009, v0.1).
fn divide(l: Operand<'_>, r: Operand<'_>, span: Span) -> Result<Lowered, ExprError> {
    let ((lx, le), (rx, re)) = (l, r);
    if !lx.is_numeric() {
        return Err(not_defined(Operator::Div, &lx, le));
    }
    let divisor =
        Divisor::of(re).ok_or_else(|| ExprError::new(ErrorKind::DivisorNotLiteral, re.span))?;
    if divisor.is_zero() {
        return Err(ExprError::new(ErrorKind::DivisionByZero, re.span));
    }
    Ok(match (lx, divisor) {
        (Lowered::Number(a), Divisor::Number(k)) => Lowered::Number(FloatIr::Div(Box::new(a), k)),
        (Lowered::Rate(a), Divisor::Number(k)) => Lowered::Rate(FloatIr::Div(Box::new(a), k)),
        (Lowered::Ratio(a), Divisor::Number(k)) => Lowered::Ratio(FloatIr::Div(Box::new(a), k)),
        (Lowered::Count(n), Divisor::Quantity(Quantity::Time(d))) => {
            Lowered::Rate(FloatIr::PerTime(Box::new(n.ir), d))
        }
        (lx, _) => {
            return Err(ExprError::new(
                ErrorKind::DivisionNotAllowed {
                    left: lx.ty(),
                    right: rx.ty(),
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
) -> Result<Lowered, ExprError> {
    numeric(operator, &l, &r)?;
    let ((lx, le), (rx, re)) = (l, r);
    let ir = match (lx, rx) {
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
    };
    Ok(Lowered::Bool(ir))
}

/// The elements of an `in` list, each of which `take` accepts or returns.
fn elements<T>(
    items: Vec<Operand<'_>>,
    left: &Side<'_>,
    take: impl Fn(Lowered) -> Result<T, Lowered>,
) -> Result<Vec<T>, ExprError> {
    items
        .into_iter()
        .map(|(x, e)| {
            take(x).map_err(|x| mismatch(Operator::In, left, &Side::of(&x, e), e.span, true))
        })
        .collect()
}

struct Checker<'e> {
    env: &'e TypeEnv,
    inputs: BTreeMap<Input, Type>,
}

impl Checker<'_> {
    fn lower(&mut self, expr: &Expr) -> Result<Lowered, ExprError> {
        Ok(match &expr.kind {
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
            ExprKind::Path(p) => self.path(p, expr.span)?,
            ExprKind::Call(c) => self.call(c, expr.span)?,
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
            ExprKind::Binary { op, lhs, rhs } => {
                let left = (self.lower(lhs)?, &**lhs);
                let right = (self.lower(rhs)?, &**rhs);
                match op {
                    BinOp::Or | BinOp::And => {
                        let a = Box::new(boolean(left.0, left.1, Operator::from(*op))?);
                        let b = Box::new(boolean(right.0, right.1, Operator::from(*op))?);
                        Lowered::Bool(if *op == BinOp::Or {
                            BoolIr::Or(a, b)
                        } else {
                            BoolIr::And(a, b)
                        })
                    }
                    BinOp::Add | BinOp::Sub => additive(*op, left, right, expr.span)?,
                    BinOp::Mul => multiply(left, right, expr.span)?,
                    BinOp::Div => divide(left, right, expr.span)?,
                }
            }
            ExprKind::Compare { op, lhs, rhs } => {
                let left = (self.lower(lhs)?, &**lhs);
                let right = (self.lower(rhs)?, &**rhs);
                self.compare(*op, left, right, expr.span)?
            }
            ExprKind::In { lhs, list } => self.membership(lhs, list, expr.span)?,
        })
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
                let values = self.env.enum_values(name).cloned().unwrap_or_default();
                let enumeration = EnumValues {
                    name: name.clone(),
                    values,
                };
                Lowered::Text(TextSide::Typed(
                    TextIr::Input(input, Some(enumeration)),
                    TextTy::Enum(name.clone()),
                ))
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
        Ok(self.input(Input::Path(path.clone()), ty))
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
        if !signature.arity().allows(call.args.len()) {
            return Err(ExprError::new(
                ErrorKind::ArgumentCount {
                    function: name(),
                    arity: signature.arity(),
                    found: call.args.len(),
                },
                span,
            ));
        }
        let mut seen = BTreeSet::new();
        for arg in &call.args {
            let id = &arg.value;
            let error = |kind| Err(ExprError::new(kind, arg.span));
            let Some(kind) = env.component_kind(id) else {
                let suggestion = suggest(id.as_str(), env.component_ids()).map(str::to_owned);
                return error(ErrorKind::UnknownComponent {
                    function: name(),
                    id: id.as_str().to_owned(),
                    suggestion,
                });
            };
            if let Some(accepted) = signature.accepts()
                && !accepted.contains(kind)
            {
                return error(ErrorKind::ComponentNotAccepted {
                    function: name(),
                    id: id.as_str().to_owned(),
                    kind: kind.to_owned(),
                    accepted: accepted.iter().cloned().collect(),
                });
            }
            if !seen.insert(id) {
                return error(ErrorKind::DuplicateArgument {
                    function: name(),
                    id: id.as_str().to_owned(),
                });
            }
        }
        let input = Input::Call {
            function: function.clone(),
            args: call.args.iter().map(|a| a.value.clone()).collect(),
        };
        Ok(self.input(input, signature.result()))
    }

    fn compare(
        &self,
        op: CmpOp,
        l: Operand<'_>,
        r: Operand<'_>,
        span: Span,
    ) -> Result<Lowered, ExprError> {
        let operator = Operator::from(op);
        match op {
            CmpOp::Eq => self.equality(EqOp::Eq, operator, l, r, span),
            CmpOp::Ne => self.equality(EqOp::Ne, operator, l, r, span),
            CmpOp::Lt => ordering(OrdOp::Lt, operator, l, r, span),
            CmpOp::Le => ordering(OrdOp::Le, operator, l, r, span),
            CmpOp::Gt => ordering(OrdOp::Gt, operator, l, r, span),
            CmpOp::Ge => ordering(OrdOp::Ge, operator, l, r, span),
        }
    }

    /// `==` and `!=`: only on types whose values compare exactly
    /// (ADR-0009).
    fn equality(
        &self,
        op: EqOp,
        operator: Operator,
        l: Operand<'_>,
        r: Operand<'_>,
        span: Span,
    ) -> Result<Lowered, ExprError> {
        let ((lx, le), (rx, re)) = (l, r);
        let ir = match (lx, rx) {
            (Lowered::Bool(a), Lowered::Bool(b)) => BoolIr::EqBool(op, Box::new(a), Box::new(b)),
            (Lowered::Int(a), Lowered::Int(b)) => BoolIr::EqInt(op, Box::new(a.ir), Box::new(b.ir)),
            (Lowered::Time(a), Lowered::Time(b)) => {
                BoolIr::EqTime(op, Box::new(a.ir), Box::new(b.ir))
            }
            (lx @ Lowered::Count(_), Lowered::Count(_))
            | (lx @ Lowered::Number(_), Lowered::Number(_))
            | (lx @ Lowered::Rate(_), Lowered::Rate(_))
            | (lx @ Lowered::Ratio(_), Lowered::Ratio(_)) => {
                return Err(ExprError::new(
                    ErrorKind::EqualityNotAllowed {
                        op: operator,
                        ty: lx.ty(),
                    },
                    span,
                ));
            }
            (Lowered::Text(a), Lowered::Text(b)) => {
                let b = self.text_element(operator, (&a, le), (b, re), span)?;
                BoolIr::EqText(op, Box::new(a.into_ir()), Box::new(b))
            }
            (lx, rx) => {
                let (l, r) = (Side::of(&lx, le), Side::of(&rx, re));
                return Err(mismatch(operator, &l, &r, span, true));
            }
        };
        Ok(Lowered::Bool(ir))
    }

    /// The right side `item` of a string comparison with `left`, checked
    /// against it. Typed sides must have the same type; a literal must be a
    /// value of the other side's enum.
    fn text_element(
        &self,
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
                self.fits(lt, &s, item_expr.span)?;
                Ok(TextIr::Const(s))
            }
            (TextSide::Literal(s), TextSide::Typed(ir, it)) => {
                self.fits(&it, s, left_expr.span)?;
                Ok(ir)
            }
            (TextSide::Literal(_), TextSide::Literal(s)) => Ok(TextIr::Const(s)),
        }
    }

    /// Checks that the string literal `value` is allowed where `ty` is.
    fn fits(&self, ty: &TextTy, value: &str, span: Span) -> Result<(), ExprError> {
        let TextTy::Enum(name) = ty else {
            return Ok(());
        };
        let empty = BTreeSet::new();
        let values = self.env.enum_values(name).unwrap_or(&empty);
        if values.contains(value) {
            return Ok(());
        }
        let suggestion = suggest(value, values.iter().map(String::as_str)).map(str::to_owned);
        Err(ExprError::new(
            ErrorKind::UnknownEnumValue {
                enumeration: name.clone(),
                value: value.to_owned(),
                allowed: values.iter().cloned().collect(),
                suggestion,
            },
            span,
        ))
    }

    /// `x in [a, b, ...]`: `x == a or x == b or ...`, with the same rules
    /// as `==` for every element.
    fn membership(&mut self, lhs: &Expr, list: &List, span: Span) -> Result<Lowered, ExprError> {
        let l = self.lower(lhs)?;
        if list.items.is_empty() {
            return Err(ExprError::new(ErrorKind::EmptyList, list.span));
        }
        let mut items = Vec::with_capacity(list.items.len());
        for item in &list.items {
            items.push((self.lower(item)?, item));
        }
        let left = Side::of(&l, lhs);
        let ir = match l {
            Lowered::Bool(a) => BoolIr::InBool(
                Box::new(a),
                elements(items, &left, |x| {
                    if let Lowered::Bool(b) = x {
                        Ok(b)
                    } else {
                        Err(x)
                    }
                })?,
            ),
            Lowered::Int(a) => BoolIr::InInt(
                Box::new(a.ir),
                elements(items, &left, |x| {
                    if let Lowered::Int(b) = x {
                        Ok(b.ir)
                    } else {
                        Err(x)
                    }
                })?,
            ),
            Lowered::Time(a) => BoolIr::InTime(
                Box::new(a.ir),
                elements(items, &left, |x| {
                    if let Lowered::Time(b) = x {
                        Ok(b.ir)
                    } else {
                        Err(x)
                    }
                })?,
            ),
            Lowered::Count(_) | Lowered::Number(_) | Lowered::Rate(_) | Lowered::Ratio(_) => {
                return Err(ExprError::new(
                    ErrorKind::EqualityNotAllowed {
                        op: Operator::In,
                        ty: left.ty,
                    },
                    span,
                ));
            }
            Lowered::Text(a) => {
                let mut texts = Vec::with_capacity(items.len());
                for (x, e) in items {
                    let Lowered::Text(item) = x else {
                        return Err(mismatch(
                            Operator::In,
                            &left,
                            &Side::of(&x, e),
                            e.span,
                            true,
                        ));
                    };
                    texts.push(self.text_element(Operator::In, (&a, lhs), (item, e), e.span)?);
                }
                BoolIr::InText(Box::new(a.into_ir()), texts)
            }
        };
        Ok(Lowered::Bool(ir))
    }
}
