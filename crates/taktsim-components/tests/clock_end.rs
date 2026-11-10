//! A wait that ends past the end of the clock never ends (ADR-0022 §6): the run goes
//! on to its horizon, and whether it fails does not depend on a seed. The clock holds
//! 213,503.98 days, so 150,000 days fit once and not twice.

mod support;

use proptest::prelude::*;
use support::{document, load, registry, run, run_model, t};
use taktsim_components::{FlowMessage, MachineState, RunOptions};
use taktsim_core::RunOutcome;

const LINE: &str = "  - [source, m, sink]";

fn on_demand(machine: &str) -> String {
    format!(
        "  - {{id: source, type: Source}}\n  - {{id: m, type: Machine, {machine}}}\n  - {{id: sink, type: Sink}}"
    )
}

fn failure(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::Failure)
}

fn repaired(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::Repaired)
}

fn cycle_done(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::CycleDone { .. })
}

fn setup_done(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::SetupDone)
}

#[test]
fn a_cycle_past_the_clock_never_ends_and_the_run_reaches_its_horizon() {
    // Starts at 0, ends at 150,000 d; the next starts there and would end at 300,000 d.
    let r = run(
        &document("213000d", &on_demand("cycle_time: 150000d"), LINE),
        1,
    );
    assert_eq!(r.times("m", cycle_done), [t("150000d")]);
    assert_eq!(r.delivered().len(), 1);
    assert_eq!(r.end, t("213000d"));
    // Nothing is left to happen, and the engine says so.
    assert_eq!(r.report.summary.outcome, RunOutcome::Idle);
    let (at, last) = *r.machine_notes("m").last().unwrap();
    assert_eq!(at, t("150000d"));
    assert_eq!(last.state, MachineState::Busy);
    assert_eq!(last.units_in_cycle, 1);
}

#[test]
fn an_end_exactly_at_the_end_of_the_clock_still_happens() {
    let limit = "18446744073.709551615s";
    let r = run(
        &document(limit, &on_demand(&format!("cycle_time: {limit}")), LINE),
        1,
    );
    assert_eq!(r.times("m", cycle_done), [t(limit)]);
    assert_eq!(r.delivered().len(), 1);
    assert_eq!(r.end, t(limit));
}

#[test]
fn an_interval_past_the_clock_brings_no_more_parts() {
    let body = "  - {id: source, type: Source, interarrival: 150000d}\n  \
                - {id: m, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}";
    let r = run(&document("213000d", body, LINE), 1);
    let delivered = r.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, t("12960000001s"));
    assert_eq!(r.end, t("213000d"));
}

#[test]
fn an_up_time_past_the_clock_means_no_more_failures() {
    // Parts every 1000 d; the failure at 150,000 d ties with the end of the 150th, which
    // resumes with nothing left at 150,001 d. Then 62 more before 213,000 d.
    let r = run(
        &document(
            "213000d",
            &on_demand("cycle_time: 1000d, mtbf: 150000d, mttr: 1d"),
            LINE,
        ),
        1,
    );
    assert_eq!(r.times("m", failure), [t("150000d")]);
    assert_eq!(r.times("m", repaired), [t("150001d")]);
    assert_eq!(r.delivered().len(), 212);
}

#[test]
fn a_repair_past_the_clock_leaves_the_machine_down() {
    // Up 1 h, down 150,000 d, up 1 h, and the next repair would end past the clock.
    let r = run(
        &document(
            "213000d",
            &on_demand("cycle_time: 1min, mtbf: 1h, mttr: 150000d"),
            LINE,
        ),
        1,
    );
    assert_eq!(r.times("m", failure), [t("1h"), t("12960007200s")]);
    assert_eq!(r.times("m", repaired), [t("12960003600s")]);
    assert_eq!(
        r.machine_notes("m").last().unwrap().1.state,
        MachineState::Down
    );
    assert_eq!(r.end, t("213000d"));
}

#[test]
fn a_paused_cycle_whose_rest_ends_past_the_clock_never_ends() {
    // A cycle of 150,000 d. Failure at 1 d, 149,999 d left. Repair at 100,001 d, so the
    // end would be 250,000 d: past the clock. The next failures find nothing to pause
    // and their repairs nothing to resume; the third repair would itself end past it.
    let r = run(
        &document(
            "213000d",
            &on_demand("cycle_time: 150000d, mtbf: 1d, mttr: 100000d"),
            LINE,
        ),
        1,
    );
    assert_eq!(r.times("m", failure), [t("1d"), t("100002d"), t("200003d")]);
    assert_eq!(r.times("m", repaired), [t("100001d"), t("200002d")]);
    assert_eq!(r.times("m", cycle_done), []);
    assert_eq!(r.delivered().len(), 0);
    assert_eq!(
        r.machine_notes("m").last().unwrap().1.state,
        MachineState::Down
    );
}

