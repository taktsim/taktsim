//! Benchmark workloads for taktsim (ADR-0016).
//!
//! Each workload is a fixed amount of work done through the public API of
//! an engine crate, with fixed inputs and a fixed seed, so it does the same
//! work on every run. Two harnesses run the same workloads:
//!
//! - `benches/wall_*.rs` (criterion): wall-clock time, for a developer
//!   tuning code on their own machine. Never cited as evidence that a
//!   target is met (ADR-0010).
//! - `benches/instructions_*.rs` (gungraun, Callgrind): instruction counts,
//!   measured in CI for the base and the head of every pull request; a rise
//!   of more than 10 % fails it (ADR-0007).
//!
//! ```
//! use taktsim_bench::random::{self, DistributionCase};
//!
//! let cycle = DistributionCase::LogNormal.distribution()?;
//! let total = random::sample_n(&cycle, &mut random::stream(), 10)?;
//! assert!(total > "6min".parse()? && total < "8min".parse()?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod queue;
pub mod random;
pub mod units;
