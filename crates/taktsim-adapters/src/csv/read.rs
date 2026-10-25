//! CSV import into the observation table.

use std::fmt;
use std::io::{self, BufReader, Read};
use std::num::NonZeroUsize;

use super::record::{Record, RecordReader, Syntax};
use crate::observation::{
    Implausibility, Measurement, NaiveTimestamps, NameError, ObservationTable, SourceOffset,
    TableBuilder, Timestamp, TimestampError, Unit, UnknownUnit, ValueError, parse_value,
    validate_name,
};

/// What a header accepts, for header errors.
const HEADER_ALLOWED: &str = "a first row naming exactly the columns timestamp, entity, signal, value and unit, \
     each once, in any order, separated by `,`";
const HEADER_EXAMPLE: &str = "timestamp,entity,signal,value,unit";
const ROW_ALLOWED: &str = "one field for each column of the header";
const ROW_EXAMPLE: &str = "2026-03-02T06:00:00+01:00,M1,good_count,1520,parts";
const QUOTING_ALLOWED: &str = "an unquoted field without `\"`, or a field enclosed in `\"` with each inner `\"` doubled (RFC 4180)";
const QUOTING_EXAMPLE: &str = "\"Press \"\"A\"\", left\"";
/// Longest `found` text kept in an error, in characters.
const FOUND_LIMIT: usize = 100;

/// A column of the observation CSV.
///
/// ```
/// use taktsim_adapters::csv::Column;
///
/// let names: Vec<&str> = Column::ALL.iter().map(|c| c.name()).collect();
/// assert_eq!(names, ["timestamp", "entity", "signal", "value", "unit"]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Column {
    /// `timestamp`: RFC 3339 with a UTC offset.
    Timestamp,
    /// `entity`: what was observed, e.g. a machine.
    Entity,
    /// `signal`: which of its signals.
    Signal,
    /// `value`: the number; empty if missing.
    Value,
    /// `unit`: the unit of the value.
    Unit,
}

impl Column {
    /// The five columns, in the order the writer uses.
    ///
    /// ```
    /// use taktsim_adapters::csv::Column;
    ///
    /// assert_eq!(Column::ALL.len(), 5);
    /// ```
    pub const ALL: [Self; 5] = [
        Self::Timestamp,
        Self::Entity,
        Self::Signal,
        Self::Value,
        Self::Unit,
    ];

    /// The column's name in the header.
    ///
    /// ```
    /// use taktsim_adapters::csv::Column;
    ///
    /// assert_eq!(Column::Signal.name(), "signal");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Timestamp => "timestamp",
            Self::Entity => "entity",
            Self::Signal => "signal",
            Self::Value => "value",
            Self::Unit => "unit",
        }
    }

    /// What a field of this column may contain.
    ///
    /// ```
    /// use taktsim_adapters::csv::Column;
    ///
    /// assert!(Column::Unit.allowed().contains("parts/h"));
    /// ```
    #[must_use]
    pub const fn allowed(self) -> &'static str {
        match self {
            Self::Timestamp => TimestampError::ALLOWED,
            Self::Entity | Self::Signal => NameError::ALLOWED,
            Self::Value => ValueError::ALLOWED,
            Self::Unit => UnknownUnit::ALLOWED,
        }
    }

    /// One valid field of this column.
    ///
    /// ```
    /// use taktsim_adapters::csv::Column;
    ///
    /// assert_eq!(Column::Timestamp.example(), "2026-03-02T06:00:00+01:00");
    /// ```
    #[must_use]
    pub const fn example(self) -> &'static str {
        match self {
            Self::Timestamp => TimestampError::EXAMPLE,
            Self::Entity => NameError::EXAMPLE,
            Self::Signal => "good_count",
            Self::Value => ValueError::EXAMPLE,
            Self::Unit => UnknownUnit::EXAMPLE,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|column| column.name() == name)
    }
}

/// Which field of a row an error is about.
///
/// ```
/// use taktsim_adapters::csv::{Column, FieldRef};
///
/// assert_ne!(FieldRef::Column(Column::Value), FieldRef::Position(4));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldRef {
    /// A field of one of the five columns.
    Column(Column),
    /// A field that is not one of them, by its 1-based position in the row:
    /// a header field, or a field beyond the last column.
    Position(usize),
    /// The row as a whole.
    Row,
}

