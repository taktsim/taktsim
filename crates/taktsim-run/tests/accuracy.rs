//! What a row over replications can claim (ADR-0025 §5, §6): one way to a replicated
//! value, by the table of the ADR.

use taktsim_model::{Encode, Format, Json, decode_str};
use taktsim_run::{Accuracy, AccuracyKind, AccuracyPlan, Replicated, SamplingInterval};

#[allow(clippy::unwrap_used)] // Test helper.
fn row(values: &[Option<f64>], plan: &AccuracyPlan) -> Replicated {
    Accuracy::from_replications(values, plan).unwrap().unwrap()
}

fn some(values: &[f64]) -> Vec<Option<f64>> {
    values.iter().copied().map(Some).collect()
}

fn reason(accuracy: &Accuracy) -> Option<&str> {
    match &accuracy.kind {
        AccuracyKind::None { reason, .. } => Some(reason),
        AccuracyKind::Sampling(_)
        | AccuracyKind::Censored { .. }
        | AccuracyKind::PartlyUndefined { .. }
        | AccuracyKind::Unknown { .. } => None,
    }
}

#[allow(clippy::panic)] // Test helper.
fn sampling(accuracy: &Accuracy) -> &SamplingInterval {
    let AccuracyKind::Sampling(interval) = &accuracy.kind else {
        panic!("not a sampling entry: {accuracy:?}");
    };
    interval
}

/// `35.5 ± 0.000000` would read as certainty, so identical values state no interval.
#[test]
fn identical_replications_state_no_interval() {
    let row = row(&some(&[12.0; 20]), &AccuracyPlan::fixed());
    assert_eq!(row.value, 12.0);
    assert_eq!(row.accuracy.len(), 1);
    assert_eq!(reason(&row.accuracy[0]), Some("no_variation_observed"));
    let text = row.accuracy[0].encode().to_string();
    assert!(
        text.contains("all 20 replications gave this value"),
        "{text}"
    );
    assert!(text.contains("would not be certainty"), "{text}");
}

/// ADR-0014's rule is met, so `reached` says so, and the accuracy says what it is worth.
#[test]
fn identical_replications_reach_a_target_from_n0_on() {
    let plan = AccuracyPlan::sequential().with_target("2%".parse().unwrap());
    let row_of_ten = row(&some(&[5.0; 10]), &plan);
    assert_eq!(row_of_ten.accuracy[0].reached, Some(true));
    assert_eq!(
        row_of_ten.accuracy[0].target.unwrap().relative,
        Some(0.02),
        "the target is stated"
    );
    let row_of_nine = row(&some(&[5.0; 9]), &plan);
    assert_eq!(row_of_nine.accuracy[0].reached, Some(false), "below n₀");
}

/// "All equal" is about the values, not a standard deviation that rounds to zero.
#[test]
fn nearly_equal_values_have_an_interval() {
    let next = f64::from_bits(1.0_f64.to_bits() + 1);
    let row = row(&some(&[1.0, next]), &AccuracyPlan::fixed());
    assert!(matches!(row.accuracy[0].kind, AccuracyKind::Sampling(_)));
}

#[test]
fn replicated_values_have_a_sampling_entry_with_the_mean() {
    let row = row(&some(&[1.0, 2.0, 3.0]), &AccuracyPlan::fixed());
    assert_eq!(row.value, 2.0);
    let interval = sampling(&row.accuracy[0]);
    assert_eq!(interval.mean, 2.0);
    assert_eq!(interval.replications, 3);
    assert_eq!(interval.stopping, None);
    assert_eq!(row.accuracy[0].target, None);
    assert_eq!(row.accuracy[0].reached, None);
}

/// `stopping: sequential` says the level is nominal: on every row of such an
/// experiment, with a target or not (ADR-0025 §7).
#[test]
fn sequential_says_so() {
    let row = row(&some(&[1.0, 2.0, 3.0]), &AccuracyPlan::sequential());
    assert_eq!(
        sampling(&row.accuracy[0]).stopping.as_deref(),
        Some("sequential")
    );
    let text = row.accuracy[0].encode().to_string();
    assert!(text.contains("\"stopping\":\"sequential\""), "{text}");
}

