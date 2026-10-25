//! The event loop: ordering, run limits, items and conservation, error
//! reports, determinism.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use taktsim_core::random::{DurationDistribution, Stream};
use taktsim_core::{
    Component, ComponentError, ComponentId, Context, Delivery, Disposal, Engine, EngineConfig,
    EngineError, EventId, ItemId, Message, NoTrace, RecentEvent, RunError, RunLimits, RunOutcome,
    TraceRecord, TraceSink, Violation,
};
use taktsim_units::{SimDuration, SimTime};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Msg {
    /// A timer, priority 1.
    Tick(u32),
    /// An urgent message, priority 0.
    Urgent(u32),
    /// Carries an item, priority 1.
    Arrive,
    /// Asks the receiver to cancel a timer of someone else.
    CancelThis(EventId),
}

impl Message for Msg {
    fn priority(&self) -> u8 {
        match self {
            Msg::Urgent(_) => 0,
            Msg::Tick(_) | Msg::Arrive | Msg::CancelThis(_) => 1,
        }
    }
}

#[allow(clippy::unwrap_used)] // test helper
fn d(text: &str) -> SimDuration {
    text.parse().unwrap()
}

#[allow(clippy::unwrap_used)] // test helper
fn t(text: &str) -> SimTime {
    text.parse().unwrap()
}

/// Records every trace record as text.
#[derive(Default)]
struct Recorder(Vec<String>);

impl TraceSink<Msg> for Recorder {
    fn record(&mut self, record: &TraceRecord<'_, Msg>) {
        self.0.push(format!("{record:?}"));
    }
}

