//! Load-time errors: what was found, what is allowed, and the fix
//! (spec §5, Contracts).

use std::fmt;

use taktsim_units::UnitError;

use super::ast::{BinOp, CmpOp};
use super::lexer::units;
use super::span::Span;
use super::types::{Arity, Dimension, Expected, Type};

/// An operator, as named in error messages.
///
/// ```
/// use taktsim_model::expr::Operator;
///
/// assert_eq!(Operator::Le.to_string(), "<=");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operator {
    /// `or`
    Or,
    /// `and`
    And,
    /// `not`
    Not,
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
    /// `in`
    In,
    /// `+`
    Add,
    /// binary `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// unary `-`
    Neg,
}

impl Operator {
    /// The operand types the operator takes, for error messages.
    ///
    /// ```
    /// use taktsim_model::expr::Operator;
    ///
    /// assert_eq!(Operator::And.operands(), "bool operands");
    /// ```
    #[must_use]
    pub const fn operands(self) -> &'static str {
        match self {
            Self::Or | Self::And | Self::Not => "bool operands",
            Self::Eq | Self::Ne | Self::In => {
                "two values of the same type: int, duration, bool, string or the same enum"
            }
            Self::Lt | Self::Le | Self::Gt | Self::Ge | Self::Add | Self::Sub => {
                "two values of the same type: int, number, or quantities of the same dimension"
            }
            Self::Mul => {
                "int * int, number * number, a duration or count times an int, \
                 or a rate or ratio times a number"
            }
            Self::Div => {
                "a number, rate or ratio divided by a number literal (`x / 2.0`), \
                 or a count divided by a duration literal (`n / 1h`, giving a rate)"
            }
            Self::Neg => "an int or a number (quantities are never negative)",
        }
    }
}

impl From<BinOp> for Operator {
    fn from(op: BinOp) -> Self {
        match op {
            BinOp::Or => Self::Or,
            BinOp::And => Self::And,
            BinOp::Add => Self::Add,
            BinOp::Sub => Self::Sub,
            BinOp::Mul => Self::Mul,
            BinOp::Div => Self::Div,
        }
    }
}

impl From<CmpOp> for Operator {
    fn from(op: CmpOp) -> Self {
        match op {
            CmpOp::Eq => Self::Eq,
            CmpOp::Ne => Self::Ne,
            CmpOp::Lt => Self::Lt,
            CmpOp::Le => Self::Le,
            CmpOp::Gt => Self::Gt,
            CmpOp::Ge => Self::Ge,
        }
    }
}

impl fmt::Display for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Or => "or",
            Self::And => "and",
            Self::Not => "not",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::In => "in",
            Self::Add => "+",
            Self::Sub | Self::Neg => "-",
            Self::Mul => "*",
            Self::Div => "/",
        })
    }
}

/// What the parser expected where it found something else.
///
/// ```
/// use taktsim_model::expr::{ErrorKind, Expectation, parse};
///
/// let err = parse("(a").unwrap_err();
/// assert!(matches!(
///     err.kind(),
///     ErrorKind::Expected { expected: Expectation::CloseParen, .. }
/// ));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Expectation {
    /// An operand.
    Expression,
    /// An element of a list after `in`.
    ListElement,
    /// An operator, or nothing more.
    OperatorOrEnd,
    /// `)` closing a parenthesis.
    CloseParen,
    /// `,` or `]` in a list.
    CommaOrBracket,
    /// `,` or `)` in a call.
    CommaOrParen,
    /// A component id in a call.
    ComponentId,
    /// A name after `.` in a path.
    Name,
    /// `[` starting the list after `in`.
    List,
}

impl fmt::Display for Expectation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Expression => "an expression such as `unit.age > 30s`",
            Self::ListElement => {
                "a list element (a literal, name, call or parenthesized expression) such as `'premium'`"
            }
            Self::OperatorOrEnd => "an operator such as `and` or `==`, or the end of the expression",
            Self::CloseParen => "`)` closing the `(`",
            Self::CommaOrBracket => "`,` or `]`, as in `[1, 2]`",
            Self::CommaOrParen => "`,` or `)`, as in `state_shares(cut, weld-1)`",
            Self::ComponentId => "a component id such as `buf1`",
            Self::Name => "a name after `.`, as in `unit.priority`",
            Self::List => "a list such as `['a', 'b']` after `in`",
        })
    }
}

