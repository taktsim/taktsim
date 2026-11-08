//! The KPI fold on facts written by hand, with no engine and no domain (ADR-0025 §1,
//! §12). Expected numbers are worked out in comments from the times.

use proptest::prelude::*;
use taktsim_stats::{
    Creation, Fact, KpiFold, KpiReport, LeadTimes, PlaceSpec, StatsError, TimeWeightedAccumulator,
    Window,
};
use taktsim_units::SimTime;

const SOURCE: u32 = 0;
const BUFFER: u32 = 1;
const SINK: u32 = 2;

fn t(seconds: u64) -> SimTime {
    SimTime::from_nanos(seconds * 1_000_000_000)
}

fn line() -> [PlaceSpec; 3] {
    [
        PlaceSpec {
            states: 2,
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
    ]
}

fn created(seconds: u64, item: u64) -> Fact {
    Fact::Created {
        time: t(seconds),
        item,
        by: SOURCE,
    }
}

fn moved(seconds: u64, item: u64, from: u32, to: u32) -> Fact {
    Fact::Moved {
        time: t(seconds),
        item,
        from,
        to,
    }
}

fn delivered(seconds: u64, item: u64) -> Fact {
    Fact::Delivered {
        time: t(seconds),
        item,
        by: SINK,
    }
}

fn state(seconds: u64, state: u16, in_progress: u32) -> Fact {
    Fact::State {
        time: t(seconds),
        resource: SOURCE,
        state,
        in_progress,
    }
}

#[allow(clippy::unwrap_used)] // Test helper.
fn fold_of(start: u64, facts: &[Fact], end: u64) -> KpiReport {
    let mut fold = KpiFold::new(&line(), t(start));
    for fact in facts {
        fold.push(*fact).unwrap();
    }
    fold.finish(t(end)).unwrap()
}

/// Item 1 is held for 2 s, in the buffer for 3 s, delivered at 5 s. Item 2 is held
/// from 4 s to 6 s and inside at the end.
fn two_items() -> Vec<Fact> {
    vec![
        state(0, 0, 0),
        created(0, 1),
        moved(2, 1, SOURCE, BUFFER),
        state(4, 1, 1),
        created(4, 2),
        moved(5, 1, BUFFER, SINK),
        delivered(5, 1),
        moved(6, 2, SOURCE, BUFFER),
        state(6, 0, 0),
    ]
}

#[test]
fn levels_held_and_wip_by_hand() {
    let report = fold_of(0, &two_items(), 10);
    // Held: item 1 for 0..2 and item 2 for 4..6, so 4 s over 10 s.
    let source = report.place(0).unwrap();
    assert_eq!(source.mean_held(&report), Some(0.4));
    assert_eq!(source.held_end(), 0);
    // The source's level is what it holds.
    assert_eq!(source.mean_level(&report), Some(0.4));
    // In the buffer: item 1 for 2..5 and item 2 for 6..10, so 7 s.
    let buffer = report.place(1).unwrap();
    assert_eq!(buffer.mean_level(&report), Some(0.7));
    assert_eq!(buffer.level_end(), 1);
    // WIP equals the buffer here, since moving to the sink and delivering are one
    // instant.
    assert_eq!(report.mean_wip(), Some(0.7));
    assert_eq!(report.wip_end(), 1);
    assert_eq!(report.items_alive(), 1);
    assert_eq!(report.peak_alive(), 2);
    // Lead time from release (2 s) to delivery (5 s).
    let sink = report.place(2).unwrap();
    assert_eq!(sink.delivered(), 1);
    assert_eq!(sink.lead_times().min().unwrap().as_nanos(), 3_000_000_000);
    assert_eq!(sink.lead_times().mean_secs(), Some(3.0));
    // State 0 for 0..4 and 6..10, state 1 for 4..6; in progress 1 for 2 s.
    assert_eq!(source.state_nanos(), [8_000_000_000_u64, 2_000_000_000_u64]);
    assert_eq!(source.mean_in_progress(&report), Some(0.2));
    assert_eq!(source.state_share(&report, 1), Some(0.2));
}

#[test]
fn nearest_rank_quantiles_are_values_that_occurred() {
    let times = LeadTimes::new(vec![50, 10, 40, 20, 30]);
    // ⌈0.5·5⌉ = 3, so the third value; ⌈0.9·5⌉ = 5.
    assert_eq!(times.percentile(50).unwrap().as_nanos(), 30);
    assert_eq!(times.percentile(90).unwrap().as_nanos(), 50);
    assert_eq!(times.percentile(1).unwrap().as_nanos(), 10);
    assert_eq!(times.min().unwrap().as_nanos(), 10);
    assert_eq!(times.max().unwrap().as_nanos(), 50);
    assert_eq!(LeadTimes::new(vec![]).percentile(50), None);
    assert_eq!(LeadTimes::new(vec![]).mean_secs(), None);
}

/// The window's end comes only with `finish`, so the same fold can end anywhere after
/// its last fact, and its integrals extend by the last level.
#[test]
fn one_pass_needs_no_end() {
    let mut fold = KpiFold::new(&line(), SimTime::START);
    for fact in two_items() {
        fold.push(fact).unwrap();
    }
    let at_ten = fold.clone().finish(t(10)).unwrap();
    let at_twenty = fold.finish(t(20)).unwrap();
    // One item stays inside: 10 s more is one item for 10 s.
    assert_eq!(
        at_twenty.wip_integral() - at_ten.wip_integral(),
        10_000_000_000
    );
    assert_eq!(at_ten.end(), t(10));
    assert_eq!(at_twenty.end(), t(20));
    assert_eq!(at_twenty.span().as_nanos(), 20_000_000_000);
    assert_eq!(at_twenty.start(), SimTime::START);
}

/// An empty window goes through the same path and gives no time-weighted value.
#[test]
fn cut_at_zero_goes_through_the_fold() {
    let report = fold_of(0, &[state(0, 0, 0), created(0, 1)], 0);
    assert_eq!(report.mean_wip(), None);
    let source = report.place(0).unwrap();
    assert_eq!(source.mean_held(&report), None);
    assert_eq!(source.mean_level(&report), None);
    assert_eq!(source.mean_in_progress(&report), None);
    assert_eq!(source.state_share(&report, 0), None);
    // Facts are still counted: this is the document's items_alive.
    assert_eq!(report.items_alive(), 1);
    // Nor does an empty window need a state.
    let none = KpiFold::new(&line(), SimTime::START)
        .finish(SimTime::START)
        .unwrap();
    assert_eq!(none.items_alive(), 0);
}

/// `TimeWeightedAccumulator` needs its window when built, so the fold does not use it
/// (ADR-0025 disagreement 11); on the same signal their means agree.
#[test]
fn fold_mean_agrees_with_time_weighted_accumulator() {
    let report = fold_of(0, &two_items(), 10);
    let mut accumulator = TimeWeightedAccumulator::new(Window::new(t(0), t(10)).unwrap());
    // The system WIP of `two_items`: 1 from 2 s, 0 from 5 s, 1 from 6 s.
    for (seconds, value) in [(0, 0.0), (2, 1.0), (5, 0.0), (6, 1.0)] {
        accumulator.record(t(seconds), value).unwrap();
    }
    let mean = accumulator.finish().unwrap().mean();
    assert!((report.mean_wip().unwrap() - mean).abs() <= 1e-12 * mean);
}

/// A later window over the same facts is what is left of the whole after the part
/// before it, in exact integers.
#[test]
fn fold_from_start_equals_the_cut_window() {
    let facts = two_items();
    let whole = fold_of(0, &facts, 10);
    let split = 3;
    let before: Vec<Fact> = facts
        .iter()
        .copied()
        .filter(|fact| fact_time(fact) < t(split))
        .collect();
    let first = fold_of(0, &before, split);
    let rest = fold_of(split, &facts, 10);
    assert_eq!(
        whole.wip_integral(),
        first.wip_integral() + rest.wip_integral()
    );
    for index in 0..3 {
        let (w, a, b) = (
            whole.place(index).unwrap(),
            first.place(index).unwrap(),
            rest.place(index).unwrap(),
        );
        assert_eq!(w.level_integral(), a.level_integral() + b.level_integral());
        assert_eq!(w.held_integral(), a.held_integral() + b.held_integral());
        assert_eq!(w.delivered(), a.delivered() + b.delivered());
        for state in 0..w.state_nanos().len() {
            assert_eq!(
                w.state_nanos()[state],
                a.state_nanos()[state] + b.state_nanos()[state]
            );
        }
    }
    // Item 1 was released before the window and still has its whole lead time.
    assert_eq!(rest.place(2).unwrap().lead_times().mean_secs(), Some(3.0));
    // From 3 s: item 1 inside until 5 s, item 2 from 6 s to 10 s; 6 s over 7 s.
    assert_eq!(rest.mean_wip(), Some(6.0 / 7.0));
}

fn fact_time(fact: &Fact) -> SimTime {
    match fact {
        Fact::Created { time, .. }
        | Fact::Moved { time, .. }
        | Fact::Delivered { time, .. }
        | Fact::Removed { time, .. }
        | Fact::State { time, .. } => *time,
        Fact::Class { .. } => SimTime::START,
    }
}

#[test]
fn created_by_a_never_type_is_conservation() {
    let mut fold = KpiFold::new(&line(), SimTime::START);
    let refused = fold.push(Fact::Created {
        time: t(1),
        item: 1,
        by: BUFFER,
    });
    assert_eq!(
        refused,
        Err(StatsError::CreatedAtNoCreator {
            item: 1,
            by: BUFFER,
            time: t(1)
        })
    );
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("conservation"), "{message}");
    assert!(message.contains("example:"), "{message}");
}