/// Runs a closure on every event; the closure decides what to do.
type Handler = Box<dyn FnMut(&Delivery<Msg>, &mut Context<'_, Msg>) -> Result<(), ComponentError>>;
type Starter = Box<dyn FnMut(&mut Context<'_, Msg>) -> Result<(), ComponentError>>;

struct Scripted {
    start: Starter,
    handle: Handler,
}

impl Component<Msg> for Scripted {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        (self.start)(ctx)
    }

    fn handle(
        &mut self,
        d: Delivery<Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        (self.handle)(&d, ctx)
    }
}

fn scripted(
    start: impl FnMut(&mut Context<'_, Msg>) -> Result<(), ComponentError> + 'static,
    handle: impl FnMut(&Delivery<Msg>, &mut Context<'_, Msg>) -> Result<(), ComponentError> + 'static,
) -> Box<dyn Component<Msg>> {
    Box::new(Scripted {
        start: Box::new(start),
        handle: Box::new(handle),
    })
}

fn idle() -> Box<dyn Component<Msg>> {
    scripted(|_| Ok(()), |_, _| Ok(()))
}

/// Creates an item every `every` and sends it to `to`; holds none.
struct Source {
    every: SimDuration,
    to: &'static str,
}

impl Component<Msg> for Source {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        ctx.schedule(self.every, Msg::Tick(0))?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        let item = ctx.create_item();
        ctx.transfer(item, ctx.lookup(self.to)?, Msg::Arrive)?;
        ctx.schedule(self.every, Msg::Tick(0))?;
        Ok(())
    }
}

/// Holds every arriving item for `hold`, then delivers it. Reports what it
/// holds honestly unless told to lie.
struct Delay {
    hold: SimDuration,
    queue: Vec<ItemId>,
    lie: Option<Lie>,
}

#[derive(Clone, Copy)]
enum Lie {
    Forget,
    Twice,
    Foreign,
}

impl Component<Msg> for Delay {
    fn handle(
        &mut self,
        d: Delivery<Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        match (&d.message, d.item) {
            (Msg::Arrive, Some(item)) => {
                ctx.receive(item)?;
                self.queue.push(item);
                ctx.schedule(self.hold, Msg::Tick(0))?;
            }
            (Msg::Tick(_), None) => {
                let item = self.queue.remove(0);
                ctx.dispose(item, Disposal::Delivered)?;
            }
            (other, _) => {
                return Err(ComponentError::Unexpected {
                    state: "running",
                    event: format!("{other:?}"),
                });
            }
        }
        Ok(())
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        match self.lie {
            None => self.queue.iter().copied().for_each(visit),
            Some(Lie::Forget) => self.queue.iter().skip(1).copied().for_each(visit),
            Some(Lie::Twice) => {
                self.queue.iter().copied().for_each(&mut *visit);
                self.queue.iter().take(1).copied().for_each(visit);
            }
            Some(Lie::Foreign) => {
                self.queue.iter().copied().for_each(&mut *visit);
                visit(ItemId::from_raw(999_999));
            }
        }
    }
}

#[allow(clippy::unwrap_used)] // test helper
fn line(lie: Option<Lie>, checks: bool) -> Engine<Msg> {
    let config = EngineConfig::new(1);
    let config = if checks {
        config
    } else {
        config.without_checks()
    };
    let mut engine = Engine::new(config);
    let source = Source {
        every: d("10s"),
        to: "delay",
    };
    engine.add_component("source", Box::new(source)).unwrap();
    let delay = Delay {
        hold: d("25s"),
        queue: Vec::new(),
        lie,
    };
    engine.add_component("delay", Box::new(delay)).unwrap();
    engine
}

#[test]
fn items_flow_and_are_conserved() {
    let mut engine = line(None, true);
    let summary = engine.run(RunLimits::until(t("1h")), &mut NoTrace).unwrap();
    assert_eq!(summary.outcome, RunOutcome::Horizon);
    let c = engine.item_counts();
    // 360 created (every 10 s up to 3600 s); an item created at c leaves at
    // c + 25 s, so those created up to 3570 s are delivered: 357. The ones
    // from 3580, 3590 and 3600 s are still held.
    assert_eq!(
        (c.created, c.delivered, c.held, c.in_transit),
        (360, 357, 3, 0)
    );
}

#[test]
fn same_instant_events_order_by_priority_then_scheduling() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "a",
            scripted(
                |ctx| {
                    let me = ctx.me();
                    ctx.schedule(d("1s"), Msg::Tick(1))?;
                    ctx.send(me, Msg::Tick(2))?;
                    ctx.send(me, Msg::Urgent(3))?;
                    ctx.schedule(SimDuration::ZERO, Msg::Tick(4))?;
                    ctx.schedule(d("1s"), Msg::Urgent(5))?;
                    Ok(())
                },
                move |d, ctx| {
                    log.lock().unwrap().push((ctx.now(), d.message.clone()));
                    Ok(())
                },
            ),
        )
        .unwrap();
    engine
        .run(RunLimits::until(t("1min")), &mut NoTrace)
        .unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [
            (SimTime::START, Msg::Urgent(3)),
            (SimTime::START, Msg::Tick(2)),
            (SimTime::START, Msg::Tick(4)),
            (t("1s"), Msg::Urgent(5)),
            (t("1s"), Msg::Tick(1)),
        ]
    );
}

