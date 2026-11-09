//! One way to run: seeds, limits, the build, and what a document from elsewhere
//! gets (ADR-0022 §2).

mod support;

use std::sync::atomic::AtomicBool;

use support::{d, document, load, registry, t};
use taktsim_components::{BuildError, RunFailure, RunOptions};
use taktsim_core::{EngineConfig, NoTrace, RunOutcome};
use taktsim_model::{Catalog, ComponentParams, Encode, Fields, Format, Json, Loader};
use taktsim_trace::TraceReader;

const LINE: &str = "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
                    - {id: sink, type: Sink}";

fn seeded(seed: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: m{seed}}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n{LINE}\nflow: [[source, sink]]\n"
    )
}

#[test]
fn seed_comes_from_options_then_model() {
    let model = load(&seeded(", seed: 42"));
    let registry = registry();
    let from_model = registry.prepare(&model, RunOptions::new()).unwrap();
    assert_eq!(from_model.header().seed, 42);
    let from_options = registry.prepare(&model, RunOptions::new().seed(7)).unwrap();
    assert_eq!(from_options.header().seed, 7);
    // The header names the seed that ran: each gives its own run.
    let run = |prepared: taktsim_components::Prepared<'_, _>| {
        let (bytes, report) = prepared.run_traced(Vec::new()).unwrap();
        let reader = TraceReader::<_, taktsim_components::FlowMessage>::new(bytes.as_slice());
        (reader.unwrap().header().seed, report.trace.unwrap().hash)
    };
    let (a_seed, a) = run(registry.prepare(&model, RunOptions::new()).unwrap());
    let (b_seed, b) = run(registry.prepare(&model, RunOptions::new().seed(7)).unwrap());
    assert_eq!((a_seed, b_seed), (42, 7));
    assert_ne!(a, b);
}

#[test]
fn no_seed_is_an_error() {
    let model = load(&seeded(""));
    let Err(failure) = registry().prepare(&model, RunOptions::new()) else {
        panic!("a run without a seed started");
    };
    let RunFailure::NoSeed { path } = &failure else {
        panic!("{failure}");
    };
    assert_eq!(path.to_string(), "model.seed");
    assert!(failure.to_string().contains("seed: 42"), "{failure}");
    // The same as fields, for the CLI and the bindings: no parsing of the text.
    assert_eq!(
        failure.path().map(ToString::to_string).as_deref(),
        Some("model.seed")
    );
    assert!(failure.found().is_some_and(|f| f.contains("no seed")));
    assert!(failure.allowed().is_some_and(|a| a.contains("a seed")));
    assert_eq!(
        failure.example().as_deref(),
        Some("model: {name: line-a, seed: 42}")
    );
}

#[test]
fn the_model_hashes_are_in_the_header() {
    let model = load(&seeded(", seed: 1"));
    let registry = registry();
    let prepared = registry.prepare(&model, RunOptions::new()).unwrap();
    let header = prepared.header();
    assert_eq!(header.model_hash.as_bytes(), model.hash().as_bytes());
    assert_eq!(
        header.system_hash.as_bytes(),
        model.system_hash().as_bytes()
    );
    assert_eq!(header.components, ["source", "sink"]);
    assert_eq!(header.message_format, "taktsim-components/1");
}

#[test]
fn another_horizon_is_another_version() {
    let registry = registry();
    let model = load(&seeded(", seed: 1"));
    let shorter = model.with_horizon(d("10min")).unwrap();
    let report = registry
        .prepare(&shorter, RunOptions::new())
        .unwrap()
        .run(&mut NoTrace)
        .unwrap();
    assert_eq!(report.summary.now, t("10min"));
    assert_eq!(
        registry
            .prepare(&shorter, RunOptions::new())
            .unwrap()
            .header()
            .system_hash
            .as_bytes(),
        model.system_hash().as_bytes()
    );
}

