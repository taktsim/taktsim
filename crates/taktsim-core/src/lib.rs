//! The taktsim engine core: the event loop, items and conservation,
//! components, runtime invariants, trace records and deterministic
//! randomness. No I/O; the only taktsim crate it depends on is
//! `taktsim-units` (ADR-0013).
//!
//! A model is a set of [`Component`]s exchanging [`Message`]s through an
//! [`Engine`]. Components never call each other: they schedule events,
//! send messages and move items through their [`Context`], so the engine
//! sees and checks every change (ADR-0018).
//!
//! ```
//! use taktsim_core::{
//!     Component, ComponentError, Context, Delivery, Disposal, Engine, EngineConfig, Message,
//!     NoTrace, RunLimits,
//! };
//!
//! #[derive(Debug)]
//! struct Arrive;
//! impl Message for Arrive {}
//!
//! /// Creates an item every 10 s and sends it to `sink`.
//! struct Source;
//! impl Component<Arrive> for Source {
//!     fn start(&mut self, ctx: &mut Context<'_, Arrive>) -> Result<(), ComponentError> {
//!         ctx.schedule("10s".parse()?, Arrive)?;
//!         Ok(())
//!     }
//!     fn handle(&mut self, _: Delivery<'_, Arrive>, ctx: &mut Context<'_, Arrive>) -> Result<(), ComponentError> {
//!         let item = ctx.create_item();
//!         ctx.transfer(item, ctx.lookup("sink")?, Arrive)?;
//!         ctx.schedule("10s".parse()?, Arrive)?;
//!         Ok(())
//!     }
//! }
//!
//! /// Delivers every item it receives.
//! struct Sink;
//! impl Component<Arrive> for Sink {
//!     fn handle(&mut self, d: Delivery<'_, Arrive>, ctx: &mut Context<'_, Arrive>) -> Result<(), ComponentError> {
//!         if let Some(item) = d.item {
//!             ctx.receive(item)?;
//!             ctx.dispose(item, Disposal::Delivered)?;
//!         }
//!         Ok(())
//!     }
//! }
//!
//! let mut engine = Engine::new(EngineConfig::new(42));
//! engine.add_component("source", Box::new(Source))?;
//! engine.add_component("sink", Box::new(Sink))?;
//! engine.run(RunLimits::until("1h".parse()?), &mut NoTrace)?;
//! assert_eq!(engine.item_counts().delivered.get(), 360);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod engine;
mod error;
mod queue;
pub mod random;
mod trace;

pub use engine::{
    Component, ComponentId, Context, Delivery, Disposal, Engine, EngineConfig, EventId, ItemCounts,
    ItemId, Message, RunLimits, RunOutcome, RunSummary,
};
pub use error::{
    ComponentError, ConservationProblem, EngineError, RecentEvent, RunError, Violation,
};
pub use trace::{NoTrace, TraceRecord, TraceSink};

/// The engine version, part of every run's provenance (spec §7 Layer 3):
/// the same model, seed and engine version give the same trace, byte for
/// byte. Trace headers record it (ADR-0021).
///
/// ```
/// assert!(!taktsim_core::ENGINE_VERSION.is_empty());
/// ```
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
