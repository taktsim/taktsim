//! The internal observation format (spec §8): a typed table with the
//! columns `timestamp`, `entity`, `signal`, `value` and `unit`.
//!
//! Every adapter emits an [`ObservationTable`] and nothing else. Its columns
//! are typed:
//!
//! - `timestamp`: a [`Timestamp`], an exact UTC instant in nanoseconds since
//!   the Unix epoch. The UTC offset the source wrote it in is kept per row as
//!   metadata, a [`SourceOffset`].
//! - `entity` and `signal`: validated [`Name`]s.
//! - `value` and `unit`: a [`Measurement`], a finite number together with
//!   the [`Unit`] it is measured in.
//!
//! The table is held in memory in v0.1; Parquet storage arrives with the
//! Parquet adapter in v0.2. Decisions: ADR-0017.
//!
//! ```
//! use taktsim_adapters::observation::{
//!     Measurement, Name, Observation, ObservationTable, SourceOffset, Timestamp, Unit,
//! };
//!
//! let (timestamp, source_offset) = Timestamp::parse_rfc3339("2026-03-02T06:00:00+01:00")?;
//! let entity = Name::new("M1")?;
//! let signal = Name::new("good_count")?;
//! let table: ObservationTable = [Observation {
//!     timestamp,
//!     source_offset,
//!     entity: &entity,
//!     signal: &signal,
//!     measurement: Measurement::new(1520.0, Unit::Parts)?,
//! }]
//! .into_iter()
//! .collect();
//! assert_eq!(table.len(), 1);
//! assert_eq!(table.get(0).map(|o| o.timestamp.to_string()).as_deref(), Some("2026-03-02T05:00:00Z"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod measurement;
mod name;
mod table;
mod time;

pub use measurement::{
    Dimension, Implausibility, Measurement, Quantity, Unit, UnknownUnit, ValueError,
};
pub use name::{Name, NameError};
pub use table::{Iter, Observation, ObservationTable};
pub use time::{NaiveTimestamps, Rfc3339, SourceOffset, Timestamp, TimestampError, UtcOffset};

pub(crate) use measurement::parse_value;
pub(crate) use name::validate as validate_name;
pub(crate) use table::TableBuilder;
