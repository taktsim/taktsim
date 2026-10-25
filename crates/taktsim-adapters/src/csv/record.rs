//! A strict RFC 4180 record reader.
//!
//! Fields are separated by `,` and records by a line break (`\n` or
//! `\r\n`). A field is either unquoted, or enclosed in `"` with each inner
//! `"` doubled; a quoted field may contain `,` and line breaks. Unlike
//! lenient readers, a `"` inside an unquoted field, text after a closing
//! quote and a quote left open at the end of the input are reported, never
//! silently turned into data (`"3"4` is not `34`). Blank lines are skipped. A
//! UTF-8 byte-order mark at the start is removed. ADR-0017.

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

    fn clear(&mut self) {
        self.row = 0;
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
}

impl<R: BufRead> RecordReader<R> {
    pub(crate) fn new(input: R) -> Self {
        Self {
            input,
            line: Vec::new(),
            line_number: 0,
        }
    }

    /// Reads the next record into `record`, skipping blank lines; `false` at
    /// the end of the input.
    pub(crate) fn read(&mut self, record: &mut Record) -> io::Result<bool> {
        record.clear();
        loop {
            if !self.next_line()? {
                return Ok(false);
            }
            if !split_line_break(&self.line).0.is_empty() {
                break;
            }
        }
        record.row = self.line_number;
        let mut state = State::FieldStart;
        loop {
            let (content, line_break) = split_line_break(&self.line);
            for &byte in content {
                state = step(state, byte, record);
            }
            if state != State::Quoted {
                record.end_field();
                return Ok(true);
            }
            // The line break is part of the quoted field.
            record.data.extend_from_slice(line_break);
            if line_break.is_empty() || !self.next_line()? {
                record.problem(Syntax::Unclosed);
                record.end_field();
                return Ok(true);
            }
        }
    }

    /// Reads the next physical line; `false` at the end of the input.
    fn next_line(&mut self) -> io::Result<bool> {
        self.line.clear();
        if self.input.read_until(b'\n', &mut self.line)? == 0 {
            return Ok(false);
        }
        self.line_number += 1;
        if self.line_number == 1 && self.line.starts_with(b"\xEF\xBB\xBF") {
            self.line.drain(..3);
        }
        Ok(true)
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
        let mut reader = RecordReader::new(input);
        let mut record = Record::default();
        let mut out = Vec::new();
        while reader.read(&mut record).unwrap() {
            let fields = record.fields().map(<[u8]>::to_vec).collect();
            out.push((record.row(), fields, record.syntax.clone()));
        }
        out
    }

    fn fields(input: &str) -> Vec<Vec<String>> {
        records(input.as_bytes())
            .into_iter()
            .map(|(_, fields, syntax)| {
                assert!(syntax.is_empty(), "{input:?}: {syntax:?}");
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
    fn field_access_out_of_range() {
        let mut reader = RecordReader::new(&b"a,b\n"[..]);
        let mut record = Record::default();
        assert!(reader.read(&mut record).unwrap());
        assert_eq!(record.len(), 2);
        assert_eq!(record.field(1), Some(&b"b"[..]));
        assert_eq!(record.field(2), None);
        assert_eq!(record.syntax(0), None);
        assert!(!reader.read(&mut record).unwrap());
        assert_eq!(record.len(), 0);
    }
}
