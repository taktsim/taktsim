//! The reference line: spec §4's example, built through the registry, as a
//! workload (ADR-0016, ADR-0022). It measures what a model costs per event,
//! with every component, the protocol between them and the checks.

use std::error::Error;
use std::io;

use taktsim_components::{FlowMessage, standard};
use taktsim_core::{Engine, EngineConfig, NoTrace, RunError, RunLimits, RunSummary};
use taktsim_model::{Format, Loader, Model};
use taktsim_trace::{Header, ModelHash, SystemHash, TraceWriter};
use taktsim_units::SimTime;

/// Events one benchmark processes.
///
/// ```
/// assert_eq!(taktsim_bench::line::EVENTS, 10_000);
/// ```
pub const EVENTS: u64 = 10_000;

/// The seed of every run of the workload.
const SEED: u64 = 42;

/// Far past the events asked for, so the event limit is what stops a run.
const HORIZON: SimTime = SimTime::from_nanos(u64::MAX);

/// Spec §4's line, with its variants: exponential arrivals in front of a
/// failing machine, a buffer and a machine with setups.
const MODEL: &str = r#"
format: "1.0"
model: {name: reference-line, seed: 42}
experiment: {type: terminating, horizon: 8h}
components:
  - id: source
    type: Source
    interarrival: { dist: exponential, mean: 40s }
    variants: { basic: 80%, premium: 20% }
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.08 }
    mtbf: { dist: exponential, mean: 6h }
    mttr: { dist: exponential, mean: 12min }
  - id: buf1
    type: Buffer
    capacity: 20 parts
  - id: weld
    type: Machine
    cycle_time: 45s
    setup: { per_variant: 4min }
  - id: sink
    type: Sink
flow:
  - [source, cut, buf1, weld, sink]
"#;

/// The reference model, loaded with the standard catalog.
///
/// # Errors
///
/// The loader's errors; the model is a constant, so a test rules them out.
///
/// ```
/// let model = taktsim_bench::line::model()?;
/// assert_eq!(model.document().components().len(), 5);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn model() -> Result<Model, Box<dyn Error>> {
    let registry = standard()?;
    Ok(Loader::new(registry.catalog()).load_str(MODEL, Format::Yaml)?)
}

/// An engine for the reference line, built through the registry, its components
/// started, with checks on or off.
///
/// # Errors
///
/// The errors of building or starting it; a test rules them out.
///
/// ```
/// let engine = taktsim_bench::line::engine(true)?;
/// assert_eq!(engine.config().seed, 42);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn engine(checks: bool) -> Result<Engine<FlowMessage>, Box<dyn Error>> {
    let registry = standard()?;
    let model = model()?;
    let config = EngineConfig::new(SEED);
    let config = if checks {
        config
    } else {
        config.without_checks()
    };
    let mut engine = registry.build(model.document(), config)?;
    engine.run(RunLimits::until(SimTime::START).max_events(0), &mut NoTrace)?;
    Ok(engine)
}

/// Processes `events` events with no trace.
///
/// # Errors
///
/// A [`RunError`] if the run fails.
///
/// ```
/// let mut engine = taktsim_bench::line::engine(true)?;
/// assert_eq!(taktsim_bench::line::run_events(&mut engine, 100)?.events, 100);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn run_events(engine: &mut Engine<FlowMessage>, events: u64) -> Result<RunSummary, RunError> {
    engine.run(RunLimits::until(HORIZON).max_events(events), &mut NoTrace)
}

/// Processes `events` events into a trace that goes nowhere, and returns its
/// records. An engine not yet started starts here.
///
/// # Errors
///
/// The errors of the run or the trace.
///
/// ```
/// let mut engine = taktsim_bench::line::engine(true)?;
/// assert!(taktsim_bench::line::run_traced(&mut engine, 100)? >= 100);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn run_traced(engine: &mut Engine<FlowMessage>, events: u64) -> Result<u64, Box<dyn Error>> {
    let header = Header::for_engine(engine, ModelHash::default(), SystemHash::default());
    let mut writer = TraceWriter::new(io::sink(), &header)?;
    let ran = engine.run(RunLimits::until(HORIZON).max_events(events), &mut writer)?;
    writer.run_ended(&ran);
    Ok(writer.finish()?.1.records)
}
