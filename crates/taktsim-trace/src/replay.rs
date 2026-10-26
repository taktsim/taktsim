//! Replay: re-execute a run and compare it with its trace at every event
//! (spec §7 Layer 3, ADR-0021).

use std::io::Read;

use taktsim_core::{
    ENGINE_VERSION, Engine, ItemCounts, RunError, RunLimits, TraceRecord, TraceSink,
};
use taktsim_units::SimTime;

use crate::codec::{EncodeError, TraceMessage};
use crate::header::{TraceHash, component_names};
use crate::read::{ReadError, TraceReader};
use crate::record::{self, EVENT};

/// What a successful replay compared.
///
/// ```
/// use taktsim_trace::{ReplayReport, TraceHash};
///
/// let r = ReplayReport { records: 3, events: 2, hash: TraceHash::default() };
/// assert_eq!(r.events, 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayReport {
    /// Records compared.
    pub records: u64,
    /// Of these, events.
    pub events: u64,
    /// The trace's stream hash, checked against its records.
    pub hash: TraceHash,
}

/// Re-executes the run recorded in `trace` with `engine`, and checks that it
/// is the same run at every event.
///
/// `engine` must be built from the same model as the trace, with the
/// trace's seed, and not have run yet. Replay checks the header first (same
/// engine version, seed and components), then runs the engine one event at
/// a time, up to the time of the trace's next event, and compares:
///
/// - every record the engine emits with the trace's next record, byte for
///   byte: the event's time, sequence number, target, sender, message and
///   item, and every item created or disposed of;
/// - after starting and after every event, the engine's clock and item
///   counts with those the trace implies.
///
/// The first difference stops the replay with [`ReplayError::Diverged`] or
/// [`ReplayError::State`], naming the record. The components' own state is
/// checked by their invariants, which the engine runs after every event when
/// checks are on.
///
/// # Errors
///
/// A [`ReplayError`]: the trace cannot be read, its header does not match
/// the engine, the run fails, or it diverges from the trace.
///
/// ```
/// use taktsim_core::{Component, ComponentError, Context, Delivery, Engine, EngineConfig, Message, RunLimits};
/// use taktsim_trace::{replay, DecodeError, Decoder, Encoder, Header, ModelHash, ReplayError, TraceMessage, TraceReader, TraceWriter};
///
/// #[derive(Debug)]
/// struct Tick(u64);
/// impl Message for Tick {}
/// impl TraceMessage for Tick {
///     const FORMAT: &'static str = "tick/1";
///     fn encode(&self, out: &mut Encoder<'_>) { out.u64(self.0) }
///     fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick(input.u64()?)) }
/// }
///
/// /// Ticks after a random delay, numbering its ticks.
/// struct Clock(Option<taktsim_core::random::Stream>, u64);
/// impl Component<Tick> for Clock {
///     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
///         self.0 = Some(ctx.stream("delay")?);
///         ctx.schedule("1s".parse()?, Tick(0))?;
///         Ok(())
///     }
///     fn handle(&mut self, _: Delivery<'_, Tick>, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
///         let ms = self.0.as_mut().map_or(1, |s| 1 + s.next_u64() % 1000);
///         self.1 += 1;
///         ctx.schedule(taktsim_units::SimDuration::from_nanos(ms * 1_000_000), Tick(self.1))?;
///         Ok(())
///     }
/// }
/// fn build(seed: u64) -> Result<Engine<Tick>, taktsim_core::EngineError> {
///     let mut engine = Engine::new(EngineConfig::new(seed));
///     engine.add_component("clock", Box::new(Clock(None, 0)))?;
///     Ok(engine)
/// }
///
/// let mut engine = build(7)?;
/// let mut writer = TraceWriter::new(Vec::new(), &Header::for_engine(&engine, ModelHash::default()))?;
/// engine.run(RunLimits::until("1min".parse()?), &mut writer)?;
/// let (trace, _) = writer.finish()?;
///
/// let report = replay(TraceReader::new(trace.as_slice())?, &mut build(7)?)?;
/// assert!(report.events > 0);
/// // Another seed is another run.
/// let other = replay(TraceReader::new(trace.as_slice())?, &mut build(8)?);
/// assert!(matches!(other, Err(ReplayError::Seed { trace: 7, engine: 8 })));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn replay<R: Read, M: TraceMessage>(
    trace: TraceReader<R, M>,
    engine: &mut Engine<M>,
) -> Result<ReplayReport, ReplayError> {
    let header = trace.header();
    if header.engine_version != ENGINE_VERSION {
        return Err(ReplayError::EngineVersion {
            trace: header.engine_version.clone(),
            engine: ENGINE_VERSION,
        });
    }
    let seed = engine.config().seed;
    if header.seed != seed {
        return Err(ReplayError::Seed {
            trace: header.seed,
            engine: seed,
        });
    }
    let names = component_names(engine);
    if names != header.components {
        let index = names
            .iter()
            .zip(&header.components)
            .take_while(|(a, b)| a == b)
            .count();
        return Err(ReplayError::Components {
            index,
            trace: header.components.get(index).cloned(),
            engine: names.get(index).cloned(),
        });
    }
    let mut sink = Replayer {
        trace,
        scratch: Vec::new(),
        problem: None,
        counts: Counts::default(),
    };
    // Start the components without delivering any event.
    let start = engine.now();
    run(engine, RunLimits::until(start).max_events(0), &mut sink)?;
    sink.check_state(engine, 0, start)?;
    while let Some(next) = sink.trace.peek_raw()? {
        let bytes = sink.trace.bytes(&next);
        if next.kind != EVENT {
            return Err(ReplayError::Diverged {
                record: next.index,
                expected: sink.render_stored(bytes),
                found: "no further record before the engine's next event".to_owned(),
            });
        }
        let time = record::event_time(bytes);
        let summary = run(engine, RunLimits::until(time).max_events(1), &mut sink)?;
        if summary.events == 0 {
            let bytes = sink.trace.bytes(&next);
            return Err(ReplayError::Diverged {
                record: next.index,
                expected: sink.render_stored(bytes),
                found: format!("no event due by {time}"),
            });
        }
        sink.check_state(engine, next.index, time)?;
    }
    let summary = sink.trace.summary();
    let summary = summary.ok_or(ReplayError::Trace(ReadError::AlreadyFailed))?;
    Ok(ReplayReport {
        records: summary.records,
        events: summary.events,
        hash: summary.hash,
    })
}

