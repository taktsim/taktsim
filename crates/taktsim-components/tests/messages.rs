//! The messages' priorities and encoding: part of the trace contract
//! (ADR-0022 §8, §9).

use taktsim_components::{
    DispatchCandidate, DispatchDecision, FlowMessage, ItemAttrs, MachineState, MachineStateNote,
    OfferedUnit, RouteCandidate, RouteDecision, RouteRule, SourceState, SourceStateNote,
    TargetStatus, VariantId,
};
use taktsim_core::{ComponentId, ItemId, Message};
use taktsim_model::DispatchRule;
use taktsim_trace::{TraceMessage, decode_message, encode_message};
use taktsim_units::SimTime;

fn attrs() -> ItemAttrs {
    ItemAttrs::new(VariantId::new(3), 9)
}

/// One of every message, with its priority and tag.
fn every_message() -> Vec<(FlowMessage, u8, u8)> {
    let machine = MachineStateNote {
        state: MachineState::Setup,
        units_in_cycle: 2,
        setup_variant: Some(VariantId::new(4)),
    };
    let source = SourceStateNote {
        state: SourceState::Blocked,
        held: 6,
    };
    vec![
        (FlowMessage::Failure, 0, 0),
        (FlowMessage::Repaired, 1, 1),
        (
            FlowMessage::CycleDone {
                item: ItemId::from_raw(77),
            },
            2,
            2,
        ),
        (FlowMessage::SetupDone, 2, 3),
        (FlowMessage::NextArrival, 3, 4),
        (FlowMessage::Arrive(attrs()), 4, 5),
        (FlowMessage::Accept, 5, 6),
        (FlowMessage::Decline, 6, 7),
        (FlowMessage::Offer, 7, 8),
        (FlowMessage::MachineState(machine), 8, 9),
        (FlowMessage::SourceState(source), 8, 10),
        (
            FlowMessage::VariantName {
                id: VariantId::new(5),
                name: "premium".to_owned(),
            },
            8,
            11,
        ),
        (FlowMessage::Withdraw, 7, 12),
        (FlowMessage::Status(Box::new(status())), 4, 13),
        (FlowMessage::OfferOf(Box::new(offered())), 7, 14),
        (FlowMessage::Return(attrs()), 4, 15),
        (FlowMessage::RouteDecision(Box::new(route())), 8, 16),
        (FlowMessage::DispatchDecision(Box::new(dispatch())), 8, 17),
        (
            FlowMessage::EdgeCondition {
                edge: 3,
                text: "variant == 'a'".to_owned(),
            },
            8,
            18,
        ),
        (FlowMessage::Pick, 8, 19),
    ]
}

fn status() -> TargetStatus {
    TargetStatus {
        room: true,
        accept_follows: false,
        queue: 7,
        running: 2,
        worked: u128::from(u64::MAX) + 5,
        follows: Some(VariantId::new(4)),
    }
}

fn offered() -> OfferedUnit {
    OfferedUnit {
        item: Some(ItemId::from_raw(9)),
        variant: VariantId::new(1),
        priority: 2,
        due: Some(SimTime::from_nanos(3)),
        since: SimTime::from_nanos(4),
    }
}

fn route() -> RouteDecision {
    RouteDecision {
        rule: RouteRule::ShortestQueue,
        item: ItemId::from_raw(11),
        edge: 1,
        chosen: 1,
        crossed: true,
        detail: 13,
        candidates: vec![
            RouteCandidate {
                id: 2,
                room: Some(false),
                key: 5,
            },
            RouteCandidate {
                id: 3,
                room: Some(true),
                key: 6,
            },
            RouteCandidate {
                id: 4,
                room: None,
                key: 7,
            },
        ],
    }
}

fn dispatch() -> DispatchDecision {
    DispatchDecision {
        rule: DispatchRule::MinimumSetup,
        taker: ComponentId::from_index(5),
        total: 2,
        candidates: vec![
            DispatchCandidate {
                from: ComponentId::from_index(1),
                item: Some(ItemId::from_raw(8)),
                key: 1,
                since: SimTime::from_nanos(2),
            },
            DispatchCandidate {
                from: ComponentId::from_index(2),
                item: None,
                key: 3,
                since: SimTime::from_nanos(4),
            },
        ],
    }
}

#[test]
fn priorities_are_pinned() {
    for (message, priority, _) in every_message() {
        assert_eq!(message.priority(), priority, "{message:?}");
    }
}

