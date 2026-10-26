//! The grammar of ADR-0009 with the lexical rules of ADR-0015: one or more
//! tests per production, positive and negative, checking the error span
//! and message; operator precedence; spans of nodes.
//!
//! Trees are compared through `sexpr`, a fully parenthesized rendering
//! written here, independent of the pretty-printer under test.

use taktsim_model::expr::{
    BinOp, CmpOp, ErrorKind, Expectation, Expr, ExprError, ExprKind, Hint, MAX_DEPTH, Quantity,
    Span, parse,
};
use taktsim_units::{Count, Rate, Ratio, SimDuration, UnitError};

fn bin(op: BinOp) -> &'static str {
    match op {
        BinOp::Or => "or",
        BinOp::And => "and",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
    }
}

fn cmp(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

/// A fully parenthesized rendering: `(op lhs rhs)`.
fn sexpr(e: &Expr) -> String {
    match e.kind() {
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Number(x) => format!("{:?}", x.get()),
        ExprKind::Quantity(Quantity::Time(d)) => format!("{}ns", d.as_nanos()),
        ExprKind::Quantity(Quantity::Count(c)) => format!("{}parts", c.get()),
        ExprKind::Quantity(Quantity::Rate(r)) => format!("{:?}/s", r.as_per_second()),
        ExprKind::Quantity(Quantity::Ratio(r)) => format!("{:?}%", r.as_percent()),
        ExprKind::Str(t) => format!("'{}'", t.as_str()),
        ExprKind::Path(p) => p
            .segments()
            .iter()
            .map(taktsim_model::expr::Ident::as_str)
            .collect::<Vec<_>>()
            .join("."),
        ExprKind::Call(c) => {
            let args: Vec<&str> = c.args.iter().map(|a| a.value.as_str()).collect();
            format!("{}({})", c.name.value.as_str(), args.join(","))
        }
        ExprKind::Neg(x) => format!("(neg {})", sexpr(x)),
        ExprKind::Not(x) => format!("(not {})", sexpr(x)),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", bin(*op), sexpr(lhs), sexpr(rhs))
        }
        ExprKind::Compare { op, lhs, rhs } => {
            format!("({} {} {})", cmp(*op), sexpr(lhs), sexpr(rhs))
        }
        ExprKind::In { lhs, list } => {
            let items: Vec<String> = list.items.iter().map(sexpr).collect();
            format!("(in {} [{}])", sexpr(lhs), items.join(" "))
        }
    }
}

#[track_caller]
#[allow(clippy::panic)] // test helper
fn tree(src: &str) -> String {
    match parse(src) {
        Ok(e) => sexpr(&e),
        Err(err) => panic!("{src:?} did not parse: {err}"),
    }
}

#[track_caller]
#[allow(clippy::panic)] // test helper
fn err(src: &str) -> ExprError {
    match parse(src) {
        Ok(e) => panic!("{src:?} parsed as {}", sexpr(&e)),
        Err(err) => err,
    }
}

/// The error for `src`: its kind and the text its span covers.
#[track_caller]
fn err_at(src: &str) -> (ErrorKind, String) {
    let e = err(src);
    (e.kind().clone(), src[e.span().range()].to_owned())
}

fn expected(expected: Expectation, found: Option<&str>, hint: Option<Hint>) -> ErrorKind {
    ErrorKind::Expected {
        expected,
        found: found.map(str::to_owned),
        hint,
    }
}

// --- atom: literals -------------------------------------------------------

#[test]
fn bool_literals() {
    assert_eq!(tree("true"), "true");
    assert_eq!(tree("false"), "false");
}

#[test]
fn int_literals() {
    assert_eq!(tree("0"), "0");
    assert_eq!(tree("42"), "42");
    assert_eq!(tree("007"), "7");
    assert_eq!(tree("18446744073709551615"), "18446744073709551615");
    let (kind, at) = err_at("x == 18446744073709551616");
    assert_eq!(
        kind,
        ErrorKind::IntTooLarge {
            text: "18446744073709551616".into()
        }
    );
    assert_eq!(at, "18446744073709551616");
}

#[test]
fn number_literals() {
    assert_eq!(tree("0.5"), "0.5");
    assert_eq!(tree("2.0"), "2.0");
    assert_eq!(tree("10.25"), "10.25");
    // A literal has no sign: `-0.5` negates `0.5`.
    assert_eq!(tree("-0.5"), "(neg 0.5)");
}

