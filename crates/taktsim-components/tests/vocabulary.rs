//! The registry: every type is registered, one call fills catalog and builder,
//! and the shared part of the format stays free of the factory (ADR-0022 §11, §12).

#[path = "../../taktsim-model/tests/support/schema_checker.rs"]
mod schema_checker;
mod support;

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;
use schema_checker::Checker;
use support::{document, load, registry, run};
use taktsim_components::{
    BuildError, ComponentType, Flow, FlowMessage, FlowShared, ItemAttrs, MachineState, Ports,
    Registry, SetupMeans, StationFacts, StatusFact, Wiring, standard,
};
use taktsim_core::{Component, ComponentError, ComponentId, Context, Delivery, ItemId};
use taktsim_model::{
    ComponentParams, Encode, Fields, Format, Json, Loader, RoutingWords, model_schema,
};

/// A hand-written behaviour on the public messages: takes one part at a time and
/// passes it on at once.
#[derive(Default)]
struct PassOn {
    downstream: String,
    to: Option<ComponentId>,
    holding: Option<(ItemId, ItemAttrs)>,
    upstream: Option<ComponentId>,
    offered: bool,
    reserved: bool,
}

impl PassOn {
    fn settle(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if self.holding.is_some() && !self.offered {
            self.offered = true;
            if let Some(to) = self.to {
                ctx.send(to, FlowMessage::Offer)?;
            }
        }
        if self.holding.is_none()
            && !self.reserved
            && let Some(upstream) = self.upstream.take()
        {
            self.reserved = true;
            ctx.send(upstream, FlowMessage::Accept)?;
        }
        Ok(())
    }
}

impl Component<FlowMessage> for PassOn {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if !self.downstream.is_empty() {
            self.to = Some(ctx.lookup(&self.downstream)?);
        }
        Ok(())
    }

    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        match d.message {
            FlowMessage::Offer => self.upstream = Some(d.from),
            FlowMessage::Withdraw => self.upstream = None,
            FlowMessage::Decline => self.reserved = false,
            FlowMessage::Arrive(attrs) => {
                let item = d.item.ok_or(ComponentError::Unexpected {
                    state: "pass-on",
                    event: "an arrival without an item".to_owned(),
                })?;
                ctx.receive(item)?;
                self.holding = Some((item, *attrs));
                self.reserved = false;
            }
            FlowMessage::Accept => {
                self.offered = false;
                match self.holding.take() {
                    Some((item, attrs)) => {
                        ctx.transfer(item, d.from, FlowMessage::Arrive(attrs))?;
                    }
                    None => ctx.send(d.from, FlowMessage::Decline)?,
                }
            }
            other @ (FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::CycleDone { .. }
            | FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }
            | FlowMessage::Status(_)
            | FlowMessage::OfferOf(_)
            | FlowMessage::Return(_)
            | FlowMessage::RouteDecision(_)
            | FlowMessage::DispatchDecision(_)
            | FlowMessage::EdgeCondition { .. }
            | FlowMessage::Pick) => {
                return Err(ComponentError::Unexpected {
                    state: "pass-on",
                    event: format!("{other:?}"),
                });
            }
        }
        self.settle(ctx)
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        if let Some((item, _)) = self.holding {
            visit(item);
        }
    }
}

/// A stand-in that is no station.
#[derive(Debug)]
struct Chute;
impl Encode for Chute {
    fn encode(&self) -> Json {
        Json::object([])
    }
}
impl ComponentParams for Chute {
    const TYPE: &'static str = "Chute";
    const EXAMPLE: &'static str = "{id: chute, type: Chute}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}
impl ComponentType<Flow> for Chute {
    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        let downstream = wiring.downstream().first().cloned().unwrap_or_default();
        Ok(Box::new(PassOn {
            downstream,
            ..PassOn::default()
        }))
    }
}