#[test]
fn created_in_system_is_wip_at_once() {
    let places = [PlaceSpec {
        states: 0,
        creation: Creation::InSystem,
    }];
    let mut fold = KpiFold::new(&places, SimTime::START);
    fold.push(Fact::Created {
        time: t(0),
        item: 1,
        by: 0,
    })
    .unwrap();
    fold.push(Fact::Delivered {
        time: t(4),
        item: 1,
        by: 0,
    })
    .unwrap();
    let report = fold.finish(t(10)).unwrap();
    assert_eq!(report.mean_wip(), Some(0.4));
    let place = report.place(0).unwrap();
    assert_eq!(place.mean_held(&report), Some(0.0));
    assert_eq!(place.lead_times().mean_secs(), Some(4.0));
}

#[test]
fn disposed_before_release_touches_no_wip() {
    let facts = [
        state(0, 0, 0),
        created(1, 1),
        Fact::Removed {
            time: t(3),
            item: 1,
            by: SOURCE,
        },
    ];
    let report = fold_of(0, &facts, 10);
    assert_eq!(report.mean_wip(), Some(0.0));
    assert_eq!(report.wip_end(), 0);
    let source = report.place(0).unwrap();
    assert_eq!(source.mean_held(&report), Some(0.2));
    assert_eq!(source.removed(), 1);
    assert!(source.lead_times().is_empty());
    assert_eq!(report.items_alive(), 0);

    // Delivered before release: counted as delivered, with no lead time.
    let facts = [
        state(0, 0, 0),
        created(1, 1),
        Fact::Delivered {
            time: t(3),
            item: 1,
            by: SOURCE,
        },
    ];
    let report = fold_of(0, &facts, 10);
    let source = report.place(0).unwrap();
    assert_eq!(source.delivered(), 1);
    assert!(source.lead_times().is_empty());
    assert_eq!(report.mean_wip(), Some(0.0));
}