#[test]
fn a_target_is_reached_by_the_rule_of_adr_0014() {
    let wide = AccuracyPlan::sequential().with_target("90%".parse().unwrap());
    let values = some(&[9.0, 11.0, 9.0, 11.0, 9.0, 11.0, 9.0, 11.0, 9.0, 11.0]);
    assert_eq!(row(&values, &wide).accuracy[0].reached, Some(true));
    let narrow = AccuracyPlan::sequential().with_target("1%".parse().unwrap());
    assert_eq!(row(&values, &narrow).accuracy[0].reached, Some(false));
    // Within the target, but below n₀.
    assert_eq!(row(&values[..9], &wide).accuracy[0].reached, Some(false));
}

#[test]
fn one_run_is_a_single_replication() {
    let row = row(&some(&[5.0]), &AccuracyPlan::fixed());
    assert_eq!(row.value, 5.0);
    assert_eq!(reason(&row.accuracy[0]), Some("single_replication"));
}

#[test]
fn one_defined_value_of_several() {
    let row = row(&[None, Some(7.0), None], &AccuracyPlan::fixed());
    assert_eq!(row.value, 7.0);
    assert_eq!(reason(&row.accuracy[0]), Some("one_defined_value"));
    let text = row.accuracy[0].encode().to_string();
    assert!(text.contains("3 replications ran"), "{text}");
    assert!(text.contains("more seeds may not help"), "{text}");
    assert!(matches!(
        row.accuracy[1].kind,
        AccuracyKind::PartlyUndefined {
            undefined: 2,
            of: 3,
            ..
        }
    ));
}

/// The mean is over runs that defined the row, conditional on that, and the row says
/// so whatever its first statement.
#[test]
fn partly_undefined_states_the_count() {
    let values = [Some(1.0), None, Some(3.0), Some(5.0)];
    let row = row(&values, &AccuracyPlan::fixed());
    assert_eq!(row.value, 3.0);
    assert_eq!(sampling(&row.accuracy[0]).replications, 3);
    let entry = row.accuracy[1].encode();
    assert_eq!(entry["kind"], "partly_undefined");
    assert_eq!(entry["undefined"], Json::from(1));
    assert_eq!(entry["of"], Json::from(4));

    let equal = [Some(2.0), None, Some(2.0)];
    let row = self::row(&equal, &AccuracyPlan::fixed());
    assert_eq!(reason(&row.accuracy[0]), Some("no_variation_observed"));
    assert!(matches!(
        row.accuracy[1].kind,
        AccuracyKind::PartlyUndefined { .. }
    ));

    // A row undefined in a replication never reaches its target.
    let plan = AccuracyPlan::sequential().with_target("90%".parse().unwrap());
    let mut values = some(&[10.0; 12]);
    values[3] = None;
    assert_eq!(self::row(&values, &plan).accuracy[0].reached, Some(false));
}

#[test]
fn undefined_in_all_is_no_row() {
    assert_eq!(
        Accuracy::from_replications(&[None, None, None], &AccuracyPlan::fixed()).unwrap(),
        None
    );
    assert_eq!(
        Accuracy::from_replications(&[], &AccuracyPlan::fixed()).unwrap(),
        None
    );
}

#[test]
fn a_value_that_is_not_finite_is_a_typed_error() {
    let values = some(&[1.0, f64::NAN]);
    assert!(Accuracy::from_replications(&values, &AccuracyPlan::fixed()).is_err());
}

/// The new kinds read back as themselves, and without their keys are refused.
#[test]
fn new_kinds_round_trip() {
    for accuracy in [
        Accuracy::censored(2.5),
        row(&[None, Some(1.0), Some(2.0)], &AccuracyPlan::sequential()).accuracy[1].clone(),
        row(&[None, Some(1.0), Some(2.0)], &AccuracyPlan::sequential()).accuracy[0].clone(),
    ] {
        let read: Accuracy = decode_str(&accuracy.encode().to_string(), Format::Json).unwrap();
        assert_eq!(read, accuracy);
    }
    for broken in [
        "{kind: censored, detail: d}",
        "{kind: censored, units_inside: 1}",
        "{kind: partly_undefined, of: 3, detail: d}",
        "{kind: partly_undefined, undefined: 1, detail: d}",
    ] {
        assert!(
            decode_str::<Accuracy>(broken, Format::Yaml).is_err(),
            "{broken}"
        );
    }
}
