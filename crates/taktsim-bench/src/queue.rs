//! Workloads for the event queue (ADR-0001, ADR-0018): synthetic models run
//! through the public engine API, so the queue is measured the way every
//! model uses it, with the queue itself staying crate-private in
//! `taktsim-core`.
//!
//! There are three families, each a way a real model loads the queue:
//!
//! - **Hold** ([`hold`]): the classic queue benchmark. A steady population
//!   of `N` pending events; every event that is handled schedules exactly
//!   one new event after a random increment. Run at several `N`, the
//!   difference in instructions per event is the cost of the queue's size.
//!   With a constant increment, all `N` events share every instant, which
//!   makes it a tie-heavy workload for timers.
//! - **Waves** ([`waves`]): a tick sends one message to each of `N` lanes at
//!   the same instant, in three priority classes, so the queue holds `N`
//!   events that differ only in priority and scheduling order, the case
//!   ADR-0001's `(time, priority, sequence)` key exists for.
//! - **Cancellation** ([`cancel_heavy`]): machines that race a cycle timer
//!   against a failure timer; whichever fires first cancels the other, like
//!   a failure interrupting a cycle. Most failure timers are cancelled long
//!   before they are due, so stale entries pile up in the queue.
//!
//! Every engine is built and started by its constructor, so the work a
//! benchmark measures is the event loop alone: [`run_events`] or
//! [`run_until`]. Checks are off (`--fast`, spec §9): the benchmark is about
//! the queue, and its components hold no items. Every run folds what
//! happened into a [`Tally`], so the compiler cannot drop any of it and a
//! test can check it.
//!
//! ```
//! use taktsim_bench::queue::{self, Increment};
//!
//! let mut engine = queue::hold(10, Increment::Constant("1s".parse()?))?;
//! let outcome = queue::run_events(&mut engine, 50)?;
//! // Ten events per second, so the 50th is the last of the fifth second.
//! assert_eq!(outcome.summary.now, "5s".parse()?);
//! assert_eq!(outcome.tally.hops, 50);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use taktsim_core::random::{DistributionError, DurationDistribution, Stream, StreamKey};
use taktsim_core::{
    Component, ComponentError, ComponentId, Context, Delivery, Engine, EngineConfig, EventId,
    Message, RunError, RunLimits, RunSummary, TraceRecord, TraceSink,
};
use taktsim_units::{SimDuration, SimTime};

/// The seed of every benchmark engine and stream of this module.
///
/// ```
/// assert_eq!(taktsim_bench::queue::SEED, 20_261_026);
/// ```
pub const SEED: u64 = 20_261_026;

/// Events per benchmark run: enough that the event loop, not the call
/// around it, dominates an instruction count, and few enough that one
/// Callgrind run takes well under a second at every size.
///
/// ```
/// assert_eq!(taktsim_bench::queue::EVENTS, 10_000);
/// ```
pub const EVENTS: u64 = 10_000;

/// The messages of all three workloads, with the same-instant order of a
/// real model: a failure comes before anything else that is due.
///
/// ```
/// use taktsim_bench::queue::Msg;
/// use taktsim_core::Message;
///
/// assert!(Msg::Failure.priority() < Msg::CycleDone.priority());
/// assert_eq!(Msg::Wave { lane: 4, class: 2 }.priority(), 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// A hold event ([`hold`]); priority 1.
    Hop,
    /// A machine finished its cycle ([`cancel_heavy`]); priority 1.
    CycleDone,
    /// A machine failed; priority 0, so before a cycle end at the same
    /// instant.
    Failure,
    /// A machine was repaired; priority 1.
    Repaired,
    /// The fan-out tick ([`waves`]); priority 1.
    Tick,
    /// A message to one lane of the fan-out; its priority is its class.
    Wave {
        /// The lane, in the order the tick sends them.
        lane: u32,
        /// The priority class, `0` to `2`.
        class: u8,
    },
}

