//! Setups: the first unit, the changeover rule, what is free (ADR-0022 §7).

mod support;

use support::{document, run, t};
use taktsim_components::{FlowMessage, MachineState};

fn setup_done(m: &FlowMessage) -> bool {
    matches!(m, FlowMessage::SetupDone)
}

/// Two sources of one variant each, merged into a machine of capacity 2 with a
/// cycle of 100 s.
fn two_variants(setup: &str) -> String {
    document(
        "340s",
        &format!(
            "  - {{id: a, type: Source, interarrival: 10s, variants: {{red: 100%}}}}\n  \
             - {{id: b, type: Source, interarrival: 15s, variants: {{blue: 100%}}}}\n  \
             - {{id: m, type: Machine, cycle_time: 100s, capacity: 2 parts, setup: {setup}}}\n  \
             - {{id: sink, type: Sink}}"
        ),
        "  - {from: [a, b], to: m}\n  - [m, sink]",
    )
}

#[test]
fn first_unit_costs_no_setup() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, variants: {red: 100%}}\n  \
         - {id: m, type: Machine, cycle_time: 1min, setup: {per_variant: 4min}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    let r = run(&yaml, 1);
    assert_eq!(r.delivered()[0].0, t("1min"));
    assert_eq!(r.times("m", setup_done).len(), 0);
    assert!(
        r.machine_notes("m")
            .iter()
            .all(|(_, n)| n.state != MachineState::Setup)
    );
    assert_eq!(r.report.items.delivered.get(), 60);
}

#[test]
fn changeover_waits_for_units_in_cycle() {
    let r = run(&two_variants("{per_variant: 7s}"), 1);
    // t=10: a red part starts, first unit, free. t=15: a blue part arrives and
    // waits: a red unit is in its cycle. t=110: the red unit finishes; the
    // changeover starts at once, without waiting for it to leave. t=117: blue
    // starts. A red part that arrived at 110 waits for the blue cycle, and its
    // changeover runs from 217 to 224. Blue again from 324 to 331.
    assert_eq!(r.times("m", setup_done), [t("117s"), t("224s"), t("331s")]);
    let delivered: Vec<_> = r.delivered().iter().map(|(time, _)| *time).collect();
    assert_eq!(delivered, [t("110s"), t("217s"), t("324s")]);
    // A setup note names the variant; the state says setup while it runs.
    let notes = r.machine_notes("m");
    let setups: Vec<_> = notes
        .iter()
        .filter(|(_, n)| n.state == MachineState::Setup)
        .collect();
    assert_eq!(setups.len(), 3);
    assert_eq!(setups[0].0, t("110s"));
    let blue = setups[0].1.setup_variant.unwrap();
    assert_eq!(r.variant_name(blue).as_deref(), Some("blue"));
    let red = setups[1].1.setup_variant.unwrap();
    assert_eq!(r.variant_name(red).as_deref(), Some("red"));
    // No unit runs during a changeover.
    assert!(setups.iter().all(|(_, n)| n.units_in_cycle == 0));
}

#[test]
fn zero_matrix_entry_is_no_changeover() {
    let r = run(
        &two_variants(
            "{per_variant: 7s, matrix: [{from: red, to: blue, time: 0s}, {from: blue, to: red, time: 0s}]}",
        ),
        1,
    );
    // No timer and no `setup` state, and a change that costs nothing does not
    // wait: blue starts beside red at 15 s.
    assert_eq!(r.times("m", setup_done).len(), 0);
    let notes = r.machine_notes("m");
    assert!(notes.iter().all(|(_, n)| n.state != MachineState::Setup));
    assert!(notes.iter().any(|(_, n)| n.units_in_cycle == 2));
    let delivered: Vec<_> = r.delivered().iter().map(|(time, _)| *time).collect();
    assert_eq!(delivered[..2], [t("110s"), t("115s")]);
}

#[test]
fn a_pair_with_no_cost_at_all_is_free() {
    // The matrix lists one change and there is no `per_variant`: the other is
    // free.
    let r = run(
        &two_variants("{matrix: [{from: red, to: blue, time: 7s}]}"),
        1,
    );
    // Red to blue: waits for the red cycle, then 7 s, until 117 s. Blue to red:
    // free, so the red part that arrived at 110 s starts beside the blue one at
    // 117 s, and both finish at 217 s. Then red to blue again until 224 s. The
    // red part offered at 227 s comes before the blue one of 232 s, so at 324 s
    // red starts free, and blue waits for its cycle, beyond this run.
    assert_eq!(r.times("m", setup_done), [t("117s"), t("224s")]);
    let notes = r.machine_notes("m");
    assert!(
        notes
            .iter()
            .any(|(time, n)| *time == t("117s") && n.units_in_cycle == 2)
    );
}

#[test]
fn the_matrix_wins_over_per_variant() {
    let r = run(
        &two_variants("{per_variant: 7s, matrix: [{from: red, to: blue, time: 20s}]}"),
        1,
    );
    assert_eq!(r.times("m", setup_done), [t("130s"), t("237s")]);
}

#[test]
fn a_failure_pauses_a_setup_as_it_pauses_a_cycle() {
    // A failure at 115 s, 5 s into the changeover from 110 s: 2 s remain after
    // the repair at 125 s. Set up at 127 s.
    let yaml = document(
        "240s",
        "  - {id: a, type: Source, interarrival: 10s, variants: {red: 100%}}\n  \
         - {id: b, type: Source, interarrival: 15s, variants: {blue: 100%}}\n  \
         - {id: m, type: Machine, cycle_time: 100s, capacity: 2 parts, setup: {per_variant: 7s}, mtbf: 115s, mttr: 10s}\n  \
         - {id: sink, type: Sink}",
        "  - {from: [a, b], to: m}\n  - [m, sink]",
    );
    let r = run(&yaml, 1);
    assert_eq!(r.times("m", setup_done)[0], t("127s"));
    let states: Vec<_> = r
        .machine_notes("m")
        .iter()
        .map(|(time, n)| (*time, n.state))
        .collect();
    assert!(states.contains(&(t("110s"), MachineState::Setup)));
    assert!(states.contains(&(t("115s"), MachineState::Down)));
    assert!(states.contains(&(t("125s"), MachineState::Setup)));
    // Only one setup was delivered for the interrupted changeover.
    assert_eq!(
        r.times("m", setup_done)
            .iter()
            .filter(|time| **time <= t("127s"))
            .count(),
        1
    );
}