/// What is wrong with a field, a row or the header of an observation CSV.
///
/// ```
/// use taktsim_adapters::csv::Problem;
/// use taktsim_adapters::observation::UnknownUnit;
///
/// assert_eq!(Problem::from(UnknownUnit).to_string(), "unknown unit");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Problem {
    /// The input is empty, or its first row is blank.
    #[error("the first row must be the header")]
    MissingHeader,
    /// The header is one field containing another separator, as written
    /// by spreadsheets in some locales.
    #[error("the header is one field containing `{delimiter}`; columns must be separated by `,`")]
    Delimiter {
        /// The separator found, e.g. `;`.
        delimiter: char,
    },
    /// A header field that is not one of the five columns.
    #[error("unknown column")]
    UnknownColumn,
    /// A column named twice in the header.
    #[error("the column appears more than once in the header")]
    DuplicateColumn,
    /// A column the header does not name.
    #[error("the column is missing from the header")]
    MissingColumn,
    /// A row with more or fewer fields than the header has columns.
    #[error("the row has {fields} fields, but the header has {columns}")]
    FieldCount {
        /// Fields in the row.
        fields: usize,
        /// Columns in the header.
        columns: usize,
    },
    /// A `"` inside an unquoted field.
    #[error("a `\"` inside an unquoted field")]
    StrayQuote,
    /// Text after the closing `"` of a quoted field, e.g. `"3"4`.
    #[error("text after the closing `\"` of a quoted field")]
    TextAfterQuote,
    /// The input ends inside a quoted field.
    #[error("a quoted field is not closed before the end of the input")]
    UnterminatedQuote,
    /// The field is not valid UTF-8.
    #[error("the field is not valid UTF-8; save the file as UTF-8")]
    InvalidUtf8,
    /// The field starts or ends with whitespace.
    #[error("the field starts or ends with whitespace")]
    Whitespace,
    /// A `timestamp` field that is not a valid timestamp.
    #[error(transparent)]
    Timestamp(#[from] TimestampError),
    /// An `entity` or `signal` field that is not a valid name.
    #[error(transparent)]
    Name(#[from] NameError),
    /// A `value` field that is not a finite number.
    #[error(transparent)]
    Value(#[from] ValueError),
    /// A `unit` field that is not a known unit.
    #[error(transparent)]
    Unit(#[from] UnknownUnit),
}

fn syntax_problem(syntax: Syntax) -> Problem {
    match syntax {
        Syntax::Stray => Problem::StrayQuote,
        Syntax::TextAfterClosing => Problem::TextAfterQuote,
        Syntax::Unclosed => Problem::UnterminatedQuote,
    }
}

/// One problem in an observation CSV: where (row and field), what was
/// found, what is wrong, what is allowed and one valid example (spec §5).
///
/// Rows are 1-based line numbers, the header being row 1. They are also
/// the record numbers, since no valid field can contain a line break;
/// blank lines are skipped but still counted, so the row is the line an
/// editor shows.
///
/// ```
/// use taktsim_adapters::csv::{Column, FieldRef, Problem, ReadError, ReadOptions, read_observations};
///
/// let csv = "timestamp,entity,signal,value,unit\n2026-03-02T06:00:00+01:00,M1,good_count,12,pcs\n";
/// let Err(ReadError::Invalid { errors, .. }) = read_observations(csv.as_bytes(), &ReadOptions::default()) else {
///     unreachable!()
/// };
/// let error = &errors[0];
/// assert_eq!((error.row, error.field), (2, FieldRef::Column(Column::Unit)));
/// assert_eq!(error.found, "pcs");
/// assert_eq!(error.example(), "parts");
/// assert_eq!(
///     error.to_string(),
///     "row 2, column `unit`: unknown unit; found `pcs`; allowed: ms, s, min, h, d, parts, \
///      parts/s, parts/min, parts/h, %, 1 (dimensionless), case-sensitive; example: parts"
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldError {
    /// The 1-based row, the header being row 1.
    pub row: u64,
    /// The field in the row.
    pub field: FieldRef,
    /// The field's text as found (invalid UTF-8 replaced by `�`, cut after
    /// 100 characters); empty if there was none.
    pub found: String,
    /// What is wrong.
    pub problem: Problem,
}

impl FieldError {
    /// What the field, row or header may contain.
    ///
    /// ```
    /// use taktsim_adapters::csv::{ReadError, ReadOptions, read_observations};
    ///
    /// let Err(ReadError::Invalid { errors, .. }) = read_observations(&b""[..], &ReadOptions::default()) else {
    ///     unreachable!()
    /// };
    /// assert!(errors[0].allowed().contains("timestamp, entity, signal, value and unit"));
    /// ```
    #[must_use]
    pub fn allowed(&self) -> &'static str {
        match self.problem {
            Problem::MissingHeader
            | Problem::Delimiter { .. }
            | Problem::UnknownColumn
            | Problem::DuplicateColumn
            | Problem::MissingColumn => HEADER_ALLOWED,
            Problem::FieldCount { .. } => ROW_ALLOWED,
            Problem::StrayQuote | Problem::TextAfterQuote | Problem::UnterminatedQuote => {
                QUOTING_ALLOWED
            }
            Problem::InvalidUtf8
            | Problem::Whitespace
            | Problem::Timestamp(_)
            | Problem::Name(_)
            | Problem::Value(_)
            | Problem::Unit(_) => match self.field {
                FieldRef::Column(column) => column.allowed(),
                FieldRef::Position(_) | FieldRef::Row => HEADER_ALLOWED,
            },
        }
    }

    /// One valid field, row or header.
    ///
    /// ```
    /// use taktsim_adapters::csv::{ReadError, ReadOptions, read_observations};
    ///
    /// let Err(ReadError::Invalid { errors, .. }) = read_observations(&b""[..], &ReadOptions::default()) else {
    ///     unreachable!()
    /// };
    /// assert_eq!(errors[0].example(), "timestamp,entity,signal,value,unit");
    /// ```
    #[must_use]
    pub fn example(&self) -> &'static str {
        match self.problem {
            Problem::MissingHeader
            | Problem::Delimiter { .. }
            | Problem::UnknownColumn
            | Problem::DuplicateColumn
            | Problem::MissingColumn => HEADER_EXAMPLE,
            Problem::FieldCount { .. } => ROW_EXAMPLE,
            Problem::StrayQuote | Problem::TextAfterQuote | Problem::UnterminatedQuote => {
                QUOTING_EXAMPLE
            }
            Problem::InvalidUtf8
            | Problem::Whitespace
            | Problem::Timestamp(_)
            | Problem::Name(_)
            | Problem::Value(_)
            | Problem::Unit(_) => match self.field {
                FieldRef::Column(column) => column.example(),
                FieldRef::Position(_) | FieldRef::Row => HEADER_EXAMPLE,
            },
        }
    }
}

impl fmt::Display for FieldError {
    /// `row R, column `C`: problem; found `F`; allowed: …; example: …`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "row {}", self.row)?;
        match self.field {
            FieldRef::Column(column) => write!(f, ", column `{}`", column.name())?,
            FieldRef::Position(position) => write!(f, ", field {position}")?,
            FieldRef::Row => {}
        }
        write!(f, ": {}; found ", self.problem)?;
        if self.found.is_empty() {
            f.write_str("nothing")?;
        } else {
            write!(f, "`{}`", self.found)?;
        }
        write!(
            f,
            "; allowed: {}; example: {}",
            self.allowed(),
            self.example()
        )
    }
}

