//! Production components built on the engine's primitives: Source, Machine,
//! Buffer and Sink, the protocol between them, and one way to run a model
//! (ADR-0022, roadmap M5). The engine knows nothing about machines; this crate
//! is the first domain package, and later ones copy its shape.
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod message;
mod state;

pub use message::{FlowMessage, ItemAttrs, MachineStateNote, SourceStateNote, VariantId};
pub use state::{MachineState, SourceState};
