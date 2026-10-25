//! The type rules of ADR-0009 with the details of ADR-0015: one or more
//! tests per rule, positive and negative, checking the error span and
//! message.

mod common;

use common::env;
use taktsim_model::expr::{
    Arity, Condition, Dimension, ErrorKind, Expected, ExprError, Input, Operator, Path, Signature,
    Type, TypeEnv, check, condition, parse,
};

const DURATION: Type = Type::Quantity(Dimension::Time);
const COUNT: Type = Type::Quantity(Dimension::Count);
const RATE: Type = Type::Quantity(Dimension::Rate);
const RATIO: Type = Type::Quantity(Dimension::Ratio);

fn enumeration(name: &str) -> Type {
    Type::Enum(name.to_owned())
}

#[track_caller]
#[allow(clippy::unwrap_used, clippy::panic)] // test helper
fn ty(src: &str) -> Type {
    let expr = parse(src).unwrap();
    match check(&expr, &env(), Expected::Any) {
        Ok(checked) => checked.ty().clone(),
        Err(e) => panic!("{src:?} did not type-check: {e}"),
    }
}

#[track_caller]
#[allow(clippy::unwrap_used, clippy::panic)] // test helper
fn err(src: &str) -> ExprError {
    let expr = parse(src).unwrap();
    match check(&expr, &env(), Expected::Any) {
        Ok(checked) => panic!("{src:?} type-checked as {}", checked.ty()),
        Err(e) => e,
    }
}

/// The error for `src`: its kind and the text its span covers.
#[track_caller]
fn err_at(src: &str) -> (ErrorKind, String) {
    let e = err(src);
    (e.kind().clone(), src[e.span().range()].to_owned())
}

fn mismatch(op: Operator, left: Type, right: Type) -> ErrorKind {
    ErrorKind::Mismatch { op, left, right }
}

fn not_defined(op: Operator, found: Type) -> ErrorKind {
    ErrorKind::NotDefinedFor { op, found }
}

// --- literals and inputs ---------------------------------------------------------

#[test]
fn literals_have_their_types() {
    assert_eq!(ty("true"), Type::Bool);
    assert_eq!(ty("42"), Type::Int);
    assert_eq!(ty("4.2"), Type::Number);
    assert_eq!(ty("30s"), DURATION);
    assert_eq!(ty("40 parts"), COUNT);
    assert_eq!(ty("120 parts/h"), RATE);
    assert_eq!(ty("2%"), RATIO);
    assert_eq!(ty("'text'"), Type::String);
}

#[test]
fn paths_have_their_declared_types() {
    assert_eq!(ty("variant"), enumeration("variant"));
    assert_eq!(ty("unit.priority"), Type::Int);
    assert_eq!(ty("unit.age"), DURATION);
    assert_eq!(ty("unit.batch"), COUNT);
    assert_eq!(ty("unit.weight"), Type::Number);
    assert_eq!(ty("unit.reworked"), Type::Bool);
    assert_eq!(ty("unit.label"), Type::String);
    assert_eq!(ty("unit.share"), RATIO);
    assert_eq!(ty("unit.rate"), RATE);
}

#[test]
fn unknown_paths_are_rejected_with_a_suggestion() {
    let (kind, at) = err_at("varient == 'premium'");
    let ErrorKind::UnknownName {
        name,
        suggestion,
        known,
    } = kind
    else {
        panic!("{kind:?}")
    };
    assert_eq!((name.as_str(), at.as_str()), ("varient", "varient"));
    assert_eq!(suggestion.as_deref(), Some("variant"));
    assert!(known.contains(&"unit.priority".to_owned()));
    let e = err("unit.priorty > 1");
    assert!(
        e.to_string().ends_with("; did you mean `unit.priority`?"),
        "{e}"
    );
    // A prefix of a path is not a value.
    let (kind, _) = err_at("unit");
    assert!(matches!(
        kind,
        ErrorKind::UnknownName {
            suggestion: None,
            ..
        }
    ));
    let e = condition("x", &TypeEnv::new()).unwrap_err();
    assert_eq!(e.to_string(), "unknown name `x`; known names: none");
}

#[test]
fn calls_have_their_declared_result_types() {
    assert_eq!(ty("throughput(sink)"), RATE);
    assert_eq!(ty("wip(buf1)"), COUNT);
    assert_eq!(ty("lead_time(sink)"), DURATION);
    assert_eq!(ty("state_shares(cut, weld-1)"), RATIO);
}

