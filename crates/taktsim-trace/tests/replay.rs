//! Replay: a run re-executed from its trace is the same run at every event,
//! and every way it can differ is reported (spec §7 Layer 3, ADR-0021).

mod support;

use proptest::prelude::*;
use support::{Sink, Source, Station, Toy};
use taktsim_core::{
    Component, ComponentError, Context, Delivery, Engine, EngineConfig, EngineError, RunLimits,
    RunOutcome, RunSummary,
};
use taktsim_trace::{
    Header, ModelHash, ReadError, ReplayError, SystemHash, TraceReader, TraceWriter, replay,
};
use taktsim_units::SimTime;

#[allow(clippy::unwrap_used)] // test helper
fn t(text: &str) -> SimTime {
    text.parse().unwrap()
}

/// Runs `engine` to `until` into an in-memory trace.
#[allow(clippy::unwrap_used)] // test helper
fn trace_of(mut engine: Engine<Toy>, until: &str) -> Vec<u8> {
    let header = Header::for_engine(
        &engine,
        ModelHash::sha256(b"replay tests"),
        SystemHash::default(),
    );
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    engine.run(RunLimits::until(t(until)), &mut writer).unwrap();
    writer.finish().unwrap().0
}

#[allow(clippy::unwrap_used)] // test helper
fn replay_with(
    trace: &[u8],
    mut engine: Engine<Toy>,
) -> Result<taktsim_trace::ReplayReport, ReplayError> {
    replay(TraceReader::new(trace).unwrap(), &mut engine)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn a_run_replays_from_its_trace(seed: u64, minutes in 0_u64..600, tandem: bool) {
        let build = if tandem { support::tandem } else { support::single_server };
        let until = format!("{minutes}min");
        let trace = trace_of(build(seed).unwrap(), &until);
        let reader = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap();
        let mut engine = build(seed).unwrap();
        let report = replay(reader, &mut engine).unwrap();
        let summary = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap().count();
        prop_assert_eq!(report.records, summary as u64);
    }
}

#[test]
fn a_replay_reports_what_it_compared() {
    let trace = trace_of(support::tandem(5).unwrap(), "8h");
    let report = replay_with(&trace, support::tandem(5).unwrap()).unwrap();
    let mut reader = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap();
    while reader.next_record().unwrap().is_some() {}
    let summary = reader.summary().unwrap();
    assert_eq!(
        (report.records, report.events, report.hash),
        (summary.records, summary.events, summary.hash)
    );
    assert!(report.events > 500);
}

#[test]
fn a_different_model_diverges_at_the_first_different_record() {
    let trace = trace_of(support::single_server(1).unwrap(), "2h");
    // Same components, faster station.
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component("source", Source::boxed("1min", "station"))
        .unwrap();
    engine
        .add_component("station", Box::new(Station::new("44s", "sink")))
        .unwrap();
    engine.add_component("sink", Box::new(Sink)).unwrap();
    let Err(e) = replay_with(&trace, engine) else {
        panic!("a different model replayed");
    };
    let ReplayError::Diverged {
        record,
        expected,
        found,
    } = &e
    else {
        panic!("{e}");
    };
    assert!(*record > 0);
    assert_ne!(expected, found);
    // Both sides are readable records with component names.
    assert!(
        expected.contains(" -> station Done") || expected.contains("event #"),
        "{expected}"
    );
    assert!(e.to_string().starts_with(&format!(
        "the replay diverges from the trace at record {record}: the trace has "
    )));
}

#[test]
fn an_engine_that_stops_early_diverges() {
    // The same components, but the source never schedules anything.
    struct Silent;
    impl Component<Toy> for Silent {
        fn handle(
            &mut self,
            _: Delivery<'_, Toy>,
            _: &mut Context<'_, Toy>,
        ) -> Result<(), ComponentError> {
            Ok(())
        }
    }
    let trace = trace_of(support::single_server(1).unwrap(), "1h");
    let mut engine = Engine::new(EngineConfig::new(1));
    engine.add_component("source", Box::new(Silent)).unwrap();
    engine
        .add_component("station", Box::new(Station::new("45s", "sink")))
        .unwrap();
    engine.add_component("sink", Box::new(Sink)).unwrap();
    let Err(ReplayError::Diverged {
        record: 0, found, ..
    }) = replay_with(&trace, engine)
    else {
        panic!("a silent source replayed");
    };
    assert!(found.starts_with("no event due by "), "{found}");
}

