//! Properties of the event loop over generated schedules and models.

use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use taktsim_core::{
    Component, ComponentError, Context, Delivery, Disposal, Engine, EngineConfig, EventId, ItemId,
    Message, NoTrace, RunLimits, RunOutcome,
};
use taktsim_units::{SimDuration, SimTime};

/// A message with a chosen priority and an id.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Prioritized {
    id: usize,
    priority: u8,
}

impl Message for Prioritized {
    fn priority(&self) -> u8 {
        self.priority
    }
}

/// Schedules timers at start, cancels some, records delivery order.
struct Scheduler {
    plan: Vec<(u64, u8, bool)>,
    seen: Arc<Mutex<Vec<usize>>>,
}

impl Component<Prioritized> for Scheduler {
    fn start(&mut self, ctx: &mut Context<'_, Prioritized>) -> Result<(), ComponentError> {
        let mut ids: Vec<(EventId, bool)> = Vec::new();
        for (id, &(delay, priority, cancel)) in self.plan.iter().enumerate() {
            let event =
                ctx.schedule(SimDuration::from_nanos(delay), Prioritized { id, priority })?;
            ids.push((event, cancel));
        }
        for (event, cancel) in ids {
            if cancel {
                ctx.cancel(event)?;
            }
        }
        Ok(())
    }

    fn handle(
        &mut self,
        d: Delivery<Prioritized>,
        _: &mut Context<'_, Prioritized>,
    ) -> Result<(), ComponentError> {
        self.seen
            .lock()
            .map_err(|_| ComponentError::Unexpected {
                state: "poisoned",
                event: String::new(),
            })?
            .push(d.message.id);
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum Flow {
    Make,
    Arrive,
    Release,
}

impl Message for Flow {}

/// A source that creates an item every `every` ns and sends it on.
struct Maker {
    every: u64,
    to: String,
}

impl Component<Flow> for Maker {
    fn start(&mut self, ctx: &mut Context<'_, Flow>) -> Result<(), ComponentError> {
        ctx.schedule(SimDuration::from_nanos(self.every), Flow::Make)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<Flow>,
        ctx: &mut Context<'_, Flow>,
    ) -> Result<(), ComponentError> {
        let item = ctx.create_item();
        ctx.transfer(item, ctx.lookup(&self.to)?, Flow::Arrive)?;
        ctx.schedule(SimDuration::from_nanos(self.every), Flow::Make)?;
        Ok(())
    }
}

/// Holds each item for `hold` ns, then passes it on (or scraps every
/// `scrap_every`-th, or delivers it at the end of the line). Honest about
/// what it holds.
struct Station {
    hold: u64,
    next: Option<String>,
    scrap_every: u64,
    held: Vec<ItemId>,
    count: u64,
}

impl Component<Flow> for Station {
    fn handle(
        &mut self,
        d: Delivery<Flow>,
        ctx: &mut Context<'_, Flow>,
    ) -> Result<(), ComponentError> {
        match (d.message, d.item) {
            (Flow::Arrive, Some(item)) => {
                ctx.receive(item)?;
                self.held.push(item);
                ctx.schedule(SimDuration::from_nanos(self.hold), Flow::Release)?;
            }
            (Flow::Release, None) => {
                let item = self.held.remove(0);
                self.count += 1;
                if self.count.is_multiple_of(self.scrap_every) {
                    ctx.dispose(item, Disposal::Scrapped)?;
                } else if let Some(next) = &self.next {
                    ctx.transfer(item, ctx.lookup(next)?, Flow::Arrive)?;
                } else {
                    ctx.dispose(item, Disposal::Delivered)?;
                }
            }
            (other, item) => {
                return Err(ComponentError::Unexpected {
                    state: "running",
                    event: format!("{other:?} with {item:?}"),
                });
            }
        }
        Ok(())
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        self.held.iter().copied().for_each(visit);
    }
}

proptest! {
    #[test]
    fn delivery_order_is_time_then_priority_then_scheduling(
        plan in proptest::collection::vec((0_u64..20, 0_u8..3, any::<bool>()), 0..60)
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut engine = Engine::new(EngineConfig::new(1));
        engine.add_component("s", Box::new(Scheduler { plan: plan.clone(), seen: Arc::clone(&seen) })).unwrap();
        let summary = engine.run(RunLimits::until(SimTime::from_nanos(100)), &mut NoTrace).unwrap();
        prop_assert_eq!(summary.outcome, RunOutcome::Idle);
        let mut expected: Vec<(u64, u8, usize)> = plan
            .iter()
            .enumerate()
            .filter(|(_, (_, _, cancel))| !cancel)
            .map(|(id, &(delay, priority, _))| (delay, priority, id))
            .collect();
        expected.sort_unstable();
        let expected: Vec<usize> = expected.into_iter().map(|(_, _, id)| id).collect();
        prop_assert_eq!(seen.lock().unwrap().clone(), expected);
    }

    #[test]
    fn items_are_conserved_in_any_line(
        every in 1_u64..50,
        holds in proptest::collection::vec(1_u64..200, 1..6),
        scrap_every in 2_u64..10,
        horizon in 0_u64..5_000,
    ) {
        let mut engine = Engine::new(EngineConfig::new(1));
        engine.add_component("source", Box::new(Maker { every, to: "s0".into() })).unwrap();
        for (i, &hold) in holds.iter().enumerate() {
            let next = (i + 1 < holds.len()).then(|| format!("s{}", i + 1));
            let station = Station { hold, next, scrap_every, held: Vec::new(), count: 0 };
            engine.add_component(&format!("s{i}"), Box::new(station)).unwrap();
        }
        // Checks are on: any conservation slip would be an error here.
        engine.run(RunLimits::until(SimTime::from_nanos(horizon)), &mut NoTrace).unwrap();
        let c = engine.item_counts();
        prop_assert_eq!(c.created, horizon / every);
        prop_assert_eq!(c.in_transit, 0);
        prop_assert_eq!(c.created, c.held + c.delivered + c.scrapped + c.consumed);
    }
}
