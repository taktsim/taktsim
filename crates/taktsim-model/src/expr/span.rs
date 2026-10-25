//! Byte ranges in the expression source.

use std::fmt;
use std::ops::Range;

/// A byte range `start..end` in the source text of an expression.
///
/// Every node of the syntax tree and every error carries one, so a message
/// can point at the exact text it is about. Both ends lie on character
/// boundaries of the source the span came from.
///
/// ```
/// use taktsim_model::expr::{Span, parse};
///
/// let expr = parse("variant == 'premium'")?;
/// assert_eq!(expr.span, Span::new(0, 20));
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    /// The first byte.
    pub start: usize,
    /// One past the last byte.
    pub end: usize,
}

impl Span {
    /// The span `start..end`.
    ///
    /// ```
    /// use taktsim_model::expr::Span;
    ///
    /// assert_eq!(Span::new(2, 5).range(), 2..5);
    /// ```
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// The span from the start of `self` to the end of `other`.
    ///
    /// ```
    /// use taktsim_model::expr::Span;
    ///
    /// assert_eq!(Span::new(0, 3).to(Span::new(7, 9)), Span::new(0, 9));
    /// ```
    #[must_use]
    pub const fn to(self, other: Self) -> Self {
        Self::new(self.start, other.end)
    }

    /// The span as a range, for slicing the source.
    ///
    /// ```
    /// use taktsim_model::expr::Span;
    ///
    /// assert_eq!(&"a == b"[Span::new(2, 4).range()], "==");
    /// ```
    #[must_use]
    pub const fn range(self) -> Range<usize> {
        self.start..self.end
    }
}

impl fmt::Display for Span {
    /// `start..end`, in bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A value together with the span of source text it came from.
///
/// ```
/// use taktsim_model::expr::{ExprKind, Span, parse};
///
/// let expr = parse("wip(buf1)")?;
/// let ExprKind::Call(call) = expr.kind else { unreachable!() };
/// assert_eq!(call.args[0].value.as_str(), "buf1");
/// assert_eq!(call.args[0].span, Span::new(4, 8));
/// # Ok::<(), taktsim_model::expr::ExprError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Spanned<T> {
    /// The value.
    pub value: T,
    /// Where it was written.
    pub span: Span,
}