impl std::error::Error for FieldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.problem)
    }
}

/// Why an observation CSV could not be read.
///
/// ```
/// use taktsim_adapters::csv::{ReadError, ReadOptions, read_observations};
///
/// let err = read_observations(&b"timestamp,entity\n"[..], &ReadOptions::default()).unwrap_err();
/// let text = err.to_string();
/// assert!(text.starts_with("3 errors in the observation CSV:\n"));
/// assert!(text.contains("row 1, column `signal`: the column is missing from the header"));
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    /// The input could not be read.
    Io(io::Error),
    /// The CSV has problems; nothing was imported.
    Invalid {
        /// Every problem found, in row order, up to
        /// [`ReadOptions::max_errors`]. A header problem stops reading.
        errors: Vec<FieldError>,
        /// Reading stopped at the error limit, so later rows were not
        /// checked.
        truncated: bool,
    },
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "cannot read the observation CSV: {err}"),
            Self::Invalid { errors, truncated } => {
                let plural = if errors.len() == 1 { "" } else { "s" };
                write!(f, "{} error{plural} in the observation CSV:", errors.len())?;
                for error in errors {
                    write!(f, "\n  {error}")?;
                }
                if *truncated {
                    write!(
                        f,
                        "\n  (stopped at the limit of {} errors; later rows were not checked)",
                        errors.len()
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Invalid { errors, .. } => errors.first().map(|e| e as _),
        }
    }
}