#[test]
fn run_limits_stop_where_they_say() {
    // Horizon: events at exactly `until` are processed, the clock ends there.
    // Up to 30 s: source ticks at 10, 20 and 30 s, each followed by an
    // arrival at the delay at the same instant.
    let mut engine = line(None, true);
    let s = engine
        .run(RunLimits::until(t("30s")), &mut NoTrace)
        .unwrap();
    assert_eq!(
        (s.outcome, s.events, s.now),
        (RunOutcome::Horizon, 6, t("30s"))
    );
    // Continuing picks up where it stopped: the first release at 35 s,
    // then the tick and the arrival at 40 s.
    let s = engine
        .run(RunLimits::until(t("40s")), &mut NoTrace)
        .unwrap();
    assert_eq!((s.events, s.now), (3, t("40s")));
    // Event limit, including zero.
    let s = engine
        .run(RunLimits::until(t("1h")).max_events(5), &mut NoTrace)
        .unwrap();
    assert_eq!((s.outcome, s.events), (RunOutcome::EventLimit, 5));
    let s = engine
        .run(RunLimits::until(t("1h")).max_events(0), &mut NoTrace)
        .unwrap();
    assert_eq!((s.outcome, s.events), (RunOutcome::EventLimit, 0));
    // A set cancel flag stops before the next event.
    let stop = AtomicBool::new(true);
    let s = engine
        .run(RunLimits::until(t("1h")).cancel_on(&stop), &mut NoTrace)
        .unwrap();
    assert_eq!((s.outcome, s.events), (RunOutcome::Cancelled, 0));
    let stop = AtomicBool::new(false);
    let s = engine
        .run(RunLimits::until(t("1min")).cancel_on(&stop), &mut NoTrace)
        .unwrap();
    assert_eq!(s.outcome, RunOutcome::Horizon);
    // The past is not a horizon.
    let err = engine
        .run(RunLimits::until(t("1s")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "cannot run until 1s: the simulation is already at 1min"
    );
}

#[test]
fn a_component_can_cancel_the_run() {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "watchdog",
            scripted(
                |ctx| {
                    ctx.schedule(d("1s"), Msg::Tick(0))?;
                    Ok(())
                },
                move |_, ctx| {
                    if ctx.now() == SimTime::START.checked_add(d("3s"))? {
                        flag.store(true, Ordering::Relaxed);
                    }
                    ctx.schedule(d("1s"), Msg::Tick(0))?;
                    Ok(())
                },
            ),
        )
        .unwrap();
    let s = engine
        .run(RunLimits::until(t("1h")).cancel_on(&stop), &mut NoTrace)
        .unwrap();
    assert_eq!(
        (s.outcome, s.events, s.now),
        (RunOutcome::Cancelled, 3, t("3s"))
    );
}

#[test]
fn an_idle_model_jumps_to_the_horizon() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine.add_component("nothing", idle()).unwrap();
    let s = engine.run(RunLimits::until(t("8h")), &mut NoTrace).unwrap();
    assert_eq!((s.outcome, s.events, s.now), (RunOutcome::Idle, 0, t("8h")));
}

#[test]
fn components_cannot_be_added_twice_or_late() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine.add_component("a", idle()).unwrap();
    assert_eq!(
        engine.add_component("a", idle()).unwrap_err(),
        EngineError::DuplicateComponent { name: "a".into() }
    );
    engine.run(RunLimits::until(t("1s")), &mut NoTrace).unwrap();
    assert_eq!(
        engine.add_component("b", idle()).unwrap_err(),
        EngineError::AlreadyStarted { name: "b".into() }
    );
    assert_eq!(engine.component_name(ComponentId::from_index(0)), Some("a"));
    assert_eq!(engine.component_name(ComponentId::from_index(1)), None);
}

