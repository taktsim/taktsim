//! Evaluation of conditions: every operator on every type it takes, exact
//! arithmetic at the extremes, IEEE semantics for floating point, and the
//! errors for environments that do not match their declared types.
//!
//! Expected values are computed by hand in the comments.

mod common;

use std::collections::BTreeMap;

use common::env;
use taktsim_model::expr::{EvalError, ExprKind, Input, InputKind, Path, Type, Value, condition};
use taktsim_units::{Count, Rate, Ratio, SimDuration};

#[allow(clippy::unwrap_used)] // test helper
fn d(s: &str) -> SimDuration {
    s.parse().unwrap()
}

/// A unit and the observables, with every input of the test environment.
#[allow(clippy::unwrap_used)] // test helper
fn values() -> BTreeMap<String, Value<'static>> {
    BTreeMap::from([
        ("variant".to_owned(), Value::Text("premium")),
        ("product".to_owned(), Value::Text("frame")),
        ("quality".to_owned(), Value::Text("rework")),
        ("unit.priority".to_owned(), Value::Int(3)),
        ("unit.rank".to_owned(), Value::Int(-2)),
        ("unit.age".to_owned(), Value::Time(d("90s"))),
        ("unit.batch".to_owned(), Value::Count(Count::new(5))),
        ("unit.weight".to_owned(), Value::Number(2.5)),
        ("unit.reworked".to_owned(), Value::Bool(true)),
        ("unit.label".to_owned(), Value::Text("red")),
        ("unit.share".to_owned(), Value::Ratio("2%".parse().unwrap())),
        (
            "unit.rate".to_owned(),
            Value::Rate("60 parts/h".parse().unwrap()),
        ),
        (
            "throughput(sink)".to_owned(),
            Value::Rate("120 parts/h".parse().unwrap()),
        ),
        ("wip(buf1)".to_owned(), Value::Count(Count::new(12))),
        ("lead_time(sink)".to_owned(), Value::Time(d("45min"))),
        (
            "state_shares(cut)".to_owned(),
            Value::Ratio("80%".parse().unwrap()),
        ),
    ])
}

#[track_caller]
#[allow(clippy::panic)] // test helper
fn eval_with(src: &str, values: &BTreeMap<String, Value<'_>>) -> bool {
    let cond = condition(src, &env()).unwrap_or_else(|e| panic!("{src:?}: {e}"));
    cond.eval(values).unwrap_or_else(|e| panic!("{src:?}: {e}"))
}

#[track_caller]
fn eval(src: &str) -> bool {
    eval_with(src, &values())
}

/// Each case is `(source, expected value)`.
#[track_caller]
fn table(cases: &[(&str, bool)]) {
    for &(src, expected) in cases {
        assert_eq!(eval(src), expected, "{src}");
    }
}

#[test]
fn literals_and_bool_inputs() {
    table(&[
        ("true", true),
        ("false", false),
        ("unit.reworked", true),
        ("(unit.reworked)", true),
    ]);
}

#[test]
fn not_and_or_truth_tables() {
    for a in [false, true] {
        for b in [false, true] {
            let mut v = values();
            v.insert("unit.reworked".into(), Value::Bool(a));
            let b_text = if b { "true" } else { "false" };
            let and = format!("unit.reworked and {b_text}");
            let or = format!("unit.reworked or {b_text}");
            assert_eq!(eval_with(&and, &v), a && b, "{and} with {a}");
            assert_eq!(eval_with(&or, &v), a || b, "{or} with {a}");
            assert_eq!(eval_with("not unit.reworked", &v), !a);
        }
    }
}

#[test]
fn equality_on_every_type_that_has_it() {
    table(&[
        // bool
        ("unit.reworked == true", true),
        ("unit.reworked != true", false),
        ("(1 < 2) == (2 < 1)", false),
        // int
        ("unit.priority == 3", true),
        ("unit.priority != 3", false),
        ("unit.rank == -2", true),
        // duration: 90s == 1.5min exactly
        ("unit.age == 1.5min", true),
        ("unit.age != 90000ms", false),
        ("lead_time(sink) == 0.75h", true),
        // count: batch = 5, wip = 12; counts are exact integers
        ("unit.batch == 5 parts", true),
        ("unit.batch != 5 parts", false),
        ("5 parts == unit.batch", true),
        ("wip(buf1) == 12 parts", true),
        ("wip(buf1) != unit.batch", true),
        ("unit.batch == wip(buf1)", false),
        ("unit.batch + 7 parts == wip(buf1)", true),
        ("unit.batch * 2 + 2 parts == wip(buf1)", true),
        ("unit.batch * unit.rank == 0 parts - 10 parts", true),
        ("unit.batch - 10 parts != 5 parts", true),
        // enum and string
        ("variant == 'premium'", true),
        ("variant != 'premium'", false),
        ("'basic' == variant", false),
        ("unit.label == 'red'", true),
        ("unit.label != 'blue'", true),
        ("'a' == 'a'", true),
    ]);
}

