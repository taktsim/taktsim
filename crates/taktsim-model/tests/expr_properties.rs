//! Properties of the expression language:
//!
//! - printing a generated tree and parsing the text gives the same tree
//!   (modulo spans), and printing is idempotent;
//! - arbitrary text never panics the parser, the checker or the error
//!   renderer, and every span lies on character boundaries of the source;
//! - evaluation is total: a condition that type-checks evaluates without
//!   error for every valid environment, including extreme values;
//! - exact arithmetic agrees with checked `i128` arithmetic computed here
//!   whenever the checker accepts an expression (so the overflow bound is
//!   sound), and boolean conditions agree with a reference interpreter
//!   written here.

mod common;

use std::collections::BTreeMap;

use common::env;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use taktsim_model::expr::{
    BinOp, Call, CmpOp, ComponentId, Condition, ErrorKind, Expected, Expr, ExprError, ExprKind,
    Ident, List, MAX_DEPTH, Number, Path, Quantity, Span, Spanned, Text, Value, check, condition,
    parse,
};
use taktsim_units::{Count, Rate, Ratio, SimDuration};

/// A node of a generated tree, which stays far below the depth limit.
#[allow(clippy::unwrap_used)] // test helper
fn node(kind: ExprKind) -> Expr {
    Expr::new(kind, Span::default()).unwrap()
}

fn unspanned<T>(value: T) -> Spanned<T> {
    Spanned {
        value,
        span: Span::default(),
    }
}

// --- generated trees -----------------------------------------------------------

fn ident() -> impl Strategy<Value = Ident> {
    "[a-zA-Z_][a-zA-Z0-9_]{0,5}".prop_filter_map("a keyword", |s| Ident::new(&s))
}

fn component_id() -> impl Strategy<Value = ComponentId> {
    "[a-z_][a-z0-9_]{0,4}(-[a-z0-9_]{1,3}){0,2}"
        .prop_filter_map("not an id", |s| ComponentId::new(&s))
}

/// Every finite non-negative `f64` (their bit patterns are ordered), plus
/// a dense sample of ordinary values.
fn non_negative() -> impl Strategy<Value = f64> {
    prop_oneof![
        (0..=f64::MAX.to_bits()).prop_map(f64::from_bits),
        0.0..1e6_f64,
        (0_u32..10_000).prop_map(|n| f64::from(n) / 4.0),
    ]
}

#[allow(clippy::unwrap_used)] // test helper; the values are valid
fn quantity() -> impl Strategy<Value = Quantity> {
    prop_oneof![
        any::<u64>().prop_map(|n| Quantity::Time(SimDuration::from_nanos(n))),
        (0_u64..1_000).prop_map(|s| Quantity::Time(SimDuration::from_secs(s * 60).unwrap())),
        any::<u64>().prop_map(|n| Quantity::Count(Count::new(n))),
        non_negative().prop_map(|x| Quantity::Rate(Rate::per_second(x).unwrap())),
        non_negative().prop_map(|x| Quantity::Ratio(Ratio::from_percent(x).unwrap())),
    ]
}

#[allow(clippy::unwrap_used)] // test helper; the values are valid
fn leaf() -> impl Strategy<Value = ExprKind> {
    prop_oneof![
        any::<bool>().prop_map(ExprKind::Bool),
        any::<u64>().prop_map(ExprKind::Int),
        non_negative().prop_map(|x| ExprKind::Number(Number::new(x).unwrap())),
        quantity().prop_map(ExprKind::Quantity),
        "[^']{0,8}".prop_map(|s| ExprKind::Str(Text::new(&s).unwrap())),
        prop::collection::vec(ident(), 1..4).prop_map(|s| ExprKind::Path(Path::new(s).unwrap())),
        (ident(), prop::collection::vec(component_id(), 0..3)).prop_map(|(name, args)| {
            ExprKind::Call(Call {
                name: unspanned(name),
                args: args.into_iter().map(unspanned).collect(),
            })
        }),
    ]
}

fn bin_op() -> impl Strategy<Value = BinOp> {
    prop_oneof![
        Just(BinOp::Or),
        Just(BinOp::And),
        Just(BinOp::Add),
        Just(BinOp::Sub),
        Just(BinOp::Mul),
        Just(BinOp::Div),
    ]
}

fn cmp_op() -> impl Strategy<Value = CmpOp> {
    prop_oneof![
        Just(CmpOp::Eq),
        Just(CmpOp::Ne),
        Just(CmpOp::Lt),
        Just(CmpOp::Le),
        Just(CmpOp::Gt),
        Just(CmpOp::Ge),
    ]
}