/// A fix for a common mistake, appended to a syntax error.
///
/// ```
/// use taktsim_model::expr::{ErrorKind, Hint, parse};
///
/// let err = parse("a && b").unwrap_err();
/// assert!(matches!(err.kind(), ErrorKind::UnexpectedText { hint: Some(Hint::UseAnd), .. }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hint {
    /// `&&` or `&` was written.
    UseAnd,
    /// `||` or `|` was written.
    UseOr,
    /// `!` was written.
    UseNot,
    /// `=` was written.
    UseEq,
    /// `<>` was written.
    UseNe,
    /// `=<` was written.
    UseLe,
    /// `=>` was written.
    UseGe,
    /// A string in double quotes.
    SingleQuotes,
    /// `%` used as an operator.
    NoModulo,
    /// A number such as `.5`.
    LeadingDot,
    /// A unit separated from its number by more than one space.
    UnitSpacing,
    /// `not` where an operand is expected.
    ParenthesizeNot,
    /// A comma before a closing bracket.
    TrailingComma,
    /// An expression as a list element.
    ParenthesizeElement,
    /// An expression as a call argument.
    ArgsAreIds,
    /// A `)` without a matching `(`.
    UnmatchedParen,
    /// A list outside `in`.
    ListOnlyAfterIn,
}

impl fmt::Display for Hint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UseAnd => f.write_str("write `and` instead"),
            Self::UseOr => f.write_str("write `or` instead"),
            Self::UseNot => f.write_str("write `not` instead"),
            Self::UseEq => f.write_str("write `==` to compare"),
            Self::UseNe => f.write_str("write `!=` instead"),
            Self::UseLe => f.write_str("write `<=` instead"),
            Self::UseGe => f.write_str("write `>=` instead"),
            Self::SingleQuotes => f.write_str("strings use single quotes, e.g. 'premium'"),
            Self::NoModulo => {
                f.write_str("`%` is only the percent unit, as in `2%`; there is no remainder operator")
            }
            Self::LeadingDot => f.write_str("write a digit before the decimal point, e.g. `0.5`"),
            Self::UnitSpacing => f.write_str(
                "write the unit right after the number, with at most one space: `30s`, `40 parts`",
            ),
            Self::ParenthesizeNot => f.write_str(
                "`not` applies to a whole comparison; put it in parentheses, e.g. `a == (not b)`",
            ),
            Self::TrailingComma => f.write_str("remove the trailing comma"),
            Self::ParenthesizeElement => f.write_str(
                "list elements are single values; put an expression in parentheses, e.g. `[(-1), 2]`",
            ),
            Self::ArgsAreIds => {
                f.write_str("call arguments are component ids such as `buf1` or `weld-1`")
            }
            Self::UnmatchedParen => f.write_str("remove it or add a matching `(`"),
            Self::ListOnlyAfterIn => {
                f.write_str("a list can only follow `in`, as in `variant in ['a', 'b']`")
            }
        }
    }
}

impl Hint {
    /// The text that replaces the error's span to apply the fix, for the
    /// hints that have one.
    const fn replacement(self) -> Option<&'static str> {
        match self {
            Self::UseAnd => Some("and"),
            Self::UseOr => Some("or"),
            Self::UseNot => Some("not "),
            Self::UseEq => Some("=="),
            Self::UseNe => Some("!="),
            Self::UseLe => Some("<="),
            Self::UseGe => Some(">="),
            Self::SingleQuotes
            | Self::NoModulo
            | Self::LeadingDot
            | Self::UnitSpacing
            | Self::ParenthesizeNot
            | Self::TrailingComma
            | Self::ParenthesizeElement
            | Self::ArgsAreIds
            | Self::UnmatchedParen
            | Self::ListOnlyAfterIn => None,
        }
    }
}

/// `; <hint>`, or what an expression is made of.
struct UnexpectedFix<'a>(&'a Option<Hint>);

impl fmt::Display for UnexpectedFix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(hint) => write!(f, "; {hint}"),
            None => f.write_str(
                "; an expression is made of names, literals such as `42`, `0.5`, `30s` and \
                 'text', the operators `and or not in == != < <= > >= + - * /`, and \
                 parentheses, e.g. `unit.age > 30s`",
            ),
        }
    }
}

