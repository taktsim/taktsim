//! Trace storage, reading and replay (ADR-0004, ADR-0021).
//!
//! A trace is the record of every event of a run and every item created or
//! disposed of, in order. The engine emits the records
//! ([`TraceRecord`](taktsim_core::TraceRecord)); this crate stores them in
//! an append-only binary stream that never has to fit in memory:
//!
//! - [`TraceWriter`] is the engine's trace sink. It writes a [`Header`]
//!   (format version, engine version, message format, model hash, seed,
//!   component ids), then the records in chunks of at most
//!   [`CHUNK_CAPACITY`] bytes with a CRC32 each, then an end marker with the
//!   record counts and the SHA-256 of the record stream ([`TraceHash`]).
//! - [`TraceReader`] reads it back record by record and detects damaged,
//!   cut-off and foreign data with a typed [`ReadError`].
//! - [`replay`] re-executes a run and checks it against its trace at every
//!   event (spec §7 Layer 3).
//!
//! Every field has a fixed width and is little-endian, floats are written
//! as their bits, and nothing depends on the platform, so the same run
//! gives the same bytes everywhere. The engine never looks inside messages;
//! a message type becomes storable by implementing [`TraceMessage`].
//!
//! ```
//! use taktsim_core::{Component, ComponentError, Context, Delivery, Disposal, Engine, EngineConfig, Message, RunLimits};
//! use taktsim_trace::{replay, DecodeError, Decoder, Encoder, Header, ModelHash, TraceMessage, TraceReader, TraceWriter};
//!
//! #[derive(Debug)]
//! struct Arrive;
//! impl Message for Arrive {}
//! impl TraceMessage for Arrive {
//!     const FORMAT: &'static str = "example.arrive/1";
//!     fn encode(&self, _: &mut Encoder<'_>) {}
//!     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Arrive) }
//! }
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
//! fn line(seed: u64) -> Result<Engine<Arrive>, taktsim_core::EngineError> {
//!     let mut engine = Engine::new(EngineConfig::new(seed));
//!     engine.add_component("source", Box::new(Source))?;
//!     engine.add_component("sink", Box::new(Sink))?;
//!     Ok(engine)
//! }
//!
//! // Write: in practice to a `File`.
//! let mut engine = line(42)?;
//! let header = Header::for_engine(&engine, ModelHash::sha256(b"the model text"));
//! let mut writer = TraceWriter::new(Vec::new(), &header)?;
//! engine.run(RunLimits::until("1h".parse()?), &mut writer)?;
//! let (trace, summary) = writer.finish()?;
//! // Each item: created, an event delivering it, disposed of; plus the
//! // source's timer events.
//! assert_eq!(summary.records, 4 * 360);
//!
//! // Read.
//! let reader = TraceReader::<_, Arrive>::new(trace.as_slice())?;
//! let names = reader.header().components.clone();
//! let first = reader.into_iter().next().transpose()?.map(|r| r.display(&names).to_string());
//! assert_eq!(first.as_deref(), Some("10s event #0 source -> source Arrive"));
//!
//! // Replay.
//! let report = replay(TraceReader::new(trace.as_slice())?, &mut line(42)?)?;
//! assert_eq!(report.hash, summary.hash);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod codec;
#[cfg(feature = "golden")]
pub mod golden;
mod header;
mod read;
mod record;
mod replay;
mod write;

pub use codec::{
    DecodeError, Decoder, EncodeError, Encoder, TraceMessage, decode_message, encode_message,
};
pub use header::{CHUNK_CAPACITY, FORMAT_VERSION, Header, MAGIC, ModelHash, TraceHash};
pub use read::{Block, ReadError, TraceReader};
pub use record::Record;
pub use replay::{ReplayError, ReplayReport, replay};
pub use write::{Summary, TraceWriter, WriteError};
