//! Toy models for trace tests, built from test components: a source, a
//! station with optional breakdowns and scrap, and a sink. Reference models
//! with real components arrive in M5 and M7 (ADR-0021).

#![allow(dead_code)] // Each test file uses a different part.

use std::collections::VecDeque;

use taktsim_core::random::{DurationDistribution, Stream};
use taktsim_core::{
    Component, ComponentError, Context, Delivery, Disposal, Engine, EngineConfig, EngineError,
    EventId, ItemId, Message, Violation,
};
use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
use taktsim_units::SimDuration;

/// The toy models' messages, with an encoding that uses every kind of
/// field: tags, integers, a float and text.
#[derive(Debug, Clone, PartialEq)]
pub enum Toy {
    /// The source's timer.
    Next,
    /// Carries an item.
    Arrive,
    /// A station finished lot `lot`.
    Done { lot: u32 },
    /// A station breaks down (priority 0).
    Breakdown,
    /// A station is repaired.
    Repaired,
    /// A note to the sink, every tenth lot.
    Note { text: String, share: f64 },
}

impl Message for Toy {
    fn priority(&self) -> u8 {
        match self {
            Toy::Breakdown => 0,
            Toy::Next | Toy::Arrive | Toy::Done { .. } | Toy::Repaired | Toy::Note { .. } => 1,
        }
    }
}

impl TraceMessage for Toy {
    const FORMAT: &'static str = "taktsim-trace.toy/1";

    fn encode(&self, out: &mut Encoder<'_>) {
        match self {
            Toy::Next => out.u8(0),
            Toy::Arrive => out.u8(1),
            Toy::Done { lot } => {
                out.u8(2);
                out.u32(*lot);
            }
            Toy::Breakdown => out.u8(3),
            Toy::Repaired => out.u8(4),
            Toy::Note { text, share } => {
                out.u8(5);
                out.str(text);
                out.f64(*share);
            }
        }
    }

    fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(match input.u8()? {
            0 => Toy::Next,
            1 => Toy::Arrive,
            2 => Toy::Done { lot: input.u32()? },
            3 => Toy::Breakdown,
            4 => Toy::Repaired,
            5 => Toy::Note {
                text: input.str()?.to_owned(),
                share: input.f64()?,
            },
            tag => return Err(input.invalid("Toy tag", tag.into(), "0 to 5")),
        })
    }
}

#[allow(clippy::unwrap_used)] // test helper
pub fn d(text: &str) -> SimDuration {
    text.parse().unwrap()
}

#[allow(clippy::unwrap_used)] // test helper
fn exponential(mean: &str) -> DurationDistribution {
    DurationDistribution::exponential(d(mean)).unwrap()
}

/// Creates an item after exponential gaps and sends it to `to`.
pub struct Source {
    pub gap: DurationDistribution,
    pub to: String,
    pub stream: Option<Stream>,
}

impl Source {
    pub fn boxed(mean_gap: &str, to: &str) -> Box<dyn Component<Toy>> {
        Box::new(Self {
            gap: exponential(mean_gap),
            to: to.to_owned(),
            stream: None,
        })
    }

    fn next(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        let stream = self.stream.as_mut().ok_or(ComponentError::Unexpected {
            state: "not started",
            event: "Next".into(),
        })?;
        ctx.schedule(self.gap.sample(stream)?, Toy::Next)?;
        Ok(())
    }
}

impl Component<Toy> for Source {
    fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        self.stream = Some(ctx.stream("arrivals")?);
        self.next(ctx)
    }

    fn handle(
        &mut self,
        _: Delivery<'_, Toy>,
        ctx: &mut Context<'_, Toy>,
    ) -> Result<(), ComponentError> {
        let item = ctx.create_item();
        ctx.transfer(item, ctx.lookup(&self.to)?, Toy::Arrive)?;
        self.next(ctx)
    }
}

/// A single server with a FIFO queue, exponential service, optional
/// breakdowns (the current lot goes back to the front of the queue and its
/// timer is cancelled) and optional scrap of every n-th lot.
pub struct Station {
    pub service: DurationDistribution,
    pub next: String,
    pub breakdowns: Option<(DurationDistribution, DurationDistribution)>,
    pub scrap_every: Option<u32>,
    pub queue: VecDeque<ItemId>,
    pub busy: Option<(ItemId, EventId)>,
    pub down: bool,
    pub lots: u32,
    pub stream: Option<Stream>,
    pub failures: Option<Stream>,
}

impl Station {
    pub fn new(mean_service: &str, next: &str) -> Self {
        Self {
            service: exponential(mean_service),
            next: next.to_owned(),
            breakdowns: None,
            scrap_every: None,
            queue: VecDeque::new(),
            busy: None,
            down: false,
            lots: 0,
            stream: None,
            failures: None,
        }
    }

    pub fn with_breakdowns(mut self, mtbf: &str, mttr: &str) -> Self {
        self.breakdowns = Some((exponential(mtbf), exponential(mttr)));
        self
    }

    pub fn with_scrap_every(mut self, n: u32) -> Self {
        self.scrap_every = Some(n);
        self
    }

    fn not_started() -> ComponentError {
        ComponentError::Unexpected {
            state: "not started",
            event: "any".into(),
        }
    }