/// `; <hint>` or nothing.
struct HintSuffix<'a>(&'a Option<Hint>);

impl fmt::Display for HintSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(hint) => write!(f, "; {hint}"),
            None => Ok(()),
        }
    }
}

/// `; did you mean <q>x<q>?` or nothing.
struct DidYouMean<'a>(&'a Option<String>, char);

impl fmt::Display for DidYouMean<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(s) => write!(f, "; did you mean {q}{s}{q}?", q = self.1),
            None => Ok(()),
        }
    }
}

/// A comma-separated list, or `none`.
struct Listing<'a>(&'a [String], char);

impl fmt::Display for Listing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("none");
        }
        for (i, item) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{q}{item}{q}", q = self.1)?;
        }
        Ok(())
    }
}

/// A comma-separated list of at most ten items, or `none`.
struct Capped<'a>(&'a [String]);

impl fmt::Display for Capped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const SHOWN: usize = 10;
        let shown = self.0.get(..SHOWN).unwrap_or(self.0);
        Listing(shown, '`').fmt(f)?;
        if self.0.len() > SHOWN {
            write!(f, " and {} more", self.0.len() - SHOWN)?;
        }
        Ok(())
    }
}

/// The found token of a syntax error.
struct Found<'a>(&'a Option<String>);

impl fmt::Display for Found<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(text) => write!(f, "`{text}`"),
            None => f.write_str("the end of the expression"),
        }
    }
}

/// An example of `literal` with the canonical unit of `dimension`.
struct WithUnit<'a>(&'a str, Dimension);

impl fmt::Display for WithUnit<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0;
        match self.1 {
            Dimension::Time => write!(f, "{n}s"),
            Dimension::Count if n.contains('.') => f.write_str("20 parts"),
            Dimension::Count => write!(f, "{n} parts"),
            Dimension::Rate => write!(f, "{n} parts/h"),
            Dimension::Ratio => write!(f, "{n}%"),
        }
    }
}

/// Exponent notation such as `e5`, which the language does not have.
fn is_exponent(unit: &str) -> bool {
    let mut chars = unit.chars();
    matches!(chars.next(), Some('e' | 'E')) && {
        let rest = chars.as_str();
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
    }
}

/// `; exponent notation is not supported` or nothing.
struct ExponentNote<'a>(&'a str);

impl fmt::Display for ExponentNote<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if is_exponent(self.0) {
            f.write_str("; exponent notation is not supported, write the digits out")?;
        }
        Ok(())
    }
}