/// Trees of every shape the AST can take, from `leaves`.
fn tree(leaves: impl Strategy<Value = ExprKind> + 'static) -> impl Strategy<Value = Expr> {
    leaves.prop_map(node).prop_recursive(6, 64, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|e| node(ExprKind::Neg(Box::new(e)))),
            inner.clone().prop_map(|e| node(ExprKind::Not(Box::new(e)))),
            (bin_op(), inner.clone(), inner.clone()).prop_map(|(op, l, r)| {
                node(ExprKind::Binary {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                })
            }),
            (cmp_op(), inner.clone(), inner.clone()).prop_map(|(op, l, r)| {
                node(ExprKind::Compare {
                    op,
                    lhs: Box::new(l),
                    rhs: Box::new(r),
                })
            }),
            (inner.clone(), prop::collection::vec(inner, 0..4)).prop_map(|(l, items)| {
                node(ExprKind::In {
                    lhs: Box::new(l),
                    list: List {
                        items,
                        span: Span::default(),
                    },
                })
            }),
        ]
    })
}

/// Leaves that the test environment knows, so that many generated trees
/// type-check.
#[allow(clippy::unwrap_used)] // test helper; the sources parse
fn env_leaf() -> impl Strategy<Value = ExprKind> {
    let paths = [
        "variant",
        "product",
        "quality",
        "unit.priority",
        "unit.rank",
        "unit.age",
        "unit.batch",
        "unit.weight",
        "unit.reworked",
        "unit.label",
        "unit.share",
        "unit.rate",
    ];
    let sources = [
        "throughput(sink)",
        "wip(buf1)",
        "lead_time(sink)",
        "state_shares(cut, weld-1)",
        "'premium'",
        "'ok'",
        "'frame'",
        "'nope'",
        "true",
        "0",
        "1",
        "9223372036854775808",
        "18446744073709551615",
        "0.0",
        "2.5",
        "0s",
        "90s",
        "0 parts",
        "5 parts",
        "120 parts/h",
        "0%",
        "2%",
    ];
    let leaves: Vec<ExprKind> = paths
        .iter()
        .chain(sources.iter())
        .map(|s| parse(s).unwrap().without_spans().into_kind())
        .collect();
    prop::sample::select(leaves)
}

/// Valid environments: every input of the test environment, at ordinary
/// and at extreme values.
#[allow(clippy::unwrap_used)] // test helper; the values are valid
fn value_sets() -> Vec<BTreeMap<String, Value<'static>>> {
    let set = |int: i64, time: SimDuration, count: u64, number: f64, rate: f64, ratio: f64| {
        let rate = Value::Rate(Rate::per_second(rate).unwrap());
        let ratio = Value::Ratio(Ratio::from_percent(ratio).unwrap());
        BTreeMap::from([
            ("variant".to_owned(), Value::Text("deluxe")),
            ("product".to_owned(), Value::Text("frame-assembly")),
            ("quality".to_owned(), Value::Text("ok")),
            ("unit.priority".to_owned(), Value::Int(int)),
            ("unit.rank".to_owned(), Value::Int(int)),
            ("unit.age".to_owned(), Value::Time(time)),
            ("unit.batch".to_owned(), Value::Count(Count::new(count))),
            ("unit.weight".to_owned(), Value::Number(number)),
            ("unit.reworked".to_owned(), Value::Bool(int % 2 == 0)),
            ("unit.label".to_owned(), Value::Text("")),
            ("unit.share".to_owned(), ratio),
            ("unit.rate".to_owned(), rate),
            ("throughput(sink)".to_owned(), rate),
            ("wip(buf1)".to_owned(), Value::Count(Count::new(count))),
            ("lead_time(sink)".to_owned(), Value::Time(time)),
            ("state_shares(cut, weld-1)".to_owned(), ratio),
        ])
    };
    vec![
        set(3, SimDuration::from_nanos(90), 5, 2.5, 0.5, 2.0),
        set(0, SimDuration::ZERO, 0, 0.0, 0.0, 0.0),
        set(
            i64::MIN,
            SimDuration::MAX,
            u64::MAX,
            f64::MAX,
            f64::MAX,
            f64::MAX,
        ),
        set(
            i64::MAX,
            SimDuration::MAX,
            u64::MAX,
            -f64::MAX,
            f64::MIN_POSITIVE,
            1e-300,
        ),
    ]
}

