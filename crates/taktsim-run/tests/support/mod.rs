//! What the tests of the run layer share: a temporary directory, and component
//! types added with `Registry::with`, which do what the standard catalog cannot.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)] // Each test crate uses a part.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use taktsim_components::{
    BuildError, ComponentType, Flow, FlowMessage, FlowShared, ItemAttrs, Ports, Registry,
    VariantId, Wiring, standard,
};
use taktsim_core::random::Stream;
use taktsim_core::{Component, ComponentError, Context, Delivery, Disposal, ItemId, Violation};
use taktsim_model::{ComponentParams, Encode, Fields, Json};
use taktsim_run::{Failure, RunRequest, RunResponse, Store};

/// A directory removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "taktsim-run-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A store below this directory.
    pub fn store(&self) -> Store {
        Store::open(&self.0.join("store")).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A model of these components and this flow.
pub fn model(head: &str, components: &str, flow: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: test{head}}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n{components}\nflow:\n{flow}\n"
    )
}

/// A model that draws, with a `seed` if `head` says so.
pub fn drawing_model(head: &str) -> String {
    model(
        head,
        "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  - {id: sink, type: Sink}",
        "  - [source, sink]",
    )
}

/// A model that draws nothing, with an unused `seed` if `head` says so.
pub fn exact_model(head: &str) -> String {
    model(
        head,
        "  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    )
}

/// Runs `yaml` with the standard catalog.
pub fn run_text(
    yaml: &str,
    request: &RunRequest<'_>,
    store: &Store,
) -> Result<RunResponse, Failure> {
    let registry = standard().unwrap();
    run_with(&registry, yaml, request, store)
}

/// Runs `yaml` with this registry.
pub fn run_with(
    registry: &Registry<Flow>,
    yaml: &str,
    request: &RunRequest<'_>,
    store: &Store,
) -> Result<RunResponse, Failure> {
    taktsim_run::run(
        registry,
        taktsim_run::ModelSource::Text(yaml, taktsim_model::Format::Yaml),
        request,
        store,
    )
}

fn attrs() -> ItemAttrs {
    ItemAttrs::new(VariantId::new(0), 0)
}

/// The standard catalog and every test type.
pub fn test_registry() -> Registry<Flow> {
    standard()
        .unwrap()
        .with::<Burst>()
        .unwrap()
        .with::<Keep>()
        .unwrap()
        .with::<Relay>()
        .unwrap()
        .with::<Lonely>()
        .unwrap()
        .with::<Liar>()
        .unwrap()
        .with::<Faulty>()
        .unwrap()
}

macro_rules! no_encode {
    ($t:ty) => {
        impl Encode for $t {
            fn encode(&self) -> Json {
                Json::object([])
            }
        }
    };
}

/// `type: Burst`: creates `count` items at the start, sends them on and does
/// nothing more, so a run can end with no events left.
#[derive(Debug)]
pub struct Burst {
    count: u32,
}

impl Encode for Burst {
    fn encode(&self) -> Json {
        Json::object([]).with("count", &self.count)
    }
}

impl ComponentParams for Burst {
    const TYPE: &'static str = "Burst";
    const EXAMPLE: &'static str = "{id: b, type: Burst, count: 3}";
    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            count: fields.required("count")?,
        })
    }
}

impl ComponentType<Flow> for Burst {
    const PORTS: Ports = Ports::Outlet;

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(BurstBehaviour {
            count: self.count,
            to: wiring.downstream()[0].clone(),
        }))
    }
}

struct BurstBehaviour {
    count: u32,
    to: String,
}

impl Component<FlowMessage> for BurstBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        let to = ctx.lookup(&self.to)?;
        for _ in 0..self.count {
            let item = ctx.create_item();
            ctx.transfer(item, to, FlowMessage::Arrive(attrs()))?;
        }
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

/// `type: Keep`: receives items, and holds them for ever (`how: hold`) or disposes
/// of them.
#[derive(Debug)]
pub struct Keep {
    how: String,
}

impl Encode for Keep {
    fn encode(&self) -> Json {
        Json::object([]).with("how", &self.how)
    }
}

impl ComponentParams for Keep {
    const TYPE: &'static str = "Keep";
    const EXAMPLE: &'static str = "{id: k, type: Keep, how: hold}";
    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            how: fields.required("how")?,
        })
    }
}

impl ComponentType<Flow> for Keep {
    const PORTS: Ports = Ports::Inlet;

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        _: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(KeepBehaviour {
            how: match self.how.as_str() {
                "hold" => None,
                "delivered" => Some(Disposal::Delivered),
                "scrapped" => Some(Disposal::Scrapped),
                "consumed" => Some(Disposal::Consumed),
                other => panic!("how: {other}"),
            },
            held: Vec::new(),
        }))
    }
}

struct KeepBehaviour {
    how: Option<Disposal>,
    held: Vec<ItemId>,
}

