//! A strict RFC 4180 record reader.
//!
//! Fields are separated by `,` and records by a line break (`\n` or
//! `\r\n`). A field is either unquoted, or enclosed in `"` with each inner
//! `"` doubled; a quoted field may contain `,` and line breaks. Unlike
//! lenient readers, a `"` inside an unquoted field, text after a closing
//! quote and a quote left open at the end of the input are reported, never
//! silently turned into data (`"3"4` is not `34`). Blank lines are skipped. A
//! UTF-8 byte-order mark at the start is removed. A record longer than a
//! limit stops reading, so an input without line breaks, or with a quote
//! that is never closed, is not buffered whole. ADR-0017.

use std::io::{self, BufRead};

/// A syntax problem in one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Syntax {
    /// A `"` inside an unquoted field, e.g. `ab"c`.
    Stray,
    /// Text after the closing quote of a quoted field, e.g. `"3"4`.
    TextAfterClosing,
    /// The input ends inside a quoted field.
    Unclosed,
}

/// One record: its row number and its fields as raw bytes.
#[derive(Debug, Default)]
pub(crate) struct Record {
    /// The 1-based line on which the record starts.
    row: u64,
    /// The record as written, without its final line break.
    raw: Vec<u8>,
    data: Vec<u8>,
    /// The end of each field in `data`.
    ends: Vec<usize>,
    /// The first syntax problem of each field that has one: (field index,
    /// problem), in field order.
    syntax: Vec<(usize, Syntax)>,
}

impl Record {
    pub(crate) fn row(&self) -> u64 {
        self.row
    }

    /// The record as written, e.g. for an error message.
    pub(crate) fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The number of fields.
    pub(crate) fn len(&self) -> usize {
        self.ends.len()
    }

    /// Field `index` (0-based), unescaped.
    pub(crate) fn field(&self, index: usize) -> Option<&[u8]> {
        let end = *self.ends.get(index)?;
        let start = match index.checked_sub(1) {
            Some(previous) => *self.ends.get(previous)?,
            None => 0,
        };
        self.data.get(start..end)
    }

    pub(crate) fn fields(&self) -> impl Iterator<Item = &[u8]> {
        (0..self.len()).filter_map(|index| self.field(index))
    }

    /// The first syntax problem in field `index`, if any.
    pub(crate) fn syntax(&self, index: usize) -> Option<Syntax> {
        self.syntax
            .iter()
            .find(|(field, _)| *field == index)
            .map(|&(_, syntax)| syntax)
    }

    /// Whether a quoted field runs to the end of the input.
    pub(crate) fn unclosed(&self) -> bool {
        self.syntax
            .iter()
            .any(|&(_, syntax)| syntax == Syntax::Unclosed)
    }

    fn clear(&mut self) {
        self.row = 0;
        self.raw.clear();
        self.data.clear();
        self.ends.clear();
        self.syntax.clear();
    }

    fn end_field(&mut self) {
        self.ends.push(self.data.len());
    }

    /// Records a problem in the current field, unless it already has one.
    fn problem(&mut self, syntax: Syntax) {
        let field = self.ends.len();
        if self.syntax.last().is_none_or(|&(last, _)| last != field) {
            self.syntax.push((field, syntax));
        }
    }
}

/// What [`RecordReader::read`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    /// A record, possibly with syntax problems.
    Record,
    /// The end of the input.
    End,
    /// A record longer than the limit. The record holds its row and the
    /// start of its text; reading cannot go on.
    TooLong,
}

/// What [`RecordReader::next_line`] found.
enum Line {
    Read,
    End,
    TooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// At the start of a field.
    FieldStart,
    /// Inside an unquoted field.
    Unquoted,
    /// Inside a quoted field.
    Quoted,
    /// Just after a `"` inside a quoted field: either the closing quote or
    /// the first half of `""`.
    QuoteInQuoted,
}

/// Reads [`Record`]s from a buffered input.
#[derive(Debug)]
pub(crate) struct RecordReader<R> {
    input: R,
    /// The current physical line, with its line break.
    line: Vec<u8>,
    /// The 1-based number of the current physical line; 0 before the first.
    line_number: u64,
    /// The longest record, in bytes as written, line breaks included.
    max_bytes: usize,
}

impl<R: BufRead> RecordReader<R> {
    pub(crate) fn new(input: R, max_bytes: usize) -> Self {
        Self {
            input,
            line: Vec::new(),
            line_number: 0,
            max_bytes,
        }
    }