/// Runs one component whose start does `start`; returns the run error.
#[allow(clippy::unwrap_used)] // test helper
fn start_error(
    start: impl FnMut(&mut Context<'_, Msg>) -> Result<(), ComponentError> + 'static,
) -> RunError {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component("c", scripted(start, |_, _| Ok(())))
        .unwrap();
    engine.add_component("other", idle()).unwrap();
    engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err()
}

#[test]
fn engine_misuse_stops_the_run_with_the_reason() {
    let cases: Vec<(RunError, &str)> = vec![
        (
            start_error(|ctx| Ok(ctx.lookup("nowhere").map(|_| ())?)),
            "at 0s, `c`: no component with id `nowhere`",
        ),
        (
            start_error(|ctx| Ok(ctx.send(ComponentId::from_index(9), Msg::Tick(0))?)),
            "at 0s, `c`: component number 9 does not exist in this run",
        ),
        (
            start_error(|ctx| {
                let item = ctx.create_item();
                ctx.transfer(item, ComponentId::from_index(9), Msg::Arrive)?;
                Ok(())
            }),
            "at 0s, `c`: component number 9 does not exist in this run",
        ),
        (
            start_error(|ctx| Ok(ctx.transfer(ItemId::from_raw(0), ctx.me(), Msg::Arrive)?)),
            "at 0s, `c`: `c` does not hold item 0",
        ),
        (
            start_error(|ctx| Ok(ctx.dispose(ItemId::from_raw(3), Disposal::Consumed)?)),
            "at 0s, `c`: `c` does not hold item 3",
        ),
        (
            start_error(|ctx| Ok(ctx.receive(ItemId::from_raw(0))?)),
            "at 0s, `c`: `c` cannot receive item 0: the current event does not carry it",
        ),
        (
            start_error(|ctx| {
                let timer = ctx.schedule(d("1s"), Msg::Tick(0))?;
                ctx.cancel(timer)?;
                Ok(ctx.cancel(timer)?)
            }),
            "at 0s, `c`: event #0 is not a pending timer",
        ),
        (
            start_error(|ctx| {
                ctx.send(ctx.me(), Msg::Tick(0))?;
                // Messages are not timers and cannot be cancelled.
                Ok(ctx.cancel(EventId::from_raw(0))?)
            }),
            "at 0s, `c`: event #0 is not a pending timer",
        ),
        (
            start_error(|ctx| {
                ctx.stream("cycle_time")?;
                Ok(ctx.stream("cycle_time").map(|_| ())?)
            }),
            "at 0s, `c`: stream `cycle_time` of `c` conflicts with stream `cycle_time` of `c`",
        ),
        (
            start_error(|_| {
                Err(ComponentError::Unexpected {
                    state: "idle",
                    event: "Start".into(),
                })
            }),
            "at 0s, `c`: unexpected event Start in state `idle`",
        ),
    ];
    for (err, message) in cases {
        assert_eq!(err.to_string(), message);
        assert!(matches!(err, RunError::Component { .. }));
    }
}

#[test]
fn scheduling_past_the_end_of_time_is_an_error() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "late",
            scripted(
                |ctx| {
                    ctx.schedule(d("1s"), Msg::Tick(0))?;
                    Ok(())
                },
                |_, ctx| {
                    ctx.schedule(SimDuration::MAX, Msg::Tick(0))?;
                    Ok(())
                },
            ),
        )
        .unwrap();
    let err = engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "at 1s, `late`: time 1s + 18446744073.709551615s overflows; \
         the largest value is 18446744073.709551615s (about 584 years)"
    );
}

#[test]
fn only_the_owner_cancels_a_timer() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "owner",
            scripted(
                |ctx| {
                    let timer = ctx.schedule(d("1min"), Msg::Tick(0))?;
                    ctx.send(ctx.lookup("thief")?, Msg::CancelThis(timer))?;
                    Ok(())
                },
                |_, _| Ok(()),
            ),
        )
        .unwrap();
    engine
        .add_component(
            "thief",
            scripted(
                |_| Ok(()),
                |d, ctx| match d.message {
                    Msg::CancelThis(timer) => Ok(ctx.cancel(timer)?),
                    Msg::Tick(_) | Msg::Urgent(_) | Msg::Arrive => Ok(()),
                },
            ),
        )
        .unwrap();
    let err = engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "at 0s, `thief`: `thief` cannot cancel event #0, a timer of `owner`"
    );
}

#[test]
fn an_item_must_be_received_by_its_target() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "giver",
            scripted(
                |ctx| {
                    let item = ctx.create_item();
                    ctx.transfer(item, ctx.lookup("ignorer")?, Msg::Arrive)?;
                    Ok(())
                },
                |_, _| Ok(()),
            ),
        )
        .unwrap();
    engine.add_component("ignorer", idle()).unwrap();
    let err = engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "at 0s, `ignorer` did not receive item 0, which the event carried"
    );
    let RunError::NotReceived { recent, .. } = err else {
        panic!("expected NotReceived")
    };
    assert_eq!(
        recent,
        [RecentEvent {
            seq: 0,
            time: SimTime::START,
            target: "ignorer".into(),
            from: "giver".into(),
            message: "Arrive".into(),
            item: Some(ItemId::from_raw(0)),
        }]
    );
    assert_eq!(
        recent[0].to_string(),
        "#0 at 0s: giver -> ignorer Arrive with item 0"
    );
}