#[test]
fn messages_round_trip() {
    assert_eq!(FlowMessage::FORMAT, "taktsim-components/2");
    for (message, _, tag) in every_message() {
        let bytes = encode_message(&message).unwrap();
        assert_eq!(bytes[0], tag, "{message:?}");
        assert_eq!(decode_message::<FlowMessage>(&bytes).unwrap(), message);
    }
    // Without a variant, a machine note round trips too; with a due date, a part.
    let note = FlowMessage::MachineState(MachineStateNote {
        state: MachineState::Idle,
        units_in_cycle: 0,
        setup_variant: None,
    });
    let bytes = encode_message(&note).unwrap();
    assert_eq!(decode_message::<FlowMessage>(&bytes).unwrap(), note);
    let mut due = attrs();
    due.due = Some(SimTime::from_nanos(5));
    let part = FlowMessage::Arrive(due);
    let bytes = encode_message(&part).unwrap();
    assert_eq!(decode_message::<FlowMessage>(&bytes).unwrap(), part);
    // The other values of what the new messages can hold.
    let round = |message: FlowMessage| {
        let bytes = encode_message(&message).unwrap();
        assert_eq!(decode_message::<FlowMessage>(&bytes).unwrap(), message);
    };
    for (room, accept_follows) in [(false, false), (false, true), (true, true)] {
        round(FlowMessage::Status(Box::new(TargetStatus {
            room,
            accept_follows,
            follows: None,
            ..status()
        })));
    }
    round(FlowMessage::OfferOf(Box::new(OfferedUnit {
        item: None,
        due: None,
        ..offered()
    })));
    for rule in [
        RouteRule::When,
        RouteRule::Split,
        RouteRule::FirstFree,
        RouteRule::ShortestQueue,
        RouteRule::LowestUtilization,
        RouteRule::Preferred,
    ] {
        assert_eq!(RouteRule::from_code(rule.code()), Some(rule));
        round(FlowMessage::RouteDecision(Box::new(RouteDecision {
            rule,
            crossed: false,
            ..route()
        })));
    }
    for rule in DispatchRule::ALL {
        round(FlowMessage::DispatchDecision(Box::new(DispatchDecision {
            rule,
            ..dispatch()
        })));
    }
}

#[test]
fn the_rule_codes_are_pinned_and_the_upper_half_is_reserved() {
    let codes = [
        RouteRule::When,
        RouteRule::Split,
        RouteRule::FirstFree,
        RouteRule::ShortestQueue,
        RouteRule::LowestUtilization,
        RouteRule::Preferred,
    ]
    .map(RouteRule::code);
    assert_eq!(codes, [0, 1, 2, 3, 4, 5]);
    for code in 6..=255 {
        assert_eq!(RouteRule::from_code(code), None);
    }
    let dispatch_bytes =
        encode_message(&FlowMessage::DispatchDecision(Box::new(DispatchDecision {
            candidates: Vec::new(),
            ..dispatch()
        })))
        .unwrap();
    assert_eq!(dispatch_bytes[1], 3, "minimum_setup");
    let code = |rule| {
        encode_message(&FlowMessage::DispatchDecision(Box::new(DispatchDecision {
            rule,
            candidates: Vec::new(),
            ..dispatch()
        })))
        .unwrap()[1]
    };
    assert_eq!(DispatchRule::ALL.map(code), [0, 1, 2, 3, 4]);
}