/// A stand-in that counts as a station.
#[derive(Debug)]
struct Station;
impl Encode for Station {
    fn encode(&self) -> Json {
        Json::object([])
    }
}
impl ComponentParams for Station {
    const TYPE: &'static str = "Station";
    const EXAMPLE: &'static str = "{id: st, type: Station}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}
impl ComponentType<Flow> for Station {
    const STATION: bool = true;
    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        let downstream = wiring.downstream().first().cloned().unwrap_or_default();
        Ok(Box::new(PassOn {
            downstream,
            ..PassOn::default()
        }))
    }
}

/// A stand-in that may not be in a flow, like a calendar.
#[derive(Debug)]
struct Calendar;
impl Encode for Calendar {
    fn encode(&self) -> Json {
        Json::object([])
    }
}
impl ComponentParams for Calendar {
    const TYPE: &'static str = "Calendar";
    const EXAMPLE: &'static str = "{id: shifts, type: Calendar}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}
impl ComponentType<Flow> for Calendar {
    const PORTS: Ports = Ports::None;
    fn build(
        &self,
        _: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(PassOn::default()))
    }
}

#[allow(clippy::unwrap_used)] // Test helper.
fn extended() -> Registry<Flow> {
    standard()
        .unwrap()
        .with::<Chute>()
        .unwrap()
        .with::<Station>()
        .unwrap()
        .with::<Calendar>()
        .unwrap()
}