#[test]
fn an_engine_that_does_more_diverges() {
    struct Eager(bool);
    impl Component<Toy> for Eager {
        fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
            if self.0 {
                let _ = ctx.create_item();
            }
            Ok(())
        }
        fn handle(
            &mut self,
            _: Delivery<'_, Toy>,
            _: &mut Context<'_, Toy>,
        ) -> Result<(), ComponentError> {
            Ok(())
        }
        fn held_items(&self, visit: &mut dyn FnMut(taktsim_core::ItemId)) {
            if self.0 {
                visit(taktsim_core::ItemId::from_raw(0));
            }
        }
    }
    // The replayed engine creates an item at start that the run did not.
    let trace = trace_of(support::single_server(1).unwrap(), "1h");
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component("source", Box::new(Eager(true)))
        .unwrap();
    engine
        .add_component("station", Box::new(Station::new("45s", "sink")))
        .unwrap();
    engine.add_component("sink", Box::new(Sink)).unwrap();
    let Err(ReplayError::Diverged {
        record: 0,
        found,
        expected,
    }) = replay_with(&trace, engine)
    else {
        panic!("an extra item replayed");
    };
    assert_eq!(found, "0s item 0 created by source");
    assert!(
        expected.contains("event #0 source -> source Next"),
        "{expected}"
    );

    // A trace whose run created an item at start, replayed by an engine
    // that does not: the record is missing.
    let mut original = Engine::new(EngineConfig::new(1));
    original
        .add_component("source", Box::new(Eager(true)))
        .unwrap();
    let trace = trace_of(original, "1h");
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component("source", Box::new(Eager(false)))
        .unwrap();
    let Err(ReplayError::Diverged {
        record: 0,
        found,
        expected,
    }) = replay_with(&trace, engine)
    else {
        panic!("a missing item replayed");
    };
    assert_eq!(expected, "0s item 0 created by source");
    assert_eq!(found, "no further record before the engine's next event");

    // And an engine with more records than the trace.
    let mut short = Engine::new(EngineConfig::new(1));
    short
        .add_component("source", Box::new(Eager(false)))
        .unwrap();
    let trace = trace_of(short, "1h");
    let mut engine = Engine::new(EngineConfig::new(1));
    engine
        .add_component("source", Box::new(Eager(true)))
        .unwrap();
    let Err(ReplayError::Diverged { expected, .. }) = replay_with(&trace, engine) else {
        panic!("an extra record replayed");
    };
    assert_eq!(expected, "the end of the trace");
}

#[test]
fn the_header_must_match_the_engine() {
    let trace = trace_of(support::single_server(1).unwrap(), "1h");
    let Err(e) = replay_with(&trace, support::single_server(2).unwrap()) else {
        panic!("another seed replayed");
    };
    assert_eq!(
        e.to_string(),
        "the trace was run with seed 1, the engine has seed 2; build it with seed 1"
    );
    let Err(e) = replay_with(&trace, support::tandem(1).unwrap()) else {
        panic!("another model replayed");
    };
    assert_eq!(
        e.to_string(),
        "component #1 is `station` in the trace and `cut` in the engine; build the engine from the trace's model"
    );
    let mut fewer: Engine<Toy> = Engine::new(EngineConfig::new(1));
    fewer
        .add_component("source", Source::boxed("1min", "station"))
        .unwrap();
    let Err(ReplayError::Components {
        index: 1,
        trace: Some(_),
        engine: None,
    }) = replay_with(&trace, fewer)
    else {
        panic!("a smaller model replayed");
    };
}

#[test]
fn a_trace_of_another_engine_version_is_not_replayed() {
    let engine = support::single_server(1).unwrap();
    let mut header = Header::for_engine(&engine, ModelHash::default(), SystemHash::default());
    header.engine_version = "0.0.0-other".to_owned();
    let (trace, _) = TraceWriter::<_, Toy>::new(Vec::new(), &header)
        .unwrap()
        .finish()
        .unwrap();
    let Err(e) = replay_with(&trace, engine) else {
        panic!("another engine version replayed");
    };
    assert!(matches!(e, ReplayError::EngineVersion { .. }));
    assert!(
        e.to_string()
            .starts_with("the trace was written by engine version 0.0.0-other, this is "),
        "{e}"
    );
}

#[test]
fn a_damaged_trace_fails_the_replay() {
    let trace = trace_of(support::tandem(3).unwrap(), "8h");
    let cut = &trace[..trace.len() - 100];
    let Err(e) = replay_with(cut, support::tandem(3).unwrap()) else {
        panic!("a cut trace replayed");
    };
    assert!(
        matches!(e, ReplayError::Trace(ReadError::Truncated { .. })),
        "{e}"
    );
}