/// Runs `engine` with the replaying sink, reporting a divergence before a
/// run error: an engine that diverged may fail later for that reason.
fn run<R: Read, M: TraceMessage>(
    engine: &mut Engine<M>,
    limits: RunLimits<'_>,
    sink: &mut Replayer<R, M>,
) -> Result<taktsim_core::RunSummary, ReplayError> {
    let result = engine.run(limits, sink);
    if let Some(problem) = sink.problem.take() {
        return Err(problem);
    }
    result.map_err(|e| ReplayError::Run(Box::new(e)))
}

/// Item counts implied by the trace.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    created: u64,
    delivered: u64,
    scrapped: u64,
    consumed: u64,
}

/// Compares what the engine emits with the trace.
struct Replayer<R: Read, M> {
    trace: TraceReader<R, M>,
    scratch: Vec<u8>,
    problem: Option<ReplayError>,
    counts: Counts,
}

impl<R: Read, M: TraceMessage> Replayer<R, M> {
    fn compare(&mut self, emitted: &TraceRecord<'_, M>) -> Result<(), ReplayError> {
        self.scratch.clear();
        let next = self.trace.peek_raw()?;
        let index = next
            .as_ref()
            .map_or_else(|| self.trace_records(), |raw| raw.index);
        record::encode(emitted, &mut self.scratch).map_err(|source| ReplayError::Encode {
            record: index,
            source,
        })?;
        let Some(next) = next else {
            return Err(ReplayError::Diverged {
                record: index,
                expected: "the end of the trace".to_owned(),
                found: self.render(emitted),
            });
        };
        if self.trace.bytes(&next) != self.scratch.as_slice() {
            return Err(ReplayError::Diverged {
                record: index,
                expected: self.render_stored(self.trace.bytes(&next)),
                found: self.render(emitted),
            });
        }
        self.trace.consume();
        match emitted {
            TraceRecord::Event { .. } => {}
            TraceRecord::ItemCreated { .. } => self.counts.created += 1,
            TraceRecord::ItemDisposed { how, .. } => match how {
                taktsim_core::Disposal::Delivered => self.counts.delivered += 1,
                taktsim_core::Disposal::Scrapped => self.counts.scrapped += 1,
                taktsim_core::Disposal::Consumed => self.counts.consumed += 1,
            },
        }
        Ok(())
    }

    /// Records read from the trace so far (the position after its end).
    fn trace_records(&self) -> u64 {
        self.trace.summary().map_or(0, |s| s.records)
    }