#[test]
fn malformed_numbers_are_rejected() {
    let (kind, at) = err_at("x < 1.");
    assert_eq!(kind, ErrorKind::MalformedNumber { text: "1.".into() });
    assert_eq!(at, "1.");
    let (kind, at) = err_at("1.2.3 > x");
    assert_eq!(
        kind,
        ErrorKind::MalformedNumber {
            text: "1.2.3".into()
        }
    );
    assert_eq!(at, "1.2.3");
    let (kind, at) = err_at("x < .5");
    assert_eq!(
        kind,
        ErrorKind::UnexpectedText {
            found: ".".into(),
            hint: Some(Hint::LeadingDot)
        }
    );
    assert_eq!(at, ".");
    let huge = format!("1{}.0", "0".repeat(400));
    let e = err(&huge);
    assert_eq!(e.kind(), &ErrorKind::NumberTooLarge { text: huge.clone() });
    assert_eq!(e.span(), Span::new(0, huge.len()));
}

#[test]
fn exponent_notation_is_not_a_number() {
    let (kind, at) = err_at("x < 2e5");
    assert_eq!(
        kind,
        ErrorKind::UnknownUnit {
            unit: "e5".into(),
            suggestion: None
        }
    );
    assert_eq!(at, "e5");
    assert!(
        err("x < 2e5")
            .to_string()
            .ends_with("; exponent notation is not supported, write the digits out")
    );
}

#[test]
fn string_literals() {
    assert_eq!(tree("'premium'"), "'premium'");
    assert_eq!(tree("''"), "''");
    assert_eq!(tree("'a b, (c) == d'"), "'a b, (c) == d'");
    assert_eq!(tree("'Größe'"), "'Größe'");
    let (kind, at) = err_at("variant == 'premium");
    assert_eq!(kind, ErrorKind::UnterminatedString);
    assert_eq!(at, "'premium");
    let (kind, at) = err_at("variant == \"premium\"");
    assert_eq!(
        kind,
        ErrorKind::UnexpectedText {
            found: "\"".into(),
            hint: Some(Hint::SingleQuotes)
        }
    );
    assert_eq!(at, "\"");
}

// --- atom: quantity literals (parsed by taktsim-units) --------------------

#[allow(clippy::unwrap_used)] // test helper
fn time(s: &str) -> String {
    format!("{}ns", s.parse::<SimDuration>().unwrap().as_nanos())
}

#[test]
fn duration_literals_in_every_unit() {
    assert_eq!(tree("30s"), "30000000000ns");
    assert_eq!(tree("30 s"), "30000000000ns");
    assert_eq!(tree("0.5h"), "1800000000000ns");
    assert_eq!(tree("250ms"), "250000000ns");
    assert_eq!(tree("12min"), "720000000000ns");
    assert_eq!(tree("2d"), "172800000000000ns");
    assert_eq!(tree("0.000000001s"), "1ns");
    assert_eq!(tree("90min"), time("1.5h"));
}

#[test]
fn count_rate_and_ratio_literals() {
    assert_eq!(tree("40 parts"), "40parts");
    assert_eq!(tree("40parts"), "40parts");
    assert_eq!(tree("1 part"), "1parts");
    assert_eq!(tree("120 parts/h"), format!("{:?}/s", 120.0 / 3600.0));
    assert_eq!(tree("3 parts/min"), format!("{:?}/s", 3.0 / 60.0));
    assert_eq!(tree("2 part/s"), "2.0/s");
    assert_eq!(tree("2%"), "2.0%");
    assert_eq!(tree("2 %"), "2.0%");
    assert_eq!(tree("0.5%"), "0.5%");
    let ExprKind::Quantity(q) = parse("40 parts").unwrap().into_kind() else {
        panic!()
    };
    assert_eq!(q, Quantity::Count(Count::new(40)));
    let ExprKind::Quantity(q) = parse("120 parts/h").unwrap().into_kind() else {
        panic!()
    };
    assert_eq!(q, Quantity::Rate("120 parts/h".parse::<Rate>().unwrap()));
    let ExprKind::Quantity(q) = parse("2%").unwrap().into_kind() else {
        panic!()
    };
    assert_eq!(q, Quantity::Ratio("2%".parse::<Ratio>().unwrap()));
}

#[test]
fn a_rate_unit_needs_the_slash_right_after_parts() {
    // `parts / h` divides a count by a name `h`.
    assert_eq!(tree("120 parts / h"), "(/ 120parts h)");
    // A digit after the slash starts a divisor: count / duration.
    assert_eq!(tree("2 parts/1h"), "(/ 2parts 3600000000000ns)");
    assert_eq!(tree("30s/2"), "(/ 30000000000ns 2)");
}

