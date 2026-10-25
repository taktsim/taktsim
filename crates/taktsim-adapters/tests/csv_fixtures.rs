//! Recorded fixtures for the CSV adapter (spec §8: "each adapter ships with
//! a recorded fixture so its tests run without hardware").
//!
//! Expected values come from Python 3.14's `csv` and `datetime` modules, not
//! from this crate: UTC nanoseconds are
//! `(datetime.fromisoformat(t) - datetime(1970, 1, 1, tzinfo=timezone.utc)) // timedelta(microseconds=1) * 1000`,
//! and `two_machines_one_day.expected.csv` is the fixture sorted by (UTC
//! instant, entity, signal) and formatted by an independent Python script.
//!
//! `two_machines_one_day.csv` is a synthetic MES export of Monday
//! 2026-03-02, 06:00 to 22:00 Central European Time (+01:00), rows grouped by
//! machine: state changes (`state`, unit `1`: 1 running, 2 idle, 3 down,
//! 4 setup), and every hour the cumulative `good_count` and `scrap_count`
//! and the mean `cycle_time`.

#![cfg(feature = "csv")]

use std::collections::BTreeMap;
use std::path::PathBuf;

use taktsim_adapters::csv::{
    Column, FieldError, FieldRef, Import, Problem, QualityIssue, ReadError, ReadOptions, Stopped,
    read_observations, write_observations,
};
use taktsim_adapters::observation::{
    Implausibility, NaiveTimestamps, NameError, SourceOffset, TimestampError, Unit, UnknownUnit,
    UtcOffset, ValueError,
};

/// The bytes of a file in `tests/fixtures`.
#[allow(clippy::panic)] // test helper
fn fixture(name: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "fixtures", name]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[allow(clippy::unwrap_used)] // test helper
fn import(name: &str) -> Import {
    read_observations(fixture(name).as_slice(), &ReadOptions::default()).unwrap()
}

/// Each error as (row, field, found, problem).
fn errors(name: &str) -> Vec<(u64, FieldRef, String, Problem)> {
    invalid(name)
        .0
        .into_iter()
        .map(|e| (e.row, e.field, e.found.clone(), e.problem.clone()))
        .collect()
}

/// Why reading stopped early, if it did.
fn stopped(name: &str) -> Option<Stopped> {
    invalid(name).1
}

#[allow(clippy::panic)] // test helper
fn invalid(name: &str) -> (Vec<FieldError>, Option<Stopped>) {
    match read_observations(fixture(name).as_slice(), &ReadOptions::default()) {
        Err(ReadError::Invalid { errors, stopped }) => (errors, stopped),
        other => panic!("{name}: {other:?}"),
    }
}

#[allow(clippy::panic)] // test helper
fn first_error(name: &str) -> FieldError {
    match read_observations(fixture(name).as_slice(), &ReadOptions::default()) {
        Err(ReadError::Invalid { mut errors, .. }) => errors.remove(0),
        other => panic!("{name}: {other:?}"),
    }
}

#[allow(clippy::unwrap_used)] // test helper
fn plus(hours: i16) -> SourceOffset {
    SourceOffset::Known(UtcOffset::from_minutes(hours * 60).unwrap())
}

#[test]
fn two_machines_one_day() {
    let Import { table, flags, .. } = import("two_machines_one_day.csv");
    assert_eq!(table.len(), 124);
    assert_eq!(flags, []);
    assert_eq!(
        table.source_offsets().into_iter().collect::<Vec<_>>(),
        [plus(1)]
    );
    let first = table.get(0).unwrap();
    let last = table.get(123).unwrap();
    assert_eq!(first.timestamp.as_unix_nanos(), 1_772_427_600_000_000_000);
    assert_eq!(last.timestamp.as_unix_nanos(), 1_772_485_200_000_000_000);
    // Sorted by time, entity, signal, although the source grouped by machine.
    let head: Vec<(&str, &str)> = table
        .iter()
        .take(4)
        .map(|o| (o.entity.as_str(), o.signal.as_str()))
        .collect();
    assert_eq!(
        head,
        [
            ("M1", "good_count"),
            ("M1", "scrap_count"),
            ("M1", "state"),
            ("M2", "good_count")
        ]
    );
    // Final counter values and the number of state changes.
    let mut latest = BTreeMap::new();
    for o in &table {
        latest.insert((o.entity.as_str(), o.signal.as_str()), o.measurement);
    }
    let value = |entity, signal| latest[&(entity, signal)].value();
    assert_eq!(
        (value("M1", "good_count"), value("M2", "good_count")),
        (811.0, 1099.0)
    );
    assert_eq!(
        (value("M1", "scrap_count"), value("M2", "scrap_count")),
        (17.0, 18.0)
    );
    assert_eq!(latest[&("M2", "cycle_time")].unit(), Unit::Second);
    assert_eq!(
        table
            .iter()
            .filter(|o| o.signal.as_str() == "state")
            .count(),
        24
    );
    assert!(
        table
            .iter()
            .all(|o| o.measurement.implausibility().is_none())
    );
}