impl Message for Msg {
    fn priority(&self) -> u8 {
        match self {
            Self::Failure => 0,
            Self::Hop | Self::CycleDone | Self::Repaired | Self::Tick => 1,
            Self::Wave { class, .. } => *class,
        }
    }
}

/// What a run delivered, by kind, and a checksum of the order: a
/// [`TraceSink`] that keeps counts instead of records.
///
/// The checksum folds each event's sequence number and time, in delivery
/// order, so it changes if the queue pops events in another order.
///
/// ```
/// use taktsim_bench::queue::Tally;
///
/// let mut tally = Tally::new();
/// let empty = tally.checksum;
/// tally.absorb(0, "1s".parse()?);
/// assert_ne!(tally.checksum, empty);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Events delivered.
    pub events: u64,
    /// [`Msg::Hop`] events.
    pub hops: u64,
    /// [`Msg::CycleDone`] events.
    pub cycles: u64,
    /// [`Msg::Failure`] events.
    pub failures: u64,
    /// [`Msg::Repaired`] events.
    pub repairs: u64,
    /// [`Msg::Tick`] events.
    pub ticks: u64,
    /// [`Msg::Wave`] events.
    pub waves: u64,
    /// A hash of the delivery order (FNV-1a over sequence number and time).
    pub checksum: u64,
}

/// The FNV-1a offset basis and prime (64 bit), the start and step of the
/// checksum.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

impl Tally {
    /// An empty tally: no events, the checksum of the empty order.
    ///
    /// ```
    /// use taktsim_bench::queue::Tally;
    ///
    /// assert_eq!(Tally::new().events, 0);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            events: 0,
            hops: 0,
            cycles: 0,
            failures: 0,
            repairs: 0,
            ticks: 0,
            waves: 0,
            checksum: FNV_OFFSET,
        }
    }

    /// Folds one delivered event into the checksum. The sink calls this for
    /// every event; a test calls it with the events it derived by hand.
    ///
    /// ```
    /// use taktsim_bench::queue::Tally;
    ///
    /// let mut a = Tally::new();
    /// let mut b = Tally::new();
    /// a.absorb(0, "1s".parse()?);
    /// a.absorb(1, "2s".parse()?);
    /// b.absorb(1, "2s".parse()?);
    /// b.absorb(0, "1s".parse()?);
    /// assert_ne!(a.checksum, b.checksum, "the order matters");
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn absorb(&mut self, seq: u64, time: SimTime) {
        self.checksum = (self.checksum ^ seq).wrapping_mul(FNV_PRIME);
        self.checksum = (self.checksum ^ time.as_nanos()).wrapping_mul(FNV_PRIME);
    }
}

impl Default for Tally {
    /// The same as [`Tally::new`].
    ///
    /// ```
    /// use taktsim_bench::queue::Tally;
    ///
    /// assert_eq!(Tally::default(), Tally::new());
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl TraceSink<Msg> for Tally {
    fn record(&mut self, record: &TraceRecord<'_, Msg>) {
        // `TraceRecord` is non-exhaustive: other records are not events.
        if let TraceRecord::Event {
            seq, time, message, ..
        } = record
        {
            self.events += 1;
            self.absorb(*seq, *time);
            let kind = match message {
                Msg::Hop => &mut self.hops,
                Msg::CycleDone => &mut self.cycles,
                Msg::Failure => &mut self.failures,
                Msg::Repaired => &mut self.repairs,
                Msg::Tick => &mut self.ticks,
                Msg::Wave { .. } => &mut self.waves,
            };
            *kind += 1;
        }
    }
}

/// What one call of [`run_events`] or [`run_until`] did.
///
/// ```
/// use taktsim_bench::queue::{self, Increment};
///
/// let mut engine = queue::hold(1, Increment::Constant("1s".parse()?))?;
/// let outcome = queue::run_events(&mut engine, 3)?;
/// assert_eq!((outcome.summary.events, outcome.tally.events), (3, 3));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// The engine's summary of the call.
    pub summary: RunSummary,
    /// What the call delivered.
    pub tally: Tally,
}