#[test]
fn quantities_rejected_by_the_units_crate() {
    let cases = [
        ("2.5 parts", "2.5 parts"),
        ("0.0000000001s", "0.0000000001s"),
        ("600000000d", "600000000d"),
    ];
    for (src, covered) in cases {
        let (kind, at) = err_at(src);
        assert!(
            matches!(kind, ErrorKind::InvalidQuantity { .. }),
            "{src}: {kind:?}"
        );
        assert_eq!(at, covered);
    }
    let e = err("x < 2.5 parts");
    assert_eq!(
        e.kind(),
        &ErrorKind::InvalidQuantity {
            error: UnitError::NotWhole {
                input: "2.5 parts".into(),
                unit: "parts".into()
            }
        }
    );
    assert_eq!(
        e.to_string(),
        "invalid quantity: `2.5 parts` is not a whole number of parts; example: 20 parts"
    );
}

#[test]
fn the_units_message_appears_once_in_an_error_chain() {
    use std::error::Error;
    let e = err("x < 2.5 parts");
    // The units crate's message is part of the message, not also a source,
    // so a printer that walks the chain shows it once.
    assert!(e.source().is_none());
    assert!(e.kind().source().is_none());
    let mut chain = e.to_string();
    let mut next = e.source();
    while let Some(cause) = next {
        chain.push_str(&cause.to_string());
        next = cause.source();
    }
    assert_eq!(chain.matches("is not a whole number").count(), 1);
}

#[test]
fn unknown_units_point_at_the_unit() {
    let (kind, at) = err_at("x > 30sec");
    assert_eq!(
        kind,
        ErrorKind::UnknownUnit {
            unit: "sec".into(),
            suggestion: None
        }
    );
    assert_eq!(at, "sec");
    let (kind, _) = err_at("x > 30 mn");
    assert_eq!(
        kind,
        ErrorKind::UnknownUnit {
            unit: "mn".into(),
            suggestion: Some("min")
        }
    );
    assert_eq!(
        err("x > 30 mn").to_string(),
        "unknown unit `mn`; allowed: d, h, min, s, ms, parts, part, parts/s, parts/min, \
         parts/h, part/s, part/min, part/h, %; did you mean `min`?"
    );
}

#[test]
fn a_keyword_after_a_number_is_not_a_unit() {
    assert_eq!(tree("3 in [3]"), "(in 3 [3])");
    assert_eq!(tree("x == 3 and y"), "(and (== x 3) y)");
    assert_eq!(tree("x == 3 or y"), "(or (== x 3) y)");
}

#[test]
fn more_than_one_space_before_a_unit_is_rejected_with_a_hint() {
    let (kind, at) = err_at("x < 30  s");
    assert_eq!(
        kind,
        expected(
            Expectation::OperatorOrEnd,
            Some("s"),
            Some(Hint::UnitSpacing)
        )
    );
    assert_eq!(at, "s");
    // Not a unit: no hint.
    let (kind, _) = err_at("x < 30  foo");
    assert_eq!(
        kind,
        expected(Expectation::OperatorOrEnd, Some("foo"), None)
    );
}

// --- atom: paths and calls ------------------------------------------------

#[test]
fn paths() {
    assert_eq!(tree("variant"), "variant");
    assert_eq!(tree("unit.priority"), "unit.priority");
    assert_eq!(tree("a.b.c"), "a.b.c");
    assert_eq!(tree("_x1.y_2"), "_x1.y_2");
    let (kind, at) = err_at("unit.");
    assert_eq!(kind, expected(Expectation::Name, None, None));
    assert_eq!(at, "");
    let (kind, at) = err_at("unit.in == 1");
    assert_eq!(kind, ErrorKind::KeywordAsName { keyword: "in" });
    assert_eq!(at, "in");
    let (kind, at) = err_at("unit.(a)");
    assert_eq!(kind, expected(Expectation::Name, Some("("), None));
    assert_eq!(at, "(");
}

#[test]
fn identifiers_are_ascii() {
    let (kind, at) = err_at("größe > 1");
    assert_eq!(
        kind,
        ErrorKind::UnexpectedText {
            found: "ö".into(),
            hint: None
        }
    );
    assert_eq!(at, "ö");
}

