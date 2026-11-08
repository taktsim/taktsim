//! The messages' priorities and encoding: part of the trace contract
//! (ADR-0022 §8, §9).

use taktsim_components::{
    FlowMessage, ItemAttrs, MachineState, MachineStateNote, SourceState, SourceStateNote, VariantId,
};
use taktsim_core::{ItemId, Message};
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
    ]
}

#[test]
fn priorities_are_pinned() {
    for (message, priority, _) in every_message() {
        assert_eq!(message.priority(), priority, "{message:?}");
    }
}

#[test]
fn messages_round_trip() {
    assert_eq!(FlowMessage::FORMAT, "taktsim-components/1");
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
    reject(&[13], "FlowMessage tag");
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