    /// Reads the next record into `record`, skipping blank lines.
    pub(crate) fn read(&mut self, record: &mut Record) -> io::Result<Next> {
        record.clear();
        loop {
            match self.next_line(self.max_bytes)? {
                Line::End => return Ok(Next::End),
                Line::TooLong => {
                    record.row = self.line_number;
                    record.raw.extend_from_slice(&self.line);
                    return Ok(Next::TooLong);
                }
                Line::Read if split_line_break(&self.line).0.is_empty() => {}
                Line::Read => break,
            }
        }
        record.row = self.line_number;
        let mut state = State::FieldStart;
        loop {
            let (content, line_break) = split_line_break(&self.line);
            record.raw.extend_from_slice(content);
            for &byte in content {
                state = step(state, byte, record);
            }
            if state != State::Quoted {
                record.end_field();
                return Ok(Next::Record);
            }
            // The line break is part of the quoted field.
            record.data.extend_from_slice(line_break);
            record.raw.extend_from_slice(line_break);
            let budget = self.max_bytes.saturating_sub(record.raw.len());
            let next = if line_break.is_empty() {
                Line::End
            } else {
                self.next_line(budget)?
            };
            match next {
                Line::Read => {}
                Line::End => {
                    record.problem(Syntax::Unclosed);
                    record.end_field();
                    return Ok(Next::Record);
                }
                Line::TooLong => {
                    record.raw.extend_from_slice(&self.line);
                    return Ok(Next::TooLong);
                }
            }
        }
    }

    /// Whether any input is left, without reading it. A failing input
    /// counts as "maybe": `true`.
    pub(crate) fn has_more(&mut self) -> bool {
        match self.input.fill_buf() {
            Ok(rest) => !rest.is_empty(),
            Err(_) => true,
        }
    }