#[test]
fn calls() {
    assert_eq!(tree("f()"), "f()");
    assert_eq!(tree("throughput(sink)"), "throughput(sink)");
    assert_eq!(
        tree("state_shares(cut, weld-1)"),
        "state_shares(cut,weld-1)"
    );
    assert_eq!(tree("f ( a ,b )"), "f(a,b)");
    // Component ids may be keywords or contain hyphens.
    assert_eq!(tree("f(in, frame-assembly-2)"), "f(in,frame-assembly-2)");
    assert_eq!(tree("throughput(sink) - 1"), "(- throughput(sink) 1)");
}

#[test]
fn malformed_calls() {
    let (kind, at) = err_at("f(a,)");
    assert_eq!(
        kind,
        expected(
            Expectation::ComponentId,
            Some(")"),
            Some(Hint::TrailingComma)
        )
    );
    assert_eq!(at, ")");
    let (kind, at) = err_at("f(1)");
    assert_eq!(
        kind,
        expected(Expectation::ComponentId, Some("1"), Some(Hint::ArgsAreIds))
    );
    assert_eq!(at, "1");
    let (kind, at) = err_at("f(a b)");
    assert_eq!(
        kind,
        expected(Expectation::CommaOrParen, Some("b"), Some(Hint::ArgsAreIds))
    );
    assert_eq!(at, "b");
    let (kind, at) = err_at("f(a + b)");
    assert_eq!(
        kind,
        expected(Expectation::CommaOrParen, Some("+"), Some(Hint::ArgsAreIds))
    );
    assert_eq!(at, "+");
    let (kind, _) = err_at("f(a");
    assert_eq!(
        kind,
        expected(Expectation::CommaOrParen, None, Some(Hint::ArgsAreIds))
    );
    let (kind, at) = err_at("unit.f(a)");
    assert_eq!(
        kind,
        ErrorKind::CallOnPath {
            path: "unit.f".into()
        }
    );
    assert_eq!(at, "unit.f");
}

#[test]
fn parentheses() {
    assert_eq!(tree("(a)"), "a");
    assert_eq!(tree("((a))"), "a");
    assert_eq!(tree("(a + b) * c"), "(* (+ a b) c)");
    let (kind, at) = err_at("(a + b");
    assert_eq!(kind, expected(Expectation::CloseParen, None, None));
    assert_eq!(at, "");
    let (kind, at) = err_at("(a b)");
    assert_eq!(kind, expected(Expectation::CloseParen, Some("b"), None));
    assert_eq!(at, "b");
    let (kind, at) = err_at("a + b)");
    assert_eq!(
        kind,
        expected(
            Expectation::OperatorOrEnd,
            Some(")"),
            Some(Hint::UnmatchedParen)
        )
    );
    assert_eq!(at, ")");
    let (kind, _) = err_at("()");
    assert_eq!(kind, expected(Expectation::Expression, Some(")"), None));
}

// --- unary, term, sum ------------------------------------------------------

#[test]
fn unary_minus() {
    assert_eq!(tree("-a"), "(neg a)");
    assert_eq!(tree("-(-a)"), "(neg (neg a))");
    assert_eq!(tree("-f(x)"), "(neg f(x))");
    assert_eq!(tree("-30s"), "(neg 30000000000ns)");
    let (kind, at) = err_at("--a");
    assert_eq!(kind, ErrorKind::DoubleMinus);
    assert_eq!(at, "-");
    assert_eq!(err("--a").span(), Span::new(1, 2));
    let (kind, at) = err_at("- not a");
    assert_eq!(
        kind,
        expected(
            Expectation::Expression,
            Some("not"),
            Some(Hint::ParenthesizeNot)
        )
    );
    assert_eq!(at, "not");
}

#[test]
fn term_is_left_associative() {
    assert_eq!(tree("a * b"), "(* a b)");
    assert_eq!(tree("a / b"), "(/ a b)");
    assert_eq!(tree("a * b / c"), "(/ (* a b) c)");
    assert_eq!(tree("a / b * c"), "(* (/ a b) c)");
    assert_eq!(tree("a * -b"), "(* a (neg b))");
    let (kind, _) = err_at("a *");
    assert_eq!(kind, expected(Expectation::Expression, None, None));
}

#[test]
fn sum_is_left_associative() {
    assert_eq!(tree("a + b"), "(+ a b)");
    assert_eq!(tree("a - b - c"), "(- (- a b) c)");
    assert_eq!(tree("a - b + c"), "(+ (- a b) c)");
    assert_eq!(tree("a - -b"), "(- a (neg b))");
    assert_eq!(tree("a--b"), "(- a (neg b))");
    let (kind, at) = err_at("a + * b");
    assert_eq!(kind, expected(Expectation::Expression, Some("*"), None));
    assert_eq!(at, "*");
}

