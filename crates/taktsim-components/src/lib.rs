//! Production components built on the engine's primitives: Source, Machine,
//! Buffer and Sink, the protocol between them, and one way to run a model
//! (ADR-0022, roadmap M5). The engine knows nothing about machines; this crate
//! is the first domain package, and later ones copy its shape.
//!
//! ```
//! use taktsim_components::{RunOptions, standard};
//! use taktsim_model::{Format, Loader};
//! use taktsim_trace::{TraceReader, replay};
//!
//! let registry = standard()?;
//! let model = Loader::new(registry.catalog()).load_str(
//!     r#"
//! format: "1.0"
//! model: {name: line-a, seed: 42}
//! experiment: {type: terminating, horizon: 8h}
//! components:
//!   - {id: source, type: Source}
//!   - {id: cut, type: Machine, cycle_time: 42s}
//!   - {id: buf1, type: Buffer, capacity: 5 parts}
//!   - {id: weld, type: Machine, cycle_time: 45s}
//!   - {id: sink, type: Sink}
//! flow:
//!   - [source, cut, buf1, weld, sink]
//! "#,
//!     Format::Yaml,
//! )?;
//! let (trace, report) = registry
//!     .prepare(&model, RunOptions::new())?
//!     .run_traced(Vec::new())?;
//! // The bottleneck sets the pace: the first part leaves after 87 s, and one
//! // more every 45 s until the 8 hours are over.
//! assert_eq!(report.items.delivered.get(), 639);
//!
//! // The trace is the run: re-execute it and compare every record.
//! let mut engine = registry.build(model.document(), taktsim_core::EngineConfig::new(42))?;
//! replay(TraceReader::new(trace.as_slice())?, &mut engine)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod buffer;
mod clock;
mod flow;
mod machine;
mod message;
mod params;
mod port;
mod run;
mod sink;
mod source;
mod state;

pub use buffer::{Buffer, Discipline};
pub use flow::{Flow, FlowShared, standard, standard_schema};
pub use machine::{Machine, Setup};
pub use message::{FlowMessage, ItemAttrs, MachineStateNote, SourceStateNote, VariantId};
pub use run::{
    BuildError, ClockEnd, ComponentType, Domain, Ports, Prepared, Registry, Roles, RunFailure,
    RunOptions, RunReport, Wiring,
};
pub use sink::Sink;
pub use source::Source;
pub use state::{MachineState, SourceState};
