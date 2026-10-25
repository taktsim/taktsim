//! CSV export: a typed CSV writer, and the observation table written with
//! it.

use std::io::{self, BufWriter, Write};

use super::Column;
use super::read::visible;
use crate::observation::{Name, ObservationTable, Shortest, SourceOffset, Timestamp, Unit};

/// One field of a CSV row, typed, so every writer formats numbers and
/// timestamps the same way and the reader accepts them back.
///
/// - Text is written as is, quoted (RFC 4180) only if it contains `,`, `"`
///   or a line break, or starts with a byte-order mark.
/// - Integers in decimal.
/// - Floats as the shortest decimal text that reads back to the same `f64`,
///   with an exponent below 10⁻⁵ and from 10¹⁶ (e.g. `1e-7`, `1e16`).
///   NaN and infinity are refused.
/// - Timestamps in RFC 3339 at the given offset
///   ([`Timestamp::rfc3339`]).
///
/// ```
/// use taktsim_adapters::csv::{Cell, CsvWriter};
///
/// let mut writer = CsvWriter::new(Vec::new(), &["name", "n", "x"])?;
/// writer.write_row(&[Cell::from("a,b"), Cell::from(42_u64), Cell::from(0.1)])?;
/// writer.write_row(&[Cell::Empty, Cell::from(-1_i64), Cell::from(1e-7)])?;
/// let csv = String::from_utf8(writer.finish()?)?;
/// assert_eq!(csv, "name,n,x\n\"a,b\",42,0.1\n,-1,1e-7\n");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell<'a> {
    /// Text.
    Text(&'a str),
    /// A signed integer.
    Integer(i64),
    /// An unsigned integer.
    Unsigned(u64),
    /// A finite floating-point number.
    Float(f64),
    /// An instant, written in RFC 3339 at the source's offset.
    Timestamp(Timestamp, SourceOffset),
    /// An empty field.
    Empty,
}

impl<'a> From<&'a str> for Cell<'a> {
    fn from(text: &'a str) -> Self {
        Self::Text(text)
    }
}

impl<'a> From<&'a Name> for Cell<'a> {
    fn from(name: &'a Name) -> Self {
        Self::Text(name.as_str())
    }
}

impl From<Unit> for Cell<'_> {
    fn from(unit: Unit) -> Self {
        Self::Text(unit.as_str())
    }
}

impl From<i64> for Cell<'_> {
    fn from(n: i64) -> Self {
        Self::Integer(n)
    }
}

impl From<u64> for Cell<'_> {
    fn from(n: u64) -> Self {
        Self::Unsigned(n)
    }
}

impl From<f64> for Cell<'_> {
    fn from(x: f64) -> Self {
        Self::Float(x)
    }
}

/// Writes CSV with a fixed header, row by row (RFC 4180, `,` between
/// fields, `\n` after every row, UTF-8 without a byte-order mark).
///
/// Output is buffered and identical on every platform. Each row must have
/// one [`Cell`] per column. The writer writes only to the `out` it is
/// given; call [`CsvWriter::finish`] to flush it and get it back.
///
/// ```
/// use taktsim_adapters::csv::{Cell, CsvWriter};
/// use taktsim_adapters::observation::{SourceOffset, Timestamp, UtcOffset};
///
/// let mut writer = CsvWriter::new(Vec::new(), &["time", "entity"])?;
/// let utc = SourceOffset::Known(UtcOffset::UTC);
/// writer.write_row(&[Cell::Timestamp(Timestamp::UNIX_EPOCH, utc), Cell::from("M1")])?;
/// assert_eq!(writer.finish()?, b"time,entity\n1970-01-01T00:00:00Z,M1\n");
/// # Ok::<(), taktsim_adapters::csv::WriteError>(())
/// ```
#[derive(Debug)]
pub struct CsvWriter<W: Write> {
    out: BufWriter<W>,
    columns: Vec<String>,
    /// Rows written, the header included.
    rows: u64,
    line: Vec<u8>,
}