/// The farthest horizon, for runs bounded by an event count.
const FOREVER: SimTime = SimTime::from_nanos(u64::MAX);

/// Processes the next `events` events of `engine`, which a constructor of
/// this module built.
///
/// # Errors
///
/// The engine's [`RunError`]; never for these workloads (tested).
///
/// ```
/// use taktsim_bench::queue::{self, Increment};
///
/// let mut engine = queue::hold(2, Increment::Constant("1s".parse()?))?;
/// assert_eq!(queue::run_events(&mut engine, 4)?.summary.events, 4);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn run_events(engine: &mut Engine<Msg>, events: u64) -> Result<Outcome, RunError> {
    let mut tally = Tally::new();
    let summary = engine.run(RunLimits::until(FOREVER).max_events(events), &mut tally)?;
    Ok(Outcome { summary, tally })
}

/// Processes the events of `engine` up to and including `until`.
///
/// # Errors
///
/// The engine's [`RunError`]; never for these workloads (tested).
///
/// ```
/// use taktsim_bench::queue::{self, Increment};
///
/// let mut engine = queue::hold(2, Increment::Constant("1s".parse()?))?;
/// assert_eq!(queue::run_until(&mut engine, "3s".parse()?)?.summary.events, 6);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn run_until(engine: &mut Engine<Msg>, until: SimTime) -> Result<Outcome, RunError> {
    let mut tally = Tally::new();
    let summary = engine.run(RunLimits::until(until), &mut tally)?;
    Ok(Outcome { summary, tally })
}

/// Starts every component of `engine` without processing an event, so the
/// first scheduling happens outside the measured run.
fn started(mut engine: Engine<Msg>) -> Result<Engine<Msg>, RunError> {
    engine.run(RunLimits::until(FOREVER).max_events(0), &mut Tally::new())?;
    Ok(engine)
}

/// A fresh engine with the benchmark seed and checks off.
fn engine() -> Engine<Msg> {
    Engine::new(EngineConfig::new(SEED).without_checks())
}

/// A fresh stream for `purpose` of component `name`.
fn stream(name: &str, purpose: &str) -> Stream {
    Stream::new(SEED, StreamKey::new(name, purpose))
}

/// How the hold model draws the time to a component's next event.
///
/// ```
/// use taktsim_bench::queue::Increment;
///
/// let step = Increment::Uniform { max: "2s".parse()? };
/// assert_ne!(step, Increment::Constant("2s".parse()?));
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Increment {
    /// Always this long: every pending event shares each instant.
    Constant(SimDuration),
    /// A whole number of nanoseconds, uniform on `1..=max` (mean
    /// `(max + 1) / 2` ns), so events almost never share an instant.
    Uniform {
        /// The longest increment, at least 1 ns.
        max: SimDuration,
    },
}

/// The hold model's only component: every event it handles schedules the
/// next one.
struct Holder {
    population: u32,
    increment: Increment,
    stream: Stream,
}

impl Holder {
    fn next(&mut self) -> SimDuration {
        match self.increment {
            Increment::Constant(d) => d,
            Increment::Uniform { max } => {
                // `max(1)` also keeps the modulus nonzero for a `max` of 0.
                let span = max.as_nanos().max(1);
                SimDuration::from_nanos(1 + self.stream.next_u64() % span)
            }
        }
    }
}

impl Component<Msg> for Holder {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        for _ in 0..self.population {
            ctx.schedule(self.next(), Msg::Hop)?;
        }
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        ctx.schedule(self.next(), Msg::Hop)?;
        Ok(())
    }
}