/// What is wrong with an expression. The [`Display`](fmt::Display) text
/// says what was found, what is allowed and how to fix it.
///
/// ```
/// use taktsim_model::expr::{ErrorKind, TypeEnv, condition};
///
/// let env = TypeEnv::new()
///     .with_enum("variant", ["basic", "premium"])?
///     .with_path("variant", taktsim_model::expr::Type::Enum("variant".into()))?;
/// let err = condition("variant == 'premum'", &env).unwrap_err();
/// assert!(matches!(err.kind(), ErrorKind::UnknownEnumValue { .. }));
/// assert_eq!(
///     err.to_string(),
///     "'premum' is not a value of enum `variant`; allowed: 'basic', 'premium'; \
///      did you mean 'premium'?"
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ErrorKind {
    /// The source is empty or only whitespace.
    #[error("the expression is empty; write one, e.g. `variant == 'premium'`")]
    Empty,
    /// A character or sequence that is not part of the language.
    #[error("unexpected `{found}`{}", UnexpectedFix(hint))]
    UnexpectedText {
        /// The text found.
        found: String,
        /// The likely fix.
        hint: Option<Hint>,
    },
    /// A string literal without its closing quote.
    #[error("unterminated string: no closing `'`; write strings in single quotes, e.g. 'premium'")]
    UnterminatedString,
    /// A number such as `1.` or `1.2.3`.
    #[error(
        "malformed number `{text}`; numbers are digits with an optional fraction, e.g. `42` or `0.5`"
    )]
    MalformedNumber {
        /// The text found.
        text: String,
    },
    /// An integer literal above `u64::MAX`.
    #[error("integer `{text}` is too large; the largest is 18446744073709551615")]
    IntTooLarge {
        /// The text found.
        text: String,
    },
    /// A number literal too large for a 64-bit float.
    #[error("number `{text}` is too large; the largest is about 1.8 × 10^308")]
    NumberTooLarge {
        /// The text found.
        text: String,
    },
    /// A word after a number that is not a unit.
    #[error(
        "unknown unit `{unit}`; allowed: {}{}{}",
        units().join(", "),
        DidYouMean(&suggestion.map(str::to_owned), '`'),
        ExponentNote(unit)
    )]
    UnknownUnit {
        /// The word found.
        unit: String,
        /// The closest unit, if one is close.
        suggestion: Option<&'static str>,
    },
    /// A quantity literal that `taktsim-units` rejects, e.g. `2.5 parts`.
    /// Its message is part of this one, so it is not also the error's
    /// [`source`](std::error::Error::source).
    #[error("invalid quantity: {error}")]
    InvalidQuantity {
        /// Why the units crate rejected it.
        error: UnitError,
    },
    /// Something other than what the grammar allows at this point.
    #[error("expected {expected}, found {}{}", Found(found), HintSuffix(hint))]
    Expected {
        /// What the grammar allows here.
        expected: Expectation,
        /// The text found, or `None` at the end of the input.
        found: Option<String>,
        /// The likely fix.
        hint: Option<Hint>,
    },
    /// A keyword in another case, e.g. `AND`. Keywords are lowercase, and
    /// such a word is not a name either.
    #[error(
        "`{found}` is the keyword `{keyword}` in the wrong case; keywords are lowercase: \
         write `{keyword}`"
    )]
    MiscasedKeyword {
        /// The word found.
        found: String,
        /// The keyword.
        keyword: &'static str,
    },
    /// `a < b < c`.
    #[error("comparisons cannot be chained; join them with `and`, e.g. `a < b and b < c`")]
    ComparisonChain,
    /// `not not a`.
    #[error("`not` cannot follow `not`; remove both, or write `not (not a)`")]
    DoubleNot,
    /// `--a`.
    #[error("`-` cannot follow `-`; remove both, or write `-(-a)`")]
    DoubleMinus,
    /// A keyword where a name is expected, e.g. `unit.in`.
    #[error(
        "`{keyword}` is a keyword and cannot be a name; names are other words, e.g. `unit.priority`"
    )]
    KeywordAsName {
        /// The keyword.
        keyword: &'static str,
    },
    /// `a.b(x)`: only built-in functions can be called.
    #[error(
        "`{path}` is a name, not a function; only built-in functions are called, e.g. `throughput(sink)`"
    )]
    CallOnPath {
        /// The path before `(`.
        path: String,
    },
    /// Nesting beyond the limit.
    #[error(
        "the expression nests more than {limit} levels deep; split it, or use \
         `x in [a, b]` instead of long `or` chains"
    )]
    TooDeep {
        /// The limit, [`MAX_DEPTH`](super::MAX_DEPTH).
        limit: usize,
    },
    /// A path that the environment does not define.
    #[error(
        "unknown name `{name}`; known names: {}{}",
        Listing(known, '`'),
        DidYouMean(suggestion, '`')
    )]
    UnknownName {
        /// The path found.
        name: String,
        /// The closest known path, if one is close.
        suggestion: Option<String>,
        /// Every path the environment defines.
        known: Vec<String>,
    },
    /// A call of a function the environment does not define.
    #[error(
        "unknown function `{name}`; functions available here: {}{}",
        Listing(known, '`'),
        DidYouMean(suggestion, '`')
    )]
    UnknownFunction {
        /// The function found.
        name: String,
        /// The closest known function, if one is close.
        suggestion: Option<String>,
        /// Every function the environment defines.
        known: Vec<String>,
    },
    /// A call with the wrong number of arguments.
    #[error("`{function}` takes {arity}, found {found}; e.g. `{example}`")]
    ArgumentCount {
        /// The function.
        function: String,
        /// What it takes.
        arity: Arity,
        /// How many were given.
        found: usize,
        /// A valid call, with component ids of the model where it has
        /// enough.
        example: String,
    },
    /// A call argument that is not a component of the model.
    #[error(
        "`{function}` refers to `{id}`, which is not a component of the model; it takes: {}{}",
        Capped(known),
        DidYouMean(suggestion, '`')
    )]
    UnknownComponent {
        /// The function.
        function: String,
        /// The id found.
        id: String,
        /// The closest component id, if one is close.
        suggestion: Option<String>,
        /// The component ids the function takes.
        known: Vec<String>,
    },
    /// A component of a type the function does not take.
    #[error(
        "`{function}` does not take `{id}`, a {kind}; it takes: {}",
        Listing(accepted, '`')
    )]
    ComponentNotAccepted {
        /// The function.
        function: String,
        /// The id found.
        id: String,
        /// Its component type.
        kind: String,
        /// The component types the function takes.
        accepted: Vec<String>,
    },
    /// The same component twice in one call.
    #[error("`{id}` appears twice in `{function}(...)`; list each component once")]
    DuplicateArgument {
        /// The function.
        function: String,
        /// The repeated id.
        id: String,
    },
    /// Operands of different types.
    #[error(
        "`{op}` cannot combine {left} and {right}; it takes {}; there are no implicit conversions",
        op.operands()
    )]
    Mismatch {
        /// The operator.
        op: Operator,
        /// The type of the left operand.
        left: Type,
        /// The type of the right operand (or list element).
        right: Type,
    },
    /// A bare number compared with or added to a quantity.
    #[error(
        "`{literal}` has no unit, but the other side is a {dimension}; \
         write it with a unit, e.g. `{}`",
        WithUnit(literal, *dimension)
    )]
    MissingUnit {
        /// The literal as written.
        literal: String,
        /// The dimension of the other side.
        dimension: Dimension,
    },
    /// An operand of a type the operator does not take at all.
    #[error("`{op}` is not defined for {found}; it takes {}", op.operands())]
    NotDefinedFor {
        /// The operator.
        op: Operator,
        /// The type found.
        found: Type,
    },
    /// `==`, `!=` or `in` on a type that is not compared exactly.
    #[error(
        "`{op}` cannot compare {ty} values: only int, duration, bool, string and enum values \
         compare exactly; compare with `<=` and `>=` instead"
    )]
    EqualityNotAllowed {
        /// The operator.
        op: Operator,
        /// The type compared.
        ty: Type,
    },
    /// A string literal that is not a value of the enum it is compared with.
    #[error(
        "'{value}' is not a value of enum `{enumeration}`; allowed: {}{}",
        Listing(allowed, '\''),
        DidYouMean(suggestion, '\'')
    )]
    UnknownEnumValue {
        /// The enumeration.
        enumeration: String,
        /// The literal found.
        value: String,
        /// The allowed values.
        allowed: Vec<String>,
        /// The closest allowed value, if one is close.
        suggestion: Option<String>,
    },
    /// A divisor that is not a literal (v0.1, ADR-0009).
    #[error(
        "the divisor must be a literal such as `2.0` or `1h`, so it cannot be zero; \
         write `x / 2.0`, not `x / y`"
    )]
    DivisorNotLiteral,
    /// A zero literal divisor.
    #[error("division by zero; divide by a literal that is not zero, e.g. `x / 2.0`")]
    DivisionByZero,
    /// A division the language does not define.
    #[error("cannot divide {left} by {right}; it takes {}", Operator::Div.operands())]
    DivisionNotAllowed {
        /// The type of the dividend.
        left: Type,
        /// The type of the divisor.
        right: Type,
    },
    /// `x in []`.
    #[error(
        "the list is empty, so `in` would always be false; list at least one value, e.g. `variant in ['premium']`"
    )]
    EmptyList,
    /// Exact arithmetic whose result could exceed the evaluator's range.
    #[error(
        "this arithmetic could exceed the exact range of ±1.7 × 10^38 for some inputs; \
         use fewer factors, e.g. `a * b` of two int inputs fits, `a * b * c` does not"
    )]
    MayOverflow,
    /// An expression of the wrong type for its context.
    #[error("the expression must be {expected}; found {found}")]
    WrongResultType {
        /// What the context needs.
        expected: Expected,
        /// The type of the expression.
        found: Type,
    },
}