// --- cmp and in ---------------------------------------------------------------

#[test]
fn every_comparison_operator() {
    assert_eq!(tree("a == b"), "(== a b)");
    assert_eq!(tree("a != b"), "(!= a b)");
    assert_eq!(tree("a < b"), "(< a b)");
    assert_eq!(tree("a <= b"), "(<= a b)");
    assert_eq!(tree("a > b"), "(> a b)");
    assert_eq!(tree("a >= b"), "(>= a b)");
}

#[test]
fn comparison_chains_are_rejected() {
    for (src, at) in [
        ("a < b < c", "<"),
        ("a == b != c", "!="),
        ("a == b in [c]", "in"),
        ("a in [b] == c", "=="),
        ("a in [b] in [c]", "in"),
    ] {
        let e = err(src);
        assert_eq!(e.kind(), &ErrorKind::ComparisonChain, "{src}");
        let span = e.span();
        assert_eq!(&src[span.range()], at, "{src}");
        assert!(span.start > 2, "{src}: points at the second operator");
    }
    // Parentheses make it a comparison of a bool.
    assert_eq!(tree("(a < b) == c"), "(== (< a b) c)");
}

#[test]
fn membership_lists() {
    assert_eq!(tree("x in [1]"), "(in x [1])");
    assert_eq!(tree("x in [1, 2, 3]"), "(in x [1 2 3])");
    assert_eq!(tree("x in []"), "(in x [])");
    assert_eq!(
        tree("variant in ['basic', 'premium']"),
        "(in variant ['basic' 'premium'])"
    );
    assert_eq!(
        tree("x in [(-1), (a + b), f(c), y.z]"),
        "(in x [(neg 1) (+ a b) f(c) y.z])"
    );
}

#[test]
fn malformed_lists() {
    let (kind, at) = err_at("x in 1");
    assert_eq!(kind, expected(Expectation::List, Some("1"), None));
    assert_eq!(at, "1");
    let (kind, at) = err_at("x in [1,]");
    assert_eq!(
        kind,
        expected(
            Expectation::ListElement,
            Some("]"),
            Some(Hint::TrailingComma)
        )
    );
    assert_eq!(at, "]");
    let (kind, _) = err_at("x in [1 2]");
    assert_eq!(
        kind,
        expected(
            Expectation::CommaOrBracket,
            Some("2"),
            Some(Hint::ParenthesizeElement)
        )
    );
    // List elements are atoms (ADR-0009), so a sign needs parentheses.
    let (kind, at) = err_at("x in [-1]");
    assert_eq!(
        kind,
        expected(
            Expectation::ListElement,
            Some("-"),
            Some(Hint::ParenthesizeElement)
        )
    );
    assert_eq!(at, "-");
    let (kind, at) = err_at("x in [a + b]");
    assert_eq!(
        kind,
        expected(
            Expectation::CommaOrBracket,
            Some("+"),
            Some(Hint::ParenthesizeElement)
        )
    );
    assert_eq!(at, "+");
    let (kind, _) = err_at("x in [1");
    assert_eq!(
        kind,
        expected(
            Expectation::CommaOrBracket,
            None,
            Some(Hint::ParenthesizeElement)
        )
    );
    let (kind, at) = err_at("[1, 2]");
    assert_eq!(
        kind,
        expected(
            Expectation::Expression,
            Some("["),
            Some(Hint::ListOnlyAfterIn)
        )
    );
    assert_eq!(at, "[");
    let (kind, _) = err_at("x [1]");
    assert_eq!(
        kind,
        expected(
            Expectation::OperatorOrEnd,
            Some("["),
            Some(Hint::ListOnlyAfterIn)
        )
    );
}

// --- not, and, or ----------------------------------------------------------------

#[test]
fn not_applies_to_a_comparison() {
    assert_eq!(tree("not a"), "(not a)");
    assert_eq!(tree("not a == b"), "(not (== a b))");
    assert_eq!(tree("not x in [1]"), "(not (in x [1]))");
    assert_eq!(tree("not (not a)"), "(not (not a))");
    assert_eq!(tree("not -a < b"), "(not (< (neg a) b))");
    let (kind, at) = err_at("not not a");
    assert_eq!(kind, ErrorKind::DoubleNot);
    assert_eq!(at, "not");
    assert_eq!(err("not not a").span(), Span::new(4, 7));
    let (kind, at) = err_at("a == not b");
    assert_eq!(
        kind,
        expected(
            Expectation::Expression,
            Some("not"),
            Some(Hint::ParenthesizeNot)
        )
    );
    assert_eq!(at, "not");
    assert_eq!(tree("a == (not b)"), "(== a (not b))");
}