impl From<io::Error> for ReadError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// How to read an observation CSV.
///
/// ```
/// use std::num::NonZeroUsize;
///
/// use taktsim_adapters::csv::ReadOptions;
/// use taktsim_adapters::observation::{NaiveTimestamps, UtcOffset};
///
/// let mut options = ReadOptions::default();
/// options.naive_timestamps = NaiveTimestamps::AtOffset(UtcOffset::UTC);
/// options.max_errors = NonZeroUsize::MIN;
/// assert_eq!(ReadOptions::default().max_errors.get(), 100);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadOptions {
    /// Timestamps without an offset: rejected (the default), or read at a
    /// declared fixed offset. See [`NaiveTimestamps`] for when that is safe.
    pub naive_timestamps: NaiveTimestamps,
    /// Stop after this many errors (default 100).
    pub max_errors: NonZeroUsize,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            naive_timestamps: NaiveTimestamps::Reject,
            max_errors: NonZeroUsize::new(100).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// A successful import: the table and the data-quality flags raised on the
/// way.
///
/// ```
/// use taktsim_adapters::csv::{QualityIssue, ReadOptions, read_observations};
///
/// let csv = "timestamp,entity,signal,value,unit\n\
///            2026-03-02T06:00:00Z,M1,good_count,,parts\n\
///            2026-03-02T06:01:00Z,M1,good_count,-3,parts\n";
/// let import = read_observations(csv.as_bytes(), &ReadOptions::default())?;
/// assert_eq!(import.table.len(), 1);
/// assert_eq!(import.flags.len(), 2);
/// assert_eq!((import.flags[0].row, import.flags[0].issue), (2, QualityIssue::MissingValue));
/// # Ok::<(), taktsim_adapters::csv::ReadError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Import {
    /// The observations.
    pub table: ObservationTable,
    /// Missing and implausible values, in row order. These rows are not
    /// errors: the data-quality report (v0.3) weighs them.
    pub flags: Vec<QualityFlag>,
}

/// A data-quality flag on one row of the CSV (spec §8).
///
/// ```
/// use taktsim_adapters::csv::{ReadOptions, read_observations};
///
/// let csv = "timestamp,entity,signal,value,unit\n2026-03-02T06:00:00Z,M1,cycle_time,-4.5,s\n";
/// let import = read_observations(csv.as_bytes(), &ReadOptions::default())?;
/// assert_eq!(
///     import.flags[0].to_string(),
///     "row 2, column `value`: implausible -4.5 s: negative, but the unit measures a quantity that cannot be"
/// );
/// # Ok::<(), taktsim_adapters::csv::ReadError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct QualityFlag {
    /// The 1-based row, the header being row 1.
    pub row: u64,
    /// What is flagged.
    pub issue: QualityIssue,
}

impl fmt::Display for QualityFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "row {}, column `value`: {}", self.row, self.issue)
    }
}

/// What a [`QualityFlag`] reports.
///
/// ```
/// use taktsim_adapters::csv::QualityIssue;
///
/// assert_eq!(QualityIssue::MissingValue.to_string(), "missing value; the row is not in the table");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum QualityIssue {
    /// The `value` field is empty. A row needs a value, so it is not in the
    /// table; the flag is its only trace.
    MissingValue,
    /// The value is implausible for its unit. The row is in the table.
    Implausible {
        /// The value and unit, as in the table.
        measurement: Measurement,
        /// Why it is implausible.
        implausibility: Implausibility,
    },
}

impl fmt::Display for QualityIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue => f.write_str("missing value; the row is not in the table"),
            Self::Implausible {
                measurement,
                implausibility,
            } => write!(
                f,
                "implausible {} {}: {implausibility}",
                measurement.value(),
                measurement.unit()
            ),
        }
    }
}

