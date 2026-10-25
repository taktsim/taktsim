//! Property tests: CSV export and import are exact inverses, timestamps
//! round-trip through RFC 3339, and no input makes the reader panic.

#![cfg(feature = "csv")]

use proptest::prelude::*;
use taktsim_adapters::csv::{QualityIssue, ReadOptions, read_observations, write_observations};
use taktsim_adapters::observation::{
    Measurement, Name, Observation, ObservationTable, SourceOffset, Timestamp, Unit, UtcOffset,
};

/// A row before its names are borrowed: (nanos, offset, entity, signal,
/// value, unit), names as indexes into a pool.
type Row = (i64, SourceOffset, usize, usize, f64, Unit);

#[allow(clippy::unwrap_used)] // test helper
fn offset() -> impl Strategy<Value = SourceOffset> {
    prop_oneof![
        1 => Just(SourceOffset::Unknown),
        1 => Just(SourceOffset::Known(UtcOffset::UTC)),
        4 => (-1439_i16..=1439).prop_map(|m| SourceOffset::Known(UtcOffset::from_minutes(m).unwrap())),
    ]
}

fn name() -> impl Strategy<Value = Name> {
    // Separators, quotes and non-ASCII text, which the writer must quote or
    // pass through; names with surrounding spaces are not valid and dropped.
    "[A-Za-z0-9_ ,\"'.;=/äé工-]{1,12}".prop_filter_map("not a valid name", |s| Name::new(&s).ok())
}

fn value() -> impl Strategy<Value = f64> {
    prop_oneof![
        any::<u64>()
            .prop_map(f64::from_bits)
            .prop_filter("finite", |x| x.is_finite()),
        (-100_000_i32..100_000).prop_map(f64::from),
        (-1_000_000_i32..1_000_000).prop_map(|n| f64::from(n) / 1000.0),
        Just(-0.0),
    ]
}

fn unit() -> impl Strategy<Value = Unit> {
    prop::sample::select(Unit::ALL.to_vec())
}

fn rows(names: usize) -> impl Strategy<Value = Vec<Row>> {
    let timestamp = prop_oneof![
        any::<i64>(),
        // Many rows at a few instants, so ties in the sort are common.
        (0_i64..4).prop_map(|t| 1_772_427_600_000_000_000 + t * 1_000_000_000),
    ];
    prop::collection::vec(
        (timestamp, offset(), 0..names, 0..names, value(), unit()),
        0..40,
    )
}

#[allow(clippy::unwrap_used)] // test helper
fn table(names: &[Name], rows: &[Row]) -> ObservationTable {
    rows.iter()
        .map(
            |&(nanos, source_offset, entity, signal, value, unit)| Observation {
                timestamp: Timestamp::from_unix_nanos(nanos),
                source_offset,
                entity: &names[entity],
                signal: &names[signal],
                measurement: Measurement::new(value, unit).unwrap(),
            },
        )
        .collect()
}

/// A pool of distinct names, so distinct indexes are distinct names, and
/// rows that refer to it.
fn names_and_rows() -> impl Strategy<Value = (Vec<Name>, Vec<Row>)> {
    prop::collection::btree_set(name(), 1..6).prop_flat_map(|names| {
        let names: Vec<Name> = names.into_iter().collect();
        let n = names.len();
        (Just(names), rows(n))
    })
}

/// Days from 1970-01-01 to `year`-`month`-01, counted month by month: an
/// implementation independent of the crate's calendar arithmetic.
fn days_by_counting(year: i64, month: u32) -> i64 {
    let leap = |y: i64| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = |y: i64, m: u32| match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(y) => 29,
        _ => 28,
    };
    let mut days = 0;
    if year >= 1970 {
        for y in 1970..year {
            days += if leap(y) { 366 } else { 365 };
        }
    } else {
        for y in year..1970 {
            days -= if leap(y) { 366 } else { 365 };
        }
    }
    for m in 1..month {
        days += month_days(year, m);
    }
    days
}

