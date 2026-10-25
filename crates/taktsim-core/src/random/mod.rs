//! Deterministic randomness: independent streams and duration
//! distributions (ADR-0013).
//!
//! Every random process in a run draws from its own [`Stream`], keyed by
//! the run's seed and a [`StreamKey`] made of the component id and a
//! purpose. A stream's values depend on nothing else, so the same model and
//! seed give the same samples on every platform, and adding a component
//! never shifts the samples of another.
//!
//! ```
//! use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
//!
//! let cycle = DurationDistribution::lognormal("42s".parse()?, 0.08)?;
//! let mut a = Stream::new(42, StreamKey::new("cut", "cycle_time"));
//! let mut b = Stream::new(42, StreamKey::new("cut", "cycle_time"));
//! assert_eq!(cycle.sample(&mut a)?, cycle.sample(&mut b)?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod distribution;
mod philox;
mod stream;

pub use distribution::{DistributionError, DurationDistribution, MAX_ERLANG_PHASES};
pub use stream::{Stream, StreamKey};