impl<W: Write> CsvWriter<W> {
    /// A writer with these columns; writes the header row.
    ///
    /// # Errors
    ///
    /// [`WriteError::NoColumns`], [`WriteError::EmptyColumnName`] or
    /// [`WriteError::DuplicateColumn`] for an invalid header,
    /// [`WriteError::Io`] if `out` fails.
    ///
    /// ```
    /// use taktsim_adapters::csv::{CsvWriter, WriteError};
    ///
    /// assert!(CsvWriter::new(Vec::new(), &["a", "b"]).is_ok());
    /// assert!(matches!(CsvWriter::new(Vec::new(), &["a", "a"]), Err(WriteError::DuplicateColumn { .. })));
    /// ```
    pub fn new(out: W, columns: &[&str]) -> Result<Self, WriteError> {
        if columns.is_empty() {
            return Err(WriteError::NoColumns);
        }
        for (index, column) in columns.iter().enumerate() {
            if column.is_empty() {
                return Err(WriteError::EmptyColumnName {
                    position: index + 1,
                });
            }
            if columns[..index].contains(column) {
                return Err(WriteError::DuplicateColumn {
                    column: (*column).to_owned(),
                });
            }
        }
        let mut writer = Self {
            out: BufWriter::new(out),
            columns: columns.iter().map(|&c| c.to_owned()).collect(),
            rows: 0,
            line: Vec::new(),
        };
        let header: Vec<Cell<'_>> = columns.iter().map(|&c| Cell::Text(c)).collect();
        writer.write_row(&header)?;
        Ok(writer)
    }