/// The classic hold model, started: `population` pending events, each
/// handled one scheduling its successor after `increment`. The population
/// stays exactly `population` until the end of the run.
///
/// # Errors
///
/// An engine error from building or starting the model; never for a
/// population of at least 1 (tested).
///
/// ```
/// use taktsim_bench::queue::{self, Increment};
///
/// let mut engine = queue::hold(100, Increment::Uniform { max: "2s".parse()? })?;
/// let outcome = queue::run_events(&mut engine, queue::EVENTS)?;
/// assert_eq!(outcome.summary.events, queue::EVENTS);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn hold(
    population: u32,
    increment: Increment,
) -> Result<Engine<Msg>, Box<dyn std::error::Error>> {
    let mut engine = engine();
    engine.add_component(
        "holder",
        Box::new(Holder {
            population,
            increment,
            stream: stream("holder", "increments"),
        }),
    )?;
    Ok(started(engine)?)
}

/// The populations the benchmarks run the hold model at, with random
/// increments ([`hold_random`]). From 1, where the queue is trivial and the
/// cost is the event loop itself, to 10 000, far above the one or two
/// pending events per component of the reference models (ADR-0001).
///
/// ```
/// assert_eq!(taktsim_bench::queue::HOLD_POPULATIONS.len(), 5);
/// ```
pub const HOLD_POPULATIONS: [u32; 5] = [1, 10, 100, 1_000, 10_000];

/// The populations of the tie-heavy hold model ([`hold_tied`]).
///
/// ```
/// assert_eq!(taktsim_bench::queue::TIED_POPULATIONS, [100, 10_000]);
/// ```
pub const TIED_POPULATIONS: [u32; 2] = [100, 10_000];

/// The longest random increment of [`hold_random`]: 2 s, so the mean
/// increment is 1 s (plus half a nanosecond).
///
/// ```
/// assert_eq!(taktsim_bench::queue::HOLD_MAX.as_nanos(), 2_000_000_000);
/// ```
pub const HOLD_MAX: SimDuration = SimDuration::from_nanos(2_000_000_000);

/// The hold model with increments uniform on 1 ns to [`HOLD_MAX`].
///
/// # Errors
///
/// As [`hold`].
///
/// ```
/// use taktsim_bench::queue;
///
/// let mut engine = queue::hold_random(10)?;
/// assert_eq!(queue::run_events(&mut engine, 5)?.tally.hops, 5);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn hold_random(population: u32) -> Result<Engine<Msg>, Box<dyn std::error::Error>> {
    hold(population, Increment::Uniform { max: HOLD_MAX })
}

/// The hold model with a constant increment of 1 s, so all `population`
/// events share every instant and pop in scheduling order.
///
/// # Errors
///
/// As [`hold`].
///
/// ```
/// use taktsim_bench::queue;
///
/// let mut engine = queue::hold_tied(4)?;
/// assert_eq!(queue::run_events(&mut engine, 8)?.summary.now, "2s".parse()?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn hold_tied(population: u32) -> Result<Engine<Msg>, Box<dyn std::error::Error>> {
    hold(population, Increment::Constant(WAVE_INTERVAL))
}

/// Sends a wave to each lane, once per tick.
struct Fanout {
    lanes: u32,
    interval: SimDuration,
    sink: ComponentId,
}

impl Component<Msg> for Fanout {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        ctx.schedule(self.interval, Msg::Tick)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        for lane in 0..self.lanes {
            let class = match lane % 3 {
                0 => 0,
                1 => 1,
                _ => 2,
            };
            ctx.send(self.sink, Msg::Wave { lane, class })?;
        }
        ctx.schedule(self.interval, Msg::Tick)?;
        Ok(())
    }
}

/// Receives the waves and does nothing with them.
struct WaveSink;