#[test]
fn limits_stop_the_run_and_the_trace_states_it() {
    let registry = registry();
    let model = load(&seeded(", seed: 1"));
    let flag = AtomicBool::new(true);
    let (_, report) = registry
        .prepare(&model, RunOptions::new().cancel_on(&flag))
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    assert_eq!(report.summary.outcome, RunOutcome::Cancelled);
    assert_eq!(
        report.trace.unwrap().end.unwrap().outcome,
        RunOutcome::Cancelled
    );
    let (_, report) = registry
        .prepare(&model, RunOptions::new().max_events(4))
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    assert_eq!(report.summary.outcome, RunOutcome::EventLimit);
    assert_eq!(report.summary.events, 4);
    // Checks can be turned off, with the same trace.
    let (a, _) = registry
        .prepare(&model, RunOptions::new())
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    let (b, _) = registry
        .prepare(&model, RunOptions::new().without_checks())
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    assert_eq!(a, b);
    assert!(a.len() > 100);
}

#[test]
fn the_registry_can_build_from_several_threads() {
    fn assert_sync<T: Sync>(_: &T) {}
    let registry = registry();
    assert_sync(&registry);
    let model = load(&seeded(", seed: 1"));
    let delivered: Vec<u64> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..3)
            .map(|seed| {
                let (registry, model) = (&registry, &model);
                scope.spawn(move || {
                    registry
                        .prepare(model, RunOptions::new().seed(seed))
                        .unwrap()
                        .run(&mut NoTrace)
                        .unwrap()
                        .items
                        .delivered
                        .get()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(delivered.len(), 3);
    assert!(delivered.iter().all(|n| *n > 20));
}

/// A stand-in `Sink` of another catalog: the same name, other parameters.
#[derive(Debug)]
struct ForeignSink;
impl Encode for ForeignSink {
    fn encode(&self) -> Json {
        Json::object([])
    }
}
impl ComponentParams for ForeignSink {
    const TYPE: &'static str = "Sink";
    const EXAMPLE: &'static str = "{id: sink, type: Sink}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

/// A stand-in type this registry does not have.
#[derive(Debug)]
struct Conveyor;
impl Encode for Conveyor {
    fn encode(&self) -> Json {
        Json::object([])
    }
}
impl ComponentParams for Conveyor {
    const TYPE: &'static str = "Conveyor";
    const EXAMPLE: &'static str = "{id: c, type: Conveyor}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

#[test]
fn a_document_from_another_catalog_cannot_reach_the_engine_unchecked() {
    let registry = registry();
    // Loaded with a catalog that has a `Conveyor`: the flow check runs again at
    // build, and names it.
    let catalog = Catalog::new().with::<Conveyor>().unwrap();
    let yaml = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                components: [{id: a, type: Conveyor}, {id: b, type: Conveyor}]\nflow: [[a, b]]\n";
    let model = Loader::new(&catalog).load_str(yaml, Format::Yaml).unwrap();
    let Err(error) = registry.build(model.document(), EngineConfig::new(1)) else {
        panic!("a foreign type was built");
    };
    assert!(matches!(error, BuildError::Model(_)), "{error}");
    assert_eq!(error.path().unwrap().to_string(), "components[0]");
    // A flow that the factory check refuses, loaded with a lax catalog.
    let lax = Catalog::new().with::<ForeignSink>().unwrap();
    let yaml = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                components: [{id: a, type: Sink}, {id: b, type: Sink}]\nflow: [[a, b]]\n";
    let model = Loader::new(&lax).load_str(yaml, Format::Yaml).unwrap();
    let Err(error) = registry.build(model.document(), EngineConfig::new(1)) else {
        panic!("a sink with an outgoing edge was built");
    };
    assert!(matches!(error, BuildError::Model(_)), "{error}");
    assert!(error.to_string().contains("an edge leaving `a`"), "{error}");
}

#[test]
fn parameters_of_another_type_are_refused_by_name() {
    // Passes the flow check (a sink behind a... sink needs a source): a source
    // of this catalog is not available in the lax one, so use only what the
    // check allows there: nothing has an outlet. The builder is reached through
    // a registry whose own check is empty.
    struct Open;
    impl taktsim_components::Domain for Open {
        type Message = taktsim_components::FlowMessage;
        type Shared = taktsim_components::FlowShared;
        fn check(
            _: &taktsim_model::Document,
            _: &taktsim_components::Roles,
            _: &mut taktsim_model::Findings,
        ) {
        }
        fn prepare(
            _: &taktsim_model::Document,
            _: &taktsim_components::Roles,
        ) -> Result<taktsim_components::FlowShared, BuildError> {
            Ok(taktsim_components::FlowShared::default())
        }
    }
    impl taktsim_components::ComponentType<Open> for taktsim_components::Sink {
        fn build(
            &self,
            _: &taktsim_components::Wiring<'_, taktsim_components::FlowShared>,
        ) -> Result<Box<dyn taktsim_core::Component<taktsim_components::FlowMessage>>, BuildError>
        {
            Err(BuildError::UnknownVariant {
                path: taktsim_model::FieldPath::default(),
                name: "never".to_owned(),
                declared: Vec::new(),
            })
        }
    }
    let open = taktsim_components::Registry::<Open>::new()
        .with::<taktsim_components::Sink>()
        .unwrap();
    let lax = Catalog::new().with::<ForeignSink>().unwrap();
    let yaml = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                components: [{id: a, type: Sink}, {id: b, type: Sink}]\nflow: [[a, b]]\n";
    let model = Loader::new(&lax).load_str(yaml, Format::Yaml).unwrap();
    let Err(error) = open.build(model.document(), EngineConfig::new(1)) else {
        panic!("foreign parameters were built");
    };
    assert!(
        matches!(error, BuildError::WrongParameters { .. }),
        "{error}"
    );
    assert_eq!(error.path().unwrap().to_string(), "components[0]");
    assert_eq!(
        error.found().as_deref(),
        Some("parameters of `Sink` read by another catalog")
    );
    assert!(
        error
            .allowed()
            .is_some_and(|a| a.contains("this registry's `Sink`"))
    );
    assert_eq!(
        error.example().as_deref(),
        Some(<taktsim_components::Sink as ComponentParams>::EXAMPLE)
    );
    // And a type that registry does not have at all.
    let catalog = Catalog::new().with::<Conveyor>().unwrap();
    let yaml = yaml.replace("Sink", "Conveyor");
    let model = Loader::new(&catalog).load_str(&yaml, Format::Yaml).unwrap();
    let Err(error) = open.build(model.document(), EngineConfig::new(1)) else {
        panic!("an unknown type was built");
    };
    assert!(matches!(error, BuildError::UnknownType { .. }), "{error}");
    assert_eq!(error.found().as_deref(), Some("Conveyor"));
    assert_eq!(
        error.allowed().as_deref(),
        Some("a component type of this registry: Sink")
    );
    assert_eq!(error.example().as_deref(), Some("type: Sink"));
    // Through a run, the failure gives the build error's fields.
    let failure = RunFailure::Build(Box::new(error));
    assert_eq!(
        failure.path().map(ToString::to_string).as_deref(),
        Some("components[0]")
    );
    assert_eq!(failure.example().as_deref(), Some("type: Sink"));
}

#[test]
fn a_model_with_only_a_timed_source_and_a_sink_runs() {
    let yaml = document("1h", LINE, "  - [source, sink]");
    let r = support::run(&yaml, 2);
    assert!(r.report.items.delivered.get() > 20);
    assert_eq!(r.report.summary.outcome, RunOutcome::Horizon);
    assert_eq!(r.end, t("1h"));
}

/// Says one message to one component at time zero, whatever the model's flow is:
/// for what a behaviour does with a message it should never get.
#[derive(Debug)]
struct Rogue {
    to: String,
    say: u32,
    with_item: bool,
}

impl Encode for Rogue {
    fn encode(&self) -> Json {
        Json::object([])
            .with("to", &self.to)
            .with("say", &self.say)
            .with("with_item", &self.with_item)
    }
}

impl ComponentParams for Rogue {
    const TYPE: &'static str = "Rogue";
    const EXAMPLE: &'static str = "{id: r, type: Rogue, to: sink, say: 0, with_item: false}";
    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let to = fields.required("to");
        let say = fields.required("say");
        let with_item = fields.required("with_item");
        Some(Self {
            to: to?,
            say: say?,
            with_item: with_item?,
        })
    }
}

struct RogueBehaviour {
    to: String,
    say: u32,
    with_item: bool,
}

impl taktsim_core::Component<taktsim_components::FlowMessage> for RogueBehaviour {
    fn start(
        &mut self,
        ctx: &mut taktsim_core::Context<'_, taktsim_components::FlowMessage>,
    ) -> Result<(), taktsim_core::ComponentError> {
        use taktsim_components::{FlowMessage as F, ItemAttrs, VariantId};
        let to = ctx.lookup(&self.to)?;
        let attrs = ItemAttrs::new(VariantId::new(0), 0);
        let message = match self.say {
            0 => F::Failure,
            1 => F::CycleDone {
                item: taktsim_core::ItemId::from_raw(99),
            },
            2 => F::SetupDone,
            3 => F::NextArrival,
            4 => F::Arrive(attrs),
            5 => F::Accept,
            6 => F::Decline,
            7 => F::Repaired,
            8 => F::VariantName {
                id: VariantId::new(0),
                name: "x".to_owned(),
            },
            _ => F::Offer,
        };
        if self.with_item {
            let item = ctx.create_item();
            ctx.transfer(item, to, message)?;
        } else {
            ctx.send(to, message)?;
        }
        Ok(())
    }

    fn handle(
        &mut self,
        _: taktsim_core::Delivery<'_, taktsim_components::FlowMessage>,
        _: &mut taktsim_core::Context<'_, taktsim_components::FlowMessage>,
    ) -> Result<(), taktsim_core::ComponentError> {
        Ok(())
    }
}

impl taktsim_components::ComponentType<taktsim_components::Flow> for Rogue {
    const PORTS: taktsim_components::Ports = taktsim_components::Ports::None;

    fn build(
        &self,
        _: &taktsim_components::Wiring<'_, taktsim_components::FlowShared>,
    ) -> Result<Box<dyn taktsim_core::Component<taktsim_components::FlowMessage>>, BuildError> {
        Ok(Box::new(RogueBehaviour {
            to: self.to.clone(),
            say: self.say,
            with_item: self.with_item,
        }))
    }
}

/// The error of a model where a rogue says message number `say` to `to`.
#[allow(clippy::unwrap_used)] // Test helper.
fn rogue_error(to: &str, say: u32, with_item: bool) -> String {
    let registry = taktsim_components::standard()
        .unwrap()
        .with::<Rogue>()
        .unwrap();
    let yaml = format!(
        "format: \"1.0\"\nmodel: {{name: m, seed: 1}}\nexperiment: {{type: terminating, horizon: 1min}}\n\
         components:\n  - {{id: source, type: Source, interarrival: 10s}}\n  \
         - {{id: m, type: Machine, cycle_time: 1s}}\n  - {{id: buf, type: Buffer, capacity: 2 parts}}\n  \
         - {{id: sink, type: Sink}}\n  - {{id: r, type: Rogue, to: {to}, say: {say}, with_item: {with_item}}}\n\
         flow: [[source, m, buf, sink]]\n"
    );
    let model = Loader::new(registry.catalog())
        .load_str(&yaml, Format::Yaml)
        .unwrap();
    let failure = registry
        .prepare(&model, RunOptions::new())
        .unwrap()
        .run(&mut NoTrace)
        .unwrap_err();
    assert!(matches!(failure, RunFailure::Run(_)), "{failure}");
    failure.to_string()
}

#[test]
fn a_message_a_component_should_never_get_fails_the_run_with_its_text() {
    // Messages that fit no state of the target, for each type.
    for (to, say, text) in [
        ("sink", 0, "Failure"),
        ("sink", 5, "Accept"),
        ("sink", 8, "VariantName"),
        ("buf", 0, "Failure"),
        ("buf", 7, "Repaired"),
        ("m", 3, "NextArrival"),
        ("m", 8, "VariantName"),
        ("source", 0, "Failure"),
        ("source", 6, "Decline"),
    ] {
        let error = rogue_error(to, say, false);
        assert!(error.contains(text), "{to} {say}: {error}");
        assert!(error.contains("unexpected event"), "{to} {say}: {error}");
    }
}

#[test]
fn the_protocol_is_checked() {
    // A reply to a reservation no one made.
    for to in ["sink", "buf", "m"] {
        let error = rogue_error(to, 6, false);
        assert!(error.contains("no place reserved"), "{to}: {error}");
    }
    // An arrival without a reservation.
    for to in ["sink", "buf", "m"] {
        let error = rogue_error(to, 4, false);
        assert!(error.contains("no place reserved"), "{to}: {error}");
    }
    // An accept from a component that is not downstream.
    for to in ["source", "m", "buf"] {
        let error = rogue_error(to, 5, false);
        assert!(error.contains("not downstream"), "{to}: {error}");
    }
    // A timer it never set.
    assert!(rogue_error("m", 1, false).contains("CycleDone"));
    assert!(rogue_error("m", 2, false).contains("SetupDone"));
}
