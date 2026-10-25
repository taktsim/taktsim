//! The syntax tree produced by the parser.
//!
//! Leaf types are validated newtypes, so every tree that can be built can
//! also be printed and parsed back (ADR-0015, "Canonical text").

use taktsim_units::{Count, Rate, Ratio, SimDuration};

use super::span::{Span, Spanned};
use super::types::Dimension;

/// The reserved words of the language. They cannot be names.
pub(crate) const KEYWORDS: [&str; 6] = ["and", "or", "not", "in", "true", "false"];

pub(crate) fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub(crate) fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `[A-Za-z_][A-Za-z0-9_]*`, keywords included.
fn is_word(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(is_ident_start) && bytes.all(is_ident_char)
}

/// A name: a path segment or a function name.
///
/// ASCII letters, digits and `_`, not starting with a digit, and not one of
/// the keywords `and`, `or`, `not`, `in`, `true`, `false`.
///
/// ```
/// use taktsim_model::expr::Ident;
///
/// assert_eq!(Ident::new("buf1").map(|i| i.to_string()), Some("buf1".to_owned()));
/// assert!(Ident::new("in").is_none());
/// assert!(Ident::new("weld-1").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ident(pub(crate) String);

impl Ident {
    /// `name` as an identifier, or `None` if it is not one.
    ///
    /// ```
    /// use taktsim_model::expr::Ident;
    ///
    /// assert!(Ident::new("variant").is_some());
    /// assert!(Ident::new("2x").is_none());
    /// ```
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        (is_word(name) && !KEYWORDS.contains(&name)).then(|| Self(name.to_owned()))
    }

    /// The name.
    ///
    /// ```
    /// use taktsim_model::expr::Ident;
    ///
    /// assert_eq!(Ident::new("sink").unwrap().as_str(), "sink");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A component id as written in a call argument, e.g. `buf1` or `weld-1`.
///
/// An identifier, optionally followed by `-` and more letters, digits or
/// `_`: `[A-Za-z_][A-Za-z0-9_]*(-[A-Za-z0-9_]+)*`. Call arguments are not
/// expressions, so the hyphen cannot be read as a minus (ADR-0015).
///
/// ```
/// use taktsim_model::expr::ComponentId;
///
/// assert!(ComponentId::new("weld-1").is_some());
/// assert!(ComponentId::new("weld-").is_none());
/// assert!(ComponentId::new("1st").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(pub(crate) String);

impl ComponentId {
    /// `id` as a component id, or `None` if it does not have that form.
    ///
    /// ```
    /// use taktsim_model::expr::ComponentId;
    ///
    /// assert_eq!(ComponentId::new("frame-assembly").unwrap().as_str(), "frame-assembly");
    /// ```
    #[must_use]
    pub fn new(id: &str) -> Option<Self> {
        let mut parts = id.split('-');
        let first_ok = parts.next().is_some_and(is_word);
        let rest_ok = parts.all(|p| !p.is_empty() && p.bytes().all(is_ident_char));
        (first_ok && rest_ok).then(|| Self(id.to_owned()))
    }

    /// The id.
    ///
    /// ```
    /// use taktsim_model::expr::ComponentId;
    ///
    /// assert_eq!(ComponentId::new("buf1").unwrap().as_str(), "buf1");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The content of a string literal, without its quotes.
///
/// Any text without a single quote `'`; there are no escapes (ADR-0015).
///
/// ```
/// use taktsim_model::expr::Text;
///
/// assert_eq!(Text::new("premium").unwrap().as_str(), "premium");
/// assert!(Text::new("it's").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Text(pub(crate) String);

impl Text {
    /// `text` as a string literal's content, or `None` if it contains `'`.
    ///
    /// ```
    /// use taktsim_model::expr::Text;
    ///
    /// assert!(Text::new("").is_some());
    /// ```
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        (!text.contains('\'')).then(|| Self(text.to_owned()))
    }

    /// The content.
    ///
    /// ```
    /// use taktsim_model::expr::Text;
    ///
    /// assert_eq!(Text::new("ok").unwrap().as_str(), "ok");
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A `number` literal such as `0.5`: finite and not negative.
///
/// Literals carry no sign; `-0.5` is a negation of `0.5`.
///
/// ```
/// use taktsim_model::expr::Number;
///
/// assert_eq!(Number::new(0.5).map(Number::get), Some(0.5));
/// assert!(Number::new(-1.0).is_none());
/// assert!(Number::new(f64::INFINITY).is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Number(pub(crate) f64);

impl Number {
    /// `value` as a literal, or `None` if it is negative (also `-0.0`), NaN
    /// or infinite.
    ///
    /// ```
    /// use taktsim_model::expr::Number;
    ///
    /// assert!(Number::new(-0.0).is_none());
    /// assert!(Number::new(0.0).is_some());
    /// ```
    #[must_use]
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && value.is_sign_positive()).then_some(Self(value))
    }

    /// The value.
    ///
    /// ```
    /// use taktsim_model::expr::Number;
    ///
    /// assert_eq!(Number::new(2.5).unwrap().get(), 2.5);
    /// ```
    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