impl Component<Msg> for WaveSink {
    fn handle(
        &mut self,
        _: Delivery<'_, Msg>,
        _: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

/// The interval between ticks of [`waves`].
///
/// ```
/// assert_eq!(taktsim_bench::queue::WAVE_INTERVAL.as_nanos(), 1_000_000_000);
/// ```
pub const WAVE_INTERVAL: SimDuration = SimDuration::from_nanos(1_000_000_000);

/// The tie-heavy model, started: every [`WAVE_INTERVAL`] a tick sends one
/// [`Msg::Wave`] to each of `lanes` lanes, lane `i` with class `i % 3`. All
/// the waves of a tick are due at the tick's instant, so they pop by class,
/// then by lane. A round is `lanes + 1` events (see [`wave_events`]).
///
/// # Errors
///
/// An engine error from building or starting the model; never (tested).
///
/// ```
/// use taktsim_bench::queue;
///
/// let mut engine = queue::waves(5)?;
/// let outcome = queue::run_events(&mut engine, queue::wave_events(5, 2))?;
/// assert_eq!((outcome.tally.ticks, outcome.tally.waves), (2, 10));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn waves(lanes: u32) -> Result<Engine<Msg>, Box<dyn std::error::Error>> {
    let mut engine = engine();
    let sink = engine.add_component("sink", Box::new(WaveSink))?;
    engine.add_component(
        "fanout",
        Box::new(Fanout {
            lanes,
            interval: WAVE_INTERVAL,
            sink,
        }),
    )?;
    Ok(started(engine)?)
}

/// The benchmarked sizes of [`waves`] as (lanes, rounds), chosen so a run
/// is about [`EVENTS`] events: 9 999, 10 010 and 10 001.
///
/// ```
/// use taktsim_bench::queue::{WAVE_CASES, wave_events};
///
/// let (lanes, rounds) = WAVE_CASES[0];
/// assert_eq!(wave_events(lanes, rounds), 9_999);
/// ```
pub const WAVE_CASES: [(u32, u32); 3] = [(100, 99), (1_000, 10), (10_000, 1)];

/// The events of `rounds` rounds of [`waves`] with `lanes` lanes: each
/// round is one tick and one wave per lane.
///
/// ```
/// assert_eq!(taktsim_bench::queue::wave_events(100, 99), 9_999);
/// ```
#[must_use]
pub fn wave_events(lanes: u32, rounds: u32) -> u64 {
    u64::from(rounds) * (u64::from(lanes) + 1)
}

/// The time distributions of the machines of [`cancel_heavy`].
///
/// ```
/// use taktsim_bench::queue::MachineParams;
///
/// let params = MachineParams::line()?;
/// assert_eq!(params.cycle.to_string(), "{dist: exponential, mean: 1min}");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct MachineParams {
    /// The length of a cycle.
    pub cycle: DurationDistribution,
    /// The time from the start of a cycle to the next failure, if the cycle
    /// were not finished first.
    pub time_to_failure: DurationDistribution,
    /// The length of a repair.
    pub repair: DurationDistribution,
}

impl MachineParams {
    /// Machines like a station of a line: cycles of 1 min on average, a
    /// failure about every 10 min of running, repairs of 2 min on average,
    /// all exponential. A failure is the first of the two clocks with
    /// probability 1/11 (rates 1/10 and 1 per minute: (1/10) / (1/10 + 1));
    /// otherwise the failure timer is cancelled while it is still about ten
    /// cycles away.
    ///
    /// # Errors
    ///
    /// The constructor's [`DistributionError`]; never for these parameters
    /// (tested).
    ///
    /// ```
    /// use taktsim_bench::queue::MachineParams;
    ///
    /// let params = MachineParams::line()?;
    /// assert_eq!(params.repair.to_string(), "{dist: exponential, mean: 2min}");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn line() -> Result<Self, DistributionError> {
        let minutes = |m: u64| SimDuration::from_nanos(m * 60 * 1_000_000_000);
        Ok(Self {
            cycle: DurationDistribution::exponential(minutes(1))?,
            time_to_failure: DurationDistribution::exponential(minutes(10))?,
            repair: DurationDistribution::exponential(minutes(2))?,
        })
    }
}

/// What a machine is doing, and the timers it has pending.
#[derive(Debug, Clone, Copy)]
enum MachineState {
    /// Racing a cycle against a failure; whichever fires first cancels the
    /// other.
    Running { cycle: EventId, failure: EventId },
    /// Waiting for the repair timer.
    Repairing,
}

/// A machine: a cycle and a failure timer race; the loser is cancelled.
struct Machine {
    params: MachineParams,
    state: MachineState,
    cycles: Stream,
    failures: Stream,
    repairs: Stream,
}