#[test]
fn a_run_that_fails_fails_its_replay() {
    // A station that breaks an invariant at its 3rd lot, both times.
    struct Breaks(u32);
    impl Component<Toy> for Breaks {
        fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
            ctx.schedule("1s".parse()?, Toy::Next)?;
            Ok(())
        }
        fn handle(
            &mut self,
            _: Delivery<'_, Toy>,
            ctx: &mut Context<'_, Toy>,
        ) -> Result<(), ComponentError> {
            self.0 += 1;
            ctx.schedule("1s".parse()?, Toy::Next)?;
            Ok(())
        }
        fn check_invariants(&self) -> Result<(), taktsim_core::Violation> {
            if self.0 >= 3 {
                return Err(taktsim_core::Violation {
                    invariant: "fewer than 3 events",
                    detail: self.0.to_string(),
                });
            }
            Ok(())
        }
    }
    let build = || -> Result<Engine<Toy>, EngineError> {
        let mut engine = Engine::new(EngineConfig::new(1));
        engine.add_component("b", Box::new(Breaks(0)))?;
        Ok(engine)
    };
    let mut engine = build().unwrap();
    let header = Header::for_engine(&engine, ModelHash::default(), SystemHash::default());
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    assert!(
        engine
            .run(RunLimits::until(t("1min")), &mut writer)
            .is_err()
    );
    let (trace, summary) = writer.finish().unwrap();
    assert_eq!(summary.events, 3);
    let Err(e) = replay_with(&trace, build().unwrap()) else {
        panic!("a failing run replayed");
    };
    assert!(matches!(e, ReplayError::Run(_)));
    assert!(
        e.to_string().starts_with("the replayed run failed: "),
        "{e}"
    );
}

/// Notes once per event, its text depending on `word`.
struct Noter(&'static str);

impl Component<Toy> for Noter {
    fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        ctx.note(&Toy::Note {
            text: "start".to_owned(),
            share: 0.5,
        });
        ctx.schedule("1s".parse()?, Toy::Next)?;
        Ok(())
    }
    fn handle(
        &mut self,
        _: Delivery<'_, Toy>,
        ctx: &mut Context<'_, Toy>,
    ) -> Result<(), ComponentError> {
        ctx.note(&Toy::Note {
            text: self.0.to_owned(),
            share: 0.25,
        });
        ctx.schedule("1s".parse()?, Toy::Next)?;
        Ok(())
    }
}

#[allow(clippy::unwrap_used)] // test helper
fn noter(word: &'static str) -> Engine<Toy> {
    let mut engine = Engine::new(EngineConfig::new(3));
    engine
        .add_component("noter", Box::new(Noter(word)))
        .unwrap();
    engine
}

#[test]
fn notes_are_kept_and_compared_by_replay() {
    let trace = trace_of(noter("busy"), "3s");
    let reader = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap();
    let names = reader.header().components.clone();
    let lines: Vec<String> = reader
        .map(|r| r.unwrap().display(&names).to_string())
        .collect();
    assert_eq!(
        lines[0],
        "0s note by noter Note { text: \"start\", share: 0.5 }"
    );
    assert_eq!(lines[1], "1s event #0 noter -> noter Next");
    assert_eq!(
        lines[2],
        "1s note by noter Note { text: \"busy\", share: 0.25 }"
    );
    assert_eq!(lines.len(), 7);

    // The same run replays; events count events only.
    let report = replay_with(&trace, noter("busy")).unwrap();
    assert_eq!((report.records, report.events), (7, 3));

    // A note that differs is a divergence, naming both.
    let Err(ReplayError::Diverged {
        record,
        expected,
        found,
    }) = replay_with(&trace, noter("down"))
    else {
        panic!("a different note replayed");
    };
    assert_eq!(record, 2);
    assert!(expected.contains("busy"), "{expected}");
    assert!(found.contains("down"), "{found}");
}

#[test]
fn a_missing_note_is_a_divergence() {
    // A run that wrote notes, replayed by an engine that writes none.
    struct Quiet;
    impl Component<Toy> for Quiet {
        fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
            ctx.schedule("1s".parse()?, Toy::Next)?;
            Ok(())
        }
        fn handle(
            &mut self,
            _: Delivery<'_, Toy>,
            ctx: &mut Context<'_, Toy>,
        ) -> Result<(), ComponentError> {
            ctx.schedule("1s".parse()?, Toy::Next)?;
            Ok(())
        }
    }
    let trace = trace_of(noter("busy"), "3s");
    let mut engine = Engine::new(EngineConfig::new(3));
    engine.add_component("noter", Box::new(Quiet)).unwrap();
    let Err(ReplayError::Diverged {
        record: 0, found, ..
    }) = replay_with(&trace, engine)
    else {
        panic!("a missing note replayed");
    };
    assert_eq!(found, "no further record before the engine's next event");
}