/// Reads an observation CSV into an [`ObservationTable`].
///
/// The input is UTF-8 CSV (RFC 4180, `,` between fields, `\n` or `\r\n`
/// between rows, an optional byte-order mark) whose first row is a header
/// naming exactly the columns `timestamp`, `entity`, `signal`, `value` and
/// `unit`, in any order. Each field is read as its column says:
///
/// - `timestamp`: RFC 3339 with `Z` or `±hh:mm`
///   ([`Timestamp::parse`]); without an offset only if
///   [`ReadOptions::naive_timestamps`] declares one.
/// - `entity`, `signal`: a [`Name`](crate::observation::Name).
/// - `value`: a finite decimal number, or empty for a missing value.
/// - `unit`: a [`Unit`].
///
/// No field may start or end with whitespace. Every problem is collected,
/// up to [`ReadOptions::max_errors`], each with its row, field, what was
/// found, what is allowed and an example. Missing and implausible values are
/// not errors but [`QualityFlag`]s.
///
/// The function reads only from `input`; it opens no files and no network
/// connections.
///
/// # Errors
///
/// [`ReadError::Io`] if `input` fails, [`ReadError::Invalid`] with every
/// problem if any field, row or the header is invalid.
///
/// ```
/// use taktsim_adapters::csv::{ReadOptions, read_observations};
///
/// let csv = "unit,value,signal,entity,timestamp\n\
///            parts,1520,good_count,M1,2026-03-02T06:00:00+01:00\n";
/// let table = read_observations(csv.as_bytes(), &ReadOptions::default())?.table;
/// let row = table.get(0).unwrap();
/// assert_eq!(row.timestamp.to_string(), "2026-03-02T05:00:00Z");
/// assert_eq!(row.measurement.value(), 1520.0);
/// # Ok::<(), taktsim_adapters::csv::ReadError>(())
/// ```
pub fn read_observations<R: Read>(input: R, options: &ReadOptions) -> Result<Import, ReadError> {
    let mut reader = RecordReader::new(BufReader::new(input));
    let mut record = Record::default();
    if !reader.read(&mut record)? || record.row() != 1 {
        return Err(ReadError::Invalid {
            errors: vec![FieldError {
                row: 1,
                field: FieldRef::Row,
                found: String::new(),
                problem: Problem::MissingHeader,
            }],
            truncated: false,
        });
    }
    let limit = options.max_errors.get();
    let columns = header(&record).map_err(|mut errors| {
        let truncated = errors.len() > limit;
        errors.truncate(limit);
        ReadError::Invalid { errors, truncated }
    })?;
    let mut rows = Rows {
        columns,
        naive: options.naive_timestamps,
        builder: TableBuilder::default(),
        flags: Vec::new(),
        errors: Vec::new(),
    };
    while reader.read(&mut record)? {
        rows.add(&record);
        if rows.errors.len() >= limit {
            let dropped = rows.errors.len() > limit;
            rows.errors.truncate(limit);
            let unread = reader.read(&mut record)?;
            return Err(ReadError::Invalid {
                errors: rows.errors,
                truncated: dropped || unread,
            });
        }
    }
    if rows.errors.is_empty() {
        Ok(Import {
            table: rows.builder.finish(),
            flags: rows.flags,
        })
    } else {
        Err(ReadError::Invalid {
            errors: rows.errors,
            truncated: false,
        })
    }
}

/// The header's column for each position, or every header problem.
fn header(record: &Record) -> Result<Vec<Column>, Vec<FieldError>> {
    let error = |field, found, problem| FieldError {
        row: 1,
        field,
        found,
        problem,
    };
    if let (1, Some(only)) = (record.len(), record.field(0))
        && let Some(&delimiter) = b";\t|".iter().find(|d| only.contains(d))
    {
        let problem = Problem::Delimiter {
            delimiter: char::from(delimiter),
        };
        return Err(vec![error(FieldRef::Row, found(only), problem)]);
    }
    let mut columns = Vec::with_capacity(record.len());
    let mut errors = Vec::new();
    for (index, bytes) in record.fields().enumerate() {
        let field = FieldRef::Position(index + 1);
        let parsed = match record.syntax(index) {
            Some(syntax) => Err(syntax_problem(syntax)),
            None => {
                text(bytes).and_then(|name| Column::from_name(name).ok_or(Problem::UnknownColumn))
            }
        };
        match parsed {
            Ok(column) if columns.contains(&column) => {
                errors.push(error(field, found(bytes), Problem::DuplicateColumn));
            }
            Ok(column) => columns.push(column),
            Err(problem) => errors.push(error(field, found(bytes), problem)),
        }
    }
    let line = record.fields().map(found).collect::<Vec<_>>().join(",");
    for column in Column::ALL {
        if !columns.contains(&column) {
            errors.push(error(
                FieldRef::Column(column),
                line.clone(),
                Problem::MissingColumn,
            ));
        }
    }
    if errors.is_empty() {
        Ok(columns)
    } else {
        Err(errors)
    }
}

/// A field as UTF-8 text without surrounding whitespace.
fn text(bytes: &[u8]) -> Result<&str, Problem> {
    let text = std::str::from_utf8(bytes).map_err(|_| Problem::InvalidUtf8)?;
    if text.trim() == text {
        Ok(text)
    } else {
        Err(Problem::Whitespace)
    }
}