impl Machine {
    fn new(name: &str, params: MachineParams) -> Self {
        Self {
            params,
            state: MachineState::Repairing,
            cycles: stream(name, "cycle"),
            failures: stream(name, "failure"),
            repairs: stream(name, "repair"),
        }
    }

    /// Starts a cycle and the failure clock that runs against it.
    fn race(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        let cycle = ctx.schedule(self.params.cycle.sample(&mut self.cycles)?, Msg::CycleDone)?;
        let failure = ctx.schedule(
            self.params.time_to_failure.sample(&mut self.failures)?,
            Msg::Failure,
        )?;
        self.state = MachineState::Running { cycle, failure };
        Ok(())
    }
}

fn unexpected(state: &'static str, message: Msg) -> ComponentError {
    ComponentError::Unexpected {
        state,
        event: format!("{message:?}"),
    }
}

impl Component<Msg> for Machine {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        self.race(ctx)
    }

    fn handle(
        &mut self,
        delivery: Delivery<'_, Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        match (self.state, *delivery.message) {
            (MachineState::Running { failure, .. }, Msg::CycleDone) => {
                ctx.cancel(failure)?;
                self.race(ctx)
            }
            (MachineState::Running { cycle, .. }, Msg::Failure) => {
                ctx.cancel(cycle)?;
                ctx.schedule(self.params.repair.sample(&mut self.repairs)?, Msg::Repaired)?;
                self.state = MachineState::Repairing;
                Ok(())
            }
            (MachineState::Repairing, Msg::Repaired) => self.race(ctx),
            (
                MachineState::Running { .. },
                message @ (Msg::Repaired | Msg::Hop | Msg::Tick | Msg::Wave { .. }),
            ) => Err(unexpected("running", message)),
            (
                MachineState::Repairing,
                message @ (Msg::CycleDone | Msg::Failure | Msg::Hop | Msg::Tick | Msg::Wave { .. }),
            ) => Err(unexpected("repairing", message)),
        }
    }
}

/// The cancellation-heavy model, started: `machines` machines with the
/// given times. Every cycle end or failure cancels the timer it raced
/// against, so after each such event one cancellation has happened; a
/// repair end cancels nothing.
///
/// # Errors
///
/// An engine error from building or starting the model; never (tested).
///
/// ```
/// use taktsim_bench::queue::{self, MachineParams};
///
/// let mut engine = queue::cancel_heavy(20, &MachineParams::line()?)?;
/// let outcome = queue::run_events(&mut engine, queue::EVENTS)?;
/// let tally = outcome.tally;
/// assert_eq!(tally.events, tally.cycles + tally.failures + tally.repairs);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn cancel_heavy(
    machines: u32,
    params: &MachineParams,
) -> Result<Engine<Msg>, Box<dyn std::error::Error>> {
    let mut engine = engine();
    for index in 0..machines {
        let name = format!("machine-{index}");
        engine.add_component(&name, Box::new(Machine::new(&name, params.clone())))?;
    }
    Ok(started(engine)?)
}

/// The machine counts the benchmarks run [`cancel_line`] at: a line of the
/// size of the reference model (ADR-0001) and a synthetic plant.
///
/// ```
/// assert_eq!(taktsim_bench::queue::MACHINE_COUNTS, [20, 1_000]);
/// ```
pub const MACHINE_COUNTS: [u32; 2] = [20, 1_000];

/// [`cancel_heavy`] with [`MachineParams::line`].
///
/// # Errors
///
/// As [`cancel_heavy`].
///
/// ```
/// use taktsim_bench::queue;
///
/// let mut engine = queue::cancel_line(3)?;
/// assert_eq!(queue::run_events(&mut engine, 10)?.summary.events, 10);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn cancel_line(machines: u32) -> Result<Engine<Msg>, Box<dyn std::error::Error>> {
    cancel_heavy(machines, &MachineParams::line()?)
}
