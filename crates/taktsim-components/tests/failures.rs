//! Failures: the clock, any state, resume (ADR-0022 §6).

mod support;

use support::{document, is_arrive, run, t};
use taktsim_components::{FlowMessage, MachineState};
use taktsim_core::{ComponentId, TraceRecord, TraceSink};
use taktsim_units::SimTime;

fn failure(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::Failure)
}

fn repaired(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::Repaired)
}

const SATURATED: &str = "  - [source, m, sink]";

fn saturated(machine: &str) -> String {
    format!(
        "  - {{id: source, type: Source}}\n  - {{id: m, type: Machine, {machine}}}\n  - {{id: sink, type: Sink}}"
    )
}

#[test]
fn constant_mtbf_fails_on_schedule() {
    // Up 6 h from time zero, down 12 min, up 6 h from the end of the repair.
    let yaml = document(
        "13h",
        &saturated("cycle_time: 42s, mtbf: 6h, mttr: 12min"),
        SATURATED,
    );
    let r = run(&yaml, 1);
    assert_eq!(r.times("m", failure), [t("6h"), t("12.2h")]);
    assert_eq!(r.times("m", repaired), [t("6.2h"), t("12.4h")]);

    // A shorter run never fails.
    let yaml = document(
        "5h",
        &saturated("cycle_time: 42s, mtbf: 6h, mttr: 12min"),
        SATURATED,
    );
    let r = run(&yaml, 1);
    assert_eq!(r.times("m", failure).len(), 0);
    assert_eq!(
        r.state_shares("m")[usize::from(MachineState::Down.code())],
        0.0
    );
}

#[test]
fn interrupted_cycle_resumes_with_remaining_time() {
    // A 1 h cycle, failing every 25 min of up time, for 5 min: 25 min, down,
    // 25 min, down, 10 min: done at 70 min.
    let yaml = document(
        "71min",
        &saturated("cycle_time: 1h, mtbf: 25min, mttr: 5min"),
        SATURATED,
    );
    let r = run(&yaml, 1);
    assert_eq!(r.times("m", failure), [t("25min"), t("55min")]);
    assert_eq!(r.times("m", repaired), [t("30min"), t("60min")]);
    assert_eq!(r.delivered()[0].0, t("70min"));
    // A paused cycle is not running: 0 units on the down note, back on repair.
    let notes = r.machine_notes("m");
    let down = notes
        .iter()
        .find(|(_, n)| n.state == MachineState::Down)
        .unwrap();
    assert_eq!(down.1.units_in_cycle, 0);
    assert!(notes.iter().any(|(time, n)| *time == t("30min")
        && n.state == MachineState::Busy
        && n.units_in_cycle == 1));
}

#[test]
fn a_failure_at_a_cycle_end_comes_first() {
    // Up 30 min from each repair, 5 min down, and a 1 h cycle: the second
    // failure and the cycle's end fall on 65 min. The failure comes first, so
    // the cycle resumes with nothing remaining when the repair ends.
    let yaml = document(
        "75min",
        &saturated("cycle_time: 1h, mtbf: 30min, mttr: 5min"),
        SATURATED,
    );
    let r = run(&yaml, 1);
    assert_eq!(r.times("m", failure), [t("30min"), t("65min")]);
    assert_eq!(r.delivered()[0].0, t("70min"));
    let done = r.times("m", |m| matches!(m, FlowMessage::CycleDone { .. }));
    assert_eq!(done[0], t("70min"));
}

#[test]
fn failure_while_blocked_or_idle() {
    // Idle: nothing arrives for 10 h, and the clock runs.
    let yaml = document(
        "2h",
        "  - {id: source, type: Source, interarrival: 10h}\n  \
         - {id: m, type: Machine, cycle_time: 1min, mtbf: 1h, mttr: 1min}\n  - {id: sink, type: Sink}",
        SATURATED,
    );
    let r = run(&yaml, 1);
    let states: Vec<_> = r.machine_notes("m").iter().map(|(_, n)| n.state).collect();
    assert_eq!(
        states[..3],
        [MachineState::Idle, MachineState::Down, MachineState::Idle]
    );
    assert_eq!(r.times("m", failure)[0], t("1h"));

    // Blocked: `a` holds a finished part in front of a very slow `b`. It fails,
    // it is repaired, and the part keeps its place.
    let yaml = document(
        "1h",
        "  - {id: source, type: Source}\n  \
         - {id: a, type: Machine, cycle_time: 1min, mtbf: 30min, mttr: 5min}\n  \
         - {id: b, type: Machine, cycle_time: 10h}\n  - {id: sink, type: Sink}",
        "  - [source, a, b, sink]",
    );
    let r = run(&yaml, 1);
    let notes = r.machine_notes("a");
    let states: Vec<_> = notes.iter().map(|(_, n)| n.state).collect();
    assert_eq!(
        states,
        [
            MachineState::Idle,
            MachineState::Busy,
            MachineState::Blocked,
            MachineState::Idle,
            MachineState::Busy,
            MachineState::Blocked,
            MachineState::Down,
            MachineState::Blocked,
        ],
        "{notes:?}"
    );
    assert_eq!(notes[6].0, t("30min"));
    assert_eq!(notes[7].0, t("35min"));
}

