//! The budget of events per instant (ADR-0022 §6, question 21 b): a run whose
//! clock stops moving ends with a typed failure naming the component and the
//! instant, the same way on every PC, and a legitimate run is not touched.

mod support;

use std::collections::BTreeMap;

use support::{d, document, load, registry, t};
use taktsim_components::{FlowMessage, RunFailure, RunOptions, TIMERS_PER_INSTANT};
use taktsim_core::{
    ComponentId, EngineConfig, NoTrace, RunLimits, RunOutcome, TraceRecord, TraceSink,
};
use taktsim_model::Model;
use taktsim_trace::{TraceReader, replay};
use taktsim_units::SimTime;

/// Disagreement 20: a gamma of mean 1 s and cv 10⁶ draws 0 ns almost always, so
/// `cut`'s cycles keep ending at time zero.
fn stopping_clock() -> Model {
    load(&document(
        "1h",
        "  - {id: source, type: Source}\n  \
         - {id: cut, type: Machine, cycle_time: {dist: gamma, mean: 1s, cv: 1000000}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    ))
}

/// The failure's fields, to compare two runs; `None` for another failure.
fn clock_stopped(failure: &RunFailure) -> Option<(String, String, String, u64, u64)> {
    let RunFailure::ClockStopped {
        path,
        component,
        time,
        timers,
        events,
    } = failure
    else {
        return None;
    };
    Some((
        path.to_string(),
        component.clone(),
        time.to_string(),
        *timers,
        *events,
    ))
}

#[test]
fn a_clock_that_stops_moving_ends_the_run_with_the_component_and_the_instant() {
    let model = stopping_clock();
    let registry = registry();
    let failure = registry
        .prepare(&model, RunOptions::new().seed(1))
        .unwrap()
        .run(&mut NoTrace)
        .unwrap_err();
    let (path, component, time, timers, events) =
        clock_stopped(&failure).expect("a clock that stopped");
    assert_eq!(
        (path.as_str(), component.as_str(), time.as_str()),
        ("components[1]", "cut", "0s")
    );
    // Exactly at the first event past the budget.
    assert_eq!(timers, TIMERS_PER_INSTANT + 1);
    assert!(events > timers, "{events}");
    assert_eq!(
        failure.path().map(ToString::to_string).as_deref(),
        Some("components[1]")
    );
    assert_eq!(
        failure.found(),
        Some(format!(
            "1000001 events `cut` addressed to itself at 0s without the clock moving, {events} \
             events in all"
        ))
    );
    assert_eq!(
        failure.allowed().as_deref(),
        Some(
            "at most 1000000 events a component addresses to itself at one instant: durations \
             whose draws are not almost all 0 ns"
        )
    );
    assert_eq!(
        failure.example().as_deref(),
        Some("{dist: lognormal, mean: 42s, sigma: 0.08}")
    );
    assert!(
        failure
            .to_string()
            .starts_with("components[1]: `cut` addressed 1000001 events to itself at 0s"),
        "{failure}"
    );

    // The same failure again, written to a trace this time: it depends on the
    // model and the seed only.
    let again = registry
        .prepare(&model, RunOptions::new().seed(1))
        .unwrap()
        .run_traced(std::io::sink())
        .unwrap_err();
    assert_eq!(clock_stopped(&again), clock_stopped(&failure));
}

/// The budget counts events, never wall-clock time: an event limit below it ends
/// the run as before, with the clock still at zero.
#[test]
fn an_event_limit_below_the_budget_still_ends_the_run() {
    let report = registry()
        .prepare(
            &stopping_clock(),
            RunOptions::new().seed(1).max_events(1000),
        )
        .unwrap()
        .run(&mut NoTrace)
        .unwrap();
    assert_eq!(report.summary.outcome, RunOutcome::EventLimit);
    assert_eq!((report.summary.events, report.summary.now), (1000, t("0s")));
}

/// The most events one component addressed to itself at one instant, and when.
#[derive(Default)]
struct MostTimers {
    now: Option<SimTime>,
    counts: BTreeMap<ComponentId, u64>,
    most: u64,
    at: Option<SimTime>,
}

impl TraceSink<FlowMessage> for MostTimers {
    fn record(&mut self, record: &TraceRecord<'_, FlowMessage>) {
        if let TraceRecord::Event {
            time, target, from, ..
        } = record
        {
            if self.now != Some(*time) {
                self.now = Some(*time);
                self.counts.clear();
            }
            if target == from {
                let count = self.counts.entry(*target).or_default();
                *count += 1;
                if *count > self.most {
                    self.most = *count;
                    self.at = Some(*time);
                }
            }
        }
    }
}

/// A batch of 65,535 parts into a machine of 65,535 units with a constant cycle:
/// 65,535 cycles of one machine end at one instant, the most the model's limits
/// allow. The run reaches its horizon, with the events of a single call to the
/// engine. Checks are off: they cost O(k log k) per event for a component holding
/// k parts, which is 65,535 here.
#[test]
fn the_most_timers_a_model_allows_at_one_instant_run_untouched() {
    let model = load(&document(
        "2h",
        "  - {id: source, type: Source, interarrival: 1h, batch: 65535 parts}\n  \
         - {id: cut, type: Machine, cycle_time: 1s, capacity: 65535 parts}\n  \
         - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    ));
    let registry = registry();
    let mut most = MostTimers::default();
    let report = registry
        .prepare(&model, RunOptions::new().seed(1).without_checks())
        .unwrap()
        .run(&mut most)
        .unwrap();
    assert_eq!(report.summary.outcome, RunOutcome::Horizon);
    // The batch is released at 1h; its cycles all end at 1h 1s.
    assert_eq!(report.items.delivered.get(), 65_535);
    assert_eq!(
        (most.most, most.at),
        (65_535, Some(t("1h").checked_add(d("1s")).unwrap()))
    );
    let mut engine = registry
        .build(model.document(), EngineConfig::new(1).without_checks())
        .unwrap();
    let direct = engine.run(RunLimits::until(t("2h")), &mut NoTrace).unwrap();
    assert_eq!(report.summary, direct);
}

/// The run layer steps the engine; a limit past one step gives the same summary
/// as one call to the engine with that limit, and a trace that replays.
#[test]
fn an_event_limit_past_one_step_is_the_engine_s() {
    let model = load(&document(
        "1000d",
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1s}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, sink]",
    ));
    let limit = TIMERS_PER_INSTANT + 1000;
    let registry = registry();
    let report = registry
        .prepare(&model, RunOptions::new().seed(3).max_events(limit))
        .unwrap()
        .run(&mut NoTrace)
        .unwrap();
    assert_eq!(report.summary.outcome, RunOutcome::EventLimit);
    assert_eq!(report.summary.events, limit);
    let mut engine = registry
        .build(model.document(), EngineConfig::new(3))
        .unwrap();
    let direct = engine
        .run(RunLimits::until(t("1000d")).max_events(limit), &mut NoTrace)
        .unwrap();
    assert_eq!(report.summary, direct);
    assert_eq!(report.items, engine.item_counts());
    let (bytes, traced) = registry
        .prepare(&model, RunOptions::new().seed(3).max_events(limit))
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    assert_eq!(traced.summary, direct);
    let mut fresh = registry
        .build(model.document(), EngineConfig::new(3))
        .unwrap();
    replay(TraceReader::new(bytes.as_slice()).unwrap(), &mut fresh).unwrap();
}
