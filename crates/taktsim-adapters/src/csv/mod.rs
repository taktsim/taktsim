//! The CSV adapter (spec §8, roadmap M11, ADR-0017): CSV import into the
//! [observation table](crate::observation::ObservationTable), CSV export of
//! it, and a typed [`CsvWriter`] for other tables such as results and
//! traces.
//!
//! The dialect is strict RFC 4180: `,` between fields, `\n` or `\r\n`
//! between rows, fields quoted with `"` when needed, UTF-8. A malformed
//! field is reported with its row and column, never silently changed.
//!
//! ```
//! use taktsim_adapters::csv::{ReadOptions, read_observations, write_observations};
//!
//! let csv = "timestamp,entity,signal,value,unit\n\
//!            2026-03-02T06:00:00+01:00,M1,state,1,1\n\
//!            2026-03-02T06:00:00+01:00,M1,good_count,0,parts\n";
//! let import = read_observations(csv.as_bytes(), &ReadOptions::default())?;
//! assert_eq!(import.table.len(), 2);
//! let out = String::from_utf8(write_observations(&import.table, Vec::new())?)?;
//! // Rows with the same timestamp and entity are sorted by signal.
//! assert!(out.ends_with("good_count,0,parts\n2026-03-02T06:00:00+01:00,M1,state,1,1\n"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod read;
mod record;
mod write;

pub use read::{
    Column, FieldError, FieldRef, Import, Problem, QualityFlag, QualityIssue, ReadError,
    ReadOptions, Stopped, read_observations,
};
pub use write::{Cell, CsvWriter, WriteError, write_observations};