// --- checks on any source text ---------------------------------------------------

fn valid_span(src: &str, span: Span) -> Result<(), TestCaseError> {
    prop_assert!(
        span.start <= span.end && span.end <= src.len(),
        "{span} in {src:?}"
    );
    prop_assert!(src.is_char_boundary(span.start) && src.is_char_boundary(span.end));
    Ok(())
}

fn valid_spans(src: &str, e: &Expr) -> Result<(), TestCaseError> {
    valid_span(src, e.span())?;
    match e.kind() {
        ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Number(_)
        | ExprKind::Quantity(_)
        | ExprKind::Str(_)
        | ExprKind::Path(_) => Ok(()),
        ExprKind::Call(c) => {
            valid_span(src, c.name.span)?;
            c.args.iter().try_for_each(|a| valid_span(src, a.span))
        }
        ExprKind::Neg(x) | ExprKind::Not(x) => valid_spans(src, x),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Compare { lhs, rhs, .. } => {
            valid_spans(src, lhs)?;
            valid_spans(src, rhs)
        }
        ExprKind::In { lhs, list } => {
            valid_spans(src, lhs)?;
            valid_span(src, list.span)?;
            list.items.iter().try_for_each(|i| valid_spans(src, i))
        }
    }
}

/// Parses, prints, re-parses, type-checks in every context and, for a
/// condition, evaluates in every valid environment. Nothing may panic,
/// every span must be valid, and a checked condition must evaluate.
fn exercise(src: &str) -> Result<(), TestCaseError> {
    let expr = match parse(src) {
        Ok(expr) => expr,
        Err(e) => {
            valid_span(src, e.span())?;
            prop_assert!(!e.to_string().is_empty());
            prop_assert!(e.render(src).contains('^'));
            return fix_moves_on(src, &e);
        }
    };
    valid_spans(src, &expr)?;
    let printed = expr.to_string();
    let again = parse(&printed).map_err(|e| TestCaseError::fail(format!("{printed:?}: {e}")))?;
    prop_assert_eq!(again.without_spans(), expr.without_spans());
    let env = env();
    for expected in [Expected::Bool, Expected::Numeric, Expected::Any] {
        if let Err(e) = check(&expr, &env, expected) {
            valid_span(src, e.span())?;
            prop_assert!(e.render(src).contains('^'));
            fix_moves_on(src, &e)?;
        }
    }
    if let Ok(cond) = Condition::new(&expr, &env) {
        for values in value_sets() {
            let result = cond.eval(&values);
            prop_assert!(result.is_ok(), "{src:?}: {result:?}");
        }
    }
    Ok(())
}

/// Applying the replacement an error offers fixes that error: the text
/// then parses and checks, or fails with a different error or elsewhere.
fn fix_moves_on(src: &str, e: &ExprError) -> Result<(), TestCaseError> {
    let Some(replacement) = e.replacement() else {
        return Ok(());
    };
    let mut fixed = src.to_owned();
    fixed.replace_range(e.span().range(), &replacement);
    let next = parse(&fixed).and_then(|expr| check(&expr, &env(), Expected::Any).map(|_| ()));
    if let Err(next) = next {
        prop_assert!(
            !(next.kind() == e.kind() && next.span() == e.span()),
            "{src:?} -> {fixed:?} still fails with {next}"
        );
    }
    Ok(())
}