#[test]
fn unknown_functions_are_rejected() {
    let (kind, at) = err_at("throughputt(sink)");
    assert!(matches!(
        &kind,
        ErrorKind::UnknownFunction { name, suggestion: Some(s), .. }
            if name == "throughputt" && s == "throughput"
    ));
    assert_eq!(at, "throughputt");
    // Edge conditions have no functions at all.
    let edge = TypeEnv::new().with_component("sink", "Sink").unwrap();
    let e = condition("throughput(sink) >= 1 parts/h", &edge).unwrap_err();
    assert_eq!(
        e.to_string(),
        "unknown function `throughput`; functions available here: none"
    );
}

#[test]
fn calls_take_the_declared_number_of_arguments() {
    let (kind, at) = err_at("throughput()");
    assert_eq!(
        kind,
        ErrorKind::ArgumentCount {
            function: "throughput".into(),
            arity: Arity::Exactly(1),
            found: 0
        }
    );
    assert_eq!(at, "throughput()");
    let (kind, _) = err_at("throughput(sink, buf1)");
    assert!(matches!(kind, ErrorKind::ArgumentCount { found: 2, .. }));
    let (kind, _) = err_at("state_shares()");
    assert!(matches!(
        kind,
        ErrorKind::ArgumentCount {
            arity: Arity::AtLeast(1),
            ..
        }
    ));
    assert_eq!(
        err("throughput(sink, buf1)").to_string(),
        "`throughput` takes 1 component id, found 2"
    );
    assert_eq!(
        err("state_shares()").to_string(),
        "`state_shares` takes at least 1 component id, found 0"
    );
}

#[test]
fn call_arguments_must_be_components_of_the_model() {
    let (kind, at) = err_at("throughput(snk)");
    assert_eq!(
        kind,
        ErrorKind::UnknownComponent {
            function: "throughput".into(),
            id: "snk".into(),
            suggestion: Some("sink".into())
        }
    );
    assert_eq!(at, "snk");
    assert_eq!(
        err("throughput(snk)").to_string(),
        "`throughput` refers to `snk`, which is not a component of the model; did you mean `sink`?"
    );
}

#[test]
fn call_arguments_must_be_of_an_accepted_kind() {
    let (kind, at) = err_at("wip(cut)");
    assert_eq!(
        kind,
        ErrorKind::ComponentNotAccepted {
            function: "wip".into(),
            id: "cut".into(),
            kind: "Machine".into(),
            accepted: vec!["Buffer".into()]
        }
    );
    assert_eq!(at, "cut");
    assert_eq!(
        err("wip(cut)").to_string(),
        "`wip` does not take `cut`, a Machine; it takes: `Buffer`"
    );
}

#[test]
fn call_arguments_are_listed_once() {
    let src = "state_shares(cut, weld-1, cut)";
    let e = err(src);
    assert_eq!(
        e.kind(),
        &ErrorKind::DuplicateArgument {
            function: "state_shares".into(),
            id: "cut".into()
        }
    );
    assert_eq!(e.span().start, 26);
}

// --- bool -------------------------------------------------------------------------------

#[test]
fn and_or_not_take_bools() {
    assert_eq!(ty("true and false"), Type::Bool);
    assert_eq!(ty("true or unit.reworked"), Type::Bool);
    assert_eq!(ty("not unit.reworked"), Type::Bool);
    let (kind, at) = err_at("unit.priority and true");
    assert_eq!(kind, not_defined(Operator::And, Type::Int));
    assert_eq!(at, "unit.priority");
    let (kind, at) = err_at("true or 'x'");
    assert_eq!(kind, not_defined(Operator::Or, Type::String));
    assert_eq!(at, "'x'");
    let (kind, at) = err_at("not 30s");
    assert_eq!(kind, not_defined(Operator::Not, DURATION));
    assert_eq!(at, "30s");
    assert_eq!(
        err("not 30s").to_string(),
        "`not` is not defined for duration; it takes bool operands"
    );
}

// --- arithmetic -----------------------------------------------------------------------