impl Component<FlowMessage> for KeepBehaviour {
    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        if let Some(item) = d.item {
            ctx.receive(item)?;
            match self.how {
                Some(how) => ctx.dispose(item, how)?,
                None => self.held.push(item),
            }
        }
        Ok(())
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        self.held.iter().copied().for_each(visit);
    }
}

/// `type: Relay`, with `Ports::Both`: passes each item on and creates one more.
#[derive(Debug)]
pub struct Relay;
no_encode!(Relay);

impl ComponentParams for Relay {
    const TYPE: &'static str = "Relay";
    const EXAMPLE: &'static str = "{id: r, type: Relay}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

impl ComponentType<Flow> for Relay {
    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(RelayBehaviour {
            to: wiring.downstream()[0].clone(),
        }))
    }
}

struct RelayBehaviour {
    to: String,
}

impl Component<FlowMessage> for RelayBehaviour {
    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let to = ctx.lookup(&self.to)?;
        if let Some(item) = d.item {
            ctx.receive(item)?;
            ctx.transfer(item, to, FlowMessage::Arrive(attrs()))?;
            let more = ctx.create_item();
            ctx.transfer(more, to, FlowMessage::Arrive(attrs()))?;
        }
        Ok(())
    }
}

/// `type: Lonely`, with `Ports::None`: outside the flow, creates and scraps items
/// at the start.
#[derive(Debug)]
pub struct Lonely;
no_encode!(Lonely);

impl ComponentParams for Lonely {
    const TYPE: &'static str = "Lonely";
    const EXAMPLE: &'static str = "{id: l, type: Lonely}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

impl ComponentType<Flow> for Lonely {
    const PORTS: Ports = Ports::None;

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        _: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(LonelyBehaviour))
    }
}

struct LonelyBehaviour;

impl Component<FlowMessage> for LonelyBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        for _ in 0..2 {
            let item = ctx.create_item();
            ctx.dispose(item, Disposal::Scrapped)?;
        }
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

/// `type: Liar`: says it draws nothing, and sends items after random delays.
#[derive(Debug)]
pub struct Liar;
no_encode!(Liar);

impl ComponentParams for Liar {
    const TYPE: &'static str = "Liar";
    const EXAMPLE: &'static str = "{id: l, type: Liar}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

impl ComponentType<Flow> for Liar {
    const PORTS: Ports = Ports::Outlet;

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(LiarBehaviour {
            to: wiring.downstream()[0].clone(),
            stream: None,
        }))
    }
}

struct LiarBehaviour {
    to: String,
    stream: Option<Stream>,
}

impl LiarBehaviour {
    fn next(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        let seconds = self.stream.as_mut().map_or(1, |s| 1 + s.next_u64() % 60);
        ctx.schedule(
            taktsim_units::SimDuration::from_secs(seconds)?,
            FlowMessage::NextArrival,
        )?;
        Ok(())
    }
}

impl Component<FlowMessage> for LiarBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.stream = Some(ctx.stream("lie")?);
        self.next(ctx)
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let item = ctx.create_item();
        let to = ctx.lookup(&self.to)?;
        ctx.transfer(item, to, FlowMessage::Arrive(attrs()))?;
        self.next(ctx)
    }
}

/// `type: Faulty`: after one minute, fails (`how: error`) or breaks an invariant
/// (`how: invariant`), or leaves an item unreceived (`how: unreceived`) or lost
/// (`how: lost`).
#[derive(Debug)]
pub struct Faulty {
    how: String,
}

impl Encode for Faulty {
    fn encode(&self) -> Json {
        Json::object([]).with("how", &self.how)
    }
}

impl ComponentParams for Faulty {
    const TYPE: &'static str = "Faulty";
    const EXAMPLE: &'static str = "{id: f, type: Faulty, how: error}";
    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            how: fields.required("how")?,
        })
    }
}

impl ComponentType<Flow> for Faulty {
    const PORTS: Ports = Ports::None;

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        _: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(FaultyBehaviour {
            how: self.how.clone(),
            broken: false,
        }))
    }
}

struct FaultyBehaviour {
    how: String,
    broken: bool,
}

impl Component<FlowMessage> for FaultyBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        ctx.schedule("1min".parse()?, FlowMessage::NextArrival)?;
        Ok(())
    }

    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        match self.how.as_str() {
            "error" => Err(ComponentError::Unexpected {
                state: "faulty",
                event: "NextArrival".to_owned(),
            }),
            "invariant" => {
                self.broken = true;
                Ok(())
            }
            "lost" => {
                // Created and reported by nobody.
                let _item = ctx.create_item();
                Ok(())
            }
            "unreceived" => {
                let me = ctx.lookup("faulty")?;
                let item = ctx.create_item();
                ctx.transfer(item, me, FlowMessage::NextArrival)?;
                self.how.clear();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        if self.broken {
            return Err(Violation {
                invariant: "it is not broken",
                detail: "broken".to_owned(),
            });
        }
        Ok(())
    }
}
