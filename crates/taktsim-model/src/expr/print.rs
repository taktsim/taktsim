//! Canonical text: the pretty-printer (ADR-0015, "Canonical text").
//!
//! Binary operators are surrounded by single spaces, list and argument
//! separators are `, `, quantities are written as `taktsim-units` displays
//! them, and parentheses appear only where the grammar needs them. Parsing
//! the output gives back the same tree, apart from spans.

use std::fmt;

use super::ast::{
    BinOp, Call, CmpOp, ComponentId, Expr, ExprKind, Ident, Number, Path, Quantity, Text,
};

/// Binding strength, weakest first. An operand is parenthesized when its
/// own level is below the level its position needs.
const OR: u8 = 1;
const AND: u8 = 2;
const NOT: u8 = 3;
const CMP: u8 = 4;
const SUM: u8 = 5;
const TERM: u8 = 6;
const UNARY: u8 = 7;
const ATOM: u8 = 8;

const fn binary_level(op: BinOp) -> u8 {
    match op {
        BinOp::Or => OR,
        BinOp::And => AND,
        BinOp::Add | BinOp::Sub => SUM,
        BinOp::Mul | BinOp::Div => TERM,
    }
}

const fn level(kind: &ExprKind) -> u8 {
    match kind {
        ExprKind::Binary { op, .. } => binary_level(*op),
        ExprKind::Not(_) => NOT,
        ExprKind::Compare { .. } | ExprKind::In { .. } => CMP,
        ExprKind::Neg(_) => UNARY,
        ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Number(_)
        | ExprKind::Quantity(_)
        | ExprKind::Str(_)
        | ExprKind::Path(_)
        | ExprKind::Call(_) => ATOM,
    }
}

fn operand(f: &mut fmt::Formatter<'_>, e: &Expr, needed: u8) -> fmt::Result {
    if level(&e.kind) < needed {
        write!(f, "({e})")
    } else {
        write!(f, "{e}")
    }
}

impl fmt::Display for Expr {
    /// The canonical text of the expression.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExprKind::Bool(b) => write!(f, "{b}"),
            ExprKind::Int(n) => write!(f, "{n}"),
            ExprKind::Number(x) => write!(f, "{x}"),
            ExprKind::Quantity(q) => write!(f, "{q}"),
            ExprKind::Str(t) => write!(f, "'{t}'"),
            ExprKind::Path(p) => write!(f, "{p}"),
            ExprKind::Call(c) => write!(f, "{c}"),
            ExprKind::Neg(x) => {
                f.write_str("-")?;
                operand(f, x, ATOM)
            }
            ExprKind::Not(x) => {
                f.write_str("not ")?;
                operand(f, x, CMP)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let level = binary_level(*op);
                operand(f, lhs, level)?;
                write!(f, " {op} ")?;
                operand(f, rhs, level + 1)
            }
            ExprKind::Compare { op, lhs, rhs } => {
                operand(f, lhs, SUM)?;
                write!(f, " {op} ")?;
                operand(f, rhs, SUM)
            }
            ExprKind::In { lhs, list } => {
                operand(f, lhs, SUM)?;
                f.write_str(" in [")?;
                for (i, item) in list.items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    operand(f, item, ATOM)?;
                }
                f.write_str("]")
            }
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Or => "or",
            Self::And => "and",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        })
    }
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        })
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Text {
    /// The content, without quotes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Number {
    /// The shortest text that parses back to the same value, always with a
    /// decimal point (`2.0`, not `2`, which would be an `int`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.0.to_string();
        if text.contains('.') {
            f.write_str(&text)
        } else {
            write!(f, "{text}.0")
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, segment) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(segment.as_str())?;
        }
        Ok(())
    }
}

impl fmt::Display for Quantity {
    /// As `taktsim-units` displays it: `90min`, `40 parts`,
    /// `0.5 parts/s`, `2%`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Time(d) => d.fmt(f),
            Self::Count(c) => c.fmt(f),
            Self::Rate(r) => r.fmt(f),
            Self::Ratio(r) => r.fmt(f),
        }
    }
}

impl fmt::Display for Call {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.name.value)?;
        for (i, arg) in self.args.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            f.write_str(arg.value.as_str())?;
        }
        f.write_str(")")
    }
}