#[test]
fn two_machines_one_day_exports_canonically_and_round_trips() {
    let table = import("two_machines_one_day.csv").table;
    let exported = write_observations(&table, Vec::new()).unwrap();
    let expected = fixture("two_machines_one_day.expected.csv");
    assert_eq!(
        String::from_utf8(exported.clone()).unwrap(),
        String::from_utf8(expected).unwrap()
    );
    let again = read_observations(exported.as_slice(), &ReadOptions::default()).unwrap();
    assert_eq!(again.table, table);
    assert_eq!(
        write_observations(&again.table, Vec::new()).unwrap(),
        exported
    );
}

#[test]
fn windows_line_endings_and_byte_order_mark_read_the_same() {
    let lf = fixture("two_machines_one_day.csv");
    let mut crlf = b"\xEF\xBB\xBF".to_vec();
    for &byte in &lf {
        if byte == b'\n' {
            crlf.push(b'\r');
        }
        crlf.push(byte);
    }
    let from_crlf = read_observations(crlf.as_slice(), &ReadOptions::default()).unwrap();
    assert_eq!(from_crlf, import("two_machines_one_day.csv"));
}

#[test]
fn daylight_saving_fall_back() {
    // Europe/Berlin, 2026-10-25: 02:00 to 03:00 local happens twice, first
    // at +02:00, then at +01:00. UTC keeps the rows apart and in order.
    let input = fixture("dst_fall_back.csv");
    let Import { table, flags, .. } = import("dst_fall_back.csv");
    assert_eq!(flags, []);
    let utc: Vec<i64> = table.iter().map(|o| o.timestamp.as_unix_nanos()).collect();
    assert_eq!(
        utc,
        [
            1_792_884_600_000_000_000,
            1_792_886_400_000_000_000,
            1_792_888_200_000_000_000,
            1_792_890_000_000_000_000,
            1_792_891_800_000_000_000,
            1_792_893_600_000_000_000,
        ]
    );
    let offsets: Vec<SourceOffset> = table.iter().map(|o| o.source_offset).collect();
    assert_eq!(
        offsets,
        [plus(2), plus(2), plus(2), plus(1), plus(1), plus(1)]
    );
    assert_eq!(
        table.source_offsets().into_iter().collect::<Vec<_>>(),
        [plus(1), plus(2)]
    );
    // Already canonical, so the export is the input byte for byte.
    assert_eq!(write_observations(&table, Vec::new()).unwrap(), input);
}

#[test]
fn missing_and_implausible_values_are_flagged() {
    let Import { table, flags, .. } = import("flagged_values.csv");
    // Every row but the one without a value.
    assert_eq!(table.len(), 7);
    // The missing value's row is not in the table; its flag keeps the row's
    // timestamp, entity and signal.
    let missing = &flags[0];
    assert_eq!(missing.table_index, None);
    assert_eq!(
        (missing.entity.as_str(), missing.signal.as_str()),
        ("M1", "scrap_count")
    );
    assert_eq!(missing.timestamp.as_unix_nanos(), 1_772_431_200_000_000_000);
    assert_eq!(missing.source_offset, SourceOffset::Known(UtcOffset::UTC));
    // The implausible ones point at their row of the sorted table.
    for flag in &flags[1..] {
        let row = table.get(flag.table_index.unwrap()).unwrap();
        assert_eq!(
            (row.timestamp, row.entity, row.signal),
            (flag.timestamp, &flag.entity, &flag.signal)
        );
    }
    let flags: Vec<(u64, QualityIssue)> = flags.iter().map(|f| (f.row, f.issue)).collect();
    let implausible = |row| match flags.iter().find(|(r, _)| *r == row) {
        Some((
            _,
            QualityIssue::Implausible {
                measurement,
                implausibility,
            },
        )) => (measurement.value(), measurement.unit(), *implausibility),
        other => panic!("row {row}: {other:?}"),
    };
    assert_eq!(
        flags.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
        [3, 4, 5, 6, 7]
    );
    assert_eq!(flags[0].1, QualityIssue::MissingValue);
    assert_eq!(
        implausible(4),
        (-3.0, Unit::Parts, Implausibility::Negative)
    );
    assert_eq!(implausible(5), (1.5, Unit::Parts, Implausibility::NotWhole));
    assert_eq!(
        implausible(6),
        (-0.5, Unit::Second, Implausibility::Negative)
    );
    assert_eq!(
        implausible(7),
        (-2.0, Unit::Percent, Implausibility::Negative)
    );
    // The flagged rows are in the table; the dimensionless one is not flagged.
    let signals: Vec<&str> = table.iter().map(|o| o.signal.as_str()).collect();
    assert_eq!(
        signals,
        [
            "good_count",
            "availability",
            "cycle_time",
            "good_count",
            "parts_per_cycle",
            "temperature_delta",
            "state"
        ]
    );
    // `-00:00`: UTC known, the source's offset unknown.
    let state = table.get(6).unwrap();
    assert_eq!(state.source_offset, SourceOffset::Unknown);
    assert_eq!(state.timestamp.as_unix_nanos(), 1_772_434_800_000_000_000);
    assert_eq!(
        table.get(0).unwrap().timestamp.as_unix_nanos(),
        1_772_431_200_000_000_000
    );
}