/// A dotted name such as `variant` or `unit.priority`, at least one segment.
///
/// ```
/// use taktsim_model::expr::Path;
///
/// let path = Path::parse("buf1.capacity").unwrap();
/// assert_eq!(path.segments().len(), 2);
/// assert_eq!(path.to_string(), "buf1.capacity");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(pub(crate) Vec<Ident>);

impl Path {
    /// A path of `segments`, or `None` if there are none.
    ///
    /// ```
    /// use taktsim_model::expr::{Ident, Path};
    ///
    /// let path = Path::new(vec![Ident::new("unit").unwrap(), Ident::new("age").unwrap()]);
    /// assert_eq!(path.unwrap().to_string(), "unit.age");
    /// assert!(Path::new(Vec::new()).is_none());
    /// ```
    #[must_use]
    pub fn new(segments: Vec<Ident>) -> Option<Self> {
        (!segments.is_empty()).then_some(Self(segments))
    }

    /// `text` split at `.` into identifiers, or `None` if a segment is not
    /// an identifier.
    ///
    /// ```
    /// use taktsim_model::expr::Path;
    ///
    /// assert!(Path::parse("unit.priority").is_some());
    /// assert!(Path::parse("unit..priority").is_none());
    /// assert!(Path::parse("unit.in").is_none());
    /// ```
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        text.split('.')
            .map(Ident::new)
            .collect::<Option<Vec<_>>>()
            .and_then(Self::new)
    }

    /// The segments, never empty.
    ///
    /// ```
    /// use taktsim_model::expr::Path;
    ///
    /// assert_eq!(Path::parse("variant").unwrap().segments()[0].as_str(), "variant");
    /// ```
    #[must_use]
    pub fn segments(&self) -> &[Ident] {
        &self.0
    }
}

/// A quantity literal, parsed by `taktsim-units`: `30s`, `40 parts`,
/// `120 parts/h`, `2%`.
///
/// ```
/// use taktsim_model::expr::{Dimension, ExprKind, Quantity, parse};
///
/// let ExprKind::Quantity(q) = parse("40 parts")?.kind else { unreachable!() };
/// assert_eq!(q, Quantity::Count(taktsim_units::Count::new(40)));
/// assert_eq!(q.dimension(), Dimension::Count);
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Quantity {
    /// A duration, e.g. `30s`.
    Time(SimDuration),
    /// A count, e.g. `40 parts`.
    Count(Count),
    /// A rate, e.g. `120 parts/h`.
    Rate(Rate),
    /// A percentage, e.g. `2%`.
    Ratio(Ratio),
}

impl Quantity {
    /// The dimension of the quantity.
    ///
    /// ```
    /// use taktsim_model::expr::{Dimension, Quantity};
    ///
    /// let q = Quantity::Time("30s".parse()?);
    /// assert_eq!(q.dimension(), Dimension::Time);
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn dimension(&self) -> Dimension {
        match self {
            Self::Time(_) => Dimension::Time,
            Self::Count(_) => Dimension::Count,
            Self::Rate(_) => Dimension::Rate,
            Self::Ratio(_) => Dimension::Ratio,
        }
    }
}

/// A call of a built-in function on component ids, e.g. `throughput(sink)`.
///
/// ```
/// use taktsim_model::expr::{ExprKind, parse};
///
/// let ExprKind::Call(call) = parse("state_shares(cut, weld-1)")?.kind else { unreachable!() };
/// assert_eq!(call.name.value.as_str(), "state_shares");
/// assert_eq!(call.args[1].value.as_str(), "weld-1");
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// The function.
    pub name: Spanned<Ident>,
    /// The component ids it is called on.
    pub args: Vec<Spanned<ComponentId>>,
}

/// A binary operator other than a comparison.
///
/// ```
/// use taktsim_model::expr::{BinOp, ExprKind, parse};
///
/// let ExprKind::Binary { op, .. } = parse("a and b")?.kind else { unreachable!() };
/// assert_eq!(op, BinOp::And);
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinOp {
    /// `or`
    Or,
    /// `and`
    And,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

/// A comparison operator.
///
/// ```
/// use taktsim_model::expr::{CmpOp, ExprKind, parse};
///
/// let ExprKind::Compare { op, .. } = parse("wip(buf1) <= 30 parts")?.kind else { unreachable!() };
/// assert_eq!(op, CmpOp::Le);
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

/// The bracketed list after `in`.
///
/// ```
/// use taktsim_model::expr::{ExprKind, parse};
///
/// let ExprKind::In { list, .. } = parse("variant in ['basic', 'premium']")?.kind else { unreachable!() };
/// assert_eq!(list.items.len(), 2);
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    /// The elements, in order.
    pub items: Vec<Expr>,
    /// The span from `[` to `]`.
    pub span: Span,
}