#[test]
fn addition_needs_one_type_on_both_sides() {
    assert_eq!(ty("1 + 2"), Type::Int);
    assert_eq!(ty("unit.priority - 2"), Type::Int);
    assert_eq!(ty("1.5 + 2.5"), Type::Number);
    assert_eq!(ty("30s + 1min"), DURATION);
    assert_eq!(ty("unit.age - 30s"), DURATION);
    assert_eq!(ty("2 parts + unit.batch"), COUNT);
    assert_eq!(ty("throughput(sink) - 1 parts/h"), RATE);
    assert_eq!(ty("2% + unit.share"), RATIO);
}

#[test]
fn there_are_no_implicit_conversions() {
    let (kind, at) = err_at("1 + 2.5");
    assert_eq!(kind, mismatch(Operator::Add, Type::Int, Type::Number));
    assert_eq!(at, "1 + 2.5");
    let (kind, _) = err_at("unit.priority < unit.weight");
    assert_eq!(kind, mismatch(Operator::Lt, Type::Int, Type::Number));
    assert_eq!(
        err("1 + 2.5").to_string(),
        "`+` cannot combine int and number; it takes two values of the same type: int, \
         number, or quantities of the same dimension; there are no implicit conversions"
    );
}

#[test]
fn quantities_of_different_dimensions_do_not_mix() {
    let (kind, at) = err_at("30s + 2 parts");
    assert_eq!(kind, mismatch(Operator::Add, DURATION, COUNT));
    assert_eq!(at, "30s + 2 parts");
    let (kind, _) = err_at("throughput(sink) - 2%");
    assert_eq!(kind, mismatch(Operator::Sub, RATE, RATIO));
    let (kind, _) = err_at("lead_time(sink) <= 30 parts");
    assert_eq!(kind, mismatch(Operator::Le, DURATION, COUNT));
}

#[test]
fn a_bare_number_next_to_a_quantity_needs_a_unit() {
    let cases = [
        ("wip(buf1) <= 30", "30", Dimension::Count, "30 parts"),
        ("30 + 1s", "30", Dimension::Time, "30s"),
        ("unit.share > 2.5", "2.5", Dimension::Ratio, "2.5%"),
        (
            "throughput(sink) >= 120",
            "120",
            Dimension::Rate,
            "120 parts/h",
        ),
        ("unit.batch < 2.5", "2.5", Dimension::Count, "20 parts"),
        ("unit.age in [30s, 60]", "60", Dimension::Time, "60s"),
    ];
    for (src, literal, dimension, example) in cases {
        let (kind, at) = err_at(src);
        assert_eq!(
            kind,
            ErrorKind::MissingUnit {
                literal: literal.into(),
                dimension
            },
            "{src}"
        );
        assert_eq!(at, literal, "{src}");
        assert!(err(src).to_string().ends_with(&format!("e.g. `{example}`")));
    }
    assert_eq!(
        err("wip(buf1) <= 30").to_string(),
        "`30` has no unit, but the other side is a count; write it with a unit, e.g. `30 parts`"
    );
}

#[test]
fn multiplication_scales_by_the_natural_factor() {
    assert_eq!(ty("2 * 3"), Type::Int);
    assert_eq!(ty("2.0 * 3.0"), Type::Number);
    assert_eq!(ty("2 * 30s"), DURATION);
    assert_eq!(ty("unit.age * unit.priority"), DURATION);
    assert_eq!(ty("3 * 2 parts"), COUNT);
    assert_eq!(ty("unit.batch * 2"), COUNT);
    assert_eq!(ty("throughput(sink) * 0.5"), RATE);
    assert_eq!(ty("0.5 * throughput(sink)"), RATE);
    assert_eq!(ty("2% * 3.0"), RATIO);
    assert_eq!(ty("unit.weight * unit.share"), RATIO);
}

#[test]
fn multiplication_rejects_other_combinations() {
    for (src, left, right) in [
        ("30s * 30s", DURATION, DURATION),
        ("30s * 2.0", DURATION, Type::Number),
        ("2 parts * 1.5", COUNT, Type::Number),
        ("throughput(sink) * 2", RATE, Type::Int),
        ("2 * 3%", Type::Int, RATIO),
        ("2 parts * 1h", COUNT, DURATION),
        ("2 * 2.0", Type::Int, Type::Number),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(kind, mismatch(Operator::Mul, left, right), "{src}");
        assert_eq!(at, src);
    }
    let (kind, at) = err_at("variant * 2");
    assert_eq!(kind, not_defined(Operator::Mul, enumeration("variant")));
    assert_eq!(at, "variant");
    let (kind, _) = err_at("2 * true");
    assert_eq!(kind, not_defined(Operator::Mul, Type::Bool));
    let (kind, _) = err_at("'a' + 'b'");
    assert_eq!(kind, not_defined(Operator::Add, Type::String));
}

