//! Output analysis for taktsim: confidence intervals from independent
//! replications (spec §6 "Output analysis"; ADR-0014). Roadmap M8 and M9.
//!
//! Everything here is a pure function of its input: no randomness, no
//! clock, no platform math (ADR-0013), so the same replication results give
//! the same statistics on every platform. Inputs that cannot give a
//! statistic (too few replications, NaN) are rejected with a typed
//! [`StatsError`], never with a panic.
//!
//! Every estimate carries its uncertainty (spec §5): a
//! [`ConfidenceInterval`] always holds its half-width, confidence level and
//! number of replications.
//!
//! ```
//! use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
//!
//! let throughput = [101.2, 98.7, 100.4, 99.9, 100.8, 99.1, 100.3, 98.9, 101.0, 100.1];
//! let ci = ConfidenceInterval::from_values(&throughput, ConfidenceLevel::DEFAULT)?;
//! assert_eq!(ci.replications(), 10);
//! assert!(ci.relative_half_width().unwrap() < 0.02);
//! # Ok::<(), taktsim_stats::StatsError>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod interval;
mod level;
mod student_t;

pub use error::StatsError;
pub use interval::{ConfidenceInterval, Summary};
pub use level::{ConfidenceLevel, Probability};
pub use student_t::StudentT;
