//! The typed form the checker lowers a syntax tree to, and the evaluator
//! runs. Every operation here is well-typed by construction: the dimension
//! of each exact or floating-point subexpression is a type parameter, so an
//! ill-typed tree cannot be represented.

use std::marker::PhantomData;
use std::sync::Arc;

use taktsim_units::SimDuration;

use super::eval::Input;
use super::types::Enumeration;

/// Dimension markers. Exact dimensions are evaluated as `i128`, the others
/// as `f64` (ADR-0015, "Evaluation").
#[derive(Debug, Clone, Copy)]
pub(crate) struct IntD;
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimeD;
#[derive(Debug, Clone, Copy)]
pub(crate) struct CountD;
#[derive(Debug, Clone, Copy)]
pub(crate) struct NumberD;
#[derive(Debug, Clone, Copy)]
pub(crate) struct RateD;
#[derive(Debug, Clone, Copy)]
pub(crate) struct RatioD;

/// An exact subexpression of dimension `D`: int, duration (nanoseconds) or
/// count (parts).
#[derive(Debug, Clone)]
pub(crate) enum ExactIr<D> {
    Const(i128),
    Input(Input, PhantomData<D>),
    Neg(Box<Self>),
    Add(Box<Self>, Box<Self>),
    Sub(Box<Self>, Box<Self>),
    /// Scaling by an int (for `D = IntD`, plain multiplication).
    Mul(Box<Self>, Box<ExactIr<IntD>>),
}

/// A floating-point subexpression of dimension `D`: number, rate (per
/// second) or ratio (percent).
#[derive(Debug, Clone)]
pub(crate) enum FloatIr<D> {
    Const(f64),
    Input(Input, PhantomData<D>),
    Neg(Box<Self>),
    Add(Box<Self>, Box<Self>),
    Sub(Box<Self>, Box<Self>),
    /// Scaling by a number (for `D = NumberD`, plain multiplication).
    Mul(Box<Self>, Box<FloatIr<NumberD>>),
    /// Division by a non-zero literal.
    Div(Box<Self>, f64),
    /// A count divided by a non-zero duration literal: a rate.
    PerTime(Box<ExactIr<CountD>>, SimDuration),
}

/// A string or enum subexpression.
#[derive(Debug, Clone)]
pub(crate) enum TextIr {
    Const(String),
    /// An input and, for an enum, its enumeration, to check values against.
    Input(Input, Option<Arc<Enumeration>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EqOp {
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrdOp {
    Lt,
    Le,
    Gt,
    Ge,
}

/// A boolean expression. Equality exists only for the types ADR-0009 and
/// ADR-0015 allow it on; ordering only for numeric types.
#[derive(Debug, Clone)]
pub(crate) enum BoolIr {
    Const(bool),
    Input(Input),
    Not(Box<Self>),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    EqBool(EqOp, Box<Self>, Box<Self>),
    EqText(EqOp, Box<TextIr>, Box<TextIr>),
    EqInt(EqOp, Box<ExactIr<IntD>>, Box<ExactIr<IntD>>),
    EqTime(EqOp, Box<ExactIr<TimeD>>, Box<ExactIr<TimeD>>),
    EqCount(EqOp, Box<ExactIr<CountD>>, Box<ExactIr<CountD>>),
    OrdInt(OrdOp, Box<ExactIr<IntD>>, Box<ExactIr<IntD>>),
    OrdTime(OrdOp, Box<ExactIr<TimeD>>, Box<ExactIr<TimeD>>),
    OrdCount(OrdOp, Box<ExactIr<CountD>>, Box<ExactIr<CountD>>),
    OrdNumber(OrdOp, Box<FloatIr<NumberD>>, Box<FloatIr<NumberD>>),
    OrdRate(OrdOp, Box<FloatIr<RateD>>, Box<FloatIr<RateD>>),
    OrdRatio(OrdOp, Box<FloatIr<RatioD>>, Box<FloatIr<RatioD>>),
    InBool(Box<Self>, Vec<Self>),
    InText(Box<TextIr>, Vec<TextIr>),
    InInt(Box<ExactIr<IntD>>, Vec<ExactIr<IntD>>),
    InTime(Box<ExactIr<TimeD>>, Vec<ExactIr<TimeD>>),
    InCount(Box<ExactIr<CountD>>, Vec<ExactIr<CountD>>),
}
