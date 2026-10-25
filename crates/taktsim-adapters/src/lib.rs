//! Data adapters, each behind its own Cargo feature (spec §8). v0.1: `csv`.
//!
//! Every adapter converts outside data into one internal format, the
//! [observation table](observation::ObservationTable) with the columns
//! `timestamp`, `entity`, `signal`, `value` and `unit`, so calibration and
//! comparison never depend on where data came from. Adapters are read-only
//! toward production systems, and library code does no network or file
//! access of its own: it reads and writes only the
//! [`BufRead`](std::io::BufRead) and [`Write`](std::io::Write) values the caller
//! passes in. Decisions: ADR-0017.
//!
//! Without features the crate is empty. With `csv`:
//!
//! ```
//! # #[cfg(feature = "csv")]
//! # {
//! use taktsim_adapters::csv::{ReadOptions, read_observations, write_observations};
//!
//! let input = "timestamp,entity,signal,value,unit\n\
//!              2026-03-02T06:00:00+01:00,M1,good_count,1520,parts\n";
//! let import = read_observations(input.as_bytes(), &ReadOptions::default())?;
//! assert_eq!(import.table.len(), 1);
//! assert!(import.flags.is_empty());
//!
//! let output = write_observations(&import.table, Vec::new())?;
//! assert_eq!(output, input.as_bytes());
//! # }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "csv")]
pub mod observation;