#[test]
fn receiving_the_wrong_item_is_an_error() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "giver",
            scripted(
                |ctx| {
                    let item = ctx.create_item();
                    ctx.transfer(item, ctx.lookup("confused")?, Msg::Arrive)?;
                    Ok(())
                },
                |_, _| Ok(()),
            ),
        )
        .unwrap();
    engine
        .add_component(
            "confused",
            scripted(|_| Ok(()), |_, ctx| Ok(ctx.receive(ItemId::from_raw(7))?)),
        )
        .unwrap();
    let err = engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "at 0s, `confused`: `confused` cannot receive item 7: the current event does not carry it"
    );
}

#[test]
fn unexpected_events_are_component_errors() {
    let mut engine = line(None, true);
    engine
        .add_component(
            "stranger",
            scripted(
                |ctx| {
                    ctx.send(ctx.lookup("delay")?, Msg::Urgent(1))?;
                    Ok(())
                },
                |_, _| Ok(()),
            ),
        )
        .unwrap();
    let err = engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "at 0s, `delay`: unexpected event Urgent(1) in state `running`"
    );
}

/// Breaks its invariant at its 40th event.
struct Fragile(u32);

impl Component<Msg> for Fragile {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        ctx.schedule(d("1s"), Msg::Tick(0))?;
        Ok(())
    }

    fn handle(
        &mut self,
        d: Delivery<Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        let Msg::Tick(n) = d.message else {
            return Ok(());
        };
        self.0 = n + 1;
        ctx.schedule(self::d("1s"), Msg::Tick(n + 1))?;
        Ok(())
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        if self.0 >= 40 {
            return Err(Violation {
                invariant: "count < 40",
                detail: format!("count {}", self.0),
            });
        }
        Ok(())
    }
}

#[test]
fn a_broken_invariant_stops_the_run_with_the_recent_events() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component("fragile", Box::new(Fragile(0)))
        .unwrap();
    let err = engine
        .run(RunLimits::until(t("1h")), &mut NoTrace)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "at 40s, `fragile`: invariant `count < 40` violated: count 40"
    );
    let RunError::Invariant { recent, .. } = err else {
        panic!("expected Invariant")
    };
    // The last 32 events, oldest first, ending with the one that broke it.
    assert_eq!(recent.len(), 32);
    assert_eq!(
        recent[0].to_string(),
        "#8 at 9s: fragile -> fragile Tick(8)"
    );
    assert_eq!(
        recent[31].to_string(),
        "#39 at 40s: fragile -> fragile Tick(39)"
    );
}

#[test]
fn conservation_catches_components_that_misreport_their_items() {
    // Forgetting or doubling needs an item (the first arrives at 10 s); an
    // item that was never there is caught by the start check.
    for (lie, at, detail) in [
        (
            Lie::Forget,
            "10s",
            "it reports 0 items, the engine records 1",
        ),
        (Lie::Twice, "10s", "it reports item 0 twice"),
        (
            Lie::Foreign,
            "0s",
            "it reports item 999999, which it does not hold",
        ),
    ] {
        let mut engine = line(Some(lie), true);
        let err = engine
            .run(RunLimits::until(t("1h")), &mut NoTrace)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            format!("at {at}, `delay`: conservation violated: {detail}")
        );
        assert!(matches!(err, RunError::Conservation { .. }));
        // With checks off (`--fast`) the same model runs.
        let mut fast = line(Some(lie), false);
        assert!(fast.run(RunLimits::until(t("1h")), &mut NoTrace).is_ok());
    }
}