#[test]
fn a_cycle_paused_and_resumed_within_the_clock_is_unchanged() {
    // Failure at 100,000 d with 50,000 d left, repair at 100,001 d: the end is 150,001 d,
    // and the next failure, at 200,001 d, is after the horizon.
    let r = run(
        &document(
            "151000d",
            &on_demand("cycle_time: 150000d, mtbf: 100000d, mttr: 1d"),
            LINE,
        ),
        1,
    );
    assert_eq!(r.times("m", failure), [t("100000d")]);
    assert_eq!(r.times("m", cycle_done), [t("150001d")]);
}

#[test]
fn a_changeover_past_the_clock_never_ends() {
    // Two sources, red every 1 s and blue every 2 s. Red is first and free. At 2 s
    // blue's offer is older, so a change of 150,000 d runs, to 150,000 d + 2 s. Blue's
    // cycle ends 1 s later, red waits there, and the change back would pass the clock.
    let body = "  - {id: a, type: Source, interarrival: 1s, variants: {red: 100%}}\n  \
                - {id: b, type: Source, interarrival: 2s, variants: {blue: 100%}}\n  \
                - {id: m, type: Machine, cycle_time: 1s, setup: {per_variant: 150000d}}\n  \
                - {id: sink, type: Sink}";
    let r = run(
        &document("213000d", body, "  - {from: [a, b], to: m}\n  - [m, sink]"),
        1,
    );
    assert_eq!(r.times("m", setup_done), [t("12960000002s")]);
    let (at, last) = *r.machine_notes("m").last().unwrap();
    assert_eq!(at, t("12960000003s"));
    assert_eq!(last.state, MachineState::Setup);
}

/// Every kind of wait that passes the clock is said on the run's `ClockEnd` and
/// reaches the report, so the run layer can tell such a run from a stall (ADR-0024 §3).
#[test]
fn each_wait_past_the_clock_reaches_the_report() {
    let changeover = "  - {id: a, type: Source, interarrival: 1s, variants: {red: 100%}}\n  \
                      - {id: b, type: Source, interarrival: 2s, variants: {blue: 100%}}\n  \
                      - {id: m, type: Machine, cycle_time: 1s, setup: {per_variant: 150000d}}\n  \
                      - {id: sink, type: Sink}";
    let interval = "  - {id: source, type: Source, interarrival: 150000d}\n  \
                    - {id: m, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}";
    let cases = [
        ("cycle", on_demand("cycle_time: 150000d"), LINE),
        ("interval", interval.to_owned(), LINE),
        (
            "up time",
            on_demand("cycle_time: 1000d, mtbf: 150000d, mttr: 1d"),
            LINE,
        ),
        (
            "repair",
            on_demand("cycle_time: 1min, mtbf: 1h, mttr: 150000d"),
            LINE,
        ),
        (
            "rest of a paused cycle",
            on_demand("cycle_time: 150000d, mtbf: 1d, mttr: 100000d"),
            LINE,
        ),
        (
            "changeover",
            changeover.to_owned(),
            "  - {from: [a, b], to: m}\n  - [m, sink]",
        ),
    ];
    for (what, body, flow) in &cases {
        let r = run(&document("213000d", body, flow), 1);
        assert!(r.report.wait_never_ends, "{what}");
    }
}

