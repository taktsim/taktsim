//! The taktsim engine core: deterministic randomness now; the event queue,
//! clock, component trait, invariants and trace records arrive with the
//! rest of roadmap M2. No I/O; depends only on `taktsim-units` and pure-Rust
//! math.
//!
//! ```
//! use taktsim_core::random::{Stream, StreamKey};
//!
//! let mut stream = Stream::new(42, StreamKey::new("source", "arrivals"));
//! assert!(stream.next_open01() < 1.0);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod random;