/// A field's text for an error message.
fn found(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    match text.char_indices().nth(FOUND_LIMIT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.into_owned(),
    }
}

/// One parsed field.
enum Parsed<'a> {
    Timestamp(Timestamp, SourceOffset),
    Name(&'a str),
    /// `None` for an empty field: a missing value.
    Value(Option<f64>),
    Unit(Unit),
}

/// The data rows read so far.
struct Rows {
    /// The header's column at each position.
    columns: Vec<Column>,
    naive: NaiveTimestamps,
    builder: TableBuilder,
    flags: Vec<QualityFlag>,
    errors: Vec<FieldError>,
}

impl Rows {
    /// Adds one row to the table, or its problems to the errors.
    fn add(&mut self, record: &Record) {
        let row = record.row();
        let errors_before = self.errors.len();
        let (mut timestamp, mut entity, mut signal, mut value, mut unit) =
            (None, None, None, None, None);
        for (index, &column) in self.columns.iter().enumerate() {
            let Some(bytes) = record.field(index) else {
                break;
            };
            match parse(column, bytes, record.syntax(index), self.naive) {
                Ok(Parsed::Timestamp(t, offset)) => timestamp = Some((t, offset)),
                Ok(Parsed::Name(name)) if column == Column::Entity => entity = Some(name),
                Ok(Parsed::Name(name)) => signal = Some(name),
                Ok(Parsed::Value(v)) => value = Some(v),
                Ok(Parsed::Unit(u)) => unit = Some(u),
                Err(problem) => self.errors.push(FieldError {
                    row,
                    field: FieldRef::Column(column),
                    found: found(bytes),
                    problem,
                }),
            }
        }
        // After the field errors, so a row's errors are in column order.
        let (fields, columns) = (record.len(), self.columns.len());
        if fields < columns {
            let field = self
                .columns
                .get(fields)
                .map_or(FieldRef::Row, |&c| FieldRef::Column(c));
            self.errors.push(FieldError {
                row,
                field,
                found: String::new(),
                problem: Problem::FieldCount { fields, columns },
            });
        } else if fields > columns {
            self.errors.push(FieldError {
                row,
                field: FieldRef::Position(columns + 1),
                found: record.field(columns).map(found).unwrap_or_default(),
                problem: Problem::FieldCount { fields, columns },
            });
        }
        if self.errors.len() > errors_before {
            return;
        }
        let (Some((timestamp, offset)), Some(entity), Some(signal), Some(value), Some(unit)) =
            (timestamp, entity, signal, value, unit)
        else {
            return;
        };
        let Some(value) = value else {
            self.flags.push(QualityFlag {
                row,
                issue: QualityIssue::MissingValue,
            });
            return;
        };
        let measurement = Measurement::from_finite(value, unit);
        if let Some(implausibility) = measurement.implausibility() {
            self.flags.push(QualityFlag {
                row,
                issue: QualityIssue::Implausible {
                    measurement,
                    implausibility,
                },
            });
        }
        self.builder
            .push(timestamp, offset, entity, signal, measurement);
    }
}