/// An error in an expression, with the span it is about.
///
/// The model loader adds the field path (e.g. `flow[3].when`); this type
/// gives the byte span within the field and the message.
///
/// ```
/// use taktsim_model::expr::{ErrorKind, Span, parse};
///
/// let err = parse("a < b < c").unwrap_err();
/// assert_eq!(err.kind(), &ErrorKind::ComparisonChain);
/// assert_eq!(err.span(), Span::new(6, 7));
/// assert_eq!(err.render("a < b < c"), format!("{err}\n  a < b < c\n        ^"));
/// ```
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{kind}")]
pub struct ExprError {
    span: Span,
    kind: Box<ErrorKind>,
}

impl ExprError {
    pub(crate) fn new(kind: ErrorKind, span: Span) -> Self {
        Self {
            span,
            kind: Box::new(kind),
        }
    }

    /// The source text the error is about.
    ///
    /// ```
    /// use taktsim_model::expr::{Span, parse};
    ///
    /// assert_eq!(parse("a +").unwrap_err().span(), Span::new(3, 3));
    /// ```
    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    /// What is wrong.
    ///
    /// ```
    /// use taktsim_model::expr::{ErrorKind, parse};
    ///
    /// assert_eq!(parse("").unwrap_err().kind(), &ErrorKind::Empty);
    /// ```
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Text that fixes the error when it replaces the text at
    /// [`span`](Self::span), where there is one: a suggested name, a unit
    /// for a bare number, a lowercase keyword, `and` for `&&`.
    ///
    /// ```
    /// use taktsim_model::expr::{TypeEnv, condition};
    ///
    /// let mut source = "1 < 2 && 30 < 1min".to_owned();
    /// let err = condition(&source, &TypeEnv::new()).unwrap_err();
    /// source.replace_range(err.span().range(), &err.replacement().unwrap());
    /// assert_eq!(source, "1 < 2 and 30 < 1min");
    ///
    /// let err = condition(&source, &TypeEnv::new()).unwrap_err();
    /// source.replace_range(err.span().range(), &err.replacement().unwrap());
    /// assert_eq!(source, "1 < 2 and 30s < 1min");
    /// assert!(condition(&source, &TypeEnv::new()).is_ok());
    /// ```
    #[must_use]
    pub fn replacement(&self) -> Option<String> {
        match self.kind.as_ref() {
            ErrorKind::MissingUnit { literal, dimension } => {
                Some(WithUnit(literal, *dimension).to_string())
            }
            ErrorKind::UnknownEnumValue { suggestion, .. } => {
                suggestion.as_ref().map(|s| format!("'{s}'"))
            }
            ErrorKind::UnknownName { suggestion, .. }
            | ErrorKind::UnknownFunction { suggestion, .. }
            | ErrorKind::UnknownComponent { suggestion, .. } => suggestion.clone(),
            ErrorKind::UnknownUnit { suggestion, .. } => suggestion.map(str::to_owned),
            ErrorKind::MiscasedKeyword { keyword, .. } => Some((*keyword).to_owned()),
            ErrorKind::UnexpectedText { hint, .. } => {
                hint.and_then(Hint::replacement).map(str::to_owned)
            }
            ErrorKind::Empty
            | ErrorKind::UnterminatedString
            | ErrorKind::MalformedNumber { .. }
            | ErrorKind::IntTooLarge { .. }
            | ErrorKind::NumberTooLarge { .. }
            | ErrorKind::InvalidQuantity { .. }
            | ErrorKind::Expected { .. }
            | ErrorKind::ComparisonChain
            | ErrorKind::DoubleNot
            | ErrorKind::DoubleMinus
            | ErrorKind::KeywordAsName { .. }
            | ErrorKind::CallOnPath { .. }
            | ErrorKind::TooDeep { .. }
            | ErrorKind::ArgumentCount { .. }
            | ErrorKind::ComponentNotAccepted { .. }
            | ErrorKind::DuplicateArgument { .. }
            | ErrorKind::Mismatch { .. }
            | ErrorKind::NotDefinedFor { .. }
            | ErrorKind::EqualityNotAllowed { .. }
            | ErrorKind::DivisorNotLiteral
            | ErrorKind::DivisionByZero
            | ErrorKind::DivisionNotAllowed { .. }
            | ErrorKind::EmptyList
            | ErrorKind::MayOverflow
            | ErrorKind::WrongResultType { .. } => None,
        }
    }