#[test]
fn a_count_divided_by_a_duration_is_a_rate() {
    assert_eq!(ty("2 parts / 1h"), RATE);
    assert_eq!(ty("wip(buf1) / 8h"), RATE);
    assert_eq!(ty("unit.batch / 90min"), RATE);
}

#[test]
fn division_by_a_literal() {
    assert_eq!(ty("unit.weight / 2.0"), Type::Number);
    assert_eq!(ty("throughput(sink) / 2.5"), RATE);
    assert_eq!(ty("unit.share / 4.0"), RATIO);
    // A parenthesized literal is still a literal.
    assert_eq!(ty("unit.weight / (2.0)"), Type::Number);
}

#[test]
fn the_divisor_must_be_a_literal() {
    for (src, divisor) in [
        ("1.0 / unit.weight", "unit.weight"),
        ("unit.weight / -2.0", "-2.0"),
        ("unit.weight / (1.0 + 1.0)", "(1.0 + 1.0)"),
        ("wip(buf1) / lead_time(sink)", "lead_time(sink)"),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(kind, ErrorKind::DivisorNotLiteral, "{src}");
        assert_eq!(at, divisor);
    }
}

#[test]
fn division_by_zero_is_rejected() {
    for (src, zero) in [
        ("unit.weight / 0.0", "0.0"),
        ("unit.weight / 0.000", "0.000"),
        ("2 parts / 0s", "0s"),
        ("unit.priority / 0", "0"),
        ("unit.batch / 0 parts", "0 parts"),
        ("throughput(sink) / 0 parts/h", "0 parts/h"),
        ("unit.share / 0%", "0%"),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(kind, ErrorKind::DivisionByZero, "{src}");
        assert_eq!(at, zero);
    }
}

#[test]
fn other_divisions_are_rejected() {
    for (src, left, right) in [
        ("unit.priority / 2", Type::Int, Type::Int),
        ("30s / 2", DURATION, Type::Int),
        ("throughput(sink) / 2", RATE, Type::Int),
        ("unit.weight / 2", Type::Number, Type::Int),
        ("2 parts / 2 parts", COUNT, COUNT),
        ("30s / 1s", DURATION, DURATION),
        ("2.0 / 1h", Type::Number, DURATION),
        ("unit.share / 2%", RATIO, RATIO),
        ("2 parts / 2.0", COUNT, Type::Number),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(kind, ErrorKind::DivisionNotAllowed { left, right }, "{src}");
        assert_eq!(at, src);
    }
    let (kind, at) = err_at("variant / 2");
    assert_eq!(kind, not_defined(Operator::Div, enumeration("variant")));
    assert_eq!(at, "variant");
}

#[test]
fn negation_takes_int_or_number() {
    assert_eq!(ty("-unit.priority"), Type::Int);
    assert_eq!(ty("-2.5"), Type::Number);
    for (src, found) in [
        ("-30s", DURATION),
        ("-unit.batch", COUNT),
        ("-throughput(sink)", RATE),
        ("-2%", RATIO),
        ("-true", Type::Bool),
        ("-variant", enumeration("variant")),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(kind, not_defined(Operator::Neg, found), "{src}");
        assert_eq!(at, &src[1..]);
    }
    assert_eq!(
        err("-30s").to_string(),
        "`-` is not defined for duration; it takes an int or a number (quantities are never negative)"
    );
}

#[test]
fn exact_arithmetic_that_could_overflow_is_rejected() {
    // |int| <= 2^63 and durations, counts < 2^64; i128 holds up to 2^127 - 1.
    assert_eq!(ty("unit.priority * unit.rank"), Type::Int); // 2^126
    assert_eq!(ty("unit.age * unit.priority"), DURATION); // < 2^127
    for src in [
        "unit.priority * unit.rank * 2",
        "unit.age * unit.priority + unit.age * unit.priority",
        "18446744073709551615 * 18446744073709551615",
        "unit.batch * 18446744073709551615 * 18446744073709551615",
    ] {
        let (kind, _) = err_at(src);
        assert_eq!(kind, ErrorKind::MayOverflow, "{src}");
    }
    let (_, at) = err_at("unit.priority * unit.rank * 2 + 1");
    assert_eq!(at, "unit.priority * unit.rank * 2");
}