/// An expression: a node of the syntax tree with its span.
///
/// ```
/// use taktsim_model::expr::{BinOp, ExprKind, parse};
///
/// let expr = parse("a or b and c")?;
/// // `and` binds tighter than `or`.
/// let ExprKind::Binary { op: BinOp::Or, rhs, .. } = expr.kind else { unreachable!() };
/// assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::And, .. }));
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// What the node is.
    pub kind: ExprKind,
    /// The source text it covers, including enclosing parentheses.
    pub span: Span,
}

/// The kinds of expression nodes. Parentheses are not nodes; they only
/// group.
///
/// ```
/// use taktsim_model::expr::{ExprKind, parse};
///
/// assert!(matches!(parse("(true)")?.kind, ExprKind::Bool(true)));
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// `true` or `false`.
    Bool(bool),
    /// An integer literal such as `42`, type `int`.
    Int(u64),
    /// A literal with a decimal point such as `0.5`, type `number`.
    Number(Number),
    /// A quantity literal such as `30s`.
    Quantity(Quantity),
    /// A string literal such as `'premium'`.
    Str(Text),
    /// A name such as `variant` or `unit.priority`.
    Path(Path),
    /// A built-in call such as `throughput(sink)`.
    Call(Call),
    /// `-x`.
    Neg(Box<Expr>),
    /// `not x`.
    Not(Box<Expr>),
    /// `lhs op rhs` for `or`, `and`, `+`, `-`, `*`, `/`.
    Binary {
        /// The operator.
        op: BinOp,
        /// The left operand.
        lhs: Box<Expr>,
        /// The right operand.
        rhs: Box<Expr>,
    },
    /// `lhs op rhs` for a comparison.
    Compare {
        /// The operator.
        op: CmpOp,
        /// The left operand.
        lhs: Box<Expr>,
        /// The right operand.
        rhs: Box<Expr>,
    },
    /// `lhs in [a, b, ...]`.
    In {
        /// The value looked up.
        lhs: Box<Expr>,
        /// The list it is looked up in.
        list: List,
    },
}

impl Expr {
    /// A copy of the tree with every span set to `0..0`, for comparing
    /// trees regardless of where they were written.
    ///
    /// ```
    /// use taktsim_model::expr::parse;
    ///
    /// let a = parse("a  +  b")?;
    /// let b = parse("a + b")?;
    /// assert_ne!(a, b);
    /// assert_eq!(a.without_spans(), b.without_spans());
    /// # Ok::<(), taktsim_model::expr::ExprError>(())
    /// ```
    #[must_use]
    pub fn without_spans(&self) -> Self {
        let erase = |e: &Self| Box::new(e.without_spans());
        let kind = match &self.kind {
            ExprKind::Bool(b) => ExprKind::Bool(*b),
            ExprKind::Int(n) => ExprKind::Int(*n),
            ExprKind::Number(x) => ExprKind::Number(*x),
            ExprKind::Quantity(q) => ExprKind::Quantity(*q),
            ExprKind::Str(t) => ExprKind::Str(t.clone()),
            ExprKind::Path(p) => ExprKind::Path(p.clone()),
            ExprKind::Call(c) => ExprKind::Call(Call {
                name: unspanned(c.name.value.clone()),
                args: c.args.iter().map(|a| unspanned(a.value.clone())).collect(),
            }),
            ExprKind::Neg(x) => ExprKind::Neg(erase(x)),
            ExprKind::Not(x) => ExprKind::Not(erase(x)),
            ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
                op: *op,
                lhs: erase(lhs),
                rhs: erase(rhs),
            },
            ExprKind::Compare { op, lhs, rhs } => ExprKind::Compare {
                op: *op,
                lhs: erase(lhs),
                rhs: erase(rhs),
            },
            ExprKind::In { lhs, list } => ExprKind::In {
                lhs: erase(lhs),
                list: List {
                    items: list.items.iter().map(Self::without_spans).collect(),
                    span: Span::default(),
                },
            },
        };
        Self {
            kind,
            span: Span::default(),
        }
    }
}

fn unspanned<T>(value: T) -> Spanned<T> {
    Spanned {
        value,
        span: Span::default(),
    }
}
