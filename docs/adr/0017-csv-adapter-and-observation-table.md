# ADR-0017: CSV adapter and the observation table

- Status: accepted (2026-10-26, by the owner, by merging the Track D pull request #8)
- Date: 2026-10-25
- Blocks: M11
- Spec: §8 (internal observation format, rules), §2 principles 2, 3 and 6,
  §5 Contracts (actionable errors), §9 Security; ADR-0004 (CSV export in
  v0.1), ADR-0006 (license allow-list), ADR-0012 (units)

## Context

Spec §8: every adapter converts outside data into one internal
observation format, a typed table with the columns `timestamp`, `entity`,
`signal`, `value` and `unit`, so calibration and comparison never depend
on where data came from. Timestamps are stored in UTC with the source
time zone as metadata; missing or implausible values are flagged; each
adapter ships with a recorded fixture. M11 brings CSV import into this
table (in memory in v0.1, Parquet in v0.2), CSV export, and malformed rows
rejected with row and column. Nothing consumes observations before
`compare` (v0.2). Results and traces, which spec §8 also exports as CSV,
do not exist yet (M3, M8).

This ADR settles the details M11 needs. None changes the spec or an
accepted ADR; the questions at the end are for the owner.

## Options

### Libraries

- **CSV**: the `csv` crate (1.4.0, MIT/Unlicense; pulls in `csv-core`,
  `itoa`, `ryu`, `serde_core`), or an own reader and writer.
- **Date-time**: `jiff` or `time`, or an own RFC 3339 parser and formatter.

### Table layout

- **Row-wise**: `Vec<Observation>` with two strings per row.
- **Columnar**: one vector per column, entity and signal names
  dictionary-encoded.

### Row order

- As read, or sorted by a canonical key.

## Trade-offs

**The `csv` crate is lenient where this adapter must not be.** Checked
with `csv` 1.4.0 (`ReaderBuilder::flexible(true)`, byte records):

| Input row | `csv` 1.4.0 returns | Wanted |
| --- | --- | --- |
| `"3"4,x` | fields `34`, `x`, no error | error: text after the closing quote |
| `ab"c,x` | field `ab"c`, no error | error: quote inside an unquoted field |
| `"unterminated,2⏎` (end of input) | one field holding the rest of the file, no error | error: quote not closed |
| a row after two blank lines | position says line 2 | line 4 |

The first case silently turns a value into another number. A strict
reader is under 200 lines of code (one state machine over lines from a
`BufRead`), so writing it is cheaper than working around the crate.

**Date-time.** Exact RFC 3339 with numeric offsets needs no time-zone
database: the civil-date arithmetic is two short integer functions
(H. Hinnant's `days_from_civil` and `civil_from_days`). `jiff` would be
clearly better only with IANA zones (`Europe/Berlin`), which v0.1 does not
accept (below).

**Layout.** Columnar storage is about 40 bytes per row (8 timestamp,
4 offset, 2 × 8 name index, 8 value, 1 unit) without a heap allocation
per row, against about 80 bytes plus two string allocations row-wise. It
is also the Arrow and Parquet layout the table gets in v0.2, where
dictionary encoding of string columns is standard.

**Order.** Sorting makes the table independent of how a source happened
to order its rows (exports are often grouped by machine), which suits
comparison and content hashing. Sorting by the UTC instant also orders
data across a daylight-saving change correctly, where the local text
(`02:30+02:00`, then `02:00+01:00`) is not monotonic.

## Recommendation

**No new dependencies.** An own strict RFC 4180 reader and writer and an
own RFC 3339 parser and formatter. The crate depends only on
`taktsim-units` and `thiserror`, both optional and enabled by the `csv`
feature; without features it is empty. `cargo deny check` passes
unchanged. Library code reads and writes only the `BufRead` and `Write`
values it is given: no files, no network.

**Observation table** (`taktsim_adapters::observation`):

- Columnar, names dictionary-encoded in a sorted dictionary shared by
  `entity` and `signal`, so comparing indexes compares names.
- Rows sorted by timestamp, then entity, then signal (byte order, which is
  Unicode code-point order); rows equal in all three keep their input
  order (stable sort). Built with `FromIterator`, so every source,
  CSV or not, gets the same order. Sorting reorders one column at a time
  and frees each original before copying the next, so the peak while
  building is about 53 bytes per row on a 64-bit target (37 for the
  finished table).
- `timestamp`: `Timestamp`, `i64` nanoseconds since the Unix epoch in UTC,
  without leap seconds (Unix time). Range 1677-09-21T00:12:43.145224192Z to
  2262-04-11T23:47:16.854775807Z. A separate type from `SimTime`, which
  counts from the start of a run.
- Source time zone: a `SourceOffset` per row, `Known(UtcOffset)` (whole
  minutes, ±23:59) or `Unknown` for RFC 3339 `-00:00`. Per row, not per
  table, because one export can span a DST change; the table's metadata
  is the set of offsets used (`source_offsets()`).
- `entity`, `signal`: `Name`, 1 to 256 bytes of UTF-8, no control
  characters, no leading or trailing whitespace; otherwise free, so OPC UA
  node ids, MQTT topics and `Fräse 2` fit. Matching names to model
  components is `compare`'s job.
- `value` + `unit`: `Measurement`, a finite `f64` (negative zero stored as
  zero) with a `Unit`. `Unit` is a closed enum of the units `taktsim-units`
  knows (ADR-0012: `ms`, `s`, `min`, `h`, `d`, `parts`, `parts/s`,
  `parts/min`, `parts/h`, `%`; `part…` accepted on input) plus `1` for
  dimensionless values such as state codes and 0/1 flags (the unit one of
  the SI and UCUM). An unknown unit is an error listing the allowed ones.
  `taktsim-units` keeps its unit tables private, so tests check both
  directions: every adapter unit parses in `taktsim-units`, and every unit
  a `taktsim-units` parser lists in its error is known here and converts
  to the same quantity.
  `Measurement::to_quantity` converts to the `taktsim-units` type of the
  unit (`SimDuration` rounded once, half to even, from the exact binary
  value; `Count`, `Rate`, `Ratio`), so no consumer converts by hand (and,
  e.g., truncates a negative duration to zero).
- **Implausible** (`Measurement::implausibility`) means exactly that
  `to_quantity` fails, decided in one place: a negative duration, count,
  rate or percentage, a count that is not whole, or a duration beyond
  about 584 years or a count beyond `u64::MAX` (too large for
  `taktsim-units`). Values are shown with the shortest text that reads
  back, with an exponent outside 10⁻⁵ to 10¹⁶, never with hundreds of
  digits.

**CSV import** (`csv::read_observations`, any `BufRead`):

- RFC 4180, strict: `,` between fields, `\n` or `\r\n` between rows,
  quoting with `"` and doubled inner quotes; a stray quote, text after a
  closing quote and an unclosed quote are errors, in every field,
  including fields beyond the header's columns. UTF-8 only; a leading
  byte-order mark is removed. Blank lines are skipped everywhere,
  including before the header, and still counted in row numbers.
- A row longer than `max_row_bytes` (default 64 KiB, line breaks
  included) stops reading with an error, so an input without line breaks,
  or with a quote that is never closed, is not read into memory whole. An
  unclosed quote within the limit makes the rest of the input one field;
  it is reported and reading stops.
- The first line that is not blank is the header. It names exactly the
  five columns, each once, in any order. Unknown, duplicate and missing
  columns are errors; a header that is one field containing `;`, tab or
  `|` gets a dedicated message. Header errors stop reading.
- Fields never start or end with whitespace.
- `timestamp`: RFC 3339 §5.6 date-time, `T`, `t` or space between date
  and time, `Z`/`z` or `±hh:mm`, converted exactly to UTC. Fractions of up
  to 9 digits (more only if the extra digits are zero; no rounding).
  Second 60 (a leap second) is rejected: it has no Unix-time
  representation.
- **Naive timestamps** (no offset) are rejected by default. The caller may
  declare one fixed offset (`NaiveTimestamps::AtOffset`), which needs no
  time-zone database and is exact for clocks that never change offset
  (UTC, or controllers kept on standard time). Named zones with DST are
  not accepted in v0.1: they need a time-zone database (e.g. `jiff`) and
  a rule for the repeated autumn hour. The docs say to write the offset at
  the source for such data.
- `value`: Rust's `f64` parser (correctly rounded, the same on every
  platform); NaN, infinity and overflow are errors; `12,5` gets a
  decimal-comma message. An **empty** value is a *missing value*: flagged,
  and the row is not in the table (a row needs a value).
- **Errors** carry the row, the field (column name, or position for a
  header field or an extra field), the text found, what is wrong, what is
  allowed and one valid example (spec §5). The text found is cut at 100
  characters and made visible: `\`, control characters, format characters
  (byte-order mark, zero-width and bidirectional marks) and line
  separators are escaped (`\n`, `\u{1b}`), from a fixed list rather than
  the toolchain's Unicode tables, so a message is one line, cannot carry
  terminal escape sequences, and is the same with every toolchain. Header
  problems show the header as written, quotes included. Row = the 1-based
  line on which the record starts. That equals the record number, because
  no valid field can contain a line break, and it stays the line an editor
  shows when blank lines are skipped. All errors are collected up to
  `max_errors` (default 100). When reading stops early, the error says why
  (`Stopped`: invalid header, error limit, unclosed quote, row too long),
  so a caller knows the rest was not checked; whether input is left after
  the error limit is checked without reading it.
- **Flags** (`QualityFlag`, not errors): missing values, and implausible
  values as defined above. A flag carries the row, timestamp, source
  offset, entity and signal, and the row's index in the table: for an
  implausible value the row stays in the table; a missing value's row is
  not in it, and the flag is its record. Deferred to the data-quality
  report (v0.3), because they need the meaning of a signal: decreasing
  cumulative counters (resets, wrap-around), gaps against an expected
  sampling interval, duplicates (same timestamp, entity and signal), stuck
  values, implausible timestamps (e.g. 1970 from an unset clock).

**CSV export** (`csv::write_observations`, `csv::CsvWriter`):

- A general, typed `CsvWriter`: fixed header, one `Cell` per column (text,
  integer, float, timestamp at an offset, empty), `\n` after every row,
  quoting only when needed (`,`, `"`, a line break, or a leading
  byte-order mark, which the reader would otherwise remove from the
  header), output identical on every platform. Floats are the shortest
  text that reads back to the same bits, with an exponent below 10⁻⁵ and
  from 10¹⁶; NaN and infinity are refused. Its errors give row, column,
  what was found, what is allowed and an example. Result and trace exports
  (M3, M8) use this writer; their formats are not decided here.