#[test]
fn and_and_or_are_left_associative() {
    assert_eq!(tree("a and b"), "(and a b)");
    assert_eq!(tree("a and b and c"), "(and (and a b) c)");
    assert_eq!(tree("a or b"), "(or a b)");
    assert_eq!(tree("a or b or c"), "(or (or a b) c)");
    let (kind, _) = err_at("a and");
    assert_eq!(kind, expected(Expectation::Expression, None, None));
    let (kind, at) = err_at("or a");
    assert_eq!(kind, expected(Expectation::Expression, Some("or"), None));
    assert_eq!(at, "or");
}

// --- precedence -----------------------------------------------------------------

#[test]
fn precedence_from_loosest_to_tightest() {
    // or < and < not < comparison/in < sum < term < unary minus < atom.
    assert_eq!(tree("a or b and c"), "(or a (and b c))");
    assert_eq!(tree("a and b or c"), "(or (and a b) c)");
    assert_eq!(tree("not a and b"), "(and (not a) b)");
    assert_eq!(tree("a and not b"), "(and a (not b))");
    assert_eq!(tree("not a or b"), "(or (not a) b)");
    assert_eq!(tree("a == b and c != d"), "(and (== a b) (!= c d))");
    assert_eq!(tree("a + b < c * d"), "(< (+ a b) (* c d))");
    assert_eq!(tree("a + b in [c]"), "(in (+ a b) [c])");
    assert_eq!(tree("a + b * c"), "(+ a (* b c))");
    assert_eq!(tree("a * b + c"), "(+ (* a b) c)");
    assert_eq!(tree("-a * b"), "(* (neg a) b)");
    assert_eq!(tree("-a + b"), "(+ (neg a) b)");
    assert_eq!(tree("a - b * -c"), "(- a (* b (neg c)))");
    assert_eq!(
        tree("not a + 1 >= 2 * b or c in [1] and d"),
        "(or (not (>= (+ a 1) (* 2 b))) (and (in c [1]) d))"
    );
    assert_eq!(
        tree("wip(buf1) <= 30 parts and throughput(sink) >= 120 parts/h"),
        format!(
            "(and (<= wip(buf1) 30parts) (>= throughput(sink) {:?}/s))",
            120.0 / 3600.0
        )
    );
    assert_eq!(tree("2 parts / 1h"), "(/ 2parts 3600000000000ns)");
}

// --- lexical errors and hints -------------------------------------------------

#[test]
fn operators_from_other_languages_get_a_fix() {
    let cases = [
        ("a && b", "&&", Hint::UseAnd),
        ("a & b", "&", Hint::UseAnd),
        ("a || b", "||", Hint::UseOr),
        ("a | b", "|", Hint::UseOr),
        ("!a", "!", Hint::UseNot),
        ("a = b", "=", Hint::UseEq),
        ("a <> b", "<>", Hint::UseNe),
        ("a =< b", "=<", Hint::UseLe),
        ("a => b", "=>", Hint::UseGe),
        ("a % b", "%", Hint::NoModulo),
    ];
    for (src, found, hint) in cases {
        let (kind, at) = err_at(src);
        assert_eq!(
            kind,
            ErrorKind::UnexpectedText {
                found: found.into(),
                hint: Some(hint)
            },
            "{src}"
        );
        assert_eq!(at, found, "{src}");
    }
    assert_eq!(
        err("a && b").to_string(),
        "unexpected `&&`; write `and` instead"
    );
}

fn miscased(found: &str, keyword: &'static str) -> ErrorKind {
    ErrorKind::MiscasedKeyword {
        found: found.into(),
        keyword,
    }
}

#[test]
fn keywords_are_lowercase_in_every_position() {
    for (src, found, keyword) in [
        // Operator position.
        ("a AND b", "AND", "and"),
        ("a Or b", "Or", "or"),
        ("x IN [1]", "IN", "in"),
        // After a number and a space, where a unit could be: not a unit.
        ("x == 1 AND b", "AND", "and"),
        ("x == 1 Or b", "Or", "or"),
        // Operand position.
        ("NOT b", "NOT", "not"),
        ("b == TRUE", "TRUE", "true"),
        ("False", "False", "false"),
        ("x in [True]", "True", "true"),
        // Path segments.
        ("unit.IN", "IN", "in"),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(kind, miscased(found, keyword), "{src}");
        assert_eq!(at, found, "{src}");
    }
    assert_eq!(
        err("a Or b").to_string(),
        "`Or` is the keyword `or` in the wrong case; keywords are lowercase: write `or`"
    );
    // Component ids in a call are not names, so any word is allowed there.
    assert_eq!(tree("f(AND, True)"), "f(AND,True)");
}

