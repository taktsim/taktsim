//! Output analysis for taktsim: confidence intervals from independent
//! replications, sequential replication control, time-weighted statistics
//! and MSER-5 warm-up detection (spec §6 "Output analysis"; ADR-0014).
//! Roadmap M8 and M9.
//!
//! Everything here is a pure function of its input: no randomness, no
//! clock, no platform math (ADR-0013), so the same replication results give
//! the same statistics on every platform. Inputs that cannot give a
//! statistic (too few replications, an empty window, NaN) are rejected with
//! a typed [`StatsError`], never with a panic.
//!
//! Every estimate carries its uncertainty (spec §5): a
//! [`ConfidenceInterval`] always holds its half-width, confidence level and
//! number of replications.
//!
//! Observed values (replication results, signal values, observations) are
//! bare `f64`: the statistics do not depend on the unit. The caller declares
//! one unit per KPI and passes every value of that KPI in it, e.g. seconds
//! from [`SimDuration::as_secs_f64`](taktsim_units::SimDuration::as_secs_f64)
//! for lead times. This is a deliberate exception to the rule that
//! quantities cross function boundaries as unit types (ADR-0014); the KPI
//! layer of M8 wraps the results in typed quantities again. Times, windows,
//! levels and precisions are typed.
//!
//! ```
//! use taktsim_stats::{
//!     ConfidenceLevel, ReplicationControl, ReplicationPlan, ReplicationStatus,
//! };
//!
//! // Replications until the 95 % half-width is within 2 % of the mean, at
//! // most 100 of them.
//! let plan = ReplicationPlan::new("2%".parse()?, 10, 100)?;
//! let mut control = ReplicationControl::new(plan);
//! let throughput = [101.2, 98.7, 100.4, 99.9, 100.8, 99.1, 100.3, 98.9, 101.0, 100.1];
//! let mut status = ReplicationStatus::Continue;
//! for value in throughput {
//!     status = control.push(value)?;
//! }
//! assert_eq!(status, ReplicationStatus::PrecisionReached);
//! let report = control.report()?;
//! assert_eq!(report.interval().replications(), 10);
//! assert!(report.precision_reached());
//! assert_eq!(report.interval().level(), ConfidenceLevel::DEFAULT);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod interval;
mod level;
mod sequential;
mod student_t;
mod time_weighted;
mod warm_up;

pub use error::StatsError;
pub use interval::{ConfidenceInterval, Summary};
pub use level::{ConfidenceLevel, Probability};
pub use sequential::{PrecisionReport, ReplicationControl, ReplicationPlan, ReplicationStatus};
pub use student_t::StudentT;
pub use time_weighted::{TimeWeighted, TimeWeightedAccumulator, Window, time_weighted};
pub use warm_up::{MSER_BATCH_SIZE, MserTruncation, TimedMserTruncation, mser5, mser5_timed};
