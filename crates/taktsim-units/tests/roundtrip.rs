//! Display and parsing are inverse: every value survives `to_string` then
//! `parse` unchanged (roadmap M1).

use proptest::prelude::*;
use taktsim_units::{Count, Rate, Ratio, SimDuration, SimTime};

/// Nanoseconds per unit, as accepted by the parser.
const UNITS: [(&str, u64); 5] = [
    ("d", 86_400_000_000_000),
    ("h", 3_600_000_000_000),
    ("min", 60_000_000_000),
    ("s", 1_000_000_000),
    ("ms", 1_000_000),
];

proptest! {
    #[test]
    fn durations_round_trip(nanos: u64) {
        let d = SimDuration::from_nanos(nanos);
        prop_assert_eq!(d.to_string().parse::<SimDuration>().unwrap(), d);
    }

    #[test]
    fn instants_display_like_durations(nanos: u64) {
        prop_assert_eq!(
            SimTime::from_nanos(nanos).to_string(),
            SimDuration::from_nanos(nanos).to_string()
        );
    }

    #[test]
    fn whole_numbers_of_any_unit_parse_exactly(
        n in 0_u64..1_000_000,
        unit in 0_usize..UNITS.len(),
        space: bool,
    ) {
        let (name, factor) = UNITS[unit];
        let text = format!("{n}{}{name}", if space { " " } else { "" });
        // Overflow (beyond about 584 years) exactly when the integers overflow.
        prop_assert_eq!(
            text.parse::<SimDuration>().ok().map(SimDuration::as_nanos),
            n.checked_mul(factor)
        );
    }

    #[test]
    fn decimal_seconds_parse_exactly(secs in 0_u64..1_000_000, frac in 0_u64..1_000_000_000) {
        let text = format!("{secs}.{frac:09}s");
        prop_assert_eq!(
            text.parse::<SimDuration>().unwrap().as_nanos(),
            secs * 1_000_000_000 + frac
        );
    }

    #[test]
    fn counts_round_trip(n: u64) {
        let c = Count::new(n);
        prop_assert_eq!(c.to_string().parse::<Count>().unwrap(), c);
    }

    #[test]
    fn rates_round_trip(per_second in 0.0_f64..=f64::MAX) {
        let r = Rate::per_second(per_second).unwrap();
        prop_assert_eq!(r.to_string().parse::<Rate>().unwrap(), r);
    }

    #[test]
    fn ratios_round_trip(percent in 0.0_f64..=f64::MAX) {
        let r = Ratio::from_percent(percent).unwrap();
        prop_assert_eq!(r.to_string().parse::<Ratio>().unwrap(), r);
    }

    #[test]
    fn checked_arithmetic_matches_integers(a: u64, b: u64) {
        let (da, db) = (SimDuration::from_nanos(a), SimDuration::from_nanos(b));
        prop_assert_eq!(da.checked_add(db).ok().map(SimDuration::as_nanos), a.checked_add(b));
        prop_assert_eq!(da.checked_sub(db).ok().map(SimDuration::as_nanos), a.checked_sub(b));
        prop_assert_eq!(da.checked_mul(b).ok().map(SimDuration::as_nanos), a.checked_mul(b));
        let t = SimTime::from_nanos(a);
        prop_assert_eq!(t.checked_add(db).ok().map(SimTime::as_nanos), a.checked_add(b));
        prop_assert_eq!(
            t.checked_duration_since(SimTime::from_nanos(b)).ok().map(SimDuration::as_nanos),
            a.checked_sub(b)
        );
        let (ca, cb) = (Count::new(a), Count::new(b));
        prop_assert_eq!(ca.checked_add(cb).ok().map(Count::get), a.checked_add(b));
        prop_assert_eq!(ca.checked_sub(cb).ok().map(Count::get), a.checked_sub(b));
    }

    #[test]
    fn parsing_arbitrary_text_never_panics(text in "\\PC{0,24}") {
        let _ = text.parse::<SimDuration>();
        let _ = text.parse::<Count>();
        let _ = text.parse::<Rate>();
        let _ = text.parse::<Ratio>();
    }
}