#[test]
fn the_layout_of_the_new_messages_is_pinned() {
    // Status: tag, room, queue u64, running u32, worked 2 x u64, has follows, follows u16.
    assert_eq!(
        encode_message(&FlowMessage::Status(Box::new(TargetStatus {
            room: true,
            accept_follows: false,
            queue: 2,
            running: 3,
            worked: (5_u128 << 64) | 4,
            follows: Some(VariantId::new(6)),
        })))
        .unwrap(),
        [
            13, 1, 2, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0,
            0, 1, 6, 0
        ]
    );
    // OfferOf: tag, has item, item u64, variant u16, priority, has due, due u64, since u64.
    assert_eq!(
        encode_message(&FlowMessage::OfferOf(Box::new(offered()))).unwrap(),
        [
            14, 1, 9, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2, 1, 3, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0,
            0
        ]
    );
    // Return: as Arrive.
    assert_eq!(
        encode_message(&FlowMessage::Return(attrs())).unwrap(),
        [15, 3, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    // RouteDecision: rule, item u64, edge u32, chosen u16, crossed, detail u64, count
    // u16, then id u32, room, key u64.
    let record = RouteDecision {
        candidates: vec![RouteCandidate {
            id: 2,
            room: None,
            key: 5,
        }],
        ..route()
    };
    assert_eq!(
        encode_message(&FlowMessage::RouteDecision(Box::new(record))).unwrap(),
        [
            16, 3, 11, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 1, 13, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2,
            0, 0, 0, 2, 5, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    // DispatchDecision: rule, taker u32, total u64, count u16, then from u32, has item,
    // item u64, key u64, since u64.
    let record = DispatchDecision {
        candidates: vec![dispatch().candidates[0]],
        ..dispatch()
    };
    assert_eq!(
        encode_message(&FlowMessage::DispatchDecision(Box::new(record))).unwrap(),
        [
            17, 3, 5, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0, 0, 0, 0,
            1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0
        ]
    );
    // EdgeCondition: tag, edge u32, text.
    assert_eq!(
        encode_message(&FlowMessage::EdgeCondition {
            edge: 1,
            text: "a".to_owned()
        })
        .unwrap(),
        [18, 1, 0, 0, 0, 1, 0, b'a']
    );
    // Pick: the tag alone.
    assert_eq!(encode_message(&FlowMessage::Pick).unwrap(), [19]);
}

#[test]
fn a_model_without_rules_moves_no_larger_message_than_it_did() {
    // A `VariantId` and a `String`: the largest message of `taktsim-components/1`.
    assert_eq!(
        std::mem::size_of::<FlowMessage>(),
        std::mem::size_of::<(u16, String)>()
    );
}

#[test]
fn the_byte_layout_is_pinned() {
    // Arrive: tag, variant u16, priority u8, has due u8, due u64.
    assert_eq!(
        encode_message(&FlowMessage::Arrive(attrs())).unwrap(),
        [5, 3, 0, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    // MachineState: tag, state, units u32, has variant, variant u16.
    assert_eq!(
        encode_message(&FlowMessage::MachineState(MachineStateNote {
            state: MachineState::Down,
            units_in_cycle: 1,
            setup_variant: None
        }))
        .unwrap(),
        [9, 4, 1, 0, 0, 0, 0, 0, 0]
    );
    // SourceState: tag, state, held u32.
    assert_eq!(
        encode_message(&FlowMessage::SourceState(SourceStateNote {
            state: SourceState::Blocked,
            held: 2
        }))
        .unwrap(),
        [10, 1, 2, 0, 0, 0]
    );
    // VariantName: tag, id u16, text.
    assert_eq!(
        encode_message(&FlowMessage::VariantName {
            id: VariantId::new(1),
            name: "a".to_owned()
        })
        .unwrap(),
        [11, 1, 0, 1, 0, b'a']
    );
}

#[test]
fn unknown_and_inconsistent_bytes_are_rejected() {
    let reject = |bytes: &[u8], text: &str| {
        let e = decode_message::<FlowMessage>(bytes).unwrap_err();
        assert!(e.to_string().contains(text), "{e} for {bytes:?}");
    };
    // The first tag after the last one in use.
    reject(&[20], "FlowMessage tag");
    reject(&[9, 6, 0, 0, 0, 0, 0, 0, 0], "machine state");
    reject(&[10, 2, 0, 0, 0, 0], "source state");
    // A due date on a part that has none, a variant on a note that has none.
    reject(
        &[5, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
        "due date of a part without one",
    );
    reject(
        &[9, 5, 0, 0, 0, 0, 0, 1, 0],
        "setup variant of a note without one",
    );
    // What the new messages can state only one way.
    let mut bytes = encode_message(&FlowMessage::Status(Box::new(status()))).unwrap();
    bytes[1] = 4;
    reject(&bytes, "room");
    let mut bytes = encode_message(&FlowMessage::Status(Box::new(TargetStatus {
        follows: None,
        ..status()
    })))
    .unwrap();
    bytes[31] = 1;
    reject(&bytes, "variant of a status that follows none");
    let mut bytes = encode_message(&FlowMessage::OfferOf(Box::new(OfferedUnit {
        item: None,
        due: None,
        ..offered()
    })))
    .unwrap();
    bytes[2] = 1;
    reject(&bytes, "item of a unit without one");
    bytes[2] = 0;
    bytes[14] = 1;
    reject(&bytes, "due date of a part without one");
    let mut bytes = encode_message(&FlowMessage::RouteDecision(Box::new(route()))).unwrap();
    bytes[1] = 128;
    reject(&bytes, "route rule");
    bytes[1] = 3;
    bytes[31] = 3;
    reject(&bytes, "room of a candidate");
    let mut bytes = encode_message(&FlowMessage::DispatchDecision(Box::new(dispatch()))).unwrap();
    bytes[1] = 128;
    reject(&bytes, "dispatching rule");
    // Bytes left over, and a message cut short.
    assert!(decode_message::<FlowMessage>(&[0, 0]).is_err());
    assert!(decode_message::<FlowMessage>(&[2, 1]).is_err());
}

#[test]
fn state_codes_are_pinned_and_the_table_is_ads_table() {
    use MachineState::{Blocked, Busy, Down, Idle, Setup, Starved};
    let all = [Idle, Busy, Blocked, Starved, Down, Setup];
    for (code, state) in all.iter().enumerate() {
        assert_eq!(usize::from(state.code()), code);
        assert_eq!(MachineState::from_code(state.code()), Some(*state));
    }
    assert_eq!(MachineState::from_code(6), None);
    assert_eq!(Idle.successors(), [Busy, Setup, Down]);
    assert_eq!(Starved.successors(), [Busy, Setup, Down]);
    assert_eq!(Setup.successors(), [Busy, Down]);
    assert_eq!(Busy.successors(), [Blocked, Setup, Down]);
    assert_eq!(Blocked.successors(), [Busy, Setup, Idle, Starved, Down]);
    assert_eq!(Down.successors(), [Idle, Starved, Busy, Blocked, Setup]);
    assert_eq!(
        all.map(MachineState::as_str),
        ["idle", "busy", "blocked", "starved", "down", "setup"]
    );
    assert_eq!(SourceState::Waiting.as_str(), "waiting");
    assert_eq!(SourceState::from_code(2), None);
}