/// Fragments that form valid and invalid expressions in about equal
/// measure.
const FRAGMENTS: [&str; 53] = [
    "variant",
    "quality",
    "unit.priority",
    "unit.age",
    "unit.batch",
    "unit.weight",
    "unit.reworked",
    "unit.share",
    "throughput(sink)",
    "wip(buf1)",
    "wip(cut)",
    "f(a,",
    "'premium'",
    "'ok'",
    "'x",
    "true",
    "false",
    "0",
    "3",
    "18446744073709551615",
    "2.5",
    "0.0",
    "30s",
    "1h",
    "0s",
    "5 parts",
    "120 parts/h",
    "2%",
    "and",
    "or",
    "not",
    "in",
    "==",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "+",
    "-",
    "*",
    "/",
    "(",
    ")",
    "[",
    "[-",
    "]",
    ",",
    ".",
    "=",
    "&&",
    "é",
    "\n",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn printing_then_parsing_gives_the_same_tree(e in tree(leaf())) {
        let text = e.to_string();
        let parsed = parse(&text).map_err(|err| TestCaseError::fail(format!("{text:?}: {err}")))?;
        prop_assert_eq!(parsed.without_spans(), e.without_spans());
        prop_assert_eq!(parsed.to_string(), text);
    }

    #[test]
    fn arbitrary_text_never_panics(src in "\\PC{0,64}") {
        exercise(&src)?;
    }

    #[test]
    fn arbitrary_characters_never_panic(src in ".{0,32}") {
        exercise(&src)?;
    }

    #[test]
    fn fragment_soup_never_panics(
        parts in prop::collection::vec(prop::sample::select(FRAGMENTS.to_vec()), 1..14),
        glue in prop::sample::select(vec![" ", ""]),
    ) {
        exercise(&parts.join(glue))?;
    }

    #[test]
    fn trees_over_the_environment_never_panic(e in tree(env_leaf())) {
        exercise(&e.to_string())?;
    }

    #[test]
    fn well_typed_conditions_always_evaluate(src in typed(Ty::Bool, 3)) {
        match condition(&src, &env()) {
            Ok(cond) => {
                for values in value_sets() {
                    let result = cond.eval(&values);
                    prop_assert!(result.is_ok(), "{}: {:?}", src, result);
                }
            }
            // The only way a well-typed expression is rejected.
            Err(e) => prop_assert_eq!(e.kind(), &ErrorKind::MayOverflow, "{}: {}", src, e),
        }
    }
}

/// The types `typed` generates expressions of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Bool,
    Int,
    Time,
    Count,
    Number,
    Rate,
    Ratio,
}

/// Leaves of type `ty`, each an atom or parenthesized.
fn typed_leaves(ty: Ty) -> Vec<&'static str> {
    match ty {
        Ty::Bool => vec![
            "true",
            "unit.reworked",
            "(variant == 'premium')",
            "(quality in ['ok', 'scrap'])",
            "(product != 'frame')",
            "('deluxe' in [variant])",
        ],
        Ty::Int => vec![
            "0",
            "3",
            "9223372036854775808",
            "unit.priority",
            "unit.rank",
        ],
        Ty::Time => vec![
            "0s",
            "90s",
            "18446744073.709551615s",
            "unit.age",
            "lead_time(sink)",
        ],
        Ty::Count => vec!["0 parts", "5 parts", "unit.batch", "wip(buf1)"],
        Ty::Number => vec!["0.0", "2.5", "unit.weight"],
        Ty::Rate => vec!["0 parts/h", "120 parts/h", "unit.rate", "throughput(sink)"],
        Ty::Ratio => vec!["0%", "2%", "unit.share", "state_shares(cut, weld-1)"],
    }
}