#[test]
fn a_down_machine_moves_nothing() {
    // `a` finishes at 1 min and fails at the same instant... later: a finished
    // part waits at `a` while it is down, and `b`'s room stays empty until the
    // repair.
    let yaml = document(
        "30min",
        "  - {id: source, type: Source}\n  \
         - {id: a, type: Machine, cycle_time: 1min, mtbf: 150s, mttr: 2min}\n  \
         - {id: b, type: Machine, cycle_time: 5min}\n  - {id: sink, type: Sink}",
        "  - [source, a, b, sink]",
    );
    let r = run(&yaml, 1);
    let down: Vec<_> = r
        .times("a", failure)
        .into_iter()
        .zip(r.times("a", repaired))
        .collect();
    assert_ne!(down.len(), 0);
    for e in r.events() {
        if is_arrive(&e.message) && (e.from == "a" || e.to == "a") {
            assert!(
                !down.iter().any(|(from, to)| e.time > *from && e.time < *to),
                "{e:?} while down"
            );
        }
    }
}

/// Adds up the time `component` spends in `down`, from its notes alone.
struct DownClock {
    component: ComponentId,
    since: Option<SimTime>,
    total: f64,
}

impl TraceSink<FlowMessage> for DownClock {
    fn record(&mut self, record: &TraceRecord<'_, FlowMessage>) {
        if let TraceRecord::Note {
            time,
            by,
            message: FlowMessage::MachineState(note),
        } = record
            && *by == self.component
        {
            match (note.state, self.since) {
                (MachineState::Down, _) => self.since = Some(*time),
                (_, Some(since)) => {
                    self.total += time.as_secs_f64() - since.as_secs_f64();
                    self.since = None;
                }
                (_, None) => {}
            }
        }
    }
}

/// Down share of `name` in `yaml`, over a long run. MTBF 1 h, MTTR 3 min: the
/// formula gives 4.76 %. With about 1,900 outages, each exponential, the
/// standard error is near 0.15 points, so 0.6 points is four of them.
#[allow(clippy::unwrap_used)] // Test helper.
fn down_share(yaml: &str, name: &str) -> f64 {
    let model = support::load(yaml);
    let registry = support::registry();
    let component = model
        .document()
        .components()
        .iter()
        .position(|c| c.id() == name)
        .map(ComponentId::from_index)
        .unwrap();
    let mut clock = DownClock {
        component,
        since: None,
        total: 0.0,
    };
    let options = taktsim_components::RunOptions::new()
        .seed(2026)
        .without_checks();
    let report = registry
        .prepare(&model, options)
        .unwrap()
        .run(&mut clock)
        .unwrap();
    clock.total / report.summary.now.as_secs_f64()
}

#[test]
fn availability_holds_at_every_load() {
    let failing = "cycle_time: 42s, mtbf: {dist: exponential, mean: 1h}, mttr: {dist: exponential, mean: 3min}";
    let formula = 3.0 / 63.0;
    // Saturated.
    let yaml = document("2000h", &saturated(failing), SATURATED);
    let share = down_share(&yaml, "m");
    assert!((share - formula).abs() < 0.006, "saturated: {share}");
    // Half loaded: parts every 84 s on average, with room for them.
    let yaml = document(
        "2000h",
        &format!(
            "  - {{id: source, type: Source, interarrival: {{dist: exponential, mean: 84s}}}}\n  \
             - {{id: buf, type: Buffer, capacity: 100000 parts}}\n  \
             - {{id: m, type: Machine, {failing}}}\n  - {{id: sink, type: Sink}}"
        ),
        "  - [source, buf, m, sink]",
    );
    let share = down_share(&yaml, "m");
    assert!((share - formula).abs() < 0.006, "half loaded: {share}");
    // Blocked half the time, by a slower station after it.
    let yaml = document(
        "2000h",
        &format!(
            "  - {{id: source, type: Source}}\n  - {{id: m, type: Machine, {failing}}}\n  \
             - {{id: slow, type: Machine, cycle_time: 84s}}\n  - {{id: sink, type: Sink}}"
        ),
        "  - [source, m, slow, sink]",
    );
    let share = down_share(&yaml, "m");
    assert!((share - formula).abs() < 0.006, "blocked: {share}");
}