    /// Writes one row.
    ///
    /// # Errors
    ///
    /// [`WriteError::CellCount`] unless there is one cell per column,
    /// [`WriteError::NotFinite`] for a NaN or infinite [`Cell::Float`],
    /// [`WriteError::Io`] if `out` fails. Nothing of a refused row is
    /// written.
    ///
    /// ```
    /// use taktsim_adapters::csv::{Cell, CsvWriter, WriteError};
    ///
    /// let mut writer = CsvWriter::new(Vec::new(), &["x"])?;
    /// writer.write_row(&[Cell::from(1.5)])?;
    /// let err = writer.write_row(&[Cell::from(f64::NAN)]).unwrap_err();
    /// assert!(err.to_string().starts_with("row 3, column `x`: not a finite number; found NaN; allowed: "));
    /// assert_eq!(writer.finish()?, b"x\n1.5\n");
    /// # Ok::<(), WriteError>(())
    /// ```
    pub fn write_row(&mut self, cells: &[Cell<'_>]) -> Result<(), WriteError> {
        let row = self.rows + 1;
        if cells.len() != self.columns.len() {
            return Err(WriteError::CellCount {
                row,
                cells: cells.len(),
                columns: self.columns.len(),
            });
        }
        let only = cells.len() == 1;
        self.line.clear();
        for (index, (cell, column)) in cells.iter().zip(&self.columns).enumerate() {
            if index > 0 {
                self.line.push(b',');
            }
            match *cell {
                Cell::Text(text) => push_text(&mut self.line, text, only),
                Cell::Integer(n) => write!(self.line, "{n}")?,
                Cell::Unsigned(n) => write!(self.line, "{n}")?,
                Cell::Float(x) if !x.is_finite() => {
                    return Err(WriteError::NotFinite {
                        row,
                        column: column.clone(),
                        value: x,
                    });
                }
                Cell::Float(x) => push_float(&mut self.line, x)?,
                Cell::Timestamp(timestamp, offset) => {
                    write!(self.line, "{}", timestamp.rfc3339(offset))?;
                }
                Cell::Empty => push_text(&mut self.line, "", only),
            }
        }
        self.line.push(b'\n');
        self.out.write_all(&self.line)?;
        self.rows = row;
        Ok(())
    }

    /// Flushes the output and returns it.
    ///
    /// # Errors
    ///
    /// [`WriteError::Io`] if flushing fails.
    ///
    /// ```
    /// use taktsim_adapters::csv::CsvWriter;
    ///
    /// let out = CsvWriter::new(Vec::new(), &["a"])?.finish()?;
    /// assert_eq!(out, b"a\n");
    /// # Ok::<(), taktsim_adapters::csv::WriteError>(())
    /// ```
    pub fn finish(self) -> Result<W, WriteError> {
        self.out
            .into_inner()
            .map_err(|err| WriteError::Io(err.into_error()))
    }
}

/// Text, quoted if it contains a separator, a quote or a line break, if it
/// starts with a byte-order mark (which the reader removes from the start
/// of the input), or if it is empty and the only field (an unquoted one
/// would be a blank line).
fn push_text(line: &mut Vec<u8>, text: &str, only: bool) {
    let quote = (only && text.is_empty())
        || text.starts_with('\u{feff}')
        || text
            .bytes()
            .any(|b| matches!(b, b',' | b'"' | b'\r' | b'\n'));
    if quote {
        line.push(b'"');
        for byte in text.bytes() {
            if byte == b'"' {
                line.push(b'"');
            }
            line.push(byte);
        }
        line.push(b'"');
    } else {
        line.extend_from_slice(text.as_bytes());
    }
}

/// The shortest text that reads back to `x` (finite), see [`Shortest`].
fn push_float(line: &mut Vec<u8>, x: f64) -> io::Result<()> {
    write!(line, "{}", Shortest(x))
}

/// Why CSV could not be written.
///
/// [`Display`](std::fmt::Display) says what is wrong, where (row and
/// column) and what was found; [`WriteError::allowed`] and
/// [`WriteError::example`] say what would be accepted, and the message
/// ends with both.
///
/// ```
/// use taktsim_adapters::csv::{CsvWriter, WriteError};
///
/// let err = CsvWriter::new(Vec::new(), &[]).unwrap_err();
/// assert!(matches!(err, WriteError::NoColumns));
/// assert_eq!(
///     err.to_string(),
///     "a CSV header needs at least one column; found no columns; \
///      allowed: one or more distinct, non-empty column names; example: [\"entity\", \"value\"]"
/// );
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum WriteError {
    /// The output failed.
    Io(io::Error),
    /// The header has no columns.
    NoColumns,
    /// A column name is empty.
    EmptyColumnName {
        /// 1-based position in the header.
        position: usize,
    },
    /// A column name appears twice.
    DuplicateColumn {
        /// The name.
        column: String,
    },
    /// A row with more or fewer cells than the header has columns.
    CellCount {
        /// The 1-based row, the header being row 1.
        row: u64,
        /// Cells given.
        cells: usize,
        /// Columns in the header.
        columns: usize,
    },
    /// A float that is NaN or infinite, which the reader would refuse.
    NotFinite {
        /// The 1-based row, the header being row 1.
        row: u64,
        /// The column's name.
        column: String,
        /// The value.
        value: f64,
    },
}

impl WriteError {
    /// What the writer accepts where this error happened.
    ///
    /// ```
    /// use taktsim_adapters::csv::{Cell, CsvWriter};
    ///
    /// let mut writer = CsvWriter::new(Vec::new(), &["x"])?;
    /// let err = writer.write_row(&[Cell::from(f64::NAN)]).unwrap_err();
    /// assert!(err.allowed().starts_with("a finite number"));
    /// # Ok::<(), taktsim_adapters::csv::WriteError>(())
    /// ```
    #[must_use]
    pub fn allowed(&self) -> &'static str {
        match self {
            Self::Io(_) => "an output that accepts every write and the final flush",
            Self::NoColumns | Self::EmptyColumnName { .. } | Self::DuplicateColumn { .. } => {
                "one or more distinct, non-empty column names"
            }
            Self::CellCount { .. } => "one cell for each column of the header, in header order",
            Self::NotFinite { .. } => {
                "a finite number, which reads back; for a missing value, an empty cell"
            }
        }
    }