#[test]
fn ordering_on_every_numeric_type() {
    // Each type at, below and above the boundary.
    table(&[
        ("unit.priority < 3", false),
        ("unit.priority <= 3", true),
        ("unit.priority > 2", true),
        ("unit.priority >= 4", false),
        ("unit.weight < 2.5", false),
        ("unit.weight <= 2.5", true),
        ("unit.weight > 2.25", true),
        ("unit.weight >= 2.75", false),
        ("unit.age < 90s", false),
        ("unit.age <= 90s", true),
        ("unit.age > 89.999999999s", true),
        ("unit.age >= 1.5000000001min", false),
        ("wip(buf1) < 12 parts", false),
        ("wip(buf1) <= 12 parts", true),
        ("wip(buf1) > 11 parts", true),
        ("wip(buf1) >= 13 parts", false),
        // 120 parts/h and 2 parts/min are the same f64: 1/30 per second.
        ("throughput(sink) < 2 parts/min", false),
        ("throughput(sink) <= 2 parts/min", true),
        ("throughput(sink) > 119 parts/h", true),
        ("throughput(sink) >= 121 parts/h", false),
        ("unit.share < 2%", false),
        ("unit.share <= 2%", true),
        ("unit.share > 1.5%", true),
        ("unit.share >= 2.5%", false),
    ]);
}

#[test]
fn int_arithmetic() {
    // priority = 3, rank = -2
    table(&[
        ("unit.priority + unit.rank == 1", true),
        ("unit.priority - unit.rank == 5", true),
        ("unit.priority * unit.rank == -6", true),
        ("-unit.priority == -3", true),
        ("-unit.rank == 2", true),
        ("2 + 3 * 4 == 14", true),
        ("(2 + 3) * 4 == 20", true),
        ("10 - 4 - 3 == 3", true),
    ]);
}

#[test]
fn number_arithmetic() {
    // weight = 2.5; every result below is exact in binary.
    table(&[
        (
            "unit.weight + 0.5 >= 3.0 and unit.weight + 0.5 <= 3.0",
            true,
        ),
        (
            "unit.weight - 3.0 < -0.4 and unit.weight - 3.0 > -0.6",
            true,
        ),
        (
            "unit.weight * 4.0 >= 10.0 and unit.weight * 4.0 <= 10.0",
            true,
        ),
        (
            "unit.weight / 2.0 >= 1.25 and unit.weight / 2.0 <= 1.25",
            true,
        ),
        ("-unit.weight < -2.4", true),
    ]);
}

#[test]
fn duration_arithmetic() {
    // age = 90s, lead_time(sink) = 45min
    table(&[
        ("unit.age + 30s == 2min", true),
        ("lead_time(sink) - unit.age == 43.5min", true),
        ("2 * unit.age == 3min", true),
        ("unit.age * 40 == 1h", true),
        ("unit.age * unit.priority == 4.5min", true),
    ]);
}

#[test]
fn negative_intermediate_durations_and_counts_are_exact() {
    // 90s - 1h = -58.5min; quantities are never negative, but an
    // intermediate result may be.
    table(&[
        ("unit.age - 1h < 0s", true),
        ("unit.age - 1h + 1h == unit.age", true),
        ("unit.age - 1h == 0s - 58.5min", true),
        // batch = 5: 5 - 10 = -5 parts
        ("unit.batch - 10 parts < 0 parts", true),
        ("unit.batch - 10 parts + 15 parts >= 10 parts", true),
    ]);
}

