//! The expression language of taktsim models (ADR-0009; details in
//! ADR-0015).
//!
//! One small, typed language serves edge conditions
//! (`when: "variant == 'premium'"`), observable references
//! (`throughput(sink)`) and, later, KPIs, objectives and constraints
//! (`wip(buf1) <= 30 parts`). Each context supplies its own [`TypeEnv`]:
//! which paths exist and their types, which built-in functions and which
//! components.
//!
//! - [`parse`] turns text into a syntax tree ([`Expr`]) whose every node
//!   has a byte [`Span`]. There are no loops, no I/O and no user functions.
//! - [`check`] type-checks a tree against an environment;
//!   [`Condition::new`] (or [`condition`] from text) does the same for a
//!   condition, which can then be evaluated.
//! - [`Condition::eval`] is total: it cannot fail for input values of the
//!   declared types.
//! - [`Expr`]'s [`Display`](std::fmt::Display) is the canonical text;
//!   parsing it gives back the same tree, apart from spans.
//!
//! Every error is found at load time and says where (a [`Span`]), what was
//! found, what is allowed and how to fix it.
//!
//! ```
//! use std::collections::BTreeMap;
//! use taktsim_model::expr::{Type, TypeEnv, Value, condition};
//!
//! let env = TypeEnv::new()
//!     .with_enum("variant", ["basic", "premium"])?
//!     .with_path("variant", Type::Enum("variant".into()))?;
//!
//! let cond = condition("variant == 'premium'", &env)?;
//! let unit = BTreeMap::from([("variant".to_owned(), Value::Text("premium"))]);
//! assert!(cond.eval(&unit)?);
//!
//! let err = condition("variant == 'premum'", &env).unwrap_err();
//! assert_eq!(err.span().range(), 11..19);
//! assert!(err.to_string().ends_with("did you mean 'premium'?"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod ast;
mod check;
mod error;
mod eval;
mod ir;
mod lexer;
mod parser;
mod print;
mod span;
mod types;

pub use ast::{
    BinOp, Call, CmpOp, ComponentId, Expr, ExprKind, Ident, List, Number, Path, Quantity, Text,
};
pub use check::{Checked, check, condition};
pub use error::{ErrorKind, Expectation, ExprError, Hint, Operator};
pub use eval::{Condition, EvalError, Input, InputKind, Value, Values};
pub use parser::parse;
pub use span::{Span, Spanned};
pub use types::{Arity, Dimension, EnvError, Expected, Signature, Type, TypeEnv};

/// The deepest nesting an expression may have: levels of the syntax tree,
/// and open parentheses in its text. [`Expr::new`] refuses a deeper tree,
/// so every pass over a tree (checking, printing, evaluating, dropping)
/// runs within a small, fixed stack, and every tree that exists prints to
/// text that parses back (ADR-0015).
///
/// ```
/// use taktsim_model::expr::{ErrorKind, MAX_DEPTH, parse};
///
/// let deep = format!("{}x{}", "(".repeat(MAX_DEPTH + 1), ")".repeat(MAX_DEPTH + 1));
/// assert!(matches!(parse(&deep).unwrap_err().kind(), ErrorKind::TooDeep { .. }));
/// ```
pub const MAX_DEPTH: usize = 64;
