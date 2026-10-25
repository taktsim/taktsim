//! Quantities with units for taktsim: simulation time, counts, rates and
//! percentages.
//!
//! Every quantity in a model carries a unit (spec §4), and units are types
//! (spec §7): a [`SimDuration`] cannot be passed where a [`Rate`] is
//! expected. Text such as `42s`, `20 parts` or `2%` is parsed exactly, and
//! [`Display`](std::fmt::Display) writes text that parses back to the same
//! value. Simulation time is integer nanoseconds (ADR-0001).
//!
//! ```
//! use taktsim_units::{Count, Ratio, SimDuration};
//!
//! let cycle: SimDuration = "42s".parse()?;
//! let capacity: Count = "20 parts".parse()?;
//! let precision: Ratio = "2%".parse()?;
//! assert_eq!(cycle.checked_mul(capacity.get())?.to_string(), "14min");
//! assert_eq!(precision.as_fraction(), 0.02);
//! # Ok::<(), taktsim_units::UnitError>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod count;
mod decimal;
mod error;
mod ratio;
mod time;

pub use count::{Count, Rate};
pub use error::UnitError;
pub use ratio::Ratio;
pub use time::{SimDuration, SimTime};
