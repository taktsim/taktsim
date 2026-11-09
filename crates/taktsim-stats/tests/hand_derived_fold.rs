//! Independent tests of the KPI fold (ADR-0025 §1 to §3), written from the ADR and the
//! public documentation only. Every expected number is worked out by hand from the
//! times of the facts, in the comment next to it; nothing is taken from a run of the
//! code under test.

use proptest::prelude::*;
use taktsim_stats::{
    Creation, Fact, KpiFold, KpiReport, LeadTimes, PlaceSpec, StatsError, Window, time_weighted,
};
use taktsim_units::SimTime;

const NS: u64 = 1_000_000_000;

/// `s` whole seconds as a time.
fn t(s: u64) -> SimTime {
    SimTime::from_nanos(s * NS)
}

#[allow(clippy::expect_used)] // Test helper.
fn close(found: Option<f64>, expected: f64) {
    let found = found.expect("a value");
    assert!(
        (found - expected).abs() <= 1e-12 * expected.abs().max(1.0),
        "found {found}, expected {expected}"
    );
}

// Places of the hand line: 0 a source (2 states: 0 waiting, 1 blocked), 1 a machine
// (6 states: 0 idle, 1 busy, 2 blocked, 3 starved, 4 down, 5 setup), 2 a sink.
const SOURCE: u32 = 0;
const MACHINE: u32 = 1;
const SINK: u32 = 2;