    fn render(&self, record: &TraceRecord<'_, M>) -> String {
        record::line(record, &self.trace.header().components)
    }

    fn render_stored(&self, bytes: &[u8]) -> String {
        match self.trace.decode(bytes) {
            Ok(stored) => record::line(&stored.as_trace(), &self.trace.header().components),
            Err(e) => format!("an undecodable record ({e})"),
        }
    }

    /// Compares the engine's clock and item counts with the trace's.
    fn check_state(
        &self,
        engine: &Engine<M>,
        record: u64,
        time: SimTime,
    ) -> Result<(), ReplayError> {
        let c = self.counts;
        let disposed = c.delivered + c.scrapped + c.consumed;
        let e: ItemCounts = engine.item_counts();
        let live = e.held.get() + e.in_transit.get();
        let checks = [
            ("items created", c.created, e.created.get()),
            ("items delivered", c.delivered, e.delivered.get()),
            ("items scrapped", c.scrapped, e.scrapped.get()),
            ("items consumed", c.consumed, e.consumed.get()),
            (
                "items held or in transit",
                c.created.saturating_sub(disposed),
                live,
            ),
        ];
        if engine.now() != time {
            return Err(ReplayError::State {
                record,
                what: "time",
                trace: time.to_string(),
                engine: engine.now().to_string(),
            });
        }
        match checks
            .into_iter()
            .find(|(_, trace, engine)| trace != engine)
        {
            Some((what, trace, engine)) => Err(ReplayError::State {
                record,
                what,
                trace: trace.to_string(),
                engine: engine.to_string(),
            }),
            None => Ok(()),
        }
    }
}

impl<R: Read, M: TraceMessage> TraceSink<M> for Replayer<R, M> {
    fn record(&mut self, emitted: &TraceRecord<'_, M>) {
        if self.problem.is_none() {
            self.problem = self.compare(emitted).err();
        }
    }
}

/// Why a replay failed.
///
/// ```
/// use taktsim_trace::ReplayError;
///
/// let e = ReplayError::Diverged { record: 12, expected: "a".into(), found: "b".into() };
/// assert_eq!(e.to_string(), "the replay diverges from the trace at record 12: the trace has a, the engine b");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// The trace cannot be read.
    #[error(transparent)]
    Trace(#[from] ReadError),
    /// The trace was written by another engine version, whose runs may
    /// differ.
    #[error(
        "the trace was written by engine version {trace}, this is {engine}; \
         replay it with engine version {trace}"
    )]
    EngineVersion {
        /// The version in the trace.
        trace: String,
        /// This engine's version.
        engine: &'static str,
    },
    /// The engine has another seed than the trace.
    #[error(
        "the trace was run with seed {trace}, the engine has seed {engine}; build it with seed {trace}"
    )]
    Seed {
        /// The trace's seed.
        trace: u64,
        /// The engine's seed.
        engine: u64,
    },
    /// The engine's components differ from the trace's.
    #[error(
        "component #{index} is {} in the trace and {} in the engine; build the engine from the trace's model",
        name(.trace.as_deref()),
        name(.engine.as_deref())
    )]
    Components {
        /// The first position that differs.
        index: usize,
        /// The trace's component there, if any.
        trace: Option<String>,
        /// The engine's component there, if any.
        engine: Option<String>,
    },
    /// The replayed run failed.
    #[error("the replayed run failed: {0}")]
    Run(#[source] Box<RunError>),
    /// The engine emitted a different record than the trace has.
    #[error(
        "the replay diverges from the trace at record {record}: the trace has {expected}, the engine {found}"
    )]
    Diverged {
        /// The record's position in the trace, from 0.
        record: u64,
        /// The trace's record.
        expected: String,
        /// What the engine did.
        found: String,
    },
    /// The engine's clock or item counts differ from the trace's after an
    /// event.
    #[error("after record {record}, {what} is {trace} in the trace and {engine} in the engine")]
    State {
        /// The event's position in the trace, from 0.
        record: u64,
        /// What differs.
        what: &'static str,
        /// The value the trace implies.
        trace: String,
        /// The engine's value.
        engine: String,
    },
    /// The engine emitted a record that does not fit the format.
    #[error("record {record} of the replay does not fit the format: {source}")]
    Encode {
        /// The record's position.
        record: u64,
        /// What does not fit.
        #[source]
        source: EncodeError,
    },
}

fn name(name: Option<&str>) -> String {
    name.map_or_else(|| "missing".to_owned(), |n| format!("`{n}`"))
}
