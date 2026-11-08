//! What a domain states for the KPIs, and what its types declare (ADR-0025 §1, §2).

use taktsim_components::{
    ComponentType, Creation, Domain, Flow, FlowMessage, ItemAttrs, Kpi, MachineState,
    MachineStateNote, MessageFact, Registry, Sink, Source, SourceState, SourceStateNote, VariantId,
    standard,
};
use taktsim_core::ItemId;

/// One of each message, by an exhaustive match: a new message does not compile here
/// until this test says what it states.
fn every_message() -> Vec<(FlowMessage, Vec<String>)> {
    let attrs = ItemAttrs::new(VariantId::new(4), 0);
    let messages = [
        FlowMessage::Failure,
        FlowMessage::Repaired,
        FlowMessage::CycleDone {
            item: ItemId::from_raw(1),
        },
        FlowMessage::SetupDone,
        FlowMessage::NextArrival,
        FlowMessage::Arrive(attrs),
        FlowMessage::Accept,
        FlowMessage::Decline,
        FlowMessage::Offer,
        FlowMessage::Withdraw,
        FlowMessage::MachineState(MachineStateNote {
            state: MachineState::Busy,
            units_in_cycle: 2,
            setup_variant: None,
        }),
        FlowMessage::SourceState(SourceStateNote {
            state: SourceState::Blocked,
            held: 7,
        }),
        FlowMessage::VariantName {
            id: VariantId::new(4),
            name: "basic".to_owned(),
        },
    ];
    messages
        .into_iter()
        .map(|message| {
            let expected = match &message {
                FlowMessage::Failure
                | FlowMessage::Repaired
                | FlowMessage::CycleDone { .. }
                | FlowMessage::SetupDone
                | FlowMessage::NextArrival
                | FlowMessage::Accept
                | FlowMessage::Decline
                | FlowMessage::Offer
                | FlowMessage::Withdraw => vec![],
                FlowMessage::Arrive(_) => {
                    vec!["Class { dimension: \"variant\", value: 4 }".to_owned()]
                }
                FlowMessage::MachineState(_) => {
                    vec!["State { name: \"busy\", in_progress: 2 }".to_owned()]
                }
                // A held part is not in progress: `held` comes from the levels.
                FlowMessage::SourceState(_) => {
                    vec!["State { name: \"blocked\", in_progress: 0 }".to_owned()]
                }
                FlowMessage::VariantName { .. } => vec![
                    "ClassName { dimension: \"variant\", value: 4, name: \"basic\" }".to_owned(),
                ],
            };
            (message, expected)
        })
        .collect()
}

#[test]
fn flow_reads_every_message() {
    for (message, expected) in every_message() {
        let mut facts = Vec::new();
        Flow::read_message(&message, &mut |fact: MessageFact<'_>| {
            facts.push(format!("{fact:?}"));
        });
        assert_eq!(facts, expected, "{message:?}");
    }
}

#[test]
fn states_match_the_enums() {
    let registry = standard().unwrap();
    let roles = registry.roles();
    let machine: Vec<&str> = MachineState::ALL.map(MachineState::as_str).to_vec();
    assert_eq!(roles.states("Machine"), machine);
    let source: Vec<&str> = SourceState::ALL.map(SourceState::as_str).to_vec();
    assert_eq!(roles.states("Source"), source);
    assert_eq!(roles.states("Buffer"), [] as [&str; 0]);
    assert_eq!(roles.states("Sink"), [] as [&str; 0]);
    // `ALL` holds each state once, in code order, so a new state must join it.
    for (code, state) in MachineState::ALL.iter().enumerate() {
        assert_eq!(usize::from(state.code()), code);
    }
    assert_eq!(MachineState::from_code(6), None);
    for (code, state) in SourceState::ALL.iter().enumerate() {
        assert_eq!(usize::from(state.code()), code);
    }
    assert_eq!(SourceState::from_code(2), None);
}

/// A state belongs to a type: a type with its own states changes nothing another type
/// declares, whichever order they are registered in.
#[test]
fn a_new_state_changes_no_other_type() {
    #[derive(Debug)]
    struct Oven;
    impl taktsim_model::Encode for Oven {
        fn encode(&self) -> taktsim_model::Json {
            taktsim_model::Json::object([])
        }
    }
    impl taktsim_model::ComponentParams for Oven {
        const TYPE: &'static str = "Oven";
        const EXAMPLE: &'static str = "{id: oven, type: Oven}";
        fn decode(_: &mut taktsim_model::Fields<'_, '_>) -> Option<Self> {
            Some(Self)
        }
    }
    impl ComponentType<Flow> for Oven {
        const STATES: &'static [&'static str] = &["heating", "soaking"];
        const CREATION: Creation = Creation::InSystem;
        fn places(&self) -> u32 {
            4
        }
        fn build(
            &self,
            _: &taktsim_components::Wiring<'_, taktsim_components::FlowShared>,
        ) -> Result<Box<dyn taktsim_core::Component<FlowMessage>>, taktsim_components::BuildError>
        {
            Err(taktsim_components::BuildError::UnknownVariant {
                path: taktsim_model::FieldPath::default(),
                name: "never".to_owned(),
            })
        }
    }
    let before = standard().unwrap();
    let after = standard().unwrap().with::<Oven>().unwrap();
    for type_name in before.catalog().types() {
        assert_eq!(
            before.roles().states(type_name),
            after.roles().states(type_name),
            "{type_name}"
        );
        assert_eq!(
            before.roles().creation(type_name),
            after.roles().creation(type_name),
            "{type_name}"
        );
    }
    assert_eq!(after.roles().states("Oven"), ["heating", "soaking"]);
    assert_eq!(after.roles().creation("Oven"), Some(Creation::InSystem));
}

#[test]
fn what_each_standard_type_creates() {
    assert_eq!(
        <Source as ComponentType<Flow>>::CREATION,
        Creation::HeldUntilFirstMove
    );
    assert_eq!(<Sink as ComponentType<Flow>>::CREATION, Creation::Never);
    let roles = standard().unwrap();
    assert_eq!(roles.roles().creation("Machine"), Some(Creation::Never));
    assert_eq!(roles.roles().creation("Buffer"), Some(Creation::Never));
}

#[test]
fn the_standard_kpis_of_adr_0025() {
    let registry = standard().unwrap();
    let kpis: Vec<(&str, Kpi, Vec<String>)> = registry
        .kpis()
        .map(|(name, signature, kpi)| {
            let accepts = signature
                .accepts()
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default();
            (name, kpi, accepts)
        })
        .collect();
    let words = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
    assert_eq!(
        kpis,
        [
            ("held", Kpi::AwaitingRelease, words(&["Source"])),
            ("lead_time", Kpi::Transit, words(&["Sink"])),
            (
                "state_shares",
                Kpi::StateShares,
                words(&["Machine", "Source"])
            ),
            ("throughput", Kpi::Deliveries, words(&["Sink"])),
            ("utilization", Kpi::InProgress, words(&["Machine"])),
            ("wip", Kpi::Occupancy, words(&["Buffer", "Machine"])),
        ]
    );
    let lone = Registry::<Flow>::new();
    assert_eq!(lone.kpis().count(), 0);
}

#[test]
fn a_kpi_name_is_refused_as_an_observable_name_would_be() {
    use taktsim_model::expr::{Arity, Signature, Type};
    let signature = Signature::new(Arity::Exactly(1), Type::Number);
    let refused = Registry::<Flow>::new().with_kpi("not a name", signature, Kpi::Occupancy);
    assert!(refused.is_err());
}