/// Parses one field of `column`.
fn parse(
    column: Column,
    bytes: &[u8],
    syntax: Option<Syntax>,
    naive: NaiveTimestamps,
) -> Result<Parsed<'_>, Problem> {
    if let Some(syntax) = syntax {
        return Err(syntax_problem(syntax));
    }
    let text = text(bytes)?;
    match column {
        Column::Timestamp => {
            let (timestamp, offset) = Timestamp::parse(text, naive)?;
            Ok(Parsed::Timestamp(timestamp, offset))
        }
        Column::Entity | Column::Signal => {
            validate_name(text)?;
            Ok(Parsed::Name(text))
        }
        Column::Value if text.is_empty() => Ok(Parsed::Value(None)),
        Column::Value => Ok(Parsed::Value(Some(parse_value(text)?))),
        Column::Unit => Ok(Parsed::Unit(text.parse::<Unit>()?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "timestamp,entity,signal,value,unit\n";

    fn errors(csv: &str) -> Vec<FieldError> {
        match read_observations(csv.as_bytes(), &ReadOptions::default()) {
            Err(ReadError::Invalid { errors, .. }) => errors,
            other => panic!("{other:?}"),
        }
    }

    fn field_error(row: u64, field: FieldRef, found: &str, problem: Problem) -> FieldError {
        FieldError {
            row,
            field,
            found: found.to_owned(),
            problem,
        }
    }

    #[test]
    fn header_problems() {
        let missing = |line: &str, column| {
            field_error(1, FieldRef::Column(column), line, Problem::MissingColumn)
        };
        assert_eq!(
            errors("timestamp,entity,signal,value,unit,unit,extra\n"),
            [
                field_error(1, FieldRef::Position(6), "unit", Problem::DuplicateColumn),
                field_error(1, FieldRef::Position(7), "extra", Problem::UnknownColumn),
            ]
        );
        assert_eq!(
            errors("Timestamp,entity, signal,value\n"),
            [
                field_error(
                    1,
                    FieldRef::Position(1),
                    "Timestamp",
                    Problem::UnknownColumn
                ),
                field_error(1, FieldRef::Position(3), " signal", Problem::Whitespace),
                missing("Timestamp,entity, signal,value", Column::Timestamp),
                missing("Timestamp,entity, signal,value", Column::Signal),
                missing("Timestamp,entity, signal,value", Column::Unit),
            ]
        );
        assert_eq!(
            errors("timestamp\tentity\tsignal\tvalue\tunit\n"),
            [field_error(
                1,
                FieldRef::Row,
                "timestamp\tentity\tsignal\tvalue\tunit",
                Problem::Delimiter { delimiter: '\t' }
            )]
        );
        let latin1 = read_observations(&b"timestamp,entit\xe9\n"[..], &ReadOptions::default());
        let Err(ReadError::Invalid { errors: latin1, .. }) = latin1 else {
            panic!()
        };
        assert_eq!(
            latin1[0],
            field_error(
                1,
                FieldRef::Position(2),
                "entit\u{fffd}",
                Problem::InvalidUtf8
            )
        );
        assert_eq!(latin1[0].allowed(), HEADER_ALLOWED);
        assert_eq!(latin1[0].example(), HEADER_EXAMPLE);
        assert_eq!(
            errors("timestamp,\"entity\"x,signal,value,unit\n")[0],
            field_error(1, FieldRef::Position(2), "entityx", Problem::TextAfterQuote)
        );
    }

    #[test]
    fn missing_header() {
        let expected = [field_error(1, FieldRef::Row, "", Problem::MissingHeader)];
        assert_eq!(errors(""), expected);
        assert_eq!(errors("\ntimestamp,entity,signal,value,unit\n"), expected);
        assert_eq!(
            expected[0].to_string(),
            "row 1: the first row must be the header; found nothing; allowed: a first row naming \
             exactly the columns timestamp, entity, signal, value and unit, each once, in any \
             order, separated by `,`; example: timestamp,entity,signal,value,unit"
        );
    }

    #[test]
    fn field_count() {
        let short = format!("{HEADER}2026-03-02T06:00:00Z,M1,state\n");
        assert_eq!(
            errors(&short),
            [field_error(
                2,
                FieldRef::Column(Column::Value),
                "",
                Problem::FieldCount {
                    fields: 3,
                    columns: 5
                }
            )]
        );
        let long = format!("{HEADER}2026-03-02T06:00:00Z,M1,state,1,1,,\n");
        let errs = errors(&long);
        assert_eq!(
            errs,
            [field_error(
                2,
                FieldRef::Position(6),
                "",
                Problem::FieldCount {
                    fields: 7,
                    columns: 5
                }
            )]
        );
        assert_eq!(errs[0].allowed(), ROW_ALLOWED);
        assert_eq!(errs[0].example(), ROW_EXAMPLE);
        assert_eq!(
            errs[0].to_string(),
            "row 2, field 6: the row has 7 fields, but the header has 5; found nothing; \
             allowed: one field for each column of the header; \
             example: 2026-03-02T06:00:00+01:00,M1,good_count,1520,parts"
        );
    }

    #[test]
    fn every_field_problem_is_reported() {
        let csv = format!(
            "{HEADER}2026-13-02T06:00:00Z, M1,a\tb,\"12,5\",pcs\n\
             2026-03-02T06:00:00Z,\"M\"1\",state,1,1\n"
        );
        let errs = errors(&csv);
        assert_eq!(
            errs,
            [
                field_error(
                    2,
                    FieldRef::Column(Column::Timestamp),
                    "2026-13-02T06:00:00Z",
                    Problem::Timestamp(TimestampError::OutOfRange {
                        field: "month",
                        value: 13,
                        min: 1,
                        max: 12
                    })
                ),
                field_error(
                    2,
                    FieldRef::Column(Column::Entity),
                    " M1",
                    Problem::Whitespace
                ),
                field_error(
                    2,
                    FieldRef::Column(Column::Signal),
                    "a\tb",
                    Problem::Name(NameError::ControlCharacter { position: 2 })
                ),
                field_error(
                    2,
                    FieldRef::Column(Column::Value),
                    "12,5",
                    Problem::Value(ValueError::DecimalComma)
                ),
                field_error(
                    2,
                    FieldRef::Column(Column::Unit),
                    "pcs",
                    Problem::Unit(UnknownUnit)
                ),
                field_error(
                    3,
                    FieldRef::Column(Column::Entity),
                    "M1\"",
                    Problem::TextAfterQuote
                ),
            ]
        );
        assert_eq!(errs[5].allowed(), QUOTING_ALLOWED);
        assert_eq!(errs[5].example(), QUOTING_EXAMPLE);
        assert_eq!(errs[1].allowed(), NameError::ALLOWED);
        assert_eq!(errs[2].example(), "good_count");
    }

    #[test]
    fn error_limit() {
        let mut csv = HEADER.to_owned();
        for _ in 0..5 {
            csv.push_str("x,M1,state,1,1\n");
        }
        let options = ReadOptions {
            max_errors: NonZeroUsize::new(3).unwrap(),
            ..ReadOptions::default()
        };
        let read = |csv: &str| match read_observations(csv.as_bytes(), &options) {
            Err(ReadError::Invalid { errors, truncated }) => (errors.len(), truncated),
            other => panic!("{other:?}"),
        };
        assert_eq!(read(&csv), (3, true));
        // Exactly at the limit with nothing left: nothing unchecked.
        let three = format!("{HEADER}{}", "x,M1,state,1,1\n".repeat(3));
        assert_eq!(read(&three), (3, false));
        // One row with more problems than the limit.
        assert_eq!(read(&format!("{HEADER}x,,,x,x\n")), (3, true));
        // The limit holds for the header, too: 4 unknown, 5 missing columns.
        assert_eq!(read("a,b,c,d\n"), (3, true));
        assert_eq!(read("timestamp,entity,signal,value\n"), (1, false));
        let err = read_observations(csv.as_bytes(), &options).unwrap_err();
        assert!(
            err.to_string()
                .ends_with("(stopped at the limit of 3 errors; later rows were not checked)")
        );
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn io_errors_are_passed_on() {
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("disk on fire"))
            }
        }
        let err = read_observations(Failing, &ReadOptions::default()).unwrap_err();
        assert!(matches!(err, ReadError::Io(_)));
        assert_eq!(
            err.to_string(),
            "cannot read the observation CSV: disk on fire"
        );
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn display_of_one_error_and_its_source() {
        let err = read_observations(&b""[..], &ReadOptions::default()).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("1 error in the observation CSV:\n  row 1: ")
        );
        let first = errors("")[0].clone();
        assert!(std::error::Error::source(&first).is_some());
    }

    #[test]
    fn found_text_is_cut() {
        assert_eq!(
            found(&"é".repeat(150).into_bytes()),
            format!("{}…", "é".repeat(100))
        );
        assert_eq!(found(b"short"), "short");
    }

    #[test]
    fn flags_read_well() {
        let csv = format!(
            "{HEADER}1970-01-01T00:00:00Z,M1,c,,parts\n1970-01-01T00:00:00Z,M1,c,0.5,parts\n"
        );
        let import = read_observations(csv.as_bytes(), &ReadOptions::default()).unwrap();
        let flags: Vec<String> = import.flags.iter().map(ToString::to_string).collect();
        assert_eq!(
            flags,
            [
                "row 2, column `value`: missing value; the row is not in the table",
                "row 3, column `value`: implausible 0.5 parts: a count that is not a whole number",
            ]
        );
    }

    #[test]
    fn naive_timestamps_with_a_declared_offset() {
        let csv = format!("{HEADER}2026-03-02T06:00:00,M1,state,1,1\n");
        assert_eq!(
            errors(&csv),
            [field_error(
                2,
                FieldRef::Column(Column::Timestamp),
                "2026-03-02T06:00:00",
                Problem::Timestamp(TimestampError::MissingOffset)
            )]
        );
        let options = ReadOptions {
            naive_timestamps: NaiveTimestamps::AtOffset(crate::observation::UtcOffset::UTC),
            ..ReadOptions::default()
        };
        let table = read_observations(csv.as_bytes(), &options).unwrap().table;
        assert_eq!(
            table.get(0).unwrap().timestamp.to_string(),
            "2026-03-02T06:00:00Z"
        );
    }
}