#[test]
fn malformed_rows_are_rejected_with_row_and_column() {
    assert_eq!(stopped("malformed_rows.csv"), None);
    let at = |column| FieldRef::Column(column);
    assert_eq!(
        errors("malformed_rows.csv"),
        [
            (
                3,
                at(Column::Value),
                "60,4".into(),
                Problem::Value(ValueError::DecimalComma)
            ),
            (
                4,
                at(Column::Value),
                "NaN".into(),
                Problem::Value(ValueError::NotFinite)
            ),
            (
                5,
                at(Column::Unit),
                "pcs".into(),
                Problem::Unit(UnknownUnit)
            ),
            // Row 6 is blank and skipped; rows keep their line numbers.
            (
                7,
                at(Column::Timestamp),
                "2026-13-02T08:00:00+01:00".into(),
                Problem::Timestamp(TimestampError::OutOfRange {
                    field: "month",
                    value: 13,
                    min: 1,
                    max: 12
                })
            ),
            (
                8,
                at(Column::Value),
                String::new(),
                Problem::FieldCount {
                    fields: 3,
                    columns: 5
                }
            ),
            (9, at(Column::Entity), "M12".into(), Problem::TextAfterQuote),
            (10, at(Column::Entity), " M2".into(), Problem::Whitespace),
            (
                11,
                FieldRef::Position(6),
                String::new(),
                Problem::FieldCount {
                    fields: 6,
                    columns: 5
                }
            ),
        ]
    );
}

#[test]
fn malformed_rows_messages() {
    let message = |e: &FieldError| e.to_string();
    let err = read_observations(
        fixture("malformed_rows.csv").as_slice(),
        &ReadOptions::default(),
    )
    .unwrap_err();
    let ReadError::Invalid { errors, .. } = &err else {
        panic!("{err:?}")
    };
    assert_eq!(
        message(&errors[0]),
        "row 3, column `value`: the number uses `,` as decimal separator; use `.`; found `60,4`; \
         allowed: a finite decimal number with `.` as decimal separator and an optional exponent, \
         or an empty field for a missing value; example: 1520"
    );
    assert_eq!(
        message(&errors[3]),
        "row 7, column `timestamp`: month 13 is out of range 01 to 12; found `2026-13-02T08:00:00+01:00`; \
         allowed: an RFC 3339 date-time YYYY-MM-DDThh:mm:ss with an optional fraction of up to 9 digits \
         and `Z` or a UTC offset `±hh:mm`, between 1677-09-21 and 2262-04-11; example: 2026-03-02T06:00:00+01:00"
    );
    assert_eq!(
        message(&errors[5]),
        "row 9, column `entity`: text after the closing `\"` of a quoted field; found `M12`; \
         allowed: an unquoted field without `\"`, or a field enclosed in `\"` with each inner `\"` doubled \
         (RFC 4180); example: \"Press \"\"A\"\", left\""
    );
    assert!(
        err.to_string()
            .starts_with("8 errors in the observation CSV:\n  row 3, column `value`: ")
    );
}

#[test]
fn unterminated_quote() {
    // Everything after the quote is one field, so the second row is not read.
    assert_eq!(
        stopped("malformed_unterminated_quote.csv"),
        Some(Stopped::UnclosedQuote)
    );
    assert_eq!(
        errors("malformed_unterminated_quote.csv"),
        [
            (
                2,
                FieldRef::Column(Column::Entity),
                "M1,state,1,1\\n2026-03-02T07:00:00+01:00,M1,state,2,1\\n".into(),
                Problem::UnterminatedQuote
            ),
            (
                2,
                FieldRef::Column(Column::Signal),
                String::new(),
                Problem::FieldCount {
                    fields: 2,
                    columns: 5
                }
            ),
        ]
    );
}