#[test]
fn disposals_are_counted_by_kind_and_traced() {
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component(
            "end",
            scripted(
                |ctx| {
                    for how in [Disposal::Delivered, Disposal::Scrapped, Disposal::Consumed] {
                        let item = ctx.create_item();
                        ctx.dispose(item, how)?;
                    }
                    Ok(())
                },
                |_, _| Ok(()),
            ),
        )
        .unwrap();
    let mut trace = Recorder::default();
    engine.run(RunLimits::until(t("1s")), &mut trace).unwrap();
    let c = engine.item_counts();
    assert_eq!(
        (c.created, c.delivered, c.scrapped, c.consumed, c.held),
        (3, 1, 1, 1, 0)
    );
    assert_eq!(trace.0.len(), 6);
    assert!(trace.0[5].starts_with("ItemDisposed") && trace.0[5].contains("Consumed"));
}

/// Waits a random lognormal time between events and records each wait.
struct Jitter {
    stream: Option<Stream>,
    waits: Arc<std::sync::Mutex<Vec<u64>>>,
}

impl Jitter {
    fn wait(&mut self) -> Result<SimDuration, ComponentError> {
        let dist = DurationDistribution::lognormal(d("10s"), 0.5).map_err(|e| {
            ComponentError::Unexpected {
                state: "start",
                event: e.to_string(),
            }
        })?;
        let Some(stream) = self.stream.as_mut() else {
            return Ok(SimDuration::ZERO);
        };
        let wait = dist.sample(stream)?;
        self.waits
            .lock()
            .map_err(|_| ComponentError::Unexpected {
                state: "lock",
                event: String::new(),
            })?
            .push(wait.as_nanos());
        Ok(wait)
    }
}

impl Component<Msg> for Jitter {
    fn start(&mut self, ctx: &mut Context<'_, Msg>) -> Result<(), ComponentError> {
        self.stream = Some(ctx.stream("wait")?);
        let wait = self.wait()?;
        ctx.schedule(wait, Msg::Tick(0))?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<Msg>,
        ctx: &mut Context<'_, Msg>,
    ) -> Result<(), ComponentError> {
        let wait = self.wait()?;
        ctx.schedule(wait, Msg::Tick(0))?;
        Ok(())
    }
}

/// Runs jitter components named `names` with `seed`; returns each one's
/// waits and the trace.
#[allow(clippy::unwrap_used)] // test helper
fn jitter_run(seed: u64, names: &[&str]) -> (Vec<Vec<u64>>, Vec<String>) {
    let mut engine = Engine::new(EngineConfig::new(seed));
    let logs: Vec<_> = names
        .iter()
        .map(|name| {
            let waits = Arc::new(std::sync::Mutex::new(Vec::new()));
            let jitter = Jitter {
                stream: None,
                waits: Arc::clone(&waits),
            };
            engine.add_component(name, Box::new(jitter)).unwrap();
            waits
        })
        .collect();
    let mut trace = Recorder::default();
    engine.run(RunLimits::until(t("1h")), &mut trace).unwrap();
    let waits = logs.iter().map(|w| w.lock().unwrap().clone()).collect();
    (waits, trace.0)
}

#[test]
fn same_seed_same_trace_and_other_seed_other_trace() {
    let (_, a) = jitter_run(42, &["cut", "weld"]);
    let (_, b) = jitter_run(42, &["cut", "weld"]);
    let (_, c) = jitter_run(43, &["cut", "weld"]);
    assert!(a.len() > 500);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn adding_a_component_leaves_other_streams_unchanged() {
    let (alone, _) = jitter_run(42, &["cut"]);
    let (with_more, _) = jitter_run(42, &["paint", "cut", "weld"]);
    assert_eq!(alone[0], with_more[1]);
    assert_ne!(alone[0], with_more[0]);
}

#[test]
fn ids_round_trip_through_their_numbers() {
    assert_eq!(ComponentId::from_index(3).index(), 3);
    assert_eq!(EventId::from_raw(5).as_u64(), 5);
    assert_eq!(ItemId::from_raw(8).as_u64(), 8);
    assert_eq!(EventId::from_raw(5).to_string(), "event #5");
}