proptest! {
    /// Export, then import: the same table, bit for bit, with the same
    /// offsets; exporting it again gives the same bytes.
    #[test]
    fn export_then_import_is_exact((names, rows) in names_and_rows()) {
        let original = table(&names, &rows);
        let csv = write_observations(&original, Vec::new()).unwrap();
        let import = read_observations(csv.as_slice(), &ReadOptions::default()).unwrap();
        prop_assert_eq!(&import.table, &original);
        for (a, b) in import.table.iter().zip(original.iter()) {
            prop_assert_eq!(a.measurement.value().to_bits(), b.measurement.value().to_bits());
            prop_assert_eq!(a.source_offset, b.source_offset);
        }
        prop_assert_eq!(write_observations(&import.table, Vec::new()).unwrap(), csv);
        // Exactly the implausible rows are flagged; none is missing.
        let implausible = original.iter().filter(|o| o.measurement.implausibility().is_some()).count();
        prop_assert_eq!(import.flags.len(), implausible);
        let all_implausible = import.flags.iter().all(|f| matches!(f.issue, QualityIssue::Implausible { .. }));
        prop_assert!(all_implausible);
    }

    /// The table is sorted by (timestamp, entity, signal), whatever order
    /// the rows come in, and ties keep their input order.
    #[test]
    fn rows_are_sorted_stably((names, rows) in names_and_rows()) {
        let table = table(&names, &rows);
        let mut expected: Vec<(i64, &str, &str, usize)> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.0, names[r.2].as_str(), names[r.3].as_str(), i))
            .collect();
        expected.sort_unstable();
        let got: Vec<(i64, &str, &str, u64)> = table
            .iter()
            .map(|o| (o.timestamp.as_unix_nanos(), o.entity.as_str(), o.signal.as_str(), o.measurement.value().to_bits()))
            .collect();
        let want: Vec<(i64, &str, &str, u64)> = expected
            .iter()
            .map(|&(t, e, s, i)| (t, e, s, (rows[i].4 + 0.0).to_bits()))
            .collect();
        prop_assert_eq!(got, want);
    }

    /// Rows with distinct keys give the same table in any input order.
    #[test]
    fn order_of_distinct_rows_does_not_matter(
        (names, rows, shuffled) in names_and_rows().prop_flat_map(|(names, mut rows)| {
            rows.sort_by_key(|r| (r.0, r.2, r.3));
            rows.dedup_by_key(|r| (r.0, r.2, r.3));
            let shuffled = Just(rows.clone()).prop_shuffle();
            (Just(names), Just(rows), shuffled)
        })
    ) {
        prop_assert_eq!(table(&names, &shuffled), table(&names, &rows));
    }

    /// Any instant at any offset: RFC 3339 text that parses back to the
    /// same instant and offset.
    #[test]
    fn timestamps_round_trip(nanos in any::<i64>(), offset in offset()) {
        let t = Timestamp::from_unix_nanos(nanos);
        let text = t.rfc3339(offset).to_string();
        prop_assert_eq!(Timestamp::parse_rfc3339(&text), Ok((t, offset)), "{}", text);
        prop_assert_eq!(text.parse::<Timestamp>(), Ok(t));
    }

    /// Midnight UTC of any date in range is a whole number of days since
    /// the epoch, as counted month by month.
    #[test]
    fn dates_match_a_day_count(year in 1678_i64..=2261, month in 1_u32..=12, day in 1_u32..=28) {
        let text = format!("{year:04}-{month:02}-{day:02}T00:00:00Z");
        let nanos = text.parse::<Timestamp>().unwrap().as_unix_nanos();
        let expected = (days_by_counting(year, month) + i64::from(day) - 1) * 86_400 * 1_000_000_000;
        prop_assert_eq!(nanos, expected, "{}", text);
    }

    /// Arbitrary bytes are read or rejected, never a panic.
    #[test]
    fn arbitrary_input_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let _ = read_observations(bytes.as_slice(), &ReadOptions::default());
        let mut with_header = b"timestamp,entity,signal,value,unit\n".to_vec();
        with_header.extend_from_slice(&bytes);
        let _ = read_observations(with_header.as_slice(), &ReadOptions::default());
    }

    /// Lines built from the pieces of valid rows, shuffled and broken, are
    /// read or rejected, never a panic.
    #[test]
    fn broken_rows_never_panic(lines in prop::collection::vec(
        "(2026-03-02T06:0[0-9]:00(Z|\\+01:00)?|9999-99-99|\"|,|M1|state|-?[0-9.,e]{0,6}|parts|%|1| |\r|\n){0,12}",
        0..12,
    )) {
        let csv = format!("timestamp,entity,signal,value,unit\n{}", lines.join("\n"));
        if let Ok(import) = read_observations(csv.as_bytes(), &ReadOptions::default()) {
            // Whatever is accepted exports and reads back unchanged.
            let out = write_observations(&import.table, Vec::new()).unwrap();
            let again = read_observations(out.as_slice(), &ReadOptions::default()).unwrap();
            prop_assert_eq!(again.table, import.table);
        }
    }
}