/// Fully parenthesized source text of a well-typed expression of type
/// `ty` over the test environment, at most `depth` operators deep.
fn typed(ty: Ty, depth: u32) -> BoxedStrategy<String> {
    let leaf = prop::sample::select(typed_leaves(ty))
        .prop_map(str::to_owned)
        .boxed();
    if depth == 0 {
        return leaf;
    }
    let sub = |t| typed(t, depth - 1);
    let op = |a: Ty, ops: Vec<&'static str>, b: Ty| {
        (sub(a), prop::sample::select(ops), sub(b))
            .prop_map(|(a, op, b)| format!("({a} {op} {b})"))
            .boxed()
    };
    let by_literal = |a: Ty, literals: Vec<&'static str>| {
        (sub(a), prop::sample::select(literals))
            .prop_map(|(a, k)| format!("({a} / {k})"))
            .boxed()
    };
    let neg = |a: Ty| sub(a).prop_map(|a| format!("-({a})")).boxed();
    // A list element: a sign on a leaf, or on a parenthesized operand.
    let signed = |t: Ty| {
        prop_oneof![
            prop::sample::select(typed_leaves(t)).prop_map(|leaf| format!("-{leaf}")),
            sub(t).prop_map(|a| format!("-({a})")),
        ]
        .boxed()
    };
    let sum = vec!["+", "-"];
    let options = match ty {
        Ty::Bool => {
            let order = vec!["<", "<=", ">", ">="];
            let mut options = vec![
                leaf,
                sub(Ty::Bool).prop_map(|a| format!("(not {a})")).boxed(),
                op(Ty::Bool, vec!["and", "or", "==", "!="], Ty::Bool),
                op(Ty::Int, vec!["==", "!="], Ty::Int),
                op(Ty::Time, vec!["==", "!="], Ty::Time),
                // A count is an exact integer (ADR-0015).
                op(Ty::Count, vec!["==", "!="], Ty::Count),
                // List elements are unary: a sign needs no parentheses.
                (sub(Ty::Int), signed(Ty::Int), signed(Ty::Int))
                    .prop_map(|(a, b, c)| format!("({a} in [{b}, {c}])"))
                    .boxed(),
            ];
            for t in [
                Ty::Int,
                Ty::Time,
                Ty::Count,
                Ty::Number,
                Ty::Rate,
                Ty::Ratio,
            ] {
                options.push(op(t, order.clone(), t));
            }
            for t in [Ty::Int, Ty::Time, Ty::Count, Ty::Bool] {
                options.push(
                    (sub(t), sub(t), sub(t))
                        .prop_map(|(a, b, c)| format!("({a} in [({b}), ({c})])"))
                        .boxed(),
                );
            }
            options
        }
        Ty::Int => vec![
            leaf,
            neg(Ty::Int),
            op(Ty::Int, vec!["+", "-", "*"], Ty::Int),
        ],
        Ty::Time => vec![
            leaf,
            op(Ty::Time, sum, Ty::Time),
            op(Ty::Time, vec!["*"], Ty::Int),
            op(Ty::Int, vec!["*"], Ty::Time),
        ],
        Ty::Count => vec![
            leaf,
            op(Ty::Count, sum, Ty::Count),
            op(Ty::Count, vec!["*"], Ty::Int),
        ],
        Ty::Number => vec![
            leaf,
            neg(Ty::Number),
            op(Ty::Number, vec!["+", "-", "*"], Ty::Number),
            by_literal(Ty::Number, vec!["2.0", "0.5"]),
        ],
        Ty::Rate => vec![
            leaf,
            op(Ty::Rate, sum, Ty::Rate),
            op(Ty::Rate, vec!["*"], Ty::Number),
            op(Ty::Number, vec!["*"], Ty::Rate),
            by_literal(Ty::Rate, vec!["4.0"]),
            by_literal(Ty::Count, vec!["1h", "0.000000001s"]),
        ],
        Ty::Ratio => vec![
            leaf,
            op(Ty::Ratio, sum, Ty::Ratio),
            op(Ty::Ratio, vec!["*"], Ty::Number),
            by_literal(Ty::Ratio, vec!["3.0"]),
        ],
    };
    prop::strategy::Union::new(options).boxed()
}

// --- exact arithmetic against a reference ----------------------------------------------

/// An int expression over `unit.priority` (p) and `unit.rank` (r).
#[derive(Debug, Clone)]
enum IntExpr {
    P,
    R,
    Lit(u64),
    Neg(Box<IntExpr>),
    Add(Box<IntExpr>, Box<IntExpr>),
    Sub(Box<IntExpr>, Box<IntExpr>),
    Mul(Box<IntExpr>, Box<IntExpr>),
}

fn int_expr() -> impl Strategy<Value = IntExpr> {
    int_tree(prop_oneof![
        Just(IntExpr::P),
        Just(IntExpr::R),
        any::<u64>().prop_map(IntExpr::Lit),
        (0_u64..10).prop_map(IntExpr::Lit),
    ])
}

fn int_tree(leaves: impl Strategy<Value = IntExpr> + 'static) -> impl Strategy<Value = IntExpr> {
    leaves.prop_recursive(4, 24, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|a| IntExpr::Neg(Box::new(a))),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| IntExpr::Add(Box::new(a), Box::new(b))),
            (inner.clone(), inner.clone())
                .prop_map(|(a, b)| IntExpr::Sub(Box::new(a), Box::new(b))),
            (inner.clone(), inner).prop_map(|(a, b)| IntExpr::Mul(Box::new(a), Box::new(b))),
        ]
    })
}

/// Fully parenthesized source text.
fn int_source(e: &IntExpr) -> String {
    match e {
        IntExpr::P => "unit.priority".into(),
        IntExpr::R => "unit.rank".into(),
        IntExpr::Lit(n) => n.to_string(),
        IntExpr::Neg(a) => format!("-({})", int_source(a)),
        IntExpr::Add(a, b) => format!("({} + {})", int_source(a), int_source(b)),
        IntExpr::Sub(a, b) => format!("({} - {})", int_source(a), int_source(b)),
        IntExpr::Mul(a, b) => format!("({} * {})", int_source(a), int_source(b)),
    }
}