/// Waits that all fit say nothing, however long, while a wait that ends exactly at
/// the end of the clock happens and the one it starts would not fit.
#[test]
fn waits_that_fit_say_nothing() {
    // Cycles end at 60,000 d and 120,000 d; the third, to 180,000 d, is pending at
    // the horizon.
    let long = document("150000d", &on_demand("cycle_time: 60000d"), LINE);
    // Failure at 30,000 d, repair at 30,001 d, cycle end at 50,001 d. The next cycle
    // is paused at 60,001 d and at 90,002 d, and its rest, resumed at 90,003 d, ends at
    // 100,003 d.
    let paused = document(
        "100000d",
        &on_demand("cycle_time: 50000d, mtbf: 30000d, mttr: 1d"),
        LINE,
    );
    let short = document("1h", &on_demand("cycle_time: 1min"), LINE);
    for model in [&long, &paused, &short] {
        let r = run(model, 1);
        assert_eq!(r.report.summary.outcome, RunOutcome::Horizon, "{model}");
        assert!(!r.report.wait_never_ends, "{model}");
    }
    let paused = run(&paused, 1);
    assert_eq!(paused.times("m", cycle_done), [t("50001d")]);
    assert_eq!(
        paused.times("m", failure),
        [t("30000d"), t("60001d"), t("90002d")]
    );

    let limit = "18446744073.709551615s";
    let r = run(
        &document(limit, &on_demand(&format!("cycle_time: {limit}")), LINE),
        1,
    );
    assert_eq!(r.times("m", cycle_done), [t(limit)]);
    assert!(
        r.report.wait_never_ends,
        "the second cycle would end past it"
    );
}

/// The model of ADR-0024 §3's question: a part arrives at 150,000 d and its cycle of
/// 100,000 d would end past the clock, as would the next interval. Core runs out of
/// events with the part in the machine, and the report says why.
#[test]
fn a_part_held_by_a_wait_past_the_clock_is_reported_with_it() {
    let body = "  - {id: source, type: Source, interarrival: 150000d}\n  \
                - {id: m, type: Machine, cycle_time: 100000d}\n  - {id: sink, type: Sink}";
    let r = run(&document("200000d", body, LINE), 1);
    assert_eq!(r.report.summary.outcome, RunOutcome::Idle);
    assert_eq!(r.report.items.held.get(), 1);
    assert!(r.report.wait_never_ends);
    assert_eq!(r.end, t("200000d"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// In the standard catalog a Source's interval, a cycle or a repair is always
    /// pending (ADR-0024 §3), so a run with items left and no event has a wait past the
    /// clock: none of these runs is a stall.
    #[test]
    fn items_left_with_no_event_always_have_a_wait_past_the_clock(
        cycle in 1_000_u32..213_503,
        up in 1_000_u32..213_503,
        repair in 1_000_u32..213_503,
        interval in 1_000_u32..213_503,
        horizon in 1_000_u32..213_503,
        exponential in any::<bool>(),
        seed in 1_u64..1_000,
    ) {
        let duration = |days: u32| {
            if exponential {
                format!("{{dist: exponential, mean: {days}d}}")
            } else {
                format!("{days}d")
            }
        };
        let body = format!(
            "  - {{id: source, type: Source, interarrival: {}}}\n  \
             - {{id: m, type: Machine, cycle_time: {}, mtbf: {}, mttr: {}}}\n  \
             - {{id: sink, type: Sink}}",
            duration(interval),
            duration(cycle),
            duration(up),
            duration(repair),
        );
        let r = run(&document(&format!("{horizon}d"), &body, LINE), seed);
        let items = r.report.items;
        let left = items.held.get() + items.in_transit.get();
        if r.report.summary.outcome == RunOutcome::Idle {
            prop_assert!(r.report.wait_never_ends, "{} items left", left);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the long durations and the seed, a model that loads runs to its horizon,
    /// and two runs of it are the same bytes.
    #[test]
    fn long_durations_never_fail_a_run(
        cycle in 1_000_u32..213_503,
        up in 1_000_u32..213_503,
        repair in 1_000_u32..213_503,
        interval in 1_000_u32..213_503,
        horizon in 1_000_u32..213_503,
        exponential in any::<bool>(),
        seed in 1_u64..1_000,
    ) {
        let duration = |days: u32| {
            if exponential {
                format!("{{dist: exponential, mean: {days}d}}")
            } else {
                format!("{days}d")
            }
        };
        let body = format!(
            "  - {{id: source, type: Source, interarrival: {}}}\n  \
             - {{id: m, type: Machine, cycle_time: {}, mtbf: {}, mttr: {}}}\n  \
             - {{id: sink, type: Sink}}",
            duration(interval),
            duration(cycle),
            duration(up),
            duration(repair),
        );
        let model = load(&document(&format!("{horizon}d"), &body, LINE));
        // `run_model` replays the trace and fails on anything but a whole run.
        let first = run_model(&model, seed);
        prop_assert_eq!(first.end, t(&format!("{horizon}d")));
        let (again, _) = registry()
            .prepare(&model, RunOptions::new().seed(seed))
            .unwrap()
            .run_traced(Vec::new())
            .unwrap();
        prop_assert!(first.bytes == again);
    }
}