#[test]
fn what_breaks_conservation_or_time_is_refused() {
    let mut fold = KpiFold::new(&line(), SimTime::START);
    fold.push(created(1, 1)).unwrap();
    assert_eq!(
        fold.push(created(1, 1)),
        Err(StatsError::ItemAlreadyAlive {
            item: 1,
            by: SOURCE,
            time: t(1)
        })
    );
    let mut fold = KpiFold::new(&line(), SimTime::START);
    assert!(matches!(
        fold.push(moved(1, 9, SOURCE, BUFFER)),
        Err(StatsError::ItemNotAlive { item: 9, .. })
    ));
    assert!(matches!(
        fold.push(delivered(1, 9)),
        Err(StatsError::ItemNotAlive { item: 9, .. })
    ));
    assert!(matches!(
        fold.push(Fact::Class {
            item: 9,
            dimension: 0,
            class: 0
        }),
        Err(StatsError::ItemNotAlive { item: 9, .. })
    ));
    fold.push(created(2, 1)).unwrap();
    assert!(matches!(
        fold.push(moved(2, 1, BUFFER, SINK)),
        Err(StatsError::ItemElsewhere {
            item: 1,
            stated: BUFFER,
            found: SOURCE,
            ..
        })
    ));
    assert!(matches!(
        fold.push(delivered(2, 1)),
        Err(StatsError::ItemElsewhere { .. })
    ));
    assert!(matches!(
        fold.push(moved(2, 2, SOURCE, 9)),
        Err(StatsError::ItemNotAlive { .. })
    ));
    let mut fold = KpiFold::new(&line(), SimTime::START);
    fold.push(created(2, 1)).unwrap();
    assert!(matches!(
        fold.push(moved(2, 1, SOURCE, 9)),
        Err(StatsError::UnknownPlace {
            place: 9,
            places: 3
        })
    ));
    assert!(matches!(
        fold.push(created(1, 2)),
        Err(StatsError::TimesOutOfOrder { .. })
    ));
    let mut fold = KpiFold::new(&line(), SimTime::START);
    assert!(matches!(
        fold.push(Fact::Created {
            time: t(0),
            item: 1,
            by: 7
        }),
        Err(StatsError::UnknownPlace { place: 7, .. })
    ));
    assert_eq!(
        fold.push(state(0, 5, 0)),
        Err(StatsError::UnknownState {
            place: SOURCE,
            state: 5,
            states: 2
        })
    );
}