#[test]
fn other_characters_are_rejected() {
    for (src, found) in [
        ("a # b", "#"),
        ("a ; b", ";"),
        ("a ≤ b", "≤"),
        ("a\u{a0}", "\u{a0}"),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(
            kind,
            ErrorKind::UnexpectedText {
                found: found.into(),
                hint: None
            },
            "{src}"
        );
        assert_eq!(at, found);
    }
}

#[test]
fn whitespace_is_space_tab_and_newline() {
    assert_eq!(tree(" a\t==\n\r b "), "(== a b)");
}

#[test]
fn empty_input_is_rejected() {
    for src in ["", "   ", "\n\t"] {
        let e = err(src);
        assert_eq!(e.kind(), &ErrorKind::Empty);
        assert_eq!(e.span(), Span::new(0, 0));
    }
}

#[test]
fn the_end_of_the_input_is_right_after_the_last_token() {
    // YAML block scalars end in a newline; the error points at the end of
    // the expression, not at an empty line after it.
    let src = "variant ==\n";
    let e = err(src);
    assert_eq!(e.span(), Span::new(10, 10));
    assert!(
        e.render(src).ends_with("\n  variant ==\n            ^"),
        "{}",
        e.render(src)
    );
    let src = "(a +\n  b\n\n";
    assert!(err(src).render(src).ends_with("\n    b\n     ^"));
}

#[test]
fn render_lines_up_with_tabs() {
    let src = "a ==\t\tb c";
    let e = err(src);
    assert_eq!(&src[e.span().range()], "c");
    // Each tab shows as four spaces, in the line and before the caret.
    assert!(
        e.render(src)
            .ends_with("\n  a ==        b c\n                ^")
    );
}

#[test]
fn trailing_input_is_rejected() {
    let (kind, at) = err_at("a b");
    assert_eq!(kind, expected(Expectation::OperatorOrEnd, Some("b"), None));
    assert_eq!(at, "b");
    let (kind, _) = err_at("1 'x'");
    assert_eq!(
        kind,
        expected(Expectation::OperatorOrEnd, Some("'x'"), None)
    );
}

// --- depth ------------------------------------------------------------------------

#[test]
fn nesting_is_limited() {
    let nested = |n: usize| format!("{}x{}", "(".repeat(n), ")".repeat(n));
    assert!(parse(&nested(MAX_DEPTH)).is_ok());
    let e = err(&nested(MAX_DEPTH + 1));
    assert_eq!(e.kind(), &ErrorKind::TooDeep { limit: MAX_DEPTH });
    assert_eq!(e.span(), Span::new(MAX_DEPTH, MAX_DEPTH + 1));
    // No stack overflow on absurd input.
    assert!(parse(&"(".repeat(1_000_000)).is_err());
    assert!(parse(&format!("x in {}", "[".repeat(100_000))).is_err());
}

#[test]
fn tree_depth_is_limited() {
    // `a or a or ...` with k operators is k + 1 levels deep.
    let chain = |k: usize| vec!["a"; k + 1].join(" or ");
    assert!(parse(&chain(MAX_DEPTH - 1)).is_ok());
    let e = err(&chain(MAX_DEPTH));
    assert_eq!(e.kind(), &ErrorKind::TooDeep { limit: MAX_DEPTH });
    let negations = |k: usize| format!("{}a{}", "-(".repeat(k), ")".repeat(k));
    assert!(parse(&negations(MAX_DEPTH - 1)).is_ok());
    assert!(matches!(
        err(&negations(MAX_DEPTH)).kind(),
        ErrorKind::TooDeep { .. }
    ));
    // A long flat list is not deep.
    let list = format!("x in [{}]", vec!["1"; 10_000].join(", "));
    assert!(parse(&list).is_ok());
}

// --- spans -----------------------------------------------------------------------