    /// One valid header, row or cell for this error.
    ///
    /// ```
    /// use taktsim_adapters::csv::{CsvWriter, WriteError};
    ///
    /// let err = CsvWriter::new(Vec::new(), &["a", "a"]).unwrap_err();
    /// assert_eq!(err.example(), "[\"entity\", \"value\"]");
    /// ```
    #[must_use]
    pub fn example(&self) -> &'static str {
        match self {
            Self::Io(_) => "a `Vec<u8>`, or a file created for writing",
            Self::NoColumns | Self::EmptyColumnName { .. } | Self::DuplicateColumn { .. } => {
                "[\"entity\", \"value\"]"
            }
            Self::CellCount { .. } => {
                "[Cell::from(\"M1\"), Cell::from(1.5)] for the header [\"entity\", \"value\"]"
            }
            Self::NotFinite { .. } => "Cell::from(1.5), or Cell::Empty",
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "cannot write the CSV: {err}"),
            Self::NoColumns => {
                f.write_str("a CSV header needs at least one column; found no columns")
            }
            Self::EmptyColumnName { position } => write!(
                f,
                "column {position} of the CSV header has an empty name; found ``"
            ),
            Self::DuplicateColumn { column } => write!(
                f,
                "a column appears more than once in the CSV header; found `{}` twice",
                visible(column)
            ),
            Self::CellCount {
                row,
                cells,
                columns,
            } => write!(
                f,
                "row {row}: the CSV header has {columns} columns; found {cells} cells"
            ),
            Self::NotFinite { row, column, value } => write!(
                f,
                "row {row}, column `{}`: not a finite number; found {value}",
                visible(column)
            ),
        }?;
        write!(
            f,
            "; allowed: {}; example: {}",
            self.allowed(),
            self.example()
        )
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::NoColumns
            | Self::EmptyColumnName { .. }
            | Self::DuplicateColumn { .. }
            | Self::CellCount { .. }
            | Self::NotFinite { .. } => None,
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Writes an observation table as CSV: the header
/// `timestamp,entity,signal,value,unit`, then one row per observation in
/// table order, each timestamp at its source's offset.
///
/// Reading the output with [`read_observations`](super::read_observations)
/// gives back an equal table, value for value and offset for offset, and
/// writing that table again gives the same bytes.
///
/// # Errors
///
/// [`WriteError::Io`] if `out` fails.
///
/// ```
/// use taktsim_adapters::csv::{ReadOptions, read_observations, write_observations};
///
/// let csv = "timestamp,entity,signal,value,unit\n\
///            2026-10-25T02:30:00+01:00,M1,state,2,1\n\
///            2026-10-25T02:30:00+02:00,M1,state,1,1\n";
/// let table = read_observations(csv.as_bytes(), &ReadOptions::default())?.table;
/// let out = String::from_utf8(write_observations(&table, Vec::new())?)?;
/// // Sorted by the UTC instant: 02:30+02:00 comes first.
/// assert_eq!(
///     out,
///     "timestamp,entity,signal,value,unit\n\
///      2026-10-25T02:30:00+02:00,M1,state,1,1\n\
///      2026-10-25T02:30:00+01:00,M1,state,2,1\n"
/// );
/// assert_eq!(read_observations(out.as_bytes(), &ReadOptions::default())?.table, table);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn write_observations<W: Write>(table: &ObservationTable, out: W) -> Result<W, WriteError> {
    let mut writer = CsvWriter::new(out, &Column::ALL.map(Column::name))?;
    for observation in table {
        writer.write_row(&[
            Cell::Timestamp(observation.timestamp, observation.source_offset),
            Cell::from(observation.entity),
            Cell::from(observation.signal),
            Cell::Float(observation.measurement.value()),
            Cell::from(observation.measurement.unit()),
        ])?;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::csv::record::{Next, Record, RecordReader};

    fn row(cells: &[Cell<'_>]) -> String {
        let columns: Vec<String> = (0..cells.len()).map(|i| format!("c{i}")).collect();
        let columns: Vec<&str> = columns.iter().map(String::as_str).collect();
        let mut writer = CsvWriter::new(Vec::new(), &columns).unwrap();
        writer.write_row(cells).unwrap();
        let out = String::from_utf8(writer.finish().unwrap()).unwrap();
        out.split_once('\n').unwrap().1.to_owned()
    }

    #[test]
    fn cells() {
        assert_eq!(
            row(&[
                Cell::from("plain"),
                Cell::from("a\"b"),
                Cell::from("x\r\ny")
            ]),
            "plain,\"a\"\"b\",\"x\r\ny\"\n"
        );
        assert_eq!(row(&[Cell::Empty]), "\"\"\n");
        assert_eq!(row(&[Cell::from("")]), "\"\"\n");
        assert_eq!(row(&[Cell::Empty, Cell::Empty]), ",\n");
        assert_eq!(
            row(&[Cell::from(i64::MIN), Cell::from(u64::MAX)]),
            "-9223372036854775808,18446744073709551615\n"
        );
        assert_eq!(row(&[Cell::from(Unit::PartsPerHour)]), "parts/h\n");
        let name = Name::new("M1").unwrap();
        assert_eq!(row(&[Cell::from(&name)]), "M1\n");
    }

    #[test]
    fn floats() {
        let float = |x: f64| row(&[Cell::from(x)]).trim_end().to_owned();
        assert_eq!(float(0.0), "0");
        assert_eq!(float(-0.0), "-0");
        assert_eq!(float(1.5), "1.5");
        assert_eq!(float(0.1), "0.1");
        assert_eq!(float(1e-5), "0.00001");
        assert_eq!(float(9.99e-6), "9.99e-6");
        assert_eq!(float(9_999_999_999_999_998.0), "9999999999999998");
        assert_eq!(float(1e16), "1e16");
        assert_eq!(float(-1.25e300), "-1.25e300");
        assert_eq!(float(f64::MAX), "1.7976931348623157e308");
        assert_eq!(float(5e-324), "5e-324");
    }

    #[test]
    fn header_and_row_errors() {
        let messages = |err: WriteError| (err.to_string(), err.allowed(), err.example());
        let (text, ..) = messages(CsvWriter::new(Vec::new(), &[]).unwrap_err());
        assert!(text.starts_with("a CSV header needs at least one column; found no columns; "));
        assert!(matches!(
            CsvWriter::new(Vec::new(), &[]),
            Err(WriteError::NoColumns)
        ));
        assert!(matches!(
            CsvWriter::new(Vec::new(), &["a", ""]),
            Err(WriteError::EmptyColumnName { position: 2 })
        ));
        let header = (
            "one or more distinct, non-empty column names",
            "[\"entity\", \"value\"]",
        );
        let (text, allowed, example) =
            messages(CsvWriter::new(Vec::new(), &["a", "b", "a"]).unwrap_err());
        assert_eq!((allowed, example), header);
        assert_eq!(
            text,
            "a column appears more than once in the CSV header; found `a` twice; \
             allowed: one or more distinct, non-empty column names; example: [\"entity\", \"value\"]"
        );
        let (text, ..) = messages(CsvWriter::new(Vec::new(), &[""]).unwrap_err());
        assert!(
            text.starts_with("column 1 of the CSV header has an empty name; found ``; allowed: ")
        );
        let mut writer = CsvWriter::new(Vec::new(), &["a", "b"]).unwrap();
        let (text, allowed, example) = messages(writer.write_row(&[Cell::Empty]).unwrap_err());
        assert_eq!(
            text,
            format!(
                "row 2: the CSV header has 2 columns; found 1 cells; allowed: {allowed}; example: {example}"
            )
        );
        assert_eq!(
            allowed,
            "one cell for each column of the header, in header order"
        );
        let err = writer
            .write_row(&[Cell::Empty, Cell::from(f64::NEG_INFINITY)])
            .unwrap_err();
        assert!(std::error::Error::source(&err).is_none());
        let (text, allowed, example) = messages(err);
        assert_eq!(
            text,
            format!(
                "row 2, column `b`: not a finite number; found -inf; allowed: {allowed}; example: {example}"
            )
        );
        assert_eq!(example, "Cell::from(1.5), or Cell::Empty");
        writer.write_row(&[Cell::Empty, Cell::from(1.0)]).unwrap();
        assert_eq!(writer.finish().unwrap(), b"a,b\n,1\n");
        // Names in messages are shown with invisible characters escaped.
        let (text, ..) = messages(CsvWriter::new(Vec::new(), &["\u{1b}x", "\u{1b}x"]).unwrap_err());
        assert!(text.contains("found `\\u{1b}x` twice"));
    }

    #[test]
    fn io_errors_are_passed_on() {
        #[derive(Debug)]
        struct Failing;
        impl Write for Failing {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("disk full"))
            }
        }
        // A row larger than the buffer goes straight to the output.
        let mut writer = CsvWriter::new(Failing, &["a"]).unwrap();
        let big = "x".repeat(100_000);
        assert!(matches!(
            writer.write_row(&[Cell::from(big.as_str())]),
            Err(WriteError::Io(_))
        ));
        let err = writer.finish().unwrap_err();
        assert!(std::error::Error::source(&err).is_some());
        assert_eq!(
            err.to_string(),
            "cannot write the CSV: disk full; allowed: an output that accepts every write and the \
             final flush; example: a `Vec<u8>`, or a file created for writing"
        );
        let table = crate::csv::read_observations(
            "timestamp,entity,signal,value,unit\n1970-01-01T00:00:00Z,M,s,1,1\n".as_bytes(),
            &crate::csv::ReadOptions::default(),
        )
        .unwrap()
        .table;
        assert!(matches!(
            write_observations(&table, Failing),
            Err(WriteError::Io(_))
        ));
    }

    #[test]
    fn a_leading_byte_order_mark_is_quoted() {
        // The reader removes a BOM from the start of the input, so a header
        // that starts with one must be quoted to read back.
        let writer = CsvWriter::new(Vec::new(), &["\u{feff}a", "b"]).unwrap();
        let out = writer.finish().unwrap();
        assert_eq!(out, "\"\u{feff}a\",b\n".as_bytes());
        assert_eq!(read_back(&out), [["\u{feff}a", "b"]]);
        assert_eq!(row(&[Cell::from("\u{feff}x")]), "\"\u{feff}x\"\n");
        assert_eq!(row(&[Cell::from("x\u{feff}")]), "x\u{feff}\n");
    }

    fn read_back(csv: &[u8]) -> Vec<Vec<String>> {
        let mut reader = RecordReader::new(csv, usize::MAX);
        let mut record = Record::default();
        let mut rows = Vec::new();
        while reader.read(&mut record).unwrap() == Next::Record {
            assert_eq!(record.syntax(0), None);
            rows.push(
                record
                    .fields()
                    .map(|f| String::from_utf8(f.to_vec()).unwrap())
                    .collect(),
            );
        }
        rows
    }

    /// Distinct, non-empty column names, any of them possibly starting with
    /// a byte-order mark.
    fn header() -> impl Strategy<Value = Vec<String>> {
        prop::collection::btree_set("\u{feff}?[a-z,\"\r\n ]{1,5}|\\PC{1,4}", 1..4)
            .prop_map(|names| names.into_iter().collect())
    }

    proptest! {
        /// Any header and any text survive the writer and the strict reader
        /// unchanged, whatever separators, quotes, line breaks and
        /// byte-order marks they contain.
        #[test]
        fn text_round_trips((header, rows) in header().prop_flat_map(|header| {
            let n = header.len();
            let cell = "\u{feff}?[a-z,\"\r\n ]{0,6}|\\PC{0,4}";
            (Just(header), prop::collection::vec(prop::collection::vec(cell, n), 0..6))
        })) {
            let columns: Vec<&str> = header.iter().map(String::as_str).collect();
            let mut writer = CsvWriter::new(Vec::new(), &columns).unwrap();
            for row in &rows {
                let cells: Vec<Cell<'_>> = row.iter().map(|t| Cell::from(t.as_str())).collect();
                writer.write_row(&cells).unwrap();
            }
            let out = writer.finish().unwrap();
            let read = read_back(&out);
            prop_assert_eq!(&read[0], &header);
            prop_assert_eq!(&read[1..], &rows[..]);
        }

        /// Single-column rows, where an empty field must not become a blank
        /// line.
        #[test]
        fn single_column_round_trips(rows in prop::collection::vec("[a\"\n]{0,3}", 0..6)) {
            let mut writer = CsvWriter::new(Vec::new(), &["only"]).unwrap();
            for text in &rows {
                writer.write_row(&[Cell::from(text.as_str())]).unwrap();
            }
            let read = read_back(&writer.finish().unwrap());
            let read: Vec<String> = read.into_iter().skip(1).map(|mut r| r.remove(0)).collect();
            prop_assert_eq!(read, rows);
        }

        /// Floats are written as text that parses back to the same bits.
        #[test]
        fn floats_round_trip(bits in any::<u64>()) {
            let x = f64::from_bits(bits);
            prop_assume!(x.is_finite());
            let mut line = Vec::new();
            push_float(&mut line, x).unwrap();
            let text = String::from_utf8(line).unwrap();
            prop_assert_eq!(text.parse::<f64>().unwrap().to_bits(), bits, "{}", text);
        }
    }
}