#[test]
fn an_end_before_the_start_or_the_last_fact_is_refused() {
    let fold = KpiFold::new(&line(), t(5));
    assert!(matches!(
        fold.finish(t(4)),
        Err(StatsError::EmptyWindow { .. })
    ));
    let mut fold = KpiFold::new(&line(), SimTime::START);
    fold.push(state(6, 0, 0)).unwrap();
    assert!(matches!(
        fold.finish(t(4)),
        Err(StatsError::TimesOutOfOrder { .. })
    ));
}

/// A place that declares states and has none at the start of the window would break
/// "state nanoseconds sum to the window", so it is refused, not reported short.
#[test]
fn a_state_must_be_known_at_the_start_of_the_window() {
    let mut fold = KpiFold::new(&line(), SimTime::START);
    fold.push(state(3, 0, 0)).unwrap();
    assert!(matches!(
        fold.finish(t(10)),
        Err(StatsError::NoValueAtStart { .. })
    ));
    let fold = KpiFold::new(&line(), SimTime::START);
    assert!(matches!(
        fold.finish(t(10)),
        Err(StatsError::NoValueAtStart { first: None, .. })
    ));
    // Stated before a later window starts: known at its start.
    let mut fold = KpiFold::new(&line(), t(5));
    fold.push(state(3, 1, 0)).unwrap();
    let report = fold.finish(t(10)).unwrap();
    assert_eq!(
        report.place(0).unwrap().state_nanos(),
        [0_u64, 5_000_000_000_u64]
    );
}

#[test]
fn classes_follow_the_item_and_the_last_one_counts() {
    let places = line();
    let mut fold = KpiFold::new(&places, SimTime::START);
    fold.push(state(0, 0, 0)).unwrap();
    fold.push(created(0, 1)).unwrap();
    for (dimension, class) in [(0, 3), (1, 4), (0, 5)] {
        fold.push(Fact::Class {
            item: 1,
            dimension,
            class,
        })
        .unwrap();
    }
    fold.push(moved(1, 1, SOURCE, SINK)).unwrap();
    fold.push(delivered(1, 1)).unwrap();
    let report = fold.finish(t(2)).unwrap();
    let classes = report.place(2).unwrap().delivered_by_class();
    assert_eq!(classes.get(&(0, 5)), Some(&1));
    assert_eq!(classes.get(&(1, 4)), Some(&1));
    assert_eq!(classes.get(&(0, 3)), None);
}

/// One plan for a random run on a line of four places: for each item, when it is
/// created and how long it stays at each stop.
#[derive(Debug, Clone)]
struct ItemPlan {
    created: u64,
    stays: [u64; 3],
    class: u16,
    second: u16,
    disposed: bool,
}

fn item_plan() -> impl Strategy<Value = ItemPlan> {
    (
        0_u64..1_000,
        [0_u64..100, 0_u64..100, 0_u64..100],
        0_u16..3,
        0_u16..2,
        prop::bool::weighted(0.9),
    )
        .prop_map(|(created, stays, class, second, disposed)| ItemPlan {
            created,
            stays,
            class,
            second,
            disposed,
        })
}

/// The facts of these plans, in time order. Item `i` goes source, 1, 2, sink, and
/// is delivered there, or stays alive if not `disposed`.
#[allow(clippy::unwrap_used)] // Test helper.
fn facts_of(plans: &[ItemPlan]) -> (Vec<(u64, u8, Fact)>, u64) {
    let mut facts = Vec::new();
    let mut last = 0;
    for (index, plan) in plans.iter().enumerate() {
        let item = index as u64;
        let mut time = plan.created;
        facts.push((
            time,
            0,
            Fact::Created {
                time: t(time),
                item,
                by: 0,
            },
        ));
        facts.push((
            time,
            0,
            Fact::Class {
                item,
                dimension: 0,
                class: plan.class,
            },
        ));
        facts.push((
            time,
            0,
            Fact::Class {
                item,
                dimension: 1,
                class: plan.second,
            },
        ));
        for (stop, stay) in plan.stays.iter().enumerate() {
            time += stay;
            let from = u32::try_from(stop).unwrap();
            facts.push((
                time,
                1,
                Fact::Moved {
                    time: t(time),
                    item,
                    from,
                    to: from + 1,
                },
            ));
        }
        if plan.disposed {
            facts.push((
                time,
                2,
                Fact::Delivered {
                    time: t(time),
                    item,
                    by: 3,
                },
            ));
        }
        last = last.max(time);
    }
    // In time order; at one instant an item's own facts stay in their order.
    facts.sort_by_key(|(time, order, fact)| (*time, item_of(fact), *order));
    (facts, last)
}