#[test]
fn count_arithmetic() {
    // batch = 5, wip = 12
    table(&[
        (
            "unit.batch + wip(buf1) >= 17 parts and unit.batch + wip(buf1) <= 17 parts",
            true,
        ),
        ("wip(buf1) - unit.batch > 6 parts", true),
        (
            "unit.batch * 3 > 14 parts and 3 * unit.batch < 16 parts",
            true,
        ),
        ("unit.batch * unit.rank < 0 parts", true),
    ]);
}

#[test]
fn rate_arithmetic() {
    // throughput = 120 parts/h, unit.rate = 60 parts/h
    table(&[
        ("throughput(sink) + unit.rate > 179 parts/h", true),
        ("throughput(sink) + unit.rate < 181 parts/h", true),
        ("throughput(sink) - unit.rate > 59 parts/h", true),
        ("throughput(sink) - unit.rate < 61 parts/h", true),
        ("throughput(sink) * 1.5 > 179 parts/h", true),
        ("0.5 * throughput(sink) < 61 parts/h", true),
        ("throughput(sink) / 4.0 > 29 parts/h", true),
        ("throughput(sink) / 4.0 < 31 parts/h", true),
        // wip = 12 parts over 1h is 12 parts/h; over 30min, 24 parts/h.
        (
            "wip(buf1) / 1h > 11.9 parts/h and wip(buf1) / 1h < 12.1 parts/h",
            true,
        ),
        (
            "wip(buf1) / 30min > 23.9 parts/h and wip(buf1) / 30min < 24.1 parts/h",
            true,
        ),
        ("2 parts / 1min >= 120 parts/h", true),
    ]);
}

#[test]
fn ratio_arithmetic() {
    // share = 2%, state_shares(cut) = 80%
    table(&[
        ("unit.share + 1% >= 3% and unit.share + 1% <= 3%", true),
        ("state_shares(cut) - unit.share > 77.9%", true),
        ("unit.share * 2.5 >= 5% and unit.share * 2.5 <= 5%", true),
        (
            "state_shares(cut) / 4.0 >= 20% and state_shares(cut) / 4.0 <= 20%",
            true,
        ),
    ]);
}

#[test]
fn membership_on_every_type_that_has_it() {
    table(&[
        ("unit.priority in [1, 2, 3]", true),
        ("unit.priority in [1, 2]", false),
        ("unit.priority in [unit.rank, (-unit.rank)]", false),
        ("unit.priority in [(unit.rank + 5)]", true),
        // A list element may be negated without parentheses.
        ("unit.rank in [-2]", true),
        ("unit.rank in [-1, -2, -3]", true),
        ("unit.rank in [-1, 2]", false),
        ("unit.priority in [-unit.rank]", false),
        ("unit.priority in [1, -unit.rank, 3]", true),
        ("unit.rank in [-unit.priority, -(2)]", true),
        ("unit.rank in [-(unit.priority - 1)]", true),
        // count
        ("unit.batch in [5 parts]", true),
        ("unit.batch in [1 parts, 5 parts, 9 parts]", true),
        ("unit.batch in [1 parts, 2 parts]", false),
        ("wip(buf1) in [unit.batch, (unit.batch + 7 parts)]", true),
        ("unit.age in [1min, 90s]", true),
        ("unit.age in [1min]", false),
        ("unit.reworked in [false]", false),
        ("unit.reworked in [false, true]", true),
        ("variant in ['basic', 'premium']", true),
        ("variant in ['basic', 'deluxe']", false),
        ("'frame' in [product]", true),
        ("unit.label in ['red']", true),
        ("not variant in ['basic']", true),
    ]);
}

#[test]
fn exact_arithmetic_has_no_overflow_at_the_extremes() {
    let mut v = values();
    v.insert("unit.priority".into(), Value::Int(i64::MIN));
    v.insert("unit.rank".into(), Value::Int(i64::MIN));
    v.insert("unit.age".into(), Value::Time(SimDuration::MAX));
    v.insert("unit.batch".into(), Value::Count(Count::new(u64::MAX)));
    // (-2^63)^2 = 2^126 > 0, and the negation of i64::MIN is 2^63.
    assert!(eval_with("unit.priority * unit.rank > 0", &v));
    assert!(eval_with("-unit.priority == 9223372036854775808", &v));
    // (2^64 - 1 ns) + (2^64 - 1 ns) > 2^64 - 1 ns, beyond SimDuration.
    assert!(eval_with("unit.age + unit.age > unit.age", &v));
    assert!(eval_with("unit.age * unit.priority < 0s", &v));
    // Counts compare exactly (ADR-0015), also beyond the u64 range.
    let back = "unit.batch * 2 - unit.batch";
    assert!(eval_with(&format!("{back} == unit.batch"), &v));
    assert!(eval_with(
        "unit.batch + unit.batch != unit.batch and unit.batch + unit.batch > unit.batch",
        &v
    ));
    assert!(eval_with("unit.batch == 18446744073709551615 parts", &v));
    assert!(eval_with(
        "18446744073709551615 * 9223372036854775807 > 18446744073709551615",
        &v
    ));
}