fn places() -> [PlaceSpec; 3] {
    [
        PlaceSpec {
            states: 2,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 6,
            creation: Creation::Never,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ]
}

/// The hand line, as facts:
///
/// | time | what |
/// | --- | --- |
/// | 0 | source waiting; machine idle, 0 in progress |
/// | 2 | item 1 created at the source and moved to the machine; machine busy, 1 |
/// | 3 | item 2 created at the source; the machine is busy, so it is held; source blocked |
/// | 7 | item 1 to the sink and delivered; item 2 to the machine; source waiting |
/// | 12 | item 2 to the sink and delivered; machine idle |
/// | 15 | item 3 created and moved to the machine; machine busy |
fn hand_line() -> Vec<Fact> {
    vec![
        Fact::State {
            time: t(0),
            resource: SOURCE,
            state: 0,
            in_progress: 0,
        },
        Fact::State {
            time: t(0),
            resource: MACHINE,
            state: 0,
            in_progress: 0,
        },
        Fact::Created {
            time: t(2),
            item: 1,
            by: SOURCE,
        },
        Fact::Moved {
            time: t(2),
            item: 1,
            from: SOURCE,
            to: MACHINE,
        },
        Fact::State {
            time: t(2),
            resource: MACHINE,
            state: 1,
            in_progress: 1,
        },
        Fact::Created {
            time: t(3),
            item: 2,
            by: SOURCE,
        },
        Fact::State {
            time: t(3),
            resource: SOURCE,
            state: 1,
            in_progress: 0,
        },
        Fact::Moved {
            time: t(7),
            item: 1,
            from: MACHINE,
            to: SINK,
        },
        Fact::Delivered {
            time: t(7),
            item: 1,
            by: SINK,
        },
        Fact::Moved {
            time: t(7),
            item: 2,
            from: SOURCE,
            to: MACHINE,
        },
        Fact::State {
            time: t(7),
            resource: SOURCE,
            state: 0,
            in_progress: 0,
        },
        Fact::Moved {
            time: t(12),
            item: 2,
            from: MACHINE,
            to: SINK,
        },
        Fact::Delivered {
            time: t(12),
            item: 2,
            by: SINK,
        },
        Fact::State {
            time: t(12),
            resource: MACHINE,
            state: 0,
            in_progress: 0,
        },
        Fact::Created {
            time: t(15),
            item: 3,
            by: SOURCE,
        },
        Fact::Moved {
            time: t(15),
            item: 3,
            from: SOURCE,
            to: MACHINE,
        },
        Fact::State {
            time: t(15),
            resource: MACHINE,
            state: 1,
            in_progress: 1,
        },
    ]
}

#[allow(clippy::expect_used)] // Test helper.
fn fold(places: &[PlaceSpec], start: SimTime, facts: &[Fact], end: SimTime) -> KpiReport {
    let mut fold = KpiFold::new(places, start);
    for fact in facts {
        fold.push(*fact).expect("a valid fact");
    }
    fold.finish(end).expect("a valid end")
}

#[test]
fn hand_line_over_the_whole_window() {
    let report = fold(&places(), t(0), &hand_line(), t(20));
    let source = report.place(0).unwrap();
    let machine = report.place(1).unwrap();
    let sink = report.place(2).unwrap();

    // Released and not yet disposed: item 1 over [2, 7], item 2 over [7, 12], item 3
    // over [15, 20]: 5 + 5 + 5 = 15 item-seconds over 20 s = 0.75.
    assert_eq!(report.wip_integral(), u128::from(15 * NS));
    close(report.mean_wip(), 0.75);
    // Held: item 2 waits at the source from 3 to 7: 4 item-seconds, 4 / 20 = 0.2.
    assert_eq!(source.held_integral(), u128::from(4 * NS));
    close(source.mean_held(&report), 0.2);
    close(report.mean_held(), 0.2);
    // The source's level is what is at it, held parts included (one table of
    // levels, ADR-0025 §1): item 2 over [3, 7] = 4, so 0.2. Item 1 and item 3 stay
    // there for no time.
    assert_eq!(source.level_integral(), u128::from(4 * NS));
    close(source.mean_level(&report), 0.2);
    // The machine's level: item 1 [2, 7], item 2 [7, 12], item 3 [15, 20] = 15.
    assert_eq!(machine.level_integral(), u128::from(15 * NS));
    close(machine.mean_level(&report), 0.75);
    // The sink holds an item for no time.
    assert_eq!(sink.level_integral(), 0);

    // Machine states: idle [0, 2] and [12, 15] = 5 s; busy [2, 12] and [15, 20] = 15 s.
    assert_eq!(machine.state_nanos(), [5 * NS, 15 * NS, 0, 0, 0, 0]);
    close(machine.state_share(&report, 1), 0.75);
    close(machine.state_share(&report, 0), 0.25);
    // One in progress while busy: 15 / 20.
    close(machine.mean_in_progress(&report), 0.75);
    // Source states: waiting [0, 3] and [7, 20] = 16 s; blocked [3, 7] = 4 s.
    assert_eq!(source.state_nanos(), [16 * NS, 4 * NS]);
    // A place with no states has no rows.
    assert_eq!(sink.state_nanos(), []);

    // Counts: three created at the source, two delivered by the sink.
    assert_eq!(source.created(), 3);
    assert_eq!(sink.delivered(), 2);
    assert_eq!(sink.removed(), 0);
    assert_eq!(machine.delivered(), 0);
    // At the end only item 3 is alive, released, in the machine.
    assert_eq!(report.items_alive(), 1);
    assert_eq!(report.wip_end(), 1);
    assert_eq!(machine.level_end(), 1);
    assert_eq!(source.held_end(), 0);
    // Items 1 and 2 are alive together over [3, 7]; never three at once.
    assert_eq!(report.peak_alive(), 2);

    // Lead times, release to delivery: item 1 2 -> 7 = 5 s; item 2 is released at its
    // first move, 7, and delivered at 12 = 5 s (its 4 s held do not count).
    let lead = sink.lead_times();
    assert_eq!(lead.len(), 2);
    assert_eq!(lead.sum_nanos(), u128::from(10 * NS));
    assert_eq!(lead.mean_secs(), Some(5.0));
    assert_eq!(
        lead.min().map(taktsim_units::SimDuration::as_nanos),
        Some(5 * NS)
    );
    assert_eq!(
        lead.max().map(taktsim_units::SimDuration::as_nanos),
        Some(5 * NS)
    );
    assert!(machine.lead_times().is_empty());
    assert_eq!(report.span().as_nanos(), 20 * NS);
}

#[test]
fn warm_up_start_counts_no_time_before_it() {
    // The same facts with the window from 5 s to 20 s (15 s).
    let report = fold(&places(), t(5), &hand_line(), t(20));
    let source = report.place(0).unwrap();
    let machine = report.place(1).unwrap();
    let sink = report.place(2).unwrap();
    assert_eq!(report.start(), t(5));
    assert_eq!(report.span().as_nanos(), 15 * NS);

    // WIP: item 1 over [5, 7] = 2, item 2 over [7, 12] = 5, item 3 over [15, 20] = 5:
    // 12 item-seconds over 15 s = 0.8.
    assert_eq!(report.wip_integral(), u128::from(12 * NS));
    close(report.mean_wip(), 0.8);
    // Held: item 2 over [5, 7] = 2 item-seconds, 2 / 15.
    assert_eq!(source.held_integral(), u128::from(2 * NS));
    close(report.mean_held(), 2.0 / 15.0);
    // Machine: busy [5, 12] + [15, 20] = 12 s, idle [12, 15] = 3 s; they sum to 15.
    assert_eq!(machine.state_nanos(), [3 * NS, 12 * NS, 0, 0, 0, 0]);
    // Source: blocked [5, 7] = 2 s, waiting [7, 20] = 13 s. The state known at the
    // start of the window (blocked, since 3 s) applies from the start.
    assert_eq!(source.state_nanos(), [13 * NS, 2 * NS]);
    // Both deliveries (7 s, 12 s) fall in the window. A lead time is release to
    // delivery, so item 1's is still 2 -> 7 = 5 s, though it was released before the
    // window: "the lead times of what it delivered in the window, release to delivery".
    assert_eq!(sink.delivered(), 2);
    assert_eq!(sink.lead_times().sum_nanos(), u128::from(10 * NS));
    // Created in the window: only item 3 (15 s); items 1 and 2 were created before.
    assert_eq!(source.created(), 1);
}

#[test]
fn window_starting_after_every_fact_sees_only_the_last_state() {
    // Window [16, 20]: every fact is before it. The state at 16 is what the last facts
    // left: machine busy with item 3, source waiting. Over 4 s: WIP 1 (item 3), held
    // 0, machine busy the whole window, nothing delivered.
    let report = fold(&places(), t(16), &hand_line(), t(20));
    close(report.mean_wip(), 1.0);
    close(report.mean_held(), 0.0);
    let machine = report.place(1).unwrap();
    assert_eq!(machine.state_nanos(), [0, 4 * NS, 0, 0, 0, 0]);
    assert_eq!(report.place(2).unwrap().delivered(), 0);
    assert!(report.place(2).unwrap().lead_times().is_empty());
}

#[test]
fn in_system_creation_is_wip_at_once() {
    // A creator whose items are released as they are created (ADR-0025 §1), then a
    // sink. Item 1 created at 2, moved to the sink at 5 and delivered at 8.
    // WIP: [2, 8] = 6 item-seconds over 10 s = 0.6; held 0; lead time 8 - 2 = 6 s.
    let places = [
        PlaceSpec {
            states: 0,
            creation: Creation::InSystem,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ];
    let facts = [
        Fact::Created {
            time: t(2),
            item: 1,
            by: 0,
        },
        Fact::Moved {
            time: t(5),
            item: 1,
            from: 0,
            to: 1,
        },
        Fact::Delivered {
            time: t(8),
            item: 1,
            by: 1,
        },
    ];
    let report = fold(&places, t(0), &facts, t(10));
    close(report.mean_wip(), 0.6);
    close(report.mean_held(), 0.0);
    assert_eq!(report.place(0).unwrap().held_integral(), 0);
    assert_eq!(
        report.place(1).unwrap().lead_times().sum_nanos(),
        u128::from(6 * NS)
    );
    // The level table: item 1 at place 0 over [2, 5] (3), at place 1 over [5, 8] (3).
    assert_eq!(
        report.place(0).unwrap().level_integral(),
        u128::from(3 * NS)
    );
    assert_eq!(
        report.place(1).unwrap().level_integral(),
        u128::from(3 * NS)
    );
}

#[test]
fn disposed_before_release_touches_no_wip() {
    // A source creates item 1 at 1 and removes it (scrap) at 4, before it ever moved.
    // It was held for 3 s and never released: WIP stays 0, held 3 / 10 = 0.3, no lead
    // time, one removal at the source, nothing delivered.
    let places = [
        PlaceSpec {
            states: 0,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ];
    let facts = [
        Fact::Created {
            time: t(1),
            item: 1,
            by: 0,
        },
        Fact::Removed {
            time: t(4),
            item: 1,
            by: 0,
        },
    ];
    let report = fold(&places, t(0), &facts, t(10));
    close(report.mean_wip(), 0.0);
    assert_eq!(report.wip_integral(), 0);
    close(report.mean_held(), 0.3);
    assert_eq!(report.place(0).unwrap().removed(), 1);
    assert_eq!(report.place(0).unwrap().delivered(), 0);
    assert!(report.place(0).unwrap().lead_times().is_empty());
    assert!(report.place(1).unwrap().lead_times().is_empty());
    assert_eq!(report.items_alive(), 0);
    assert_eq!(report.wip_end(), 0);
}

#[test]
fn removed_after_release_leaves_wip_without_a_lead_time() {
    // Item 1 created at 0, moved to place 1 at 2, scrapped there at 6. WIP over [2, 6]
    // = 4 item-seconds over 10 s; it was not delivered, so no lead time anywhere.
    let places = [
        PlaceSpec {
            states: 0,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ];
    let facts = [
        Fact::Created {
            time: t(0),
            item: 1,
            by: 0,
        },
        Fact::Moved {
            time: t(2),
            item: 1,
            from: 0,
            to: 1,
        },
        Fact::Removed {
            time: t(6),
            item: 1,
            by: 1,
        },
    ];
    let report = fold(&places, t(0), &facts, t(10));
    close(report.mean_wip(), 0.4);
    close(report.mean_held(), 0.2);
    assert_eq!(report.place(1).unwrap().removed(), 1);
    assert!(report.place(1).unwrap().lead_times().is_empty());
}

#[test]
fn units_in_progress_of_a_two_place_machine() {
    // Capacity 2: two units in a cycle over [0, 5], one over [5, 10], both busy.
    // Mean in progress (10 + 5) / 10 = 1.5, which over 2 places is a utilization of
    // 0.75, while the busy share is 1.0 (ADR-0025 §2: busy is not utilization).
    let places = [PlaceSpec {
        states: 6,
        creation: Creation::Never,
    }];
    let facts = [
        Fact::State {
            time: t(0),
            resource: 0,
            state: 1,
            in_progress: 2,
        },
        Fact::State {
            time: t(5),
            resource: 0,
            state: 1,
            in_progress: 1,
        },
    ];
    let report = fold(&places, t(0), &facts, t(10));
    let machine = report.place(0).unwrap();
    close(machine.mean_in_progress(&report), 1.5);
    close(machine.state_share(&report, 1), 1.0);
    // A state the place does not declare has no share.
    assert_eq!(machine.state_share(&report, 6), None);
}

#[test]
fn classes_count_the_last_one_stated() {
    // Dimension 0 (say variant) and dimension 1 (say material). Item 1 is class 2 in
    // dimension 0 and class 0 in dimension 1; item 2 is first class 1, then restated
    // as 3, in dimension 0, and class 0 in dimension 1. Both are delivered.
    // By (dimension, class): (0, 2) 1, (0, 3) 1, (1, 0) 2. Each dimension sums to the
    // two deliveries.
    let places = [
        PlaceSpec {
            states: 0,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ];
    let facts = [
        Fact::Created {
            time: t(0),
            item: 1,
            by: 0,
        },
        Fact::Class {
            item: 1,
            dimension: 0,
            class: 2,
        },
        Fact::Class {
            item: 1,
            dimension: 1,
            class: 0,
        },
        Fact::Created {
            time: t(1),
            item: 2,
            by: 0,
        },
        Fact::Class {
            item: 2,
            dimension: 0,
            class: 1,
        },
        Fact::Class {
            item: 2,
            dimension: 1,
            class: 0,
        },
        Fact::Moved {
            time: t(2),
            item: 1,
            from: 0,
            to: 1,
        },
        Fact::Moved {
            time: t(3),
            item: 2,
            from: 0,
            to: 1,
        },
        Fact::Class {
            item: 2,
            dimension: 0,
            class: 3,
        },
        Fact::Delivered {
            time: t(4),
            item: 1,
            by: 1,
        },
        Fact::Delivered {
            time: t(5),
            item: 2,
            by: 1,
        },
    ];
    let report = fold(&places, t(0), &facts, t(10));
    let by_class: Vec<_> = report
        .place(1)
        .unwrap()
        .delivered_by_class()
        .iter()
        .map(|(k, v)| (*k, *v))
        .collect();
    assert_eq!(by_class, [((0, 2), 1), ((0, 3), 1), ((1, 0), 2)]);
}

#[test]
fn simultaneous_moves_hold_for_no_time() {
    // Item 1 passes three places at one instant, 4 s, and is delivered at 4 s. It was
    // created at 0 and is held 4 s, then released and delivered at once: lead time 0,
    // WIP integral 0, held 4 / 8 = 0.5.
    let places = [
        PlaceSpec {
            states: 0,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ];
    let facts = [
        Fact::Created {
            time: t(0),
            item: 1,
            by: 0,
        },
        Fact::Moved {
            time: t(4),
            item: 1,
            from: 0,
            to: 1,
        },
        Fact::Moved {
            time: t(4),
            item: 1,
            from: 1,
            to: 2,
        },
        Fact::Moved {
            time: t(4),
            item: 1,
            from: 2,
            to: 3,
        },
        Fact::Delivered {
            time: t(4),
            item: 1,
            by: 3,
        },
    ];
    let report = fold(&places, t(0), &facts, t(8));
    assert_eq!(report.wip_integral(), 0);
    close(report.mean_held(), 0.5);
    let lead = report.place(3).unwrap().lead_times();
    assert_eq!(lead.len(), 1);
    assert_eq!(
        lead.max().map(taktsim_units::SimDuration::as_nanos),
        Some(0)
    );
}

#[test]
fn an_item_id_can_be_used_again_after_it_left() {
    // Only an id that is alive is refused (ItemAlreadyAlive). Item 1 is created,
    // delivered, and created again: two creations, one alive at the end.
    let places = [
        PlaceSpec {
            states: 0,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ];
    let facts = [
        Fact::Created {
            time: t(0),
            item: 1,
            by: 0,
        },
        Fact::Moved {
            time: t(1),
            item: 1,
            from: 0,
            to: 1,
        },
        Fact::Delivered {
            time: t(2),
            item: 1,
            by: 1,
        },
        Fact::Created {
            time: t(3),
            item: 1,
            by: 0,
        },
    ];
    let report = fold(&places, t(0), &facts, t(4));
    assert_eq!(report.place(0).unwrap().created(), 2);
    assert_eq!(report.items_alive(), 1);
    assert_eq!(report.place(0).unwrap().held_end(), 1);
}

#[test]
fn empty_window_has_no_time_weighted_value() {
    // Ended at its start: no time in it, so no mean; the counts still exist.
    let report = fold(&places(), t(0), &hand_line()[..2], t(0));
    assert_eq!(report.mean_wip(), None);
    assert_eq!(report.mean_held(), None);
    let machine = report.place(1).unwrap();
    assert_eq!(machine.mean_level(&report), None);
    assert_eq!(machine.mean_in_progress(&report), None);
    assert_eq!(machine.state_share(&report, 0), None);
}

// Refusals: each a typed error naming the item or the place (ADR-0025 §1, "a level
// below zero, or an event for an item that is not at `from`").

#[allow(clippy::unwrap_used, clippy::expect_used)] // Test helper.
fn refused(places: &[PlaceSpec], facts: &[Fact]) -> StatsError {
    let mut fold = KpiFold::new(places, t(0));
    let (last, before) = facts.split_last().unwrap();
    for fact in before {
        fold.push(*fact)
            .expect("a valid fact before the refused one");
    }
    fold.push(*last).expect_err("the last fact is refused")
}

#[test]
fn a_move_of_an_item_never_created_is_refused() {
    let err = refused(
        &places(),
        &[Fact::Moved {
            time: t(1),
            item: 9,
            from: SOURCE,
            to: MACHINE,
        }],
    );
    assert!(
        matches!(err, StatsError::ItemNotAlive { item: 9, .. }),
        "{err:?}"
    );
}

#[test]
fn a_move_from_the_wrong_place_is_refused() {
    // Item 1 is at the source; a move that says it leaves the machine is false.
    let err = refused(
        &places(),
        &[
            Fact::Created {
                time: t(1),
                item: 1,
                by: SOURCE,
            },
            Fact::Moved {
                time: t(2),
                item: 1,
                from: MACHINE,
                to: SINK,
            },
        ],
    );
    assert!(
        matches!(
            err,
            StatsError::ItemElsewhere {
                item: 1,
                stated: MACHINE,
                found: SOURCE,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn a_delivery_by_a_place_the_item_is_not_at_is_refused() {
    let err = refused(
        &places(),
        &[
            Fact::Created {
                time: t(1),
                item: 1,
                by: SOURCE,
            },
            Fact::Moved {
                time: t(2),
                item: 1,
                from: SOURCE,
                to: MACHINE,
            },
            Fact::Delivered {
                time: t(3),
                item: 1,
                by: SINK,
            },
        ],
    );
    assert!(
        matches!(
            err,
            StatsError::ItemElsewhere {
                item: 1,
                stated: SINK,
                found: MACHINE,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn a_delivered_item_cannot_move_again() {
    let err = refused(
        &places(),
        &[
            Fact::Created {
                time: t(1),
                item: 1,
                by: SOURCE,
            },
            Fact::Moved {
                time: t(2),
                item: 1,
                from: SOURCE,
                to: SINK,
            },
            Fact::Delivered {
                time: t(2),
                item: 1,
                by: SINK,
            },
            Fact::Moved {
                time: t(3),
                item: 1,
                from: SINK,
                to: MACHINE,
            },
        ],
    );
    assert!(
        matches!(err, StatsError::ItemNotAlive { item: 1, .. }),
        "{err:?}"
    );
}

#[test]
fn a_place_beyond_the_list_is_refused() {
    let err = refused(
        &places(),
        &[Fact::Created {
            time: t(1),
            item: 1,
            by: 3,
        }],
    );
    assert!(
        matches!(
            err,
            StatsError::UnknownPlace {
                place: 3,
                places: 3
            }
        ),
        "{err:?}"
    );
}

#[test]
fn a_state_beyond_the_declared_ones_is_refused() {
    // The machine declares 6 states, positions 0 to 5.
    let err = refused(
        &places(),
        &[Fact::State {
            time: t(0),
            resource: MACHINE,
            state: 6,
            in_progress: 0,
        }],
    );
    assert!(
        matches!(
            err,
            StatsError::UnknownState {
                place: MACHINE,
                state: 6,
                states: 6
            }
        ),
        "{err:?}"
    );
}

#[test]
fn a_state_for_a_place_with_none_is_refused() {
    let err = refused(
        &places(),
        &[Fact::State {
            time: t(0),
            resource: SINK,
            state: 0,
            in_progress: 0,
        }],
    );
    assert!(
        matches!(
            err,
            StatsError::UnknownState {
                place: SINK,
                state: 0,
                states: 0
            }
        ),
        "{err:?}"
    );
}

#[test]
fn time_going_back_is_refused() {
    let err = refused(
        &places(),
        &[
            Fact::Created {
                time: t(5),
                item: 1,
                by: SOURCE,
            },
            Fact::Created {
                time: t(4),
                item: 2,
                by: SOURCE,
            },
        ],
    );
    assert!(matches!(err, StatsError::TimesOutOfOrder { .. }), "{err:?}");
}

#[test]
fn an_end_before_the_last_fact_is_refused() {
    let mut fold = KpiFold::new(&places(), t(0));
    for fact in hand_line() {
        fold.push(fact).unwrap();
    }
    // The last fact is at 15 s.
    let err = fold.finish(t(14)).unwrap_err();
    assert!(matches!(err, StatsError::TimesOutOfOrder { .. }), "{err:?}");
}

#[test]
fn an_end_before_the_start_is_refused() {
    let err = KpiFold::new(&[], t(5)).finish(t(4)).unwrap_err();
    assert!(matches!(err, StatsError::EmptyWindow { .. }), "{err:?}");
}

#[test]
fn a_place_with_states_that_stated_none_has_no_value() {
    // The machine declares states and states none by the start: refused at the end,
    // since its shares would not sum to the window.
    let only_source = [Fact::State {
        time: t(0),
        resource: SOURCE,
        state: 0,
        in_progress: 0,
    }];
    let mut fold = KpiFold::new(&places(), t(0));
    fold.push(only_source[0]).unwrap();
    let err = fold.finish(t(10)).unwrap_err();
    assert!(matches!(err, StatsError::NoValueAtStart { .. }), "{err:?}");
}

#[test]
fn a_state_stated_after_the_window_start_has_no_value_at_it() {
    // The machine's first state is at 2 s, the window starts at 0: nothing is known
    // over [0, 2], so the report is refused.
    let facts = [
        Fact::State {
            time: t(0),
            resource: SOURCE,
            state: 0,
            in_progress: 0,
        },
        Fact::State {
            time: t(2),
            resource: MACHINE,
            state: 0,
            in_progress: 0,
        },
    ];
    let mut fold = KpiFold::new(&places(), t(0));
    for fact in facts {
        fold.push(fact).unwrap();
    }
    let err = fold.finish(t(10)).unwrap_err();
    assert!(matches!(err, StatsError::NoValueAtStart { .. }), "{err:?}");
}

// Nearest rank, `sorted[⌈p·n⌉ − 1]` (ADR-0025 §3), worked out by hand.

#[test]
fn nearest_rank_on_ten_values() {
    // 1..=10 ns. p50: ⌈5⌉ = 5 -> 5. p90: ⌈9⌉ = 9 -> 9. p95: ⌈9.5⌉ = 10 -> 10.
    // p99: ⌈9.9⌉ = 10 -> 10. p1: ⌈0.1⌉ = 1 -> 1. p100: 10.
    let times = LeadTimes::new((1..=10).rev().collect());
    let p = |q| {
        times
            .percentile(q)
            .map(taktsim_units::SimDuration::as_nanos)
    };
    assert_eq!(p(50), Some(5));
    assert_eq!(p(90), Some(9));
    assert_eq!(p(95), Some(10));
    assert_eq!(p(99), Some(10));
    assert_eq!(p(1), Some(1));
    assert_eq!(p(100), Some(10));
}

#[test]
fn nearest_rank_on_seven_values() {
    // Sorted 10, 20, ..., 70. p14: 14·7 = 98, ⌈0.98⌉ = 1 -> 10. p15: 105, ⌈1.05⌉ = 2
    // -> 20. p28: 196 -> 2 -> 20. p29: 203 -> 3 -> 30. p50: 350 -> 4 -> 40.
    // p90: 630 -> 7 -> 70. p86: 602 -> 7 -> 70. p85: 595 -> 6 -> 60.
    let times = LeadTimes::new(vec![70, 10, 60, 20, 50, 30, 40]);
    let p = |q| {
        times
            .percentile(q)
            .map(taktsim_units::SimDuration::as_nanos)
    };
    assert_eq!(p(14), Some(10));
    assert_eq!(p(15), Some(20));
    assert_eq!(p(28), Some(20));
    assert_eq!(p(29), Some(30));
    assert_eq!(p(50), Some(40));
    assert_eq!(p(85), Some(60));
    assert_eq!(p(86), Some(70));
    assert_eq!(p(90), Some(70));
    assert_eq!(
        times.min().map(taktsim_units::SimDuration::as_nanos),
        Some(10)
    );
    assert_eq!(
        times.max().map(taktsim_units::SimDuration::as_nanos),
        Some(70)
    );
    assert_eq!(times.sum_nanos(), 280);
}

#[test]
fn nearest_rank_on_one_and_three_values() {
    // n = 1: every rank is the one value. n = 3: p50 -> ⌈1.5⌉ = 2, the middle one;
    // p33 -> ⌈0.99⌉ = 1; p34 -> ⌈1.02⌉ = 2; p67 -> ⌈2.01⌉ = 3.
    let one = LeadTimes::new(vec![42]);
    for q in [1, 50, 99, 100] {
        assert_eq!(
            one.percentile(q).map(taktsim_units::SimDuration::as_nanos),
            Some(42)
        );
    }
    let three = LeadTimes::new(vec![300, 100, 200]);
    let p = |q| {
        three
            .percentile(q)
            .map(taktsim_units::SimDuration::as_nanos)
    };
    assert_eq!(p(50), Some(200));
    assert_eq!(p(33), Some(100));
    assert_eq!(p(34), Some(200));
    assert_eq!(p(67), Some(300));
    assert_eq!(p(66), Some(200));
}

#[test]
fn lead_time_sum_does_not_overflow() {
    // Two lead times of u64::MAX ns: their sum is 2·(2^64 − 1), exact in a u128, and
    // the mean is (2^64 − 1) ns = 18,446,744,073.709551615 s.
    let times = LeadTimes::new(vec![u64::MAX, u64::MAX]);
    assert_eq!(times.sum_nanos(), 2 * u128::from(u64::MAX));
    let mean = times.mean_secs().unwrap();
    assert!((mean - 18_446_744_073.709_553).abs() < 1e-3, "{mean}");
}

// Properties, over generated drained runs of a three-stage line: a source holding
// each part from creation to release, two stages, and a sink.

#[derive(Debug, Clone)]
struct Part {
    created: u64,
    // Seconds held, in stage 1, in stage 2.
    held: u64,
    stage1: u64,
    stage2: u64,
}

fn parts() -> impl Strategy<Value = Vec<Part>> {
    prop::collection::vec(
        (0u64..50, 0u64..5, 0u64..7, 0u64..7).prop_map(|(created, held, stage1, stage2)| Part {
            created,
            held,
            stage1,
            stage2,
        }),
        0..30,
    )
}

/// Facts of these parts, in time order; each part's own facts keep their order at a
/// tie, since a stable sort by time keeps the order they were pushed in.
fn facts_of(parts: &[Part]) -> Vec<(u64, usize, Fact)> {
    let mut facts = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let item = i as u64 + 1;
        let released = part.created + part.held;
        let into2 = released + part.stage1;
        let done = into2 + part.stage2;
        facts.push((
            part.created,
            facts.len(),
            Fact::Created {
                time: t(part.created),
                item,
                by: 0,
            },
        ));
        facts.push((
            released,
            facts.len(),
            Fact::Moved {
                time: t(released),
                item,
                from: 0,
                to: 1,
            },
        ));
        facts.push((
            into2,
            facts.len(),
            Fact::Moved {
                time: t(into2),
                item,
                from: 1,
                to: 2,
            },
        ));
        facts.push((
            done,
            facts.len(),
            Fact::Moved {
                time: t(done),
                item,
                from: 2,
                to: 3,
            },
        ));
        facts.push((
            done,
            facts.len(),
            Fact::Delivered {
                time: t(done),
                item,
                by: 3,
            },
        ));
    }
    facts.sort_by_key(|(time, seq, _)| (*time, *seq));
    facts
}

fn line_places() -> [PlaceSpec; 4] {
    [
        PlaceSpec {
            states: 0,
            creation: Creation::HeldUntilFirstMove,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
        PlaceSpec {
            states: 0,
            creation: Creation::Never,
        },
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Little's law in its finite form: on a run that is empty at both ends, the WIP
    /// integral equals the sum of lead times, exactly, in integer nanoseconds. Each
    /// part contributes (delivery − release) to both. Likewise the held integral is
    /// the sum of (release − creation), and the levels of all places sum to both.
    #[test]
    fn drained_run_satisfies_littles_law_exactly(parts in parts()) {
        let facts = facts_of(&parts);
        let end = 50 + 5 + 7 + 7 + 1;
        let report = fold(&line_places(), t(0), &facts.iter().map(|f| f.2).collect::<Vec<_>>(), t(end));
        let lead_sum: u128 = parts.iter().map(|p| u128::from((p.stage1 + p.stage2) * NS)).sum();
        let held_sum: u128 = parts.iter().map(|p| u128::from(p.held * NS)).sum();
        let sink = report.place(3).unwrap();
        prop_assert_eq!(report.wip_integral(), sink.lead_times().sum_nanos());
        prop_assert_eq!(report.wip_integral(), lead_sum);
        prop_assert_eq!(report.place(0).unwrap().held_integral(), held_sum);
        let levels: u128 = report.places().iter().map(taktsim_stats::PlaceKpis::level_integral).sum();
        prop_assert_eq!(levels, lead_sum + held_sum);
        prop_assert_eq!(sink.delivered(), parts.len() as u64);
        prop_assert_eq!(report.items_alive(), 0);
        prop_assert_eq!(sink.lead_times().len(), parts.len());
    }

    /// At every fact, Σ levels = created − disposed; checked by cutting the run after
    /// each prefix and reading `items_alive` and the level ends.
    #[test]
    fn levels_sum_to_created_minus_disposed_at_every_prefix(parts in parts()) {
        let facts = facts_of(&parts);
        for cut in 0..=facts.len() {
            let prefix: Vec<Fact> = facts[..cut].iter().map(|f| f.2).collect();
            let end = facts[..cut].last().map_or(0, |f| f.0);
            let report = fold(&line_places(), t(0), &prefix, t(end));
            let created = prefix.iter().filter(|f| matches!(f, Fact::Created { .. })).count() as u64;
            let disposed = prefix.iter().filter(|f| matches!(f, Fact::Delivered { .. })).count() as u64;
            let levels: u64 = report.places().iter().map(taktsim_stats::PlaceKpis::level_end).sum();
            prop_assert_eq!(levels, created - disposed);
            prop_assert_eq!(report.items_alive(), created - disposed);
            prop_assert_eq!(report.wip_end() + report.place(0).unwrap().held_end(), created - disposed);
        }
    }

    /// The fold's mean WIP agrees with the time-weighted statistics of the same
    /// signal, built independently from the facts (ADR-0025 §1, disagreement 11).
    #[test]
    fn fold_mean_agrees_with_time_weighted(parts in parts(), extra in 1u64..20) {
        let facts = facts_of(&parts);
        let end = 50 + 5 + 7 + 7 + extra;
        let report = fold(&line_places(), t(0), &facts.iter().map(|f| f.2).collect::<Vec<_>>(), t(end));
        // The WIP signal: +1 at each release (move out of place 0), −1 at each delivery.
        let mut changes = vec![(t(0), 0.0)];
        let mut level = 0.0;
        for (time, _, fact) in &facts {
            match fact {
                Fact::Moved { from: 0, .. } => level += 1.0,
                Fact::Delivered { .. } => level -= 1.0,
                Fact::Created { .. }
                | Fact::Moved { .. }
                | Fact::Removed { .. }
                | Fact::State { .. }
                | Fact::Class { .. } => continue,
            }
            match changes.last_mut() {
                Some(last) if last.0 == t(*time) => last.1 = level,
                _ => changes.push((t(*time), level)),
            }
        }
        let expected = time_weighted(&changes, Window::new(t(0), t(end)).unwrap()).unwrap().mean();
        let found = report.mean_wip().unwrap();
        prop_assert!((found - expected).abs() <= 1e-12 * expected.abs().max(1.0), "{} {}", found, expected);
    }

    /// State nanoseconds sum to the window, for any window start, over generated state
    /// changes of one six-state place.
    #[test]
    fn state_nanoseconds_sum_to_the_window(
        changes in prop::collection::vec((0u64..10, 0u16..6), 0..20),
        start in 0u64..60,
        extra in 0u64..10,
    ) {
        let places = [PlaceSpec { states: 6, creation: Creation::Never }];
        let mut facts = vec![Fact::State { time: t(0), resource: 0, state: 0, in_progress: 0 }];
        let mut now = 0;
        for (gap, state) in changes {
            now += gap;
            facts.push(Fact::State { time: t(now), resource: 0, state, in_progress: 0 });
        }
        let end = now.max(start) + extra;
        let report = fold(&places, t(start), &facts, t(end));
        let total: u64 = report.place(0).unwrap().state_nanos().iter().sum();
        prop_assert_eq!(total, (end - start) * NS);
    }
}