/// The value with checked `i128` arithmetic, or `None` on overflow, for
/// `unit.priority` and `unit.rank` equal to `priority` and `rank`.
fn int_reference(e: &IntExpr, priority: i64, rank: i64) -> Option<i128> {
    let eval = |x: &IntExpr| int_reference(x, priority, rank);
    match e {
        IntExpr::P => Some(priority.into()),
        IntExpr::R => Some(rank.into()),
        IntExpr::Lit(n) => Some((*n).into()),
        IntExpr::Neg(x) => eval(x)?.checked_neg(),
        IntExpr::Add(x, y) => eval(x)?.checked_add(eval(y)?),
        IntExpr::Sub(x, y) => eval(x)?.checked_sub(eval(y)?),
        IntExpr::Mul(x, y) => eval(x)?.checked_mul(eval(y)?),
    }
}

fn int_values(p: i64, r: i64) -> BTreeMap<String, Value<'static>> {
    BTreeMap::from([
        ("unit.priority".to_owned(), Value::Int(p)),
        ("unit.rank".to_owned(), Value::Int(r)),
    ])
}

fn extreme_int() -> impl Strategy<Value = i64> {
    prop_oneof![
        any::<i64>(),
        Just(i64::MIN),
        Just(i64::MAX),
        Just(0),
        -3_i64..3
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn exact_arithmetic_is_exact_or_rejected(e in int_expr(), p in extreme_int(), r in extreme_int()) {
        let src = int_source(&e);
        let env = env();
        match condition(&format!("{src} < 0"), &env) {
            Err(err) => prop_assert_eq!(err.kind(), &ErrorKind::MayOverflow, "{}", src),
            Ok(below) => {
                // Accepted: the reference must not overflow for any input,
                // in particular not for these.
                let v = int_reference(&e, p, r);
                prop_assert!(v.is_some(), "accepted {} but it overflows i128", src);
                let v = v.unwrap_or_default();
                let values = int_values(p, r);
                prop_assert_eq!(below.eval(&values).unwrap(), v < 0);
                let magnitude = u64::try_from(v.unsigned_abs());
                if let Ok(m) = magnitude {
                    let sign = if v < 0 { "-" } else { "" };
                    let exact = condition(&format!("{src} == {sign}{m}"), &env).unwrap();
                    prop_assert!(exact.eval(&values).unwrap(), "{} != {}", src, v);
                }
            }
        }
    }

    #[test]
    fn small_exact_expressions_are_accepted(e in int_tree((0_u64..10).prop_map(IntExpr::Lit))) {
        // With literals below 10 and at most 4 levels, every value is below
        // 10^16, so no expression may be rejected.
        let src = int_source(&e);
        let cond = condition(&format!("{src} >= 0"), &env()).unwrap();
        let v = int_reference(&e, 0, 0).unwrap();
        prop_assert_eq!(cond.eval(&int_values(0, 0)).unwrap(), v >= 0);
    }
}

// --- conditions against a reference interpreter ---------------------------------------------

const VARIANTS: [&str; 3] = ["basic", "premium", "deluxe"];
const QUALITIES: [&str; 3] = ["ok", "rework", "scrap"];

#[derive(Debug, Clone)]
enum Cond {
    Variant { value: usize, negate: bool },
    QualityIn(Vec<usize>),
    Reworked,
    Priority(CmpOp, i64),
    Not(Box<Cond>),
    And(Box<Cond>, Box<Cond>),
    Or(Box<Cond>, Box<Cond>),
}

fn cond() -> impl Strategy<Value = Cond> {
    prop_oneof![
        (0..3_usize, any::<bool>()).prop_map(|(value, negate)| Cond::Variant { value, negate }),
        prop::collection::vec(0..3_usize, 1..4).prop_map(Cond::QualityIn),
        Just(Cond::Reworked),
        (cmp_op(), -4_i64..4).prop_map(|(op, k)| Cond::Priority(op, k)),
    ]
    .prop_recursive(5, 32, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|c| Cond::Not(Box::new(c))),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Cond::And(Box::new(a), Box::new(b))),
            (inner.clone(), inner).prop_map(|(a, b)| Cond::Or(Box::new(a), Box::new(b))),
        ]
    })
}