/// The types an entry schema names.
#[allow(clippy::unwrap_used)] // Test helper.
fn schema_types(schema: &Json) -> BTreeSet<String> {
    schema["$defs"]["Component"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            e["properties"]["type"]["const"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn every_component_type_is_registered() {
    let registry = registry();
    let registered: BTreeSet<String> = registry.catalog().types().map(str::to_owned).collect();
    // (1) The schema lists exactly the catalog's types.
    assert_eq!(schema_types(&model_schema(registry.catalog())), registered);
    // (2) Registering a stand-in makes it appear, with the generator untouched.
    let extended = extended();
    let more = schema_types(&model_schema(extended.catalog()));
    assert!(more.contains("Chute") && more.contains("Calendar"));
    assert_eq!(more.len(), registered.len() + 3);
    // (3) Nothing in this crate's source implements the trait without being
    // registered, the one weakness of a table.
    let pattern = Regex::new(r"impl ComponentType<Flow> for (\w+)").unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut implemented = BTreeSet::new();
    for entry in std::fs::read_dir(source).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for capture in pattern.captures_iter(&text) {
            implemented.insert(capture[1].to_owned());
        }
    }
    // Doc examples show `Calendar` as well; it is not part of the crate.
    implemented.remove("Calendar");
    assert_eq!(implemented, registered);
}

/// Every `$defs` name `schema` refers to.
fn references(schema: &Json, out: &mut BTreeSet<String>) {
    match schema {
        Json::Object(map) => {
            for (key, value) in map {
                if key == "$ref"
                    && let Some(name) = value
                        .as_str()
                        .and_then(|reference| reference.strip_prefix("#/$defs/"))
                {
                    out.insert(name.to_owned());
                }
                references(value, out);
            }
        }
        Json::Array(items) => {
            for item in items {
                references(item, out);
            }
        }
        Json::Null | Json::Bool(_) | Json::Integer(_) | Json::Float(_) | Json::String(_) => {}
    }
}

#[test]
fn the_envelope_stays_apart_from_the_factory() {
    // ADR-0020 §11: no definition of the envelope reaches the catalog's.
    let schema = model_schema(registry().catalog());
    let defs = schema["$defs"].as_object().unwrap();
    let mut checked = 0;
    for name in ["Model", "Experiment", "Include", "ObservableRef", "Control"] {
        let mut seen = BTreeSet::new();
        let mut todo = vec![name.to_owned()];
        while let Some(next) = todo.pop() {
            let Some(definition) = defs.get(&next) else {
                continue;
            };
            let mut found = BTreeSet::new();
            references(definition, &mut found);
            for reference in found {
                if seen.insert(reference.clone()) {
                    todo.push(reference);
                }
            }
        }
        for factory in ["Component", "Observable", "FlowEntry"] {
            assert!(!seen.contains(factory), "{name} reaches {factory}");
        }
        checked += 1;
    }
    assert_eq!(checked, 5);
    assert!(defs.contains_key("Control"));
    // The factory's value types stay inline in their entries.
    for name in ["VariantMix", "Setup", "Change", "Discipline", "Matrix"] {
        assert!(!defs.contains_key(name), "{name} in $defs");
    }
}

#[test]
fn a_registered_stand_in_loads_and_builds() {
    let registry = extended();
    let yaml = "format: \"1.0\"\nmodel: {name: m, seed: 1}\nexperiment: {type: terminating, horizon: 1h}\n\
                components:\n  - {id: source, type: Source, interarrival: 1min}\n  - {id: chute, type: Chute}\n  \
                - {id: m, type: Machine, cycle_time: 20s}\n  - {id: st, type: Station}\n  \
                - {id: n, type: Machine, cycle_time: 10s}\n  - {id: sink, type: Sink}\n  - {id: shifts, type: Calendar}\n\
                flow:\n  - [source, chute, m, st, press, sink]\n"
        .replace("id: n,", "id: press,");
    let model = Loader::new(registry.catalog())
        .load_str(&yaml, Format::Yaml)
        .unwrap();
    let report = registry
        .prepare(&model, taktsim_components::RunOptions::new())
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    assert_eq!(report.1.items.delivered.get(), 59);
    // Roles are read from the types, not their names: a Source reaches `m` through
    // a non-station, so `m` is idle; `press` is behind a station, so it is starved.
    let r = support::read(report.0, report.1);
    assert_eq!(r.machine_notes("m")[0].1.state, MachineState::Idle);
    assert_eq!(r.machine_notes("press")[0].1.state, MachineState::Starved);
}

#[test]
fn roles_decide_what_the_flow_may_do() {
    let registry = extended();
    let loader = Loader::new(registry.catalog());
    let yaml = |components: &str, flow: &str| document("1h", components, flow);
    // A type without ports may not appear in `flow`.
    let errors = loader
        .load_str(
            &yaml(
                "  - {id: source, type: Source, interarrival: 1min}\n  - {id: shifts, type: Calendar}\n  - {id: sink, type: Sink}",
                "  - [source, shifts, sink]",
            ),
            Format::Yaml,
        )
        .unwrap_err();
    assert_eq!(errors.len(), 2, "{errors}");
    // On demand through a non-station to a sink is refused; through a station it is not.
    assert!(
        loader
            .load_str(
                &yaml(
                    "  - {id: source, type: Source}\n  - {id: chute, type: Chute}\n  - {id: sink, type: Sink}",
                    "  - [source, chute, sink]",
                ),
                Format::Yaml,
            )
            .is_err()
    );
    assert!(
        loader
            .load_str(
                &yaml(
                    "  - {id: source, type: Source}\n  - {id: st, type: Station}\n  - {id: sink, type: Sink}",
                    "  - [source, st, sink]",
                ),
                Format::Yaml,
            )
            .is_ok()
    );
    assert_eq!(registry.roles().ports("Calendar"), Some(Ports::None));
    assert!(registry.roles().is_station("Station"));
    assert!(!registry.roles().is_station("Chute"));
}

#[test]
fn a_catalog_refuses_a_type_twice() {
    let result = standard().unwrap().with::<Chute>().unwrap().with::<Chute>();
    assert!(result.is_err());
}

#[test]
fn the_standard_catalog_loads_the_stand_ins_tests_shape() {
    let schema = model_schema(registry().catalog());
    let checker = Checker { root: &schema };
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    );
    let tree = taktsim_model::parse(&yaml, Format::Yaml).unwrap().to_json();
    assert!(checker.valid(&schema, &tree));
    load(&yaml);
    assert!(run(&yaml, 1).report.items.delivered.get() > 50);
}

/// A domain with no routing words: its catalog offers none, and its models cannot name
/// a rule it would ignore (ADR-0023 §2).
struct Bare;

impl taktsim_components::Domain for Bare {
    type Message = FlowMessage;
    type Shared = ();
    type Facts = ();
    fn check(
        _: &taktsim_model::Document,
        _: &taktsim_components::Roles,
        _: &mut taktsim_model::Findings,
    ) {
    }
    fn prepare(
        _: &taktsim_model::Document,
        _: &taktsim_components::Roles,
    ) -> Result<(), BuildError> {
        Ok(())
    }
}

impl ComponentType<Bare> for taktsim_components::Sink {
    fn build(&self, _: &Wiring<'_, ()>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(PassOn::default()))
    }
}

#[test]
fn a_domain_offers_the_routing_words_it_honours_and_no_others() {
    assert_eq!(standard().unwrap().catalog().routing(), RoutingWords::ALL);
    let bare = Registry::<Bare>::new()
        .with::<taktsim_components::Sink>()
        .unwrap();
    assert_eq!(bare.catalog().routing(), RoutingWords::NONE);
    let flow = |edge: &str| {
        document(
            "1h",
            "  - {id: a, type: Sink}\n  - {id: b, type: Sink}\n  - {id: c, type: Sink}",
            edge,
        )
    };
    let loader = Loader::new(bare.catalog());
    assert!(
        loader
            .load_str(&flow("  - [a, b]\n  - [b, c]"), Format::Yaml)
            .is_ok()
    );
    for (edge, path) in [
        (
            "  - {from: a, to: b, when: \"true\"}\n  - {from: a, to: c}",
            "flow[0].when",
        ),
        (
            "  - {from: a, to: [b, c], choose: preferred}",
            "flow[0].choose",
        ),
    ] {
        let errors = loader.load_str(&flow(edge), Format::Yaml).unwrap_err();
        assert_eq!(errors.first().path().to_string(), path, "{errors}");
    }
    let schema = model_schema(bare.catalog());
    assert_eq!(schema["properties"]["control"], Json::Null);
    assert!(
        !schema["$defs"]["FlowEntry"]
            .to_string()
            .contains("\"when\"")
    );
}

/// What each standard type states for a rule (ADR-0023 §13).
#[test]
fn the_standard_types_state_their_facts() {
    let registry = registry();
    let model = load(&document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min, variants: {red: 50%, blue: 50%}}\n  \
         - {id: m, type: Machine, cycle_time: 45s, cycle_time_by_variant: {red: {dist: uniform, min: 10s, max: 20s}}, \
         capacity: 3 parts, setup: {per_variant: 4min, matrix: [{from: red, to: blue, time: 0s}]}}\n  \
         - {id: plain, type: Machine, cycle_time: 45s}\n  \
         - {id: buf, type: Buffer, capacity: 7 parts}\n  - {id: sink, type: Sink}",
        "  - [source, m, plain, buf, sink]",
    ));
    let facts =
        |i: usize| -> StationFacts { registry.roles().facts(&model.document().components()[i]) };
    let source = facts(0);
    assert!(source.routes && !source.work && !source.takes_rule && !source.takes_returns);
    assert_eq!(source.status, StatusFact::None);
    assert_eq!(source.places, None);

    let machine = facts(1);
    assert!(machine.routes && machine.work && machine.takes_rule && machine.takes_returns);
    assert_eq!(machine.status, StatusFact::Reports);
    assert_eq!(machine.places, Some(3));
    let times = machine.mean_time.unwrap();
    assert_eq!(times.of("blue").to_string(), "45s");
    assert_eq!(times.of("red").to_string(), "15s");
    let setup = machine.setup.unwrap();
    assert_eq!(
        setup.of("red", "blue").map(|d| d.to_string()).as_deref(),
        Some("0s")
    );
    assert_eq!(
        setup.of("blue", "red").map(|d| d.to_string()).as_deref(),
        Some("4min")
    );

    // No `setup` is a stated "no setups", not an unstated fact.
    assert_eq!(facts(2).setup, Some(SetupMeans::default()));

    let buffer = facts(3);
    assert!(buffer.routes && !buffer.work && !buffer.takes_rule && !buffer.takes_returns);
    assert_eq!(buffer.status, StatusFact::Reports);
    assert_eq!(buffer.places, Some(7));
    assert_eq!(buffer.mean_time, None);

    let sink = facts(4);
    assert_eq!(sink.status, StatusFact::AlwaysRoom);
    assert!(!sink.routes && !sink.work);
}