#[test]
fn header_with_unknown_and_missing_columns() {
    assert_eq!(
        stopped("malformed_header.csv"),
        Some(Stopped::InvalidHeader)
    );
    let header = "time,entity,signal,value,unit,comment";
    assert_eq!(
        errors("malformed_header.csv"),
        [
            (
                1,
                FieldRef::Position(1),
                "time".into(),
                Problem::UnknownColumn
            ),
            (
                1,
                FieldRef::Position(6),
                "comment".into(),
                Problem::UnknownColumn
            ),
            (
                1,
                FieldRef::Column(Column::Timestamp),
                header.into(),
                Problem::MissingColumn
            ),
        ]
    );
    assert_eq!(
        first_error("malformed_header.csv").to_string(),
        "row 1, field 1: unknown column; found `time`; allowed: a first row naming exactly the columns \
         timestamp, entity, signal, value and unit, each once, in any order, separated by `,`; \
         example: timestamp,entity,signal,value,unit"
    );
}

#[test]
fn header_with_semicolons() {
    assert_eq!(
        stopped("malformed_semicolons.csv"),
        Some(Stopped::InvalidHeader)
    );
    assert_eq!(
        errors("malformed_semicolons.csv"),
        [(
            1,
            FieldRef::Row,
            "timestamp;entity;signal;value;unit".into(),
            Problem::Delimiter { delimiter: ';' }
        )]
    );
    assert!(
        first_error("malformed_semicolons.csv")
            .to_string()
            .starts_with(
                "row 1: the header is one field containing `;`; columns must be separated by `,`"
            )
    );
}

#[test]
fn not_utf8() {
    assert_eq!(stopped("malformed_latin1.csv"), None);
    assert_eq!(
        errors("malformed_latin1.csv"),
        [(
            2,
            FieldRef::Column(Column::Entity),
            "Fr\u{fffd}se 2".into(),
            Problem::InvalidUtf8
        )]
    );
    assert_eq!(first_error("malformed_latin1.csv").example(), "M1");
}

#[test]
fn naive_timestamps_are_rejected_unless_an_offset_is_declared() {
    let missing = |row, found: &str| {
        (
            row,
            FieldRef::Column(Column::Timestamp),
            found.to_owned(),
            Problem::Timestamp(TimestampError::MissingOffset),
        )
    };
    assert_eq!(
        errors("malformed_naive_timestamps.csv"),
        [
            missing(2, "2026-03-02 06:00:00"),
            missing(3, "2026-03-02 07:00:00"),
            missing(4, "2026-03-02 08:00:00.500"),
        ]
    );
    let mut options = ReadOptions::default();
    options.naive_timestamps = NaiveTimestamps::AtOffset(UtcOffset::from_minutes(60).unwrap());
    let table = read_observations(
        fixture("malformed_naive_timestamps.csv").as_slice(),
        &options,
    )
    .unwrap()
    .table;
    let rows: Vec<(i64, SourceOffset)> = table
        .iter()
        .map(|o| (o.timestamp.as_unix_nanos(), o.source_offset))
        .collect();
    assert_eq!(
        rows,
        [
            (1_772_427_600_000_000_000, plus(1)),
            (1_772_431_200_000_000_000, plus(1)),
            (1_772_434_800_500_000_000, plus(1)),
        ]
    );
    // The export writes the declared offset, so it reads back without it.
    let exported = write_observations(&table, Vec::new()).unwrap();
    let again = read_observations(exported.as_slice(), &ReadOptions::default()).unwrap();
    assert_eq!(again.table, table);
}

#[test]
fn names_and_quoting_survive_the_round_trip() {
    let csv = "timestamp,entity,signal,value,unit\n\
               2026-03-02T06:00:00Z,\"Press 1, left\",\"say \"\"hi\"\"\",1,1\n\
               2026-03-02T06:00:00Z,Fräse_2,工位,2,1\n";
    let table = read_observations(csv.as_bytes(), &ReadOptions::default())
        .unwrap()
        .table;
    let names: Vec<(&str, &str)> = table
        .iter()
        .map(|o| (o.entity.as_str(), o.signal.as_str()))
        .collect();
    assert_eq!(
        names,
        [("Fräse_2", "工位"), ("Press 1, left", "say \"hi\"")]
    );
    let exported = write_observations(&table, Vec::new()).unwrap();
    assert_eq!(
        read_observations(exported.as_slice(), &ReadOptions::default())
            .unwrap()
            .table,
        table
    );
    // Names are validated as names, too.
    let tab = "timestamp,entity,signal,value,unit\n2026-03-02T06:00:00Z,M1,a\tb,1,1\n";
    let Err(ReadError::Invalid { errors, .. }) =
        read_observations(tab.as_bytes(), &ReadOptions::default())
    else {
        panic!()
    };
    assert_eq!(
        errors[0].problem,
        Problem::Name(NameError::ControlCharacter { position: 2 })
    );
}