#[test]
fn every_node_has_its_span() {
    let src = "(a + b) * f(c, weld-1) <= -x.y";
    let e = parse(src).unwrap();
    assert_eq!(e.span(), Span::new(0, src.len()));
    let ExprKind::Compare { lhs, rhs, .. } = e.kind() else {
        panic!()
    };
    assert_eq!(&src[lhs.span().range()], "(a + b) * f(c, weld-1)");
    assert_eq!(&src[rhs.span().range()], "-x.y");
    let ExprKind::Binary {
        lhs: sum,
        rhs: call,
        ..
    } = lhs.kind()
    else {
        panic!()
    };
    assert_eq!(&src[sum.span().range()], "(a + b)");
    assert_eq!(&src[call.span().range()], "f(c, weld-1)");
    let ExprKind::Call(c) = call.kind() else {
        panic!()
    };
    assert_eq!(&src[c.name.span.range()], "f");
    assert_eq!(&src[c.args[1].span.range()], "weld-1");
    let ExprKind::Neg(path) = rhs.kind() else {
        panic!()
    };
    assert_eq!(&src[path.span().range()], "x.y");

    let src = "not v in ['a', 'b']";
    let e = parse(src).unwrap();
    let ExprKind::Not(inner) = e.kind() else {
        panic!()
    };
    let ExprKind::In { list, .. } = inner.kind() else {
        panic!()
    };
    assert_eq!(&src[list.span.range()], "['a', 'b']");
    assert_eq!(&src[list.items[1].span().range()], "'b'");
}

#[test]
fn messages_name_the_fix() {
    assert_eq!(
        err("x < 30  s").to_string(),
        "expected an operator such as `and` or `==`, or the end of the expression, found `s`; \
         write the unit right after the number, with at most one space: `30s`, `40 parts`"
    );
    assert_eq!(
        err("a == not b").to_string(),
        "expected an expression such as `unit.age > 30s`, found `not`; `not` applies to a \
         whole comparison; put it in parentheses, e.g. `a == (not b)`"
    );
    assert_eq!(
        err("x in [-1]").to_string(),
        "expected a list element (a literal, name, call or parenthesized expression) such as \
         `'premium'`, found `-`; list elements are single values; put an expression in \
         parentheses, e.g. `[(-1), 2]`"
    );
    assert_eq!(
        err("f(a,)").to_string(),
        "expected a component id such as `buf1`, found `)`; remove the trailing comma"
    );
    assert_eq!(
        err("a)").to_string(),
        "expected an operator such as `and` or `==`, or the end of the expression, found `)`; \
         remove it or add a matching `(`"
    );
    assert_eq!(
        err("").to_string(),
        "the expression is empty; write one, e.g. `variant == 'premium'`"
    );
    assert_eq!(Span::new(2, 5).to_string(), "2..5");
    // Unexpected characters without a specific fix say what is allowed.
    assert_eq!(
        err("a # b").to_string(),
        "unexpected `#`; an expression is made of names, literals such as `42`, `0.5`, `30s` \
         and 'text', the operators `and or not in == != < <= > >= + - * /`, and parentheses, \
         e.g. `unit.age > 30s`"
    );
}

#[test]
fn a_percent_sign_between_operands_is_not_a_remainder() {
    for src in ["10 % 3", "x == 10% 3", "10 % (3)", "10 % y", "10% 'a'"] {
        let (kind, at) = err_at(src);
        assert_eq!(
            kind,
            ErrorKind::UnexpectedText {
                found: "%".into(),
                hint: Some(Hint::NoModulo)
            },
            "{src}"
        );
        assert_eq!(at, "%", "{src}");
    }
    // A percentage followed by an operator or a keyword is fine.
    assert_eq!(tree("x == 10% and y"), "(and (== x 10.0%) y)");
    assert_eq!(tree("x in [10%] or y"), "(or (in x [10.0%]) y)");
    assert_eq!(tree("10% - 3"), "(- 10.0% 3)");
}

#[test]
fn quantities_know_their_dimension() {
    use taktsim_model::expr::Dimension;
    for (src, dimension) in [
        ("1s", Dimension::Time),
        ("1 parts", Dimension::Count),
        ("1 parts/h", Dimension::Rate),
        ("1%", Dimension::Ratio),
    ] {
        let ExprKind::Quantity(q) = parse(src).unwrap().into_kind() else {
            panic!("{src}")
        };
        assert_eq!(q.dimension(), dimension);
    }
}

#[test]
fn render_points_at_the_span() {
    let src = "a == 1 and\n  b < 2 < 3";
    let e = err(src);
    assert_eq!(
        e.render(src),
        "comparisons cannot be chained; join them with `and`, e.g. `a < b and b < c`\n    b < 2 < 3\n          ^"
    );
}