    fn try_start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        if self.busy.is_some() || self.down {
            return Ok(());
        }
        let Some(item) = self.queue.pop_front() else {
            return Ok(());
        };
        let stream = self.stream.as_mut().ok_or_else(Self::not_started)?;
        self.lots += 1;
        let timer = ctx.schedule(self.service.sample(stream)?, Toy::Done { lot: self.lots })?;
        self.busy = Some((item, timer));
        Ok(())
    }

    fn schedule_breakdown(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        if let Some((mtbf, _)) = &self.breakdowns {
            let stream = self.failures.as_mut().ok_or_else(Self::not_started)?;
            ctx.schedule(mtbf.sample(stream)?, Toy::Breakdown)?;
        }
        Ok(())
    }
}

impl Component<Toy> for Station {
    fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        self.stream = Some(ctx.stream("service")?);
        if self.breakdowns.is_some() {
            self.failures = Some(ctx.stream("failures")?);
        }
        self.schedule_breakdown(ctx)
    }

    fn handle(
        &mut self,
        d: Delivery<'_, Toy>,
        ctx: &mut Context<'_, Toy>,
    ) -> Result<(), ComponentError> {
        match d.message {
            Toy::Arrive => {
                let item = d.item.ok_or(ComponentError::Unexpected {
                    state: "any",
                    event: "Arrive without an item".into(),
                })?;
                ctx.receive(item)?;
                self.queue.push_back(item);
            }
            Toy::Done { lot } => {
                let (item, _) = self.busy.take().ok_or(ComponentError::Unexpected {
                    state: "idle",
                    event: "Done".into(),
                })?;
                if self.scrap_every.is_some_and(|n| lot % n == 0) {
                    ctx.dispose(item, Disposal::Scrapped)?;
                } else {
                    ctx.transfer(item, ctx.lookup(&self.next)?, Toy::Arrive)?;
                }
                if lot % 10 == 0 {
                    let note = Toy::Note {
                        text: format!("lot {lot} done"),
                        share: f64::from(*lot) / 3.0,
                    };
                    ctx.send(ctx.lookup(&self.next)?, note)?;
                }
            }
            Toy::Breakdown => {
                self.down = true;
                if let Some((item, timer)) = self.busy.take() {
                    ctx.cancel(timer)?;
                    self.queue.push_front(item);
                    self.lots -= 1;
                }
                let stream = self.failures.as_mut().ok_or_else(Self::not_started)?;
                let mttr = self
                    .breakdowns
                    .as_ref()
                    .map(|(_, r)| r)
                    .ok_or_else(Self::not_started)?;
                ctx.schedule(mttr.sample(stream)?, Toy::Repaired)?;
            }
            Toy::Repaired => {
                self.down = false;
                self.schedule_breakdown(ctx)?;
            }
            Toy::Next | Toy::Note { .. } => {}
        }
        self.try_start(ctx)
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        self.queue.iter().copied().for_each(&mut *visit);
        if let Some((item, _)) = self.busy {
            visit(item);
        }
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        if self.down && self.busy.is_some() {
            return Err(Violation {
                invariant: "a station that is down is not busy",
                detail: "down and busy".into(),
            });
        }
        Ok(())
    }
}

/// Delivers every item; ignores notes.
pub struct Sink;

impl Component<Toy> for Sink {
    fn handle(
        &mut self,
        d: Delivery<'_, Toy>,
        ctx: &mut Context<'_, Toy>,
    ) -> Result<(), ComponentError> {
        if let Some(item) = d.item {
            ctx.receive(item)?;
            ctx.dispose(item, Disposal::Delivered)?;
        }
        Ok(())
    }
}

/// Source → station → sink: an M/M/1 queue.
pub fn single_server(seed: u64) -> Result<Engine<Toy>, EngineError> {
    let mut engine = Engine::new(EngineConfig::new(seed));
    engine.add_component("source", Source::boxed("1min", "station"))?;
    engine.add_component("station", Box::new(Station::new("45s", "sink")))?;
    engine.add_component("sink", Box::new(Sink))?;
    Ok(engine)
}

/// Source → cut (breakdowns) → weld (scraps every 7th lot) → sink.
pub fn tandem(seed: u64) -> Result<Engine<Toy>, EngineError> {
    let mut engine = Engine::new(EngineConfig::new(seed));
    engine.add_component("source", Source::boxed("1min", "cut"))?;
    let cut = Station::new("40s", "weld").with_breakdowns("20min", "3min");
    engine.add_component("cut", Box::new(cut))?;
    let weld = Station::new("50s", "sink").with_scrap_every(7);
    engine.add_component("weld", Box::new(weld))?;
    engine.add_component("sink", Box::new(Sink))?;
    Ok(engine)
}

/// A line of `stations` stations between a source and a sink: per item,
/// about two events per station. Used for the 10 M event measurement.
pub fn line(seed: u64, stations: usize) -> Result<Engine<Toy>, EngineError> {
    let names: Vec<String> = (0..stations)
        .map(|i| format!("s{i}"))
        .chain(std::iter::once("sink".to_owned()))
        .collect();
    let mut engine = Engine::new(EngineConfig::new(seed));
    engine.add_component("source", Source::boxed("1min", &names[0]))?;
    for pair in names.windows(2) {
        engine.add_component(&pair[0], Box::new(Station::new("40s", &pair[1])))?;
    }
    engine.add_component("sink", Box::new(Sink))?;
    Ok(engine)
}