/// Decides once per event, on what depending on `word`, and notes too.
struct Decider(&'static str);

impl Component<Toy> for Decider {
    fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        ctx.schedule("1s".parse()?, Toy::Next)?;
        Ok(())
    }
    fn handle(
        &mut self,
        _: Delivery<'_, Toy>,
        ctx: &mut Context<'_, Toy>,
    ) -> Result<(), ComponentError> {
        ctx.decide(&Toy::Note {
            text: self.0.to_owned(),
            share: 0.75,
        });
        ctx.schedule("1s".parse()?, Toy::Next)?;
        Ok(())
    }
}

#[allow(clippy::unwrap_used)] // test helper
fn decider(word: &'static str) -> Engine<Toy> {
    let mut engine = Engine::new(EngineConfig::new(3));
    engine
        .add_component("decider", Box::new(Decider(word)))
        .unwrap();
    engine
}

/// As [`Decider`], writing a note instead of the decision.
struct Noting;

impl Component<Toy> for Noting {
    fn start(&mut self, ctx: &mut Context<'_, Toy>) -> Result<(), ComponentError> {
        ctx.schedule("1s".parse()?, Toy::Next)?;
        Ok(())
    }
    fn handle(
        &mut self,
        _: Delivery<'_, Toy>,
        ctx: &mut Context<'_, Toy>,
    ) -> Result<(), ComponentError> {
        ctx.note(&Toy::Note {
            text: "left".to_owned(),
            share: 0.75,
        });
        ctx.schedule("1s".parse()?, Toy::Next)?;
        Ok(())
    }
}

#[test]
fn decisions_are_kept_and_compared_by_replay() {
    let trace = trace_of(decider("left"), "3s");
    let reader = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap();
    let names = reader.header().components.clone();
    let lines: Vec<String> = reader
        .map(|r| r.unwrap().display(&names).to_string())
        .collect();
    assert_eq!(lines[0], "1s event #0 decider -> decider Next");
    assert_eq!(
        lines[1],
        "1s decision by decider Note { text: \"left\", share: 0.75 }"
    );
    assert_eq!(lines.len(), 6);

    let report = replay_with(&trace, decider("left")).unwrap();
    assert_eq!((report.records, report.events), (6, 3));

    let Err(ReplayError::Diverged {
        record,
        expected,
        found,
    }) = replay_with(&trace, decider("right"))
    else {
        panic!("a different decision replayed");
    };
    assert_eq!(record, 1);
    assert!(expected.contains("left"), "{expected}");
    assert!(found.contains("right"), "{found}");

    // A note in its place is not the same record, though the fields are.
    let mut engine = Engine::new(EngineConfig::new(3));
    engine.add_component("decider", Box::new(Noting)).unwrap();
    assert!(matches!(
        replay_with(&trace, engine),
        Err(ReplayError::Diverged { record: 1, .. })
    ));
}

#[test]
fn the_hash_of_a_trace_with_decisions_is_pinned() {
    let trace = trace_of(decider("left"), "3s");
    let mut reader = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap();
    while reader.next_record().unwrap().is_some() {}
    assert_eq!(
        reader.summary().unwrap().hash.to_string(),
        "beca4171ef84aea4f573819016c3fa86d6364f7b9cb98e1463664d7de3ecbefb"
    );
}

/// Runs `engine` with `limits`, stating the end, into an in-memory trace.
#[allow(clippy::unwrap_used)] // test helper
fn trace_stating_end(mut engine: Engine<Toy>, limits: RunLimits<'_>) -> Vec<u8> {
    let header = Header::for_engine(
        &engine,
        ModelHash::sha256(b"replay tests"),
        SystemHash::default(),
    );
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    let ran = engine.run(limits, &mut writer).unwrap();
    writer.run_ended(&ran);
    writer.finish().unwrap().0
}

#[test]
fn cancelled_run_states_its_end() {
    let flag = std::sync::atomic::AtomicBool::new(true);
    let trace = trace_stating_end(
        support::single_server(1).unwrap(),
        RunLimits::until(t("1h")).cancel_on(&flag),
    );
    let mut reader = TraceReader::<_, Toy>::new(trace.as_slice()).unwrap();
    while reader.next_record().unwrap().is_some() {}
    let end = reader.summary().unwrap().end.unwrap();
    assert_eq!(end.outcome, RunOutcome::Cancelled);
    assert_eq!(end.time, SimTime::START);
    assert_eq!(end.to_string(), "0s (cancelled)");
    // A cancelled run had events left: replay compares its clock, and does
    // not look for them.
    let report = replay_with(&trace, support::single_server(1).unwrap()).unwrap();
    assert_eq!(report.end, Some(end));
}

