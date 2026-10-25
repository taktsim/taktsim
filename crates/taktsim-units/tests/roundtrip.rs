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

/// Finite non-negative floats across every magnitude (zero, subnormal,
/// normal), plus a dense sample of realistic values.
fn non_negative_f64() -> impl Strategy<Value = f64> {
    use proptest::num::f64::{NORMAL, POSITIVE, SUBNORMAL, ZERO};
    prop_oneof![POSITIVE | ZERO | SUBNORMAL | NORMAL, 0.0_f64..1_000.0]
}

/// The nearest tick to `secs`, computed from its exact decimal expansion
/// (an f64 has at most 1074 fractional digits), independently of the
/// integer method in `SimDuration::from_secs_f64`.
#[allow(clippy::unwrap_used)] // test helper; the expansion always has this shape
fn nearest_tick_reference(secs: f64) -> Option<u64> {
    let text = format!("{secs:.1100}");
    let (int, frac) = text.split_once('.').unwrap();
    let (ns_digits, tail) = frac.split_at(9);
    let nanos = int.parse::<u128>().unwrap() * 1_000_000_000 + ns_digits.parse::<u128>().unwrap();
    let tail = tail.trim_end_matches('0');
    let round_up = match tail.as_bytes().first() {
        None => false,
        Some(b'5') if tail.len() == 1 => nanos % 2 == 1,
        Some(&d) => d >= b'5',
    };
    u64::try_from(nanos + u128::from(round_up)).ok()
}

proptest! {
    #[test]
    fn sampled_durations_match_the_decimal_reference(
        // Non-negative floats order like their bits, so this covers zero,
        // subnormals and every magnitude up to 2e10 s (beyond 2^64 ns).
        secs in (0..=2e10_f64.to_bits()).prop_map(f64::from_bits)
    ) {
        prop_assert_eq!(
            SimDuration::from_secs_f64(secs).ok().map(SimDuration::as_nanos),
            nearest_tick_reference(secs)
        );
    }

    #[test]
    fn sampled_ties_match_the_decimal_reference(k in 0_u64..1 << 40, shift in 1_u32..30) {
        // k × 2^-shift seconds includes exact ties (e.g. 1/1024 s).
        #[allow(clippy::cast_precision_loss)] // k < 2^40 is exact in f64
        let secs = k as f64 / f64::from(1_u32 << shift);
        prop_assert_eq!(
            SimDuration::from_secs_f64(secs).ok().map(SimDuration::as_nanos),
            nearest_tick_reference(secs)
        );
    }
}

proptest! {
    #[test]
    fn durations_round_trip(nanos: u64) {
        let d = SimDuration::from_nanos(nanos);
        prop_assert_eq!(d.to_string().parse::<SimDuration>().unwrap(), d);
    }

    #[test]
    fn instants_round_trip(nanos: u64) {
        let t = SimTime::from_nanos(nanos);
        prop_assert_eq!(t.to_string().parse::<SimTime>().unwrap(), t);
        prop_assert_eq!(t.to_string(), SimDuration::from_nanos(nanos).to_string());
    }

    #[test]
    fn seconds_round_trip_through_f64(nanos in 0_u64..(1 << 52)) {
        // Below 2^52 ns the f64 error of as_secs_f64 is under half a tick,
        // so rounding to the nearest tick recovers the duration.
        let d = SimDuration::from_nanos(nanos);
        prop_assert_eq!(SimDuration::from_secs_f64(d.as_secs_f64()).unwrap(), d);
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
    fn rates_round_trip(per_second in non_negative_f64()) {
        let r = Rate::per_second(per_second).unwrap();
        prop_assert_eq!(r.to_string().parse::<Rate>().unwrap(), r);
    }

    #[test]
    fn ratios_round_trip(percent in non_negative_f64()) {
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