// --- comparisons ----------------------------------------------------------------------

#[test]
fn ordering_needs_numeric_operands_of_one_type() {
    for src in [
        "unit.priority < 3",
        "unit.weight >= 0.5",
        "unit.age > 1h",
        "wip(buf1) <= 30 parts",
        "throughput(sink) >= 120 parts/h",
        "unit.share < 5%",
    ] {
        assert_eq!(ty(src), Type::Bool, "{src}");
    }
    for (src, operand, found) in [
        ("variant < 'premium'", "variant", enumeration("variant")),
        ("true <= false", "true", Type::Bool),
        ("unit.label > 'a'", "unit.label", Type::String),
        ("1 >= 'a'", "'a'", Type::String),
    ] {
        let (kind, at) = err_at(src);
        assert!(
            matches!(&kind, ErrorKind::NotDefinedFor { found: f, .. } if *f == found),
            "{src}: {kind:?}"
        );
        assert_eq!(at, operand);
    }
}

#[test]
fn equality_is_allowed_on_exact_types() {
    for src in [
        "unit.priority == 1",
        "unit.age == 30s",
        "unit.age != lead_time(sink)",
        "unit.reworked == true",
        "unit.reworked != (1 < 2)",
        "unit.label == 'x'",
        "variant == 'premium'",
        "variant != 'basic'",
        "'premium' == variant",
        "variant == variant",
        "'a' == 'b'",
    ] {
        assert_eq!(ty(src), Type::Bool, "{src}");
    }
}

#[test]
fn equality_is_rejected_on_inexact_types() {
    for (src, op, found) in [
        ("unit.weight == 1.0", Operator::Eq, Type::Number),
        ("wip(buf1) == 3 parts", Operator::Eq, COUNT),
        ("throughput(sink) != 1 parts/h", Operator::Ne, RATE),
        ("2% == unit.share", Operator::Eq, RATIO),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(
            kind,
            ErrorKind::EqualityNotAllowed { op, ty: found },
            "{src}"
        );
        assert_eq!(at, src);
    }
    assert_eq!(
        err("unit.weight == 1.0").to_string(),
        "`==` cannot compare number values: only int, duration, bool, string and enum values \
         compare exactly; compare with `<=` and `>=` instead"
    );
}

#[test]
fn equality_needs_one_type_on_both_sides() {
    for (src, left, right) in [
        (
            "variant == product",
            enumeration("variant"),
            enumeration("product"),
        ),
        (
            "variant == unit.label",
            enumeration("variant"),
            Type::String,
        ),
        ("variant == 1", enumeration("variant"), Type::Int),
        ("1 == true", Type::Int, Type::Bool),
        ("'a' != 1", Type::String, Type::Int),
        ("unit.age == 2 parts", DURATION, COUNT),
    ] {
        let (kind, at) = err_at(src);
        assert!(
            matches!(&kind, ErrorKind::Mismatch { left: l, right: r, .. } if *l == left && *r == right),
            "{src}: {kind:?}"
        );
        assert_eq!(at, src);
    }
    let (kind, at) = err_at("unit.age == 30");
    assert!(matches!(kind, ErrorKind::MissingUnit { .. }));
    assert_eq!(at, "30");
}

#[test]
fn enum_literals_are_checked_against_the_values() {
    let src = "variant == 'premum'";
    let (kind, at) = err_at(src);
    assert_eq!(
        kind,
        ErrorKind::UnknownEnumValue {
            enumeration: "variant".into(),
            value: "premum".into(),
            allowed: vec!["basic".into(), "deluxe".into(), "premium".into()],
            suggestion: Some("premium".into())
        }
    );
    assert_eq!(at, "'premum'");
    let (_, at) = err_at("'premum' != variant");
    assert_eq!(at, "'premum'");
    let (kind, _) = err_at("quality == 'gold'");
    assert!(matches!(
        kind,
        ErrorKind::UnknownEnumValue {
            suggestion: None,
            ..
        }
    ));
    assert_eq!(
        err("quality == 'gold'").to_string(),
        "'gold' is not a value of enum `quality`; allowed: 'ok', 'rework', 'scrap'"
    );
    // A string-typed path takes any literal.
    assert_eq!(ty("unit.label == 'gold'"), Type::Bool);
}

