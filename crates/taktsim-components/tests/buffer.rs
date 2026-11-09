//! Buffer: fill, reservation and disciplines (ADR-0022 §7).

mod support;

use std::collections::BTreeMap;

use support::{document, t};
use taktsim_components::FlowMessage;
use taktsim_core::ItemId;

/// Two sources with priorities 0 and 1 into a buffer, in front of a slow machine.
fn line(discipline: &str) -> String {
    document(
        "2h",
        &format!(
            "  - {{id: plain, type: Source, interarrival: 10s}}\n  \
             - {{id: rush, type: Source, interarrival: 25s, priority: 1}}\n  \
             - {{id: buf, type: Buffer, capacity: 100 parts, discipline: {discipline}}}\n  \
             - {{id: m, type: Machine, cycle_time: 100s}}\n  - {{id: sink, type: Sink}}"
        ),
        "  - {from: [plain, rush], to: buf}\n  - [buf, m, sink]",
    )
}

/// Replays the buffer from its trace, and checks that each part leaving is the
/// one `rule` picks among what it holds, by (entry number, priority, id).
fn check_rule(yaml: &str, rule: impl Fn(&[(u64, u8, ItemId)]) -> ItemId) -> u64 {
    let r = support::run(yaml, 5);
    let mut held: BTreeMap<ItemId, (u64, u8)> = BTreeMap::new();
    let mut entered = 0;
    let mut left = 0;
    for e in r.events() {
        let (FlowMessage::Arrive(attrs), Some(item)) = (&e.message, e.item) else {
            continue;
        };
        if e.to == "buf" {
            held.insert(item, (entered, attrs.priority));
            entered += 1;
        } else if e.from == "buf" {
            let mut parts: Vec<_> = held.iter().map(|(i, (n, p))| (*n, *p, *i)).collect();
            parts.sort_unstable();
            assert_eq!(item, rule(&parts), "at {}", e.time);
            held.remove(&item);
            left += 1;
        }
    }
    left
}

#[test]
fn fifo_gives_the_first_into_the_buffer() {
    let left = check_rule(&line("fifo"), |parts| parts[0].2);
    assert!(left > 50);
}

#[test]
fn fifo_is_the_default() {
    let left = check_rule(&line("fifo").replace(", discipline: fifo", ""), |parts| {
        parts[0].2
    });
    assert!(left > 50);
}

#[test]
fn lifo_gives_the_last_into_the_buffer() {
    let left = check_rule(&line("lifo"), |parts| parts[parts.len() - 1].2);
    assert!(left > 50);
}

#[test]
fn priority_gives_the_highest_then_the_first_into_the_buffer() {
    let left = check_rule(&line("priority"), |parts| {
        let top = parts.iter().map(|p| p.1).max().unwrap();
        parts.iter().find(|p| p.1 == top).unwrap().2
    });
    assert!(left > 50);
    // The rush parts overtake: higher goes first, so a rush part leaves
    // before the plain part that entered before it.
    let r = support::run(&line("priority"), 5);
    let leaving: Vec<u8> = r
        .events()
        .iter()
        .filter(|e| e.from == "buf")
        .filter_map(|e| match e.message {
            FlowMessage::Arrive(attrs) => Some(attrs.priority),
            FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::CycleDone { .. }
            | FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::Accept
            | FlowMessage::Decline
            | FlowMessage::Offer
            | FlowMessage::Withdraw
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }
            | FlowMessage::Status(_)
            | FlowMessage::OfferOf(_)
            | FlowMessage::Return(_)
            | FlowMessage::RouteDecision(_)
            | FlowMessage::DispatchDecision(_)
            | FlowMessage::EdgeCondition { .. }
            | FlowMessage::Pick => None,
        })
        .collect();
    assert_eq!(leaving[..3], [0, 1, 1]);
}

#[test]
fn fifo_is_entry_order_under_overtaking() {
    // Two parallel paths with different times: parts overtake before the
    // buffer, so entry order differs from creation order, and FIFO must follow
    // entry order.
    let yaml = document(
        "2h",
        "  - {id: source, type: Source, interarrival: 3s}\n  \
         - {id: slow, type: Machine, cycle_time: 7s}\n  \
         - {id: fast, type: Machine, cycle_time: 2s}\n  \
         - {id: buf, type: Buffer, capacity: 50 parts}\n  \
         - {id: m, type: Machine, cycle_time: 20s}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [slow, fast]}\n  - {from: [slow, fast], to: buf}\n  - [buf, m, sink]",
    );
    let r = support::run(&yaml, 1);
    let events = r.events();
    let entering: Vec<ItemId> = events
        .iter()
        .filter(|e| e.to == "buf" && support::is_arrive(&e.message))
        .filter_map(|e| e.item)
        .collect();
    let leaving: Vec<ItemId> = events
        .iter()
        .filter(|e| e.from == "buf" && support::is_arrive(&e.message))
        .filter_map(|e| e.item)
        .collect();
    assert!(leaving.len() > 10);
    assert_eq!(leaving[..], entering[..leaving.len()]);
    // The test means something: ids are out of order at the buffer.
    assert!(entering.windows(2).any(|w| w[0] > w[1]), "{entering:?}");
}

#[test]
fn fill_never_passes_capacity_and_a_full_buffer_blocks() {
    let yaml = document(
        "10min",
        "  - {id: source, type: Source}\n  - {id: a, type: Machine, cycle_time: 1s}\n  \
         - {id: buf, type: Buffer, capacity: 3 parts}\n  \
         - {id: b, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}",
        "  - [source, a, buf, b, sink]",
    );
    let r = support::run(&yaml, 1);
    let mut fill = 0_i64;
    let mut most = 0;
    for e in r.events() {
        if support::is_arrive(&e.message) {
            if e.to == "buf" {
                fill += 1;
            }
            if e.from == "buf" {
                fill -= 1;
            }
            assert!((0..=3).contains(&fill), "{fill} at {}", e.time);
            most = most.max(fill);
        }
    }
    assert_eq!(most, 3);
    // Blocked before the buffer: `a` waits for room.
    assert!(
        r.state_shares("a")[usize::from(taktsim_components::MachineState::Blocked.code())] > 0.5
    );
    assert_eq!(r.delivered()[1].0, t("21s"));
}