#[test]
fn floating_point_follows_ieee_754() {
    let mut v = values();
    let big = 1e308;
    v.insert("unit.weight".into(), Value::Number(big));
    let big_literal = format!("1{}.0", "0".repeat(308));
    // 1e308 * 10 overflows to +infinity, which is larger than any number.
    assert!(eval_with(
        &format!("unit.weight * 10.0 > {big_literal}"),
        &v
    ));
    // infinity - infinity is NaN: every ordering comparison with NaN is
    // false, so `x < 1` and `not (x < 1)` are both what IEEE says.
    let nan = "unit.weight * 10.0 - unit.weight * 10.0";
    assert!(!eval_with(&format!("{nan} < 1.0"), &v));
    assert!(!eval_with(&format!("{nan} >= 1.0"), &v));
    assert!(eval_with(&format!("not ({nan} < 1.0)"), &v));
}

#[test]
fn rates_from_counts_match_count_per() {
    // `n / d` evaluates as taktsim-units' `Count::per`.
    let rate = Count::new(12).per(d("1h")).unwrap();
    let mut v = values();
    v.insert("unit.rate".into(), Value::Rate(rate));
    assert!(eval_with(
        "unit.rate <= wip(buf1) / 1h and unit.rate >= wip(buf1) / 1h",
        &v
    ));
    let tiny = Rate::per_second(0.0).unwrap();
    v.insert("unit.rate".into(), Value::Rate(tiny));
    assert!(eval_with("unit.rate < 1 parts/h", &v));
}

#[test]
fn every_input_is_read() {
    // `true or x` would short-circuit; evaluation still reads `x`, so a
    // missing value is always reported.
    let mut v = values();
    v.remove("unit.reworked");
    let cond = condition("true or unit.reworked", &env()).unwrap();
    assert_eq!(
        cond.eval(&v),
        Err(EvalError::Missing {
            input: Input::path(Path::parse("unit.reworked").unwrap()),
            expected: Type::Bool,
        })
    );
    let cond = condition("unit.priority in [1, unit.rank]", &env()).unwrap();
    v.remove("unit.rank");
    assert!(matches!(cond.eval(&v), Err(EvalError::Missing { .. })));
}

#[test]
fn values_of_the_wrong_type_are_errors() {
    let cases: [(&str, &str, Value<'static>); 9] = [
        ("unit.reworked", "unit.reworked", Value::Int(1)),
        ("unit.priority > 1", "unit.priority", Value::Number(1.0)),
        ("unit.age > 1s", "unit.age", Value::Count(Count::new(1))),
        ("unit.batch > 1 parts", "unit.batch", Value::Time(d("1s"))),
        ("unit.weight > 1.0", "unit.weight", Value::Text("1")),
        (
            "unit.rate > 1 parts/h",
            "unit.rate",
            Value::Ratio(Ratio::from_percent(1.0).unwrap()),
        ),
        (
            "unit.share > 1%",
            "unit.share",
            Value::Rate(Rate::per_second(1.0).unwrap()),
        ),
        ("unit.label == 'a'", "unit.label", Value::Bool(true)),
        ("variant == 'basic'", "variant", Value::Int(2)),
    ];
    for (src, input, value) in cases {
        let mut v = values();
        v.insert(input.into(), value);
        let err = condition(src, &env()).unwrap().eval(&v).unwrap_err();
        assert!(
            matches!(&err, EvalError::WrongType { found, .. } if *found == value.kind()),
            "{src}: {err:?}"
        );
    }
    let mut v = values();
    v.insert("variant".into(), Value::Int(2));
    assert_eq!(
        condition("variant == 'basic'", &env())
            .unwrap()
            .eval(&v)
            .unwrap_err()
            .to_string(),
        "`variant` is an int, but the condition needs an enum `variant`"
    );
}

#[test]
fn numbers_must_be_finite() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut v = values();
        v.insert("unit.weight".into(), Value::Number(bad));
        let err = condition("unit.weight > 1.0", &env())
            .unwrap()
            .eval(&v)
            .unwrap_err();
        assert!(matches!(err, EvalError::NotFinite { .. }), "{bad}");
    }
    let mut v = values();
    v.insert("unit.weight".into(), Value::Number(f64::INFINITY));
    assert_eq!(
        condition("unit.weight > 1.0", &env())
            .unwrap()
            .eval(&v)
            .unwrap_err()
            .to_string(),
        "`unit.weight` is inf, which is not a finite number; supply a finite one, e.g. 2.5"
    );
}