#[test]
fn every_way_a_run_ends_replays() {
    // The horizon, with an event still to come.
    let trace = trace_stating_end(
        support::single_server(1).unwrap(),
        RunLimits::until(t("30min")),
    );
    let report = replay_with(&trace, support::single_server(1).unwrap()).unwrap();
    let end = report.end.unwrap();
    assert_eq!((end.outcome, end.time), (RunOutcome::Horizon, t("30min")));
    // Stopped by the event limit: the clock is at the last event.
    let trace = trace_stating_end(
        support::single_server(1).unwrap(),
        RunLimits::until(t("30min")).max_events(10),
    );
    let report = replay_with(&trace, support::single_server(1).unwrap()).unwrap();
    assert_eq!(report.end.unwrap().outcome, RunOutcome::EventLimit);
    // No events left.
    let empty = || Engine::<Toy>::new(EngineConfig::new(1));
    let trace = trace_stating_end(empty(), RunLimits::until(t("5min")));
    let report = replay_with(&trace, empty()).unwrap();
    let end = report.end.unwrap();
    assert_eq!((end.outcome, end.time), (RunOutcome::Idle, t("5min")));
}

#[test]
fn replay_finds_an_event_missing_before_the_end() {
    // The trace stops at 10 min but states an end at 1 h: an event was due.
    let mut engine = support::single_server(1).unwrap();
    let header = Header::for_engine(
        &engine,
        ModelHash::sha256(b"replay tests"),
        SystemHash::default(),
    );
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    let ran = engine
        .run(RunLimits::until(t("10min")), &mut writer)
        .unwrap();
    writer.run_ended(&RunSummary {
        now: t("1h"),
        ..ran
    });
    let (trace, summary) = writer.finish().unwrap();
    let Err(ReplayError::Diverged {
        record,
        expected,
        found,
    }) = replay_with(&trace, support::single_server(1).unwrap())
    else {
        panic!("a trace with a missing event replayed");
    };
    assert_eq!(record, summary.records);
    assert_eq!(expected, "the end of the run at 1h (horizon)");
    assert_eq!(found, "an event due by 1h");
}

#[test]
fn replay_compares_the_stated_outcome_and_clock() {
    fn stated(ran: RunSummary, engine: &Engine<Toy>) -> Vec<u8> {
        let header = Header::for_engine(
            engine,
            ModelHash::sha256(b"replay tests"),
            SystemHash::default(),
        );
        let mut writer = TraceWriter::<_, Toy>::new(Vec::new(), &header).unwrap();
        writer.run_ended(&ran);
        writer.finish().unwrap().0
    }
    let empty = || Engine::<Toy>::new(EngineConfig::new(1));
    // Stated: an event was left at the horizon; the engine has none.
    let ran = RunSummary {
        outcome: RunOutcome::Horizon,
        events: 0,
        now: t("5min"),
    };
    let Err(ReplayError::Diverged { found, .. }) = replay_with(&stated(ran, &empty()), empty())
    else {
        panic!("another outcome replayed");
    };
    assert_eq!(found, "5min (idle)");
    // Stated: stopped at a time the engine's clock is not at.
    let ran = RunSummary {
        outcome: RunOutcome::EventLimit,
        events: 0,
        now: t("5min"),
    };
    let Err(ReplayError::Diverged { found, .. }) = replay_with(&stated(ran, &empty()), empty())
    else {
        panic!("another clock replayed");
    };
    assert_eq!(found, "the engine's clock at 0s");
    // Stated: an end before the last event.
    let mut engine = support::single_server(1).unwrap();
    let header = Header::for_engine(
        &engine,
        ModelHash::sha256(b"replay tests"),
        SystemHash::default(),
    );
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    let ran = engine
        .run(RunLimits::until(t("10min")), &mut writer)
        .unwrap();
    writer.run_ended(&RunSummary {
        now: t("1s"),
        ..ran
    });
    let trace = writer.finish().unwrap().0;
    let Err(ReplayError::Diverged { found, .. }) =
        replay_with(&trace, support::single_server(1).unwrap())
    else {
        panic!("an end before the last event replayed");
    };
    assert!(found.starts_with("the engine's clock at "), "{found}");
}