- The observation table is written in table order, each timestamp in its
  source offset (fraction with 0, 3, 6 or 9 digits, zero offset as `Z`),
  so export then import gives an equal table, offsets included, and
  exporting again gives the same bytes (property-tested).

**Fixtures** (`crates/taktsim-adapters/tests/fixtures/`): a synthetic MES
export of two machines over one day at +01:00 with state changes and
counters, a DST fall-back day, flagged values, and one file per malformed
case, each with a test of the exact errors. Expected values come from
Python's `csv` and `datetime` modules, including the canonical export of
the day fixture.

## Consequences

- The CSV dialect and the RFC 3339 subset are ours to maintain and are
  covered by unit, fixture and property tests (round trips, never
  panicking on arbitrary bytes, calendar against a day-by-day count).
- The `observation` module sits behind the `csv` feature. When the
  Parquet adapter arrives (v0.2) it is needed by both; it then becomes
  always available or gets its own feature.
- v0.2 decides how the per-row source offset is stored in Parquet (see
  question 1).

## Questions for the owner

1. Source time zone per row: the table keeps the offset of every row
   (needed across DST changes) and summarises the set as table metadata.
   In Parquet (v0.2) that means a sixth physical column next to the five of
   spec §8, or key-value metadata with run-length ranges. Is a sixth
   column acceptable, or must the stored table have exactly five?
2. Unit `1` for dimensionless values (state codes, flags) is not a
   `taktsim-units` unit. Keep `1` (SI, UCUM), or prefer another token?
3. Naive timestamps with a declared fixed offset are accepted on request.
   Do you want IANA time zones (with a time-zone database dependency) in
   v0.2 together with OPC UA and MQTT?

## Owner's answers (2026-10-26)

The owner took the recommendations:

1. A sixth physical column for the source offset is acceptable.
2. Keep `1` as the dimensionless unit.
3. Yes: IANA time zones come in v0.2, together with OPC UA and MQTT.