fn cmp_text(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

fn cond_source(c: &Cond) -> String {
    match c {
        Cond::Variant { value, negate } => {
            let op = if *negate { "!=" } else { "==" };
            format!("variant {op} '{}'", VARIANTS[*value])
        }
        Cond::QualityIn(values) => {
            let items: Vec<String> = values
                .iter()
                .map(|&i| format!("'{}'", QUALITIES[i]))
                .collect();
            format!("quality in [{}]", items.join(", "))
        }
        Cond::Reworked => "unit.reworked".into(),
        Cond::Priority(op, k) => format!("unit.priority {} {k}", cmp_text(*op)),
        Cond::Not(a) => format!("not ({})", cond_source(a)),
        Cond::And(a, b) => format!("({}) and ({})", cond_source(a), cond_source(b)),
        Cond::Or(a, b) => format!("({}) or ({})", cond_source(a), cond_source(b)),
    }
}

struct Unit {
    variant: usize,
    quality: usize,
    reworked: bool,
    priority: i64,
}

fn cond_reference(c: &Cond, u: &Unit) -> bool {
    match c {
        Cond::Variant { value, negate } => (u.variant == *value) != *negate,
        Cond::QualityIn(values) => values.contains(&u.quality),
        Cond::Reworked => u.reworked,
        Cond::Priority(op, k) => match op {
            CmpOp::Eq => u.priority == *k,
            CmpOp::Ne => u.priority != *k,
            CmpOp::Lt => u.priority < *k,
            CmpOp::Le => u.priority <= *k,
            CmpOp::Gt => u.priority > *k,
            CmpOp::Ge => u.priority >= *k,
        },
        Cond::Not(a) => !cond_reference(a, u),
        Cond::And(a, b) => cond_reference(a, u) && cond_reference(b, u),
        Cond::Or(a, b) => cond_reference(a, u) || cond_reference(b, u),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn conditions_agree_with_a_reference_interpreter(
        c in cond(),
        variant in 0..3_usize,
        quality in 0..3_usize,
        reworked in any::<bool>(),
        priority in -5_i64..5,
    ) {
        let src = cond_source(&c);
        let compiled = condition(&src, &env()).map_err(|e| TestCaseError::fail(format!("{src}: {e}")))?;
        let values = BTreeMap::from([
            ("variant".to_owned(), Value::Text(VARIANTS[variant])),
            ("quality".to_owned(), Value::Text(QUALITIES[quality])),
            ("unit.reworked".to_owned(), Value::Bool(reworked)),
            ("unit.priority".to_owned(), Value::Int(priority)),
        ]);
        let unit = Unit { variant, quality, reworked, priority };
        prop_assert_eq!(compiled.eval(&values).unwrap(), cond_reference(&c, &unit), "{}", src);
    }
}

// --- the deepest trees ------------------------------------------------------------------

/// `e` wrapped in one more level, in each shape the syntax tree has, with
/// `e` in every operand position.
#[allow(clippy::unwrap_used)] // test helper
fn wrap(shape: u8, e: Expr) -> Expr {
    let leaf = || Box::new(node(ExprKind::Bool(true)));
    let e = Box::new(e);
    let kind = match shape % 11 {
        0 => ExprKind::Neg(e),
        1 => ExprKind::Not(e),
        2 => ExprKind::Binary {
            op: BinOp::Sub,
            lhs: e,
            rhs: leaf(),
        },
        3 => ExprKind::Binary {
            op: BinOp::Sub,
            lhs: leaf(),
            rhs: e,
        },
        4 => ExprKind::Binary {
            op: BinOp::Div,
            lhs: leaf(),
            rhs: e,
        },
        5 => ExprKind::Binary {
            op: BinOp::Or,
            lhs: leaf(),
            rhs: e,
        },
        6 => ExprKind::Compare {
            op: CmpOp::Lt,
            lhs: e,
            rhs: leaf(),
        },
        7 => ExprKind::Compare {
            op: CmpOp::Eq,
            lhs: leaf(),
            rhs: e,
        },
        8 => ExprKind::In {
            lhs: e,
            list: List {
                items: vec![*leaf()],
                span: Span::default(),
            },
        },
        9 => ExprKind::In {
            lhs: leaf(),
            list: List {
                items: vec![*leaf(), *e],
                span: Span::default(),
            },
        },
        _ => ExprKind::Binary {
            op: BinOp::And,
            lhs: e,
            rhs: leaf(),
        },
    };
    Expr::new(kind, Span::default()).unwrap()
}

/// A tree of exactly `MAX_DEPTH` levels, from `shapes`.
fn deepest(shapes: &[u8]) -> Expr {
    shapes
        .iter()
        .fold(node(ExprKind::Bool(true)), |e, &shape| wrap(shape, e))
}

/// The deepest tree prints to text that parses back to it, one more level
/// is refused, and every pass over it works.
fn deepest_round_trip(shapes: &[u8]) -> Result<(), TestCaseError> {
    let e = deepest(shapes);
    prop_assert_eq!(e.depth(), MAX_DEPTH);
    let text = e.to_string();
    let parsed = parse(&text).map_err(|err| TestCaseError::fail(format!("{text:?}: {err}")))?;
    prop_assert_eq!(parsed.without_spans(), e.clone());
    let deeper = Expr::new(ExprKind::Not(Box::new(e)), Span::default());
    let refused = matches!(
        deeper.map_err(|err| err.kind().clone()),
        Err(ErrorKind::TooDeep { limit: MAX_DEPTH })
    );
    prop_assert!(refused, "a tree deeper than the limit was built");
    exercise(&text)
}

#[test]
fn the_deepest_tree_of_every_shape_prints_and_parses_back() {
    for shape in 0..11 {
        let shapes = vec![shape; MAX_DEPTH - 1];
        deepest_round_trip(&shapes).unwrap();
    }
}

#[test]
fn no_tree_deeper_than_the_limit_can_be_built() {
    // The review's probe: a million nested negations. Building stops at the
    // limit, so no pass (check, print, evaluate, drop) can overflow the stack.
    let mut e = node(ExprKind::Int(1));
    let mut levels = 1;
    let err = loop {
        match Expr::new(ExprKind::Neg(Box::new(e.clone())), Span::default()) {
            Ok(deeper) => {
                e = deeper;
                levels += 1;
                assert!(levels <= 1_000_000);
            }
            Err(err) => break err,
        }
    };
    assert_eq!(levels, MAX_DEPTH);
    assert_eq!(err.kind(), &ErrorKind::TooDeep { limit: MAX_DEPTH });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn the_deepest_trees_print_and_parse_back(
        shapes in prop::collection::vec(0_u8..11, MAX_DEPTH - 1)
    ) {
        deepest_round_trip(&shapes)?;
    }
}

// --- signed list elements (ADR-0015, answer 6) --------------------------------------------

/// `x in [items]`, with a path on the left.
#[allow(clippy::unwrap_used)] // test helper
fn membership(items: Vec<Expr>) -> Expr {
    let lhs = node(ExprKind::Path(
        Path::new(vec![Ident::new("x").unwrap()]).unwrap(),
    ));
    node(ExprKind::In {
        lhs: Box::new(lhs),
        list: List {
            items,
            span: Span::default(),
        },
    })
}

#[test]
#[allow(clippy::unwrap_used)]
fn a_signed_list_element_prints_without_parentheses() {
    let neg = |e| node(ExprKind::Neg(Box::new(e)));
    let int = |n| node(ExprKind::Int(n));
    let add = |a, b| {
        node(ExprKind::Binary {
            op: BinOp::Add,
            lhs: Box::new(a),
            rhs: Box::new(b),
        })
    };
    for (items, text) in [
        (vec![neg(int(1))], "x in [-1]"),
        (vec![int(1), neg(int(2))], "x in [1, -2]"),
        // The sign applies to an atom; anything else keeps its parentheses.
        (vec![neg(add(int(1), int(2)))], "x in [-(1 + 2)]"),
        (vec![neg(neg(int(1)))], "x in [-(-1)]"),
        (vec![add(neg(int(1)), int(2))], "x in [(-1 + 2)]"),
        (
            vec![node(ExprKind::Not(Box::new(int(1))))],
            "x in [(not 1)]",
        ),
    ] {
        let e = membership(items);
        assert_eq!(e.to_string(), text);
        assert_eq!(parse(text).unwrap().without_spans(), e, "{text}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1_000))]

    /// A list of signed elements, whatever they are signed on, prints to
    /// text that parses back to the same tree.
    #[test]
    fn signed_list_elements_print_and_parse_back(
        operands in prop::collection::vec(tree(leaf()), 1..4),
        plain in prop::collection::vec(tree(leaf()), 0..3),
    ) {
        let items: Vec<Expr> = operands
            .into_iter()
            .map(|e| node(ExprKind::Neg(Box::new(e))))
            .chain(plain)
            .collect();
        let e = membership(items);
        let text = e.to_string();
        prop_assert!(text.contains("[-"), "{}", text);
        let parsed = parse(&text).map_err(|err| TestCaseError::fail(format!("{text:?}: {err}")))?;
        prop_assert_eq!(parsed.without_spans(), e.without_spans());
        prop_assert_eq!(parsed.to_string(), text);
    }
}