#[test]
fn membership_follows_the_equality_rules() {
    for src in [
        "unit.priority in [1, 2, unit.rank]",
        "unit.age in [30s, 1min]",
        "unit.reworked in [true]",
        "variant in ['basic', 'premium']",
        "'frame' in [product]",
        "unit.label in ['a', unit.label]",
    ] {
        assert_eq!(ty(src), Type::Bool, "{src}");
    }
    for (src, found) in [
        ("unit.weight in [1.0]", Type::Number),
        ("wip(buf1) in [1 parts]", COUNT),
        ("unit.rate in [1 parts/h]", RATE),
        ("unit.share in [1%]", RATIO),
    ] {
        let (kind, at) = err_at(src);
        assert_eq!(
            kind,
            ErrorKind::EqualityNotAllowed {
                op: Operator::In,
                ty: found
            },
            "{src}"
        );
        assert_eq!(at, src);
    }
}

#[test]
fn list_elements_must_match_the_left_side() {
    for (src, element, right) in [
        ("unit.priority in [1, 2.0]", "2.0", Type::Number),
        (
            "variant in ['basic', product]",
            "product",
            enumeration("product"),
        ),
        ("unit.reworked in [1]", "1", Type::Int),
        ("unit.label in [variant]", "variant", enumeration("variant")),
        ("'frame' in [1]", "1", Type::Int),
    ] {
        let (kind, at) = err_at(src);
        assert!(
            matches!(&kind, ErrorKind::Mismatch { op: Operator::In, right: r, .. } if *r == right),
            "{src}: {kind:?}"
        );
        assert_eq!(at, element, "{src}");
    }
    let (kind, at) = err_at("variant in ['basic', 'deluxx']");
    assert!(matches!(
        &kind,
        ErrorKind::UnknownEnumValue { suggestion: Some(s), .. } if s == "deluxe"
    ));
    assert_eq!(at, "'deluxx'");
    let (kind, at) = err_at("'basic' in [variant, product]");
    assert!(
        matches!(&kind, ErrorKind::UnknownEnumValue { enumeration, .. } if enumeration == "product")
    );
    assert_eq!(at, "'basic'");
}

#[test]
fn empty_lists_are_rejected() {
    let (kind, at) = err_at("variant in []");
    assert_eq!(kind, ErrorKind::EmptyList);
    assert_eq!(at, "[]");
}

// --- contexts ----------------------------------------------------------------------------

#[test]
fn the_context_decides_the_result_type() {
    let expr = parse("1 + 2").unwrap();
    let e = check(&expr, &env(), Expected::Bool).unwrap_err();
    assert_eq!(
        e.kind(),
        &ErrorKind::WrongResultType {
            expected: Expected::Bool,
            found: Type::Int
        }
    );
    assert_eq!(e.span().range(), 0..5);
    assert_eq!(
        e.to_string(),
        "the expression must be a bool, e.g. `variant == 'premium'`; found int"
    );
    let kpi = parse("throughput(sink) * 0.5").unwrap();
    assert_eq!(check(&kpi, &env(), Expected::Numeric).unwrap().ty(), &RATE);
    for src in ["1", "1.0", "30s", "wip(buf1)", "2%"] {
        assert!(check(&parse(src).unwrap(), &env(), Expected::Numeric).is_ok());
    }
    for src in ["variant == 'premium'", "variant", "'a'"] {
        let e = check(&parse(src).unwrap(), &env(), Expected::Numeric).unwrap_err();
        assert!(
            matches!(e.kind(), ErrorKind::WrongResultType { .. }),
            "{src}"
        );
    }
    assert!(check(&parse("'a'").unwrap(), &env(), Expected::Any).is_ok());
    // A condition must be a bool.
    let e = condition("throughput(sink)", &env()).unwrap_err();
    assert!(matches!(
        e.kind(),
        ErrorKind::WrongResultType { expected: Expected::Bool, found } if *found == RATE
    ));
    let e = Condition::new(&parse("variant").unwrap(), &env()).unwrap_err();
    assert!(matches!(e.kind(), ErrorKind::WrongResultType { .. }));
}