    /// The message followed by the source line and a caret under the span,
    /// for people. A tab is shown as four spaces and a carriage return is
    /// left out, so the caret lines up. `source` must be the text the error
    /// came from; other text gives a meaningless caret but never panics.
    ///
    /// ```
    /// use taktsim_model::expr::parse;
    ///
    /// let source = "rate >= 120 parts/hour";
    /// let err = parse(source).unwrap_err();
    /// assert!(err.render(source).ends_with("\n  rate >= 120 parts/hour\n              ^^^^^^^^^^"));
    /// ```
    #[must_use]
    pub fn render(&self, source: &str) -> String {
        let start = floor_boundary(source, self.span.start);
        let end = floor_boundary(source, self.span.end.max(start));
        let line_start = source
            .get(..start)
            .and_then(|s| s.rfind('\n'))
            .map_or(0, |i| i + 1);
        let line_end = source
            .get(start..)
            .and_then(|s| s.find('\n'))
            .map_or(source.len(), |i| start + i);
        let mut line = String::new();
        let (mut column, mut width) = (0, 0);
        for (i, c) in source
            .get(line_start..line_end)
            .unwrap_or_default()
            .char_indices()
        {
            let columns = match c {
                '\t' => {
                    line.push_str("    ");
                    4
                }
                '\r' => 0,
                _ => {
                    line.push(c);
                    1
                }
            };
            let at = line_start + i;
            if at < start {
                column += columns;
            } else if at < end {
                width += columns;
            }
        }
        format!(
            "{self}\n  {line}\n  {}{}",
            " ".repeat(column),
            "^".repeat(width.max(1))
        )
    }
}