#[test]
fn enum_values_must_be_allowed() {
    let mut v = values();
    v.insert("quality".into(), Value::Text("gold"));
    let err = condition("quality == 'ok'", &env())
        .unwrap()
        .eval(&v)
        .unwrap_err();
    assert_eq!(
        err,
        EvalError::NotAllowed {
            input: Input::path(Path::parse("quality").unwrap()),
            value: "gold".into(),
            allowed: vec!["ok".into(), "rework".into(), "scrap".into()],
        }
    );
    assert_eq!(
        err.to_string(),
        "`quality` is 'gold', which is not one of its values: ok, rework, scrap"
    );
    // A string takes any text.
    v.insert("unit.label".into(), Value::Text("gold"));
    assert!(eval_with("unit.label == 'gold'", &v));
}

#[test]
fn a_missing_value_names_its_type() {
    let empty: BTreeMap<String, Value<'_>> = BTreeMap::new();
    let err = condition("lead_time(sink) > 1h", &env())
        .unwrap()
        .eval(&empty)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "no value for `lead_time(sink)`; supply a value of type duration"
    );
    let err = condition("variant == 'basic'", &env())
        .unwrap()
        .eval(&empty)
        .unwrap_err();
    assert!(
        matches!(err, EvalError::Missing { expected: Type::Enum(name), .. } if name == "variant")
    );
    let err = condition("unit.label == 'basic'", &env())
        .unwrap()
        .eval(&empty)
        .unwrap_err();
    assert!(matches!(
        err,
        EvalError::Missing {
            expected: Type::String,
            ..
        }
    ));
}

#[test]
fn conditions_expose_their_tree_and_inputs() {
    let cond = condition("variant == 'premium' and wip(buf1) > 3 parts", &env()).unwrap();
    assert_eq!(
        cond.expr().to_string(),
        "variant == 'premium' and wip(buf1) > 3 parts"
    );
    let inputs: Vec<String> = cond.inputs().keys().map(ToString::to_string).collect();
    assert_eq!(inputs, ["variant", "wip(buf1)"]);
}

#[test]
fn an_input_is_keyed_by_the_text_its_call_prints_as() {
    // One printer for calls and inputs: the key a value map uses is the
    // call's canonical text.
    let src = "state_shares(cut, weld-1) > 50%";
    let cond = condition(src, &env()).unwrap();
    let ExprKind::Compare { lhs, .. } = cond.expr().kind() else {
        panic!("{src}")
    };
    let (input, _) = cond.inputs().iter().next().unwrap();
    assert_eq!(input.as_str(), lhs.to_string());
    assert_eq!(input.to_string(), "state_shares(cut, weld-1)");
    assert!(matches!(input.kind(), InputKind::Call { args, .. } if args.len() == 2));
    let values = BTreeMap::from([(input.to_string(), Value::Ratio("80%".parse().unwrap()))]);
    assert!(cond.eval(&values).unwrap());
}

#[test]
fn evaluation_is_deterministic() {
    let src = "variant in ['premium', 'deluxe'] and unit.age * 2 > lead_time(sink) / 1 \
               or throughput(sink) * 0.5 >= unit.rate";
    // `/ 1` on a duration is rejected; use the valid form instead.
    assert!(condition(src, &env()).is_err());
    let src = "variant in ['premium', 'deluxe'] and unit.age * 2 > 1min \
               or throughput(sink) * 0.5 >= unit.rate";
    let cond = condition(src, &env()).unwrap();
    let first = cond.eval(&values()).unwrap();
    for _ in 0..100 {
        assert_eq!(cond.eval(&values()).unwrap(), first);
    }
    assert!(first);
}