fn item_of(fact: &Fact) -> u64 {
    match fact {
        Fact::Created { item, .. }
        | Fact::Moved { item, .. }
        | Fact::Delivered { item, .. }
        | Fact::Removed { item, .. }
        | Fact::Class { item, .. } => *item,
        Fact::State { .. } => 0,
    }
}

fn four_places() -> [PlaceSpec; 4] {
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
    /// At every fact, Σ levels = created − disposed; at the end, Σ levels = items
    /// alive = wip_end + Σ held_end.
    #[test]
    fn levels_conserve_items(plans in prop::collection::vec(item_plan(), 0..40)) {
        let (facts, last) = facts_of(&plans);
        let places = four_places();
        let mut fold = KpiFold::new(&places, SimTime::START);
        let (mut created, mut disposed) = (0_u64, 0_u64);
        for (_, _, fact) in &facts {
            match fact {
                Fact::Created { .. } => created += 1,
                Fact::Delivered { .. } | Fact::Removed { .. } => disposed += 1,
                Fact::Moved { .. } | Fact::State { .. } | Fact::Class { .. } => {}
            }
            fold.push(*fact).unwrap();
            let report = fold.clone().finish(t(last + 1)).unwrap();
            let levels: u64 = report.places().iter().map(taktsim_stats::PlaceKpis::level_end).sum();
            let held: u64 = report.places().iter().map(taktsim_stats::PlaceKpis::held_end).sum();
            prop_assert_eq!(levels, created - disposed);
            prop_assert_eq!(report.items_alive(), levels);
            prop_assert_eq!(report.wip_end() + held, levels);
        }
    }

    /// State nanoseconds sum to the window, in any window, with any changes.
    #[test]
    fn state_nanoseconds_sum_to_the_window(
        changes in prop::collection::vec((0_u64..50, 0_u16..4, 0_u32..3), 1..40),
        start in 0_u64..10,
        extra in 1_u64..20,
    ) {
        let places = [PlaceSpec { states: 4, creation: Creation::Never }];
        let mut fold = KpiFold::new(&places, t(start));
        let mut time = 0;
        fold.push(Fact::State { time: t(0), resource: 0, state: 0, in_progress: 0 }).unwrap();
        for (gap, state, in_progress) in &changes {
            time += gap;
            fold.push(Fact::State { time: t(time), resource: 0, state: *state, in_progress: *in_progress }).unwrap();
        }
        let end = time.max(start) + extra;
        let report = fold.finish(t(end)).unwrap();
        let total: u64 = report.place(0).unwrap().state_nanos().iter().sum();
        prop_assert_eq!(total, report.span().as_nanos());
    }

    /// Delivered by class sums to delivered in each dimension.
    #[test]
    fn classes_sum_to_the_total_per_dimension(plans in prop::collection::vec(item_plan(), 0..40)) {
        let (facts, last) = facts_of(&plans);
        let places = four_places();
        let mut fold = KpiFold::new(&places, SimTime::START);
        for (_, _, fact) in &facts {
            fold.push(*fact).unwrap();
        }
        let report = fold.finish(t(last + 1)).unwrap();
        let sink = report.place(3).unwrap();
        for dimension in 0..2 {
            let sum: u64 = sink
                .delivered_by_class()
                .iter()
                .filter(|((d, _), _)| *d == dimension)
                .map(|(_, n)| *n)
                .sum();
            prop_assert_eq!(sum, sink.delivered());
        }
    }

    /// Little's law, finite form, exact in integers: when every released item is
    /// delivered by the end, the integral of WIP is the sum of the lead times.
    #[test]
    fn wip_integral_equals_lead_times(plans in prop::collection::vec(item_plan(), 0..40)) {
        let drained: Vec<ItemPlan> = plans
            .into_iter()
            .map(|plan| ItemPlan { disposed: true, ..plan })
            .collect();
        let (facts, last) = facts_of(&drained);
        let places = four_places();
        let mut fold = KpiFold::new(&places, SimTime::START);
        for (_, _, fact) in &facts {
            fold.push(*fact).unwrap();
        }
        let report = fold.finish(t(last + 1)).unwrap();
        prop_assert_eq!(report.wip_end(), 0);
        let sink = report.place(3).unwrap();
        prop_assert_eq!(report.wip_integral(), sink.lead_times().sum_nanos());
    }
}