/// The largest character boundary of `s` at or below `i`.
fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The candidate closest to `word` by edit distance, if it is within a
/// third of the word's length (at least one edit) and keeps at least one
/// character of it. Ties go to the first candidate, so the result is
/// deterministic for ordered candidates.
pub(crate) fn suggest<'a, I>(word: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let len = word.chars().count();
    let limit = (len / 3).max(1);
    candidates
        .into_iter()
        .map(|c| (edit_distance(word, c), c))
        .filter(|&(d, _)| d <= limit && d < len)
        .min_by_key(|&(d, _)| d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = Vec::with_capacity(b.len() + 1);
    for (i, ca) in a.chars().enumerate() {
        row.clear();
        row.push(i + 1);
        for (j, &cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != cb);
            let delete = prev[j + 1] + 1;
            let insert = row[j] + 1;
            row.push(substitute.min(delete).min(insert));
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distances() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("premum", "premium"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("ä", "a"), 1);
    }

    #[test]
    fn suggestions_are_close_and_deterministic() {
        let names = ["basic", "deluxe", "premium"];
        assert_eq!(suggest("premum", names), Some("premium"));
        assert_eq!(suggest("xyz", names), None);
        // Short words allow one edit; ties go to the first candidate.
        assert_eq!(suggest("mn", ["min", "ms"]), Some("min"));
        assert_eq!(suggest("mn", ["ms", "min"]), Some("ms"));
        // A one-letter word would match every one-letter candidate.
        assert_eq!(suggest("m", ["d", "h", "min"]), None);
        assert_eq!(suggest("", ["a"]), None);
    }

    #[test]
    fn render_lines_up_tabs_and_drops_carriage_returns() {
        // The span covers `=` and a tab: one column and four.
        let err = ExprError::new(ErrorKind::Empty, Span::new(3, 5));
        assert!(
            err.render("\ta =\tb\r\n")
                .ends_with("\n      a =    b\n        ^^^^^")
        );
        let err = ExprError::new(ErrorKind::Empty, Span::new(1, 2));
        assert!(err.render("a\r\nb").ends_with("\n  a\n   ^"));
    }

    #[test]
    fn render_survives_foreign_sources() {
        let err = ExprError::new(ErrorKind::Empty, Span::new(1, 40));
        assert!(err.render("ä").ends_with("\n  ä\n  ^"));
        let err = ExprError::new(ErrorKind::Empty, Span::new(9, 2));
        assert!(err.render("a\nbc").ends_with("\n  bc\n    ^"));
    }

    #[test]
    fn exponent_detection() {
        assert!(is_exponent("e5"));
        assert!(is_exponent("E10"));
        assert!(!is_exponent("e"));
        assert!(!is_exponent("ex"));
        assert!(!is_exponent("s"));
    }
}