    /// Reads the next physical line, line break included, but no more than
    /// `budget` bytes of it.
    fn next_line(&mut self, budget: usize) -> io::Result<Line> {
        self.line.clear();
        loop {
            let available = match self.input.fill_buf() {
                Ok(available) => available,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            if available.is_empty() {
                break;
            }
            let (len, complete) = match available.iter().position(|&b| b == b'\n') {
                Some(end) => (end + 1, true),
                None => (available.len(), false),
            };
            let room = budget.saturating_sub(self.line.len());
            if len > room {
                let start = available.get(..room).unwrap_or_default();
                self.line.extend_from_slice(start);
                self.input.consume(room);
                self.start_line();
                return Ok(Line::TooLong);
            }
            let chunk = available.get(..len).unwrap_or_default();
            self.line.extend_from_slice(chunk);
            self.input.consume(len);
            if complete {
                break;
            }
        }
        if self.line.is_empty() {
            return Ok(Line::End);
        }
        self.start_line();
        Ok(Line::Read)
    }

    /// Counts the line just read, and removes a byte-order mark from the
    /// first.
    fn start_line(&mut self) {
        self.line_number += 1;
        if self.line_number == 1 && self.line.starts_with(b"\xEF\xBB\xBF") {
            self.line.drain(..3);
        }
    }
}

/// A line without and with its line break (`\r\n`, `\n`, or none at the end
/// of the input).
fn split_line_break(line: &[u8]) -> (&[u8], &[u8]) {
    let n = if line.ends_with(b"\r\n") {
        2
    } else {
        usize::from(line.ends_with(b"\n"))
    };
    line.split_at(line.len() - n)
}

fn step(state: State, byte: u8, record: &mut Record) -> State {
    match state {
        State::FieldStart => match byte {
            b'"' => State::Quoted,
            b',' => {
                record.end_field();
                State::FieldStart
            }
            _ => {
                record.data.push(byte);
                State::Unquoted
            }
        },
        State::Unquoted => {
            if byte == b',' {
                record.end_field();
                return State::FieldStart;
            }
            if byte == b'"' {
                record.problem(Syntax::Stray);
            }
            record.data.push(byte);
            State::Unquoted
        }
        State::Quoted => {
            if byte == b'"' {
                State::QuoteInQuoted
            } else {
                record.data.push(byte);
                State::Quoted
            }
        }
        State::QuoteInQuoted => match byte {
            b'"' => {
                record.data.push(b'"');
                State::Quoted
            }
            b',' => {
                record.end_field();
                State::FieldStart
            }
            _ => {
                record.problem(Syntax::TextAfterClosing);
                record.data.push(byte);
                State::Unquoted
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record as (row, fields, syntax problems).
    type Parsed = (u64, Vec<Vec<u8>>, Vec<(usize, Syntax)>);

    fn records(input: &[u8]) -> Vec<Parsed> {
        let mut reader = RecordReader::new(input, 1 << 20);
        let mut record = Record::default();
        let mut out = Vec::new();
        while reader.read(&mut record).unwrap() == Next::Record {
            let fields = record.fields().map(<[u8]>::to_vec).collect();
            out.push((record.row(), fields, record.syntax.clone()));
        }
        out
    }

    fn fields(input: &str) -> Vec<Vec<String>> {
        records(input.as_bytes())
            .into_iter()
            .map(|(_, fields, syntax)| {
                assert_eq!(syntax, [], "{input:?}");
                fields
                    .into_iter()
                    .map(|f| String::from_utf8(f).unwrap())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn plain_and_quoted_fields() {
        assert_eq!(fields("a,b,c\n"), [["a", "b", "c"]]);
        assert_eq!(fields("a,b,c"), [["a", "b", "c"]]);
        assert_eq!(
            fields("a,,c\r\n,,\n"),
            [vec!["a", "", "c"], vec!["", "", ""]]
        );
        assert_eq!(
            fields("\"a,b\",\"say \"\"hi\"\"\",\"\"\n"),
            [["a,b", "say \"hi\"", ""]]
        );
        assert_eq!(fields("\"line\nbreak\",x\n"), [["line\nbreak", "x"]]);
        assert_eq!(fields("\"crlf\r\nkept\",x\r\n"), [["crlf\r\nkept", "x"]]);
        assert_eq!(fields("a\rb\n"), [["a\rb"]]);
        assert_eq!(fields("\"\"\n"), [[""]]);
        assert_eq!(fields("x,\"\"\n"), [["x", ""]]);
        assert_eq!(fields(""), Vec::<Vec<String>>::new());
    }

    #[test]
    fn rows_are_line_numbers_and_blank_lines_are_skipped() {
        let rows: Vec<u64> = records(b"h\n\na\r\n\r\n\"b\nc\"\nd")
            .into_iter()
            .map(|(row, _, _)| row)
            .collect();
        assert_eq!(rows, [1, 3, 5, 7]);
        assert_eq!(records(b"\n\n"), []);
    }

    #[test]
    fn byte_order_mark_is_removed_from_the_first_line_only() {
        assert_eq!(
            fields("\u{feff}a,b\n\u{feff}c\n"),
            [vec!["a", "b"], vec!["\u{feff}c"]]
        );
        // A quoted field may start with one: the mark is inside the quotes.
        assert_eq!(fields("\"\u{feff}a\",b\n"), [["\u{feff}a", "b"]]);
    }

    #[test]
    fn syntax_problems_are_reported_per_field() {
        let one = |input: &str| {
            let mut all = records(input.as_bytes());
            assert_eq!(all.len(), 1, "{input:?}");
            let (_, fields, syntax) = all.remove(0);
            let fields: Vec<String> = fields
                .into_iter()
                .map(|f| String::from_utf8(f).unwrap())
                .collect();
            (fields, syntax)
        };
        assert_eq!(
            one("a,\"3\"4,c\n"),
            (
                vec!["a".into(), "34".into(), "c".into()],
                vec![(1, Syntax::TextAfterClosing)]
            )
        );
        assert_eq!(
            one("ab\"c,d\n"),
            (vec!["ab\"c".into(), "d".into()], vec![(0, Syntax::Stray)])
        );
        assert_eq!(
            one("a\"b\"c,\"x\"y\"z\"\n").1,
            [(0, Syntax::Stray), (1, Syntax::TextAfterClosing)]
        );
        assert_eq!(
            one("a,\"open\nrest,x\n"),
            (
                vec!["a".into(), "open\nrest,x\n".into()],
                vec![(1, Syntax::Unclosed)]
            )
        );
        assert_eq!(one("\"open").1, [(0, Syntax::Unclosed)]);
    }

    #[test]
    fn raw_text_and_unclosed_quotes() {
        let mut reader = RecordReader::new(&b"a,\"b\"\"\nc\",d\r\n\"open\n"[..], 100);
        let mut record = Record::default();
        assert_eq!(reader.read(&mut record).unwrap(), Next::Record);
        assert_eq!(record.raw(), b"a,\"b\"\"\nc\",d");
        assert!(!record.unclosed());
        assert_eq!(reader.read(&mut record).unwrap(), Next::Record);
        assert_eq!(record.raw(), b"\"open\n");
        assert!(record.unclosed());
        assert!(!reader.has_more());
        assert_eq!(reader.read(&mut record).unwrap(), Next::End);
    }

    #[test]
    fn records_longer_than_the_limit_stop_reading() {
        let read = |input: &[u8], limit| {
            let mut reader = RecordReader::new(input, limit);
            let mut record = Record::default();
            let mut out = Vec::new();
            loop {
                let next = reader.read(&mut record).unwrap();
                out.push((next, record.row(), record.raw().to_vec()));
                if next != Next::Record {
                    return (out, reader.has_more());
                }
            }
        };
        // Exactly the limit, line break included, is fine.
        let (out, _) = read(b"abcd\n", 5);
        assert_eq!(out[0], (Next::Record, 1, b"abcd".to_vec()));
        // One byte more is not; the start of the record is kept.
        let (out, more) = read(b"ok\nabcdef\nlater\n", 5);
        assert_eq!(out[0], (Next::Record, 1, b"ok".to_vec()));
        assert_eq!(out[1], (Next::TooLong, 2, b"abcde".to_vec()));
        assert!(more);
        // A line without a line break at all.
        let (out, _) = read(&[b'x'; 50], 8);
        assert_eq!(out[0], (Next::TooLong, 1, vec![b'x'; 8]));
        // A quote that is never closed counts across lines.
        let (out, more) = read(b"\"ab\ncd\nef\ngh\n", 8);
        assert_eq!(out[0], (Next::TooLong, 1, b"\"ab\ncd\ne".to_vec()));
        assert!(more);
        // Blank lines before a long one keep the line number.
        let (out, _) = read(b"\n\nabcdefgh\n", 4);
        assert_eq!(out[0], (Next::TooLong, 3, b"abcd".to_vec()));
    }

    #[test]
    fn input_arriving_in_small_chunks_reads_the_same() {
        let input = b"a,\"b\nc\",d\r\n\n\"x\"\"y\",z\n";
        let small = std::io::BufReader::with_capacity(1, &input[..]);
        let mut reader = RecordReader::new(small, 1 << 20);
        let mut record = Record::default();
        let mut rows = Vec::new();
        while reader.read(&mut record).unwrap() == Next::Record {
            rows.push((
                record.row(),
                record.fields().map(<[u8]>::to_vec).collect::<Vec<_>>(),
            ));
        }
        assert_eq!(
            rows,
            [
                (1, vec![b"a".to_vec(), b"b\nc".to_vec(), b"d".to_vec()]),
                (4, vec![b"x\"y".to_vec(), b"z".to_vec()]),
            ]
        );
    }

    #[test]
    fn field_access_out_of_range() {
        let mut reader = RecordReader::new(&b"a,b\n"[..], 100);
        let mut record = Record::default();
        assert_eq!(reader.read(&mut record).unwrap(), Next::Record);
        assert_eq!(record.len(), 2);
        assert_eq!(record.field(1), Some(&b"b"[..]));
        assert_eq!(record.field(2), None);
        assert_eq!(record.syntax(0), None);
        assert_eq!(reader.read(&mut record).unwrap(), Next::End);
        assert_eq!(record.len(), 0);
    }

    #[test]
    fn failing_and_interrupted_input() {
        struct Flaky {
            calls: u32,
        }
        impl std::io::Read for Flaky {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("unused"))
            }
        }
        impl BufRead for Flaky {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                self.calls += 1;
                match self.calls {
                    1 => Err(io::ErrorKind::Interrupted.into()),
                    2 => Ok(b"a\n"),
                    _ => Err(io::Error::other("gone")),
                }
            }
            fn consume(&mut self, _: usize) {}
        }
        let mut reader = RecordReader::new(Flaky { calls: 0 }, 100);
        let mut record = Record::default();
        assert_eq!(reader.read(&mut record).unwrap(), Next::Record);
        assert!(reader.has_more());
        assert!(reader.read(&mut record).is_err());
    }
}