#[test]
fn context_messages() {
    let e = check(&parse("variant").unwrap(), &env(), Expected::Numeric).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the expression must be an int, a number or a quantity, e.g. `throughput(sink)`; \
         found enum `variant`"
    );
    assert_eq!(Expected::Any.to_string(), "any type");
}

#[test]
fn parse_errors_come_first() {
    let e = condition("variant ==", &env()).unwrap_err();
    assert!(matches!(e.kind(), ErrorKind::Expected { .. }));
}

#[test]
fn the_first_error_is_reported() {
    let (kind, at) = err_at("varient == 'x' and 1 + 2.0 > 0");
    assert!(matches!(kind, ErrorKind::UnknownName { .. }));
    assert_eq!(at, "varient");
    // A parenthesized expression's span includes the parentheses.
    let (_, at) = err_at("(1 + 2.0) * unknown");
    assert_eq!(at, "(1 + 2.0)");
}

#[test]
fn checked_expressions_list_their_inputs() {
    let checked = check(
        &parse("wip(buf1) / 1h > throughput(sink) and unit.age > 1h and unit.age < 2h").unwrap(),
        &env(),
        Expected::Bool,
    )
    .unwrap();
    let inputs: Vec<(String, Type)> = checked
        .inputs()
        .iter()
        .map(|(i, t)| (i.to_string(), t.clone()))
        .collect();
    assert_eq!(
        inputs,
        [
            ("unit.age".to_owned(), DURATION),
            ("throughput(sink)".to_owned(), RATE),
            ("wip(buf1)".to_owned(), COUNT),
        ]
    );
    assert!(
        checked
            .inputs()
            .contains_key(&Input::Path(Path::parse("unit.age").unwrap()))
    );
    assert_eq!(
        checked.expr().to_string(),
        "wip(buf1) / 1h > throughput(sink) and unit.age > 1h and unit.age < 2h"
    );
}

#[test]
fn environments_reject_names_the_language_cannot_write() {
    use taktsim_model::expr::EnvError;
    assert!(matches!(
        TypeEnv::new().with_path("a..b", Type::Int),
        Err(EnvError::InvalidName { .. })
    ));
    assert!(matches!(
        TypeEnv::new().with_path("x", Type::Enum("nope".into())),
        Err(EnvError::UnknownEnum { .. })
    ));
    assert!(matches!(
        TypeEnv::new()
            .with_path("x", Type::Int)
            .unwrap()
            .with_path("x", Type::Int),
        Err(EnvError::Duplicate { .. })
    ));
    assert!(matches!(
        TypeEnv::new().with_function("not", Signature::new(Arity::Exactly(0), Type::Int)),
        Err(EnvError::InvalidName { .. })
    ));
    assert!(matches!(
        TypeEnv::new().with_function(
            "f",
            Signature::new(Arity::Exactly(0), Type::Enum("e".into()))
        ),
        Err(EnvError::UnknownEnum { .. })
    ));
    let f = || Signature::new(Arity::Exactly(0), Type::Int);
    assert!(matches!(
        TypeEnv::new()
            .with_function("f", f())
            .unwrap()
            .with_function("f", f()),
        Err(EnvError::Duplicate { .. })
    ));
    assert!(matches!(
        TypeEnv::new().with_component("-a", "Machine"),
        Err(EnvError::InvalidComponentId { .. })
    ));
    assert!(matches!(
        TypeEnv::new()
            .with_component("a", "Machine")
            .unwrap()
            .with_component("a", "Buffer"),
        Err(EnvError::Duplicate { .. })
    ));
    assert!(matches!(
        TypeEnv::new().with_enum("e", ["it's"]),
        Err(EnvError::InvalidEnumValue { .. })
    ));
    assert!(matches!(
        TypeEnv::new()
            .with_enum("e", ["a"])
            .unwrap()
            .with_enum("e", ["b"]),
        Err(EnvError::Duplicate { .. })
    ));
    assert_eq!(
        TypeEnv::new()
            .with_enum("e", Vec::<&str>::new())
            .unwrap_err()
            .to_string(),
        "enum `e` has no values"
    );
}

#[test]
fn a_zero_argument_function_can_be_called() {
    let env = TypeEnv::new()
        .with_function("now", Signature::new(Arity::Exactly(0), DURATION))
        .unwrap();
    let e = parse("now() > 1h").unwrap();
    assert!(Condition::new(&e, &env).is_ok());
}
