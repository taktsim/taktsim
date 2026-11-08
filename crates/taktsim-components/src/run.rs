//! From document to run (ADR-0022 §2). Nothing here is about factories: a
//! [`Domain`] brings its messages and what spans a document, a
//! [`ComponentType`] says how to build one behaviour, and a [`Registry`] holds
//! them, filled by one call per type. So a type cannot be readable but not
//! runnable, and M7 and M12 are written against one way to run.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use taktsim_core::random::DistributionError;
use taktsim_core::{
    Component as Behaviour, Engine, EngineConfig, EngineError, ItemCounts, RunError, RunLimits,
    RunSummary, TraceSink,
};
use taktsim_model::expr::Signature;
use taktsim_model::{
    Catalog, CatalogError, Component as Entry, ComponentParams, Document, FieldPath, Findings,
    LoadErrors, Model, Segment,
};
use taktsim_trace::{
    Header, ModelHash, ReplayError, ReplayReport, Summary, SystemHash, TraceMessage, TraceReader,
    TraceWriter, WriteError, replay,
};
use taktsim_units::SimTime;

/// A family of components with their messages (`Flow` for production lines; a
/// later domain brings its own).
///
/// ```
/// use taktsim_components::{BuildError, Domain, Flow, MessageFact, Roles};
/// use taktsim_model::{Document, Findings};
///
/// /// Flow's messages, with no rules of its own.
/// struct Lax;
///
/// impl Domain for Lax {
///     type Message = taktsim_components::FlowMessage;
///     type Shared = ();
///     const PACKAGE: &'static str = "lax";
///     const PACKAGE_VERSION: &'static str = "1.0.0";
///     fn check(_: &Document, _: &Roles, _: &mut Findings) {}
///     fn prepare(_: &Document, _: &Roles) -> Result<(), BuildError> { Ok(()) }
///     fn draws_random(_: &Document) -> bool { false }
///     fn read_message(_: &Self::Message, _: &mut dyn FnMut(MessageFact<'_>)) {}
/// }
///
/// let _ = taktsim_components::Registry::<Lax>::new();
/// let _ = std::marker::PhantomData::<Flow>;
/// ```
pub trait Domain: 'static {
    /// The messages of its components.
    type Message: TraceMessage;
    /// What every builder reads, such as the model's variants.
    type Shared;

    /// The package's name, in every run's provenance and run id (ADR-0024 §6):
    /// the name before the `/` in its message format.
    const PACKAGE: &'static str;
    /// The package's version, usually `env!("CARGO_PKG_VERSION")`. A package
    /// that changes what its components do gets a new version, and so new run
    /// ids, even when its message format stays.
    const PACKAGE_VERSION: &'static str;

    /// What it checks across a document, reported at a place and field path.
    /// Run at load time, and again when building.
    fn check(document: &Document, roles: &Roles, findings: &mut Findings);

    /// What every builder of `document` reads.
    ///
    /// # Errors
    ///
    /// A [`BuildError`] with the field path of what cannot be prepared.
    fn prepare(document: &Document, roles: &Roles) -> Result<Self::Shared, BuildError>;

    /// Whether a run of `document` draws random numbers for what no single
    /// component's parameters state, such as a split on an edge (ADR-0024 §5).
    /// Required, so a domain must answer for it.
    fn draws_random(document: &Document) -> bool;

    /// What a message states for the KPIs (ADR-0025 §1), through a callback, since one
    /// message can state several facts: a later item has a variant, a product and a
    /// material at once. Required, and a domain should match every message without
    /// `_`, so a new message does not compile until it answers.
    fn read_message(message: &Self::Message, facts: &mut dyn FnMut(MessageFact<'_>));
}

/// What a message states for the KPIs: the domain's words, which `taktsim-run` maps to
/// positions.
///
/// ```
/// use taktsim_components::{Domain, Flow, FlowMessage, ItemAttrs, MessageFact, VariantId};
///
/// let mut facts = Vec::new();
/// let arrive = FlowMessage::Arrive(ItemAttrs::new(VariantId::new(2), 0));
/// Flow::read_message(&arrive, &mut |fact| facts.push(format!("{fact:?}")));
/// assert_eq!(facts, ["Class { dimension: \"variant\", value: 2 }"]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageFact<'a> {
    /// The resource is in this state now, with this many units in progress. The name
    /// is one of its type's [`STATES`](ComponentType::STATES).
    State {
        /// The state.
        name: &'static str,
        /// Units in progress.
        in_progress: u32,
    },
    /// The item this message carries is of this class.
    Class {
        /// The dimension, such as `variant`.
        dimension: &'static str,
        /// The class.
        value: u16,
    },
    /// This is the name of a class. A package that names every class of a dimension at
    /// the start of a run gets the same class rows from every run, whatever each one
    /// happened to deliver.
    ClassName {
        /// The dimension.
        dimension: &'static str,
        /// The class.
        value: u16,
        /// Its name.
        name: &'a str,
    },
}

/// When an item a component creates counts as work in progress (ADR-0025 §1).
/// A type's ports do not say what its output means, so it says this itself.
///
/// ```
/// use taktsim_components::{ComponentType, Creation, Flow, Source, Sink};
///
/// assert_eq!(<Source as ComponentType<Flow>>::CREATION, Creation::HeldUntilFirstMove);
/// assert_eq!(<Sink as ComponentType<Flow>>::CREATION, Creation::Never);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Creation {
    /// It never creates an item; a trace that says so breaks conservation.
    Never,
    /// An item waits here and is released at its first move, like a Source's part.
    HeldUntilFirstMove,
    /// An item is released as it is created, like an assembly's output.
    InSystem,
}

/// What an observable computes, as data, so a package can give a KPI its own word
/// (ADR-0025 §2). A new kind of computation is a new variant here and a change in
/// `taktsim-stats` and `taktsim-run`.
///
/// ```
/// use taktsim_components::{Flow, Kpi, Registry};
/// use taktsim_model::expr::{Arity, Signature, Type};
///
/// let census = Signature::new(Arity::Exactly(1), Type::Number).accepting(["Bed"]);
/// let registry = Registry::<Flow>::new().with_kpi("census", census, Kpi::Occupancy)?;
/// assert_eq!(registry.kpis().next().map(|(name, _, kpi)| (name, kpi)), Some(("census", Kpi::Occupancy)));
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kpi {
    /// Delivered by it in the window, per time; per class too.
    Deliveries,
    /// The time-weighted mean of what is here; of the whole system with no argument.
    Occupancy,
    /// The share of the window in each of its type's states.
    StateShares,
    /// Time-weighted units in progress, over its places.
    InProgress,
    /// The time-weighted mean of what it created and has not released; of the whole
    /// system with no argument.
    AwaitingRelease,
    /// Release to delivery, as a distribution.
    Transit,
}

/// Which edges a component type may have (ADR-0022 §3). A type with inlets or
/// outlets must be connected; a type with [`None`](Self::None) may not appear
/// in `flow`.
///
/// ```
/// use taktsim_components::Ports;
///
/// assert!(Ports::Both.has_inlet() && Ports::Both.has_outlet());
/// assert!(!Ports::Outlet.has_inlet());
/// assert!(!Ports::None.has_outlet());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ports {
    /// Incoming edges only, like a Sink.
    Inlet,
    /// Outgoing edges only, like a Source.
    Outlet,
    /// Both, like a Machine or a Buffer.
    Both,
    /// None, like a calendar or an operator.
    None,
}

impl Ports {
    /// Whether edges may enter.
    ///
    /// ```
    /// assert!(taktsim_components::Ports::Inlet.has_inlet());
    /// ```
    #[must_use]
    pub const fn has_inlet(self) -> bool {
        match self {
            Self::Inlet | Self::Both => true,
            Self::Outlet | Self::None => false,
        }
    }

    /// Whether edges may leave.
    ///
    /// ```
    /// assert!(taktsim_components::Ports::Outlet.has_outlet());
    /// ```
    #[must_use]
    pub const fn has_outlet(self) -> bool {
        match self {
            Self::Outlet | Self::Both => true,
            Self::Inlet | Self::None => false,
        }
    }
}

/// A component type: its parameters, its place in a flow, and how to build its
/// behaviour.
///
/// ```
/// use taktsim_components::{BuildError, ComponentType, Creation, Flow, FlowMessage, FlowShared, Ports, Wiring};
/// use taktsim_core::{Component, ComponentError, Context, Delivery};
/// use taktsim_model::{ComponentParams, Encode, Fields, Json};
///
/// /// A shift calendar: not in the flow, does nothing yet.
/// #[derive(Debug)]
/// struct Calendar;
/// impl Encode for Calendar {
///     fn encode(&self) -> Json { Json::object([]) }
/// }
/// impl ComponentParams for Calendar {
///     const TYPE: &'static str = "Calendar";
///     const EXAMPLE: &'static str = "{id: shifts, type: Calendar}";
///     fn decode(_: &mut Fields<'_, '_>) -> Option<Self> { Some(Self) }
/// }
///
/// struct Quiet;
/// impl Component<FlowMessage> for Quiet {
///     fn handle(&mut self, _: Delivery<'_, FlowMessage>, _: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
///         Ok(())
///     }
/// }
///
/// impl ComponentType<Flow> for Calendar {
///     const PORTS: Ports = Ports::None;
///     const STATES: &'static [&'static str] = &[];
///     const CREATION: Creation = Creation::Never;
///     fn places(&self) -> u32 { 1 }
///     fn build(&self, _: &Wiring<'_, FlowShared>) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
///         Ok(Box::new(Quiet))
///     }
/// }
///
/// let registry = taktsim_components::standard()?.with::<Calendar>()?;
/// assert!(registry.catalog().types().any(|t| t == "Calendar"));
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
pub trait ComponentType<D: Domain>: ComponentParams {
    /// Which edges it may have.
    const PORTS: Ports = Ports::Both;
    /// Whether it does work: a station a part waits for (ADR-0022 §5).
    const STATION: bool = false;
    /// The states its notes can state, in the order of its `state_shares` rows. A
    /// state on a new type adds no row to a line without that type (ADR-0025 §1).
    const STATES: &'static [&'static str];
    /// What creating an item here means. Required: a wrong default would miscount.
    const CREATION: Creation;

    /// The units it can hold in progress: what `utilization` divides by. 1 for a type
    /// that does no work. Required, because a wrong default would state a wrong number
    /// (ADR-0025 §2).
    fn places(&self) -> u32;

    /// Whether a run of this entry draws random numbers (ADR-0024 §5). `true`
    /// unless the type says otherwise, because a false `false` labels one sample
    /// path as exact. Taking a stream is not drawing, and sampling a constant
    /// draws nothing. Destructure the parameters without `..`, so a new key
    /// does not compile until it is answered for.
    fn draws_random(&self) -> bool {
        true
    }

    /// Its behaviour in one run.
    ///
    /// # Errors
    ///
    /// A [`BuildError`] with the field path of the component.
    fn build(
        &self,
        wiring: &Wiring<'_, D::Shared>,
    ) -> Result<Box<dyn Behaviour<D::Message>>, BuildError>;
}

/// What every registered type declares, for what a domain checks without
/// matching on a type's name.
///
/// ```
/// use taktsim_components::Ports;
///
/// let registry = taktsim_components::standard()?;
/// let roles = registry.roles();
/// assert_eq!(roles.ports("Sink"), Some(Ports::Inlet));
/// assert!(roles.is_station("Machine"));
/// assert!(!roles.is_station("Buffer"));
/// assert_eq!(roles.ports("Nothing"), None);
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct Roles {
    types: BTreeMap<&'static str, Role>,
}

/// What one type declares.
#[derive(Debug, Clone, Copy)]
struct Role {
    ports: Ports,
    station: bool,
    states: &'static [&'static str],
    creation: Creation,
}

impl Roles {
    /// The ports of type `name`, if it is registered.
    ///
    /// ```
    /// assert!(taktsim_components::Roles::default().ports("Sink").is_none());
    /// ```
    #[must_use]
    pub fn ports(&self, name: &str) -> Option<Ports> {
        self.types.get(name).map(|role| role.ports)
    }

    /// Whether type `name` is a station.
    ///
    /// ```
    /// assert!(!taktsim_components::Roles::default().is_station("Machine"));
    /// ```
    #[must_use]
    pub fn is_station(&self, name: &str) -> bool {
        self.types.get(name).is_some_and(|role| role.station)
    }

    /// The states type `name` declares; none if it is not registered.
    ///
    /// ```
    /// let registry = taktsim_components::standard()?;
    /// assert_eq!(registry.roles().states("Source"), ["waiting", "blocked"]);
    /// assert!(registry.roles().states("Buffer").is_empty());
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    #[must_use]
    pub fn states(&self, name: &str) -> &'static [&'static str] {
        self.types.get(name).map_or(&[], |role| role.states)
    }

    /// What creating an item means for type `name`, if it is registered.
    ///
    /// ```
    /// use taktsim_components::Creation;
    ///
    /// let registry = taktsim_components::standard()?;
    /// assert_eq!(registry.roles().creation("Source"), Some(Creation::HeldUntilFirstMove));
    /// assert_eq!(registry.roles().creation("Nothing"), None);
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    #[must_use]
    pub fn creation(&self, name: &str) -> Option<Creation> {
        self.types.get(name).map(|role| role.creation)
    }
}

/// What a builder knows about its component: the id, the neighbours in declared
/// order, and what the domain shares (ADR-0022 §2).
///
/// ```
/// use taktsim_components::Wiring;
///
/// fn describe<S>(wiring: &Wiring<'_, S>) -> String {
///     format!("{} at {}: {:?} -> {:?}", wiring.id(), wiring.path(), wiring.upstream(), wiring.downstream())
/// }
/// ```
#[derive(Debug)]
pub struct Wiring<'a, S> {
    id: &'a str,
    path: &'a FieldPath,
    upstream: &'a [String],
    downstream: &'a [String],
    shared: &'a S,
}

impl<'a, S> Wiring<'a, S> {
    /// The component's id.
    ///
    /// ```
    /// fn id<'a>(wiring: &taktsim_components::Wiring<'a, ()>) -> &'a str {
    ///     wiring.id()
    /// }
    /// ```
    #[must_use]
    pub const fn id(&self) -> &'a str {
        self.id
    }

    /// The field path of its entry, for a [`BuildError`].
    ///
    /// ```
    /// fn path(wiring: &taktsim_components::Wiring<'_, ()>) -> String {
    ///     wiring.path().to_string()
    /// }
    /// ```
    #[must_use]
    pub const fn path(&self) -> &'a FieldPath {
        self.path
    }

    /// The ids of the components feeding it, in the order `flow` first names
    /// them.
    ///
    /// ```
    /// fn feeders(wiring: &taktsim_components::Wiring<'_, ()>) -> usize {
    ///     wiring.upstream().len()
    /// }
    /// ```
    #[must_use]
    pub const fn upstream(&self) -> &'a [String] {
        self.upstream
    }

    /// The ids of the components it feeds, in declared order.
    ///
    /// ```
    /// fn targets(wiring: &taktsim_components::Wiring<'_, ()>) -> usize {
    ///     wiring.downstream().len()
    /// }
    /// ```
    #[must_use]
    pub const fn downstream(&self) -> &'a [String] {
        self.downstream
    }

    /// What the domain shares with every builder.
    ///
    /// ```
    /// fn shared(wiring: &taktsim_components::Wiring<'_, u32>) -> u32 {
    ///     *wiring.shared()
    /// }
    /// ```
    #[must_use]
    pub const fn shared(&self) -> &'a S {
        self.shared
    }
}

/// Builds the behaviour of one entry.
type Builder<D> = fn(
    &Entry,
    &Wiring<'_, <D as Domain>::Shared>,
) -> Result<Box<dyn Behaviour<<D as Domain>::Message>>, BuildError>;

fn build_entry<D: Domain, T: ComponentType<D>>(
    entry: &Entry,
    wiring: &Wiring<'_, D::Shared>,
) -> Result<Box<dyn Behaviour<D::Message>>, BuildError> {
    let Some(params) = entry.params::<T>() else {
        return Err(BuildError::WrongParameters {
            path: entry.path().clone(),
            type_name: entry.type_name().to_owned(),
        });
    };
    params.build(wiring)
}

/// Whether `entry` draws; an entry of another type is taken to draw.
fn entry_draws<D: Domain, T: ComponentType<D>>(entry: &Entry) -> bool {
    entry
        .params::<T>()
        .is_none_or(<T as ComponentType<D>>::draws_random)
}

/// The places of `entry`; an entry of another type has one.
fn entry_places<D: Domain, T: ComponentType<D>>(entry: &Entry) -> u32 {
    entry
        .params::<T>()
        .map_or(1, <T as ComponentType<D>>::places)
}

/// The component types of a domain: its catalog and one builder per type,
/// filled by one call per type, so a type cannot be readable but not runnable.
/// "Registered at compile time" (spec §4) through an ordinary call (ADR-0022
/// §11).
///
/// The determinism promise (same model, seed, engine version) covers
/// [`standard`](crate::standard) and in-tree types, whose versions move with
/// the workspace. A registry with a type from outside is not named by the trace
/// header or a run id.
///
/// ```
/// use taktsim_core::{EngineConfig, NoTrace, RunLimits};
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
///      flow: [[source, cut, sink]]\n",
///     Format::Yaml,
/// )?;
/// // Directly, one engine per seed, for instance on several threads.
/// let mut engine = registry.build(model.document(), EngineConfig::new(1))?;
/// engine.run(RunLimits::until("1h".parse()?), &mut NoTrace)?;
/// assert_eq!(engine.item_counts().delivered.get(), 60);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Registry<D: Domain> {
    catalog: Catalog,
    builders: BTreeMap<&'static str, Builder<D>>,
    drawing: BTreeMap<&'static str, fn(&Entry) -> bool>,
    places: BTreeMap<&'static str, fn(&Entry) -> u32>,
    kpis: BTreeMap<String, (Signature, Kpi)>,
    roles: Roles,
    _domain: PhantomData<fn() -> D>,
}

impl<D: Domain> fmt::Debug for Registry<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("catalog", &self.catalog)
            .field("roles", &self.roles)
            .finish_non_exhaustive()
    }
}

impl<D: Domain> Default for Registry<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Domain> Registry<D> {
    /// An empty registry.
    ///
    /// ```
    /// use taktsim_components::{Flow, Registry};
    ///
    /// assert_eq!(Registry::<Flow>::new().catalog().types().count(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            catalog: Catalog::new(),
            builders: BTreeMap::new(),
            drawing: BTreeMap::new(),
            places: BTreeMap::new(),
            kpis: BTreeMap::new(),
            roles: Roles::default(),
            _domain: PhantomData,
        }
    }

    /// Adds the component type `T` to the catalog and the builders. The
    /// domain's check sees the roles of every type added so far.
    ///
    /// # Errors
    ///
    /// A [`CatalogError`] if the catalog refuses the type.
    ///
    /// ```
    /// use taktsim_components::{Flow, Registry, Sink};
    ///
    /// let registry = Registry::<Flow>::new().with::<Sink>()?;
    /// assert_eq!(registry.catalog().types().collect::<Vec<_>>(), ["Sink"]);
    /// assert!(registry.with::<Sink>().is_err());
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    pub fn with<T: ComponentType<D>>(mut self) -> Result<Self, CatalogError> {
        let catalog = self.catalog.with::<T>()?;
        self.roles.types.insert(
            T::TYPE,
            Role {
                ports: T::PORTS,
                station: T::STATION,
                states: T::STATES,
                creation: T::CREATION,
            },
        );
        self.builders.insert(T::TYPE, build_entry::<D, T>);
        self.drawing.insert(T::TYPE, entry_draws::<D, T>);
        self.places.insert(T::TYPE, entry_places::<D, T>);
        let roles = Arc::new(self.roles.clone());
        self.catalog = catalog.with_check(Box::new(move |document, findings| {
            D::check(document, &roles, findings);
        }));
        Ok(self)
    }

    /// Adds a KPI: an observable that `observe` and `experiment.precision` may name,
    /// with what computes it, so a registered observable always has an answer. Every
    /// component its signature accepts gets its rows in each result, declared or not
    /// (ADR-0025 §2). A signature that allows no argument adds a row for the whole
    /// system, for [`Kpi::Occupancy`] and [`Kpi::AwaitingRelease`].
    ///
    /// # Errors
    ///
    /// A [`CatalogError`] if the catalog refuses it.
    ///
    /// ```
    /// use taktsim_components::{Flow, Kpi, Registry};
    /// use taktsim_model::expr::{Arity, Signature, Type};
    ///
    /// let registry = Registry::<Flow>::new()
    ///     .with_kpi("wip", Signature::new(Arity::Exactly(1), Type::Number), Kpi::Occupancy)?;
    /// assert_eq!(registry.catalog().observables().collect::<Vec<_>>(), ["wip"]);
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    pub fn with_kpi(
        mut self,
        name: &str,
        signature: Signature,
        kpi: Kpi,
    ) -> Result<Self, CatalogError> {
        self.catalog = self.catalog.with_observable(name, signature.clone())?;
        self.kpis.insert(name.to_owned(), (signature, kpi));
        Ok(self)
    }

    /// The registered KPIs, sorted by name.
    ///
    /// ```
    /// use taktsim_components::Kpi;
    ///
    /// let registry = taktsim_components::standard()?;
    /// let names: Vec<_> = registry.kpis().map(|(name, _, _)| name).collect();
    /// assert_eq!(names, ["held", "lead_time", "state_shares", "throughput", "utilization", "wip"]);
    /// assert!(registry.kpis().any(|(_, _, kpi)| kpi == Kpi::Transit));
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    pub fn kpis(&self) -> impl Iterator<Item = (&str, &Signature, Kpi)> {
        self.kpis
            .iter()
            .map(|(name, (signature, kpi))| (name.as_str(), signature, *kpi))
    }

    /// The places of a component of `document`: what `utilization` divides by.
    ///
    /// ```
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min, capacity: 2 parts}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, cut, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// assert_eq!(registry.places(&model.document().components()[1]), 2);
    /// assert_eq!(registry.places(&model.document().components()[2]), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn places(&self, entry: &Entry) -> u32 {
        self.places
            .get(entry.type_name())
            .map_or(1, |places| places(entry))
    }

    /// The catalog, for the loader and the schema generator.
    ///
    /// ```
    /// let registry = taktsim_components::standard()?;
    /// assert_eq!(registry.catalog().types().collect::<Vec<_>>(), ["Buffer", "Machine", "Sink", "Source"]);
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    #[must_use]
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// What the registered types declare.
    ///
    /// ```
    /// let registry = taktsim_components::standard()?;
    /// assert!(registry.roles().is_station("Machine"));
    /// # Ok::<(), taktsim_model::CatalogError>(())
    /// ```
    #[must_use]
    pub const fn roles(&self) -> &Roles {
        &self.roles
    }

    /// Whether a run of `document` draws random numbers: the union of what its
    /// components and the domain answer (ADR-0024 §5). A type this registry does
    /// not know is taken to draw.
    ///
    /// ```
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let loader = Loader::new(registry.catalog());
    /// let text = |cycle: &str| format!(
    ///     "format: \"1.0\"\nmodel: {{name: m}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
    ///      components: [{{id: source, type: Source}}, {{id: cut, type: Machine, cycle_time: {cycle}}}, {{id: sink, type: Sink}}]\n\
    ///      flow: [[source, cut, sink]]\n"
    /// );
    /// let constant = loader.load_str(&text("1min"), Format::Yaml)?;
    /// assert!(!registry.draws_random(constant.document()));
    /// let drawn = loader.load_str(&text("{dist: exponential, mean: 1min}"), Format::Yaml)?;
    /// assert!(registry.draws_random(drawn.document()));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn draws_random(&self, document: &Document) -> bool {
        D::draws_random(document) || self.drawing_component(document).is_some()
    }

    /// The first component of `document` that draws.
    fn drawing_component<'d>(&self, document: &'d Document) -> Option<&'d Entry> {
        document.components().iter().find(|entry| {
            self.drawing
                .get(entry.type_name())
                .is_none_or(|draws| draws(entry))
        })
    }

    /// An engine for `document`, with the components in document order
    /// (ADR-0020 §7). It runs the domain's check again, so a document loaded
    /// with another catalog cannot reach the engine unchecked. Can be called
    /// once per seed, from several threads.
    ///
    /// # Errors
    ///
    /// A [`BuildError`] naming the component's field path.
    ///
    /// ```
    /// use taktsim_core::EngineConfig;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let engine = registry.build(model.document(), EngineConfig::new(7))?;
    /// assert_eq!(engine.config().seed, 7);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(
        &self,
        document: &Document,
        config: EngineConfig,
    ) -> Result<Engine<D::Message>, BuildError> {
        let mut findings = Findings::new();
        self.catalog.check(document, &mut findings);
        findings
            .finish()
            .map_err(|errors| BuildError::Model(Box::new(errors)))?;
        let shared = D::prepare(document, &self.roles)?;
        let (upstream, downstream) = neighbours(document);
        let none: Vec<String> = Vec::new();
        let mut engine = Engine::new(config);
        for entry in document.components() {
            let Some(builder) = self.builders.get(entry.type_name()) else {
                return Err(BuildError::UnknownType {
                    path: entry.path().clone(),
                    type_name: entry.type_name().to_owned(),
                });
            };
            let wiring = Wiring {
                id: entry.id(),
                path: entry.path(),
                upstream: upstream.get(entry.id()).unwrap_or(&none),
                downstream: downstream.get(entry.id()).unwrap_or(&none),
                shared: &shared,
            };
            let behaviour = builder(entry, &wiring)?;
            engine
                .add_component(entry.id(), behaviour)
                .map_err(|source| BuildError::Engine {
                    path: entry.path().clone(),
                    source: Box::new(source),
                })?;
        }
        Ok(engine)
    }

    /// A run of `model`, ready to go: the engine, its header and its limits,
    /// together, so a header cannot name a seed that did not run. The horizon is
    /// always the model's; [`Model::with_horizon`] makes another version.
    ///
    /// # Errors
    ///
    /// [`RunFailure::NoSeed`] without a seed in the options or the model,
    /// [`RunFailure::DrawsRandom`] if [`RunOptions::no_seed`] asks for no seed
    /// and the model draws, or [`RunFailure::Build`].
    ///
    /// ```
    /// use taktsim_components::RunOptions;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m, seed: 42}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let prepared = registry.prepare(&model, RunOptions::new())?;
    /// assert_eq!(prepared.header().seed, Some(42));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn prepare<'a>(
        &self,
        model: &Model,
        options: RunOptions<'a>,
    ) -> Result<Prepared<'a, D>, RunFailure> {
        let document = model.document();
        let seed_path = || {
            FieldPath::from(vec![
                Segment::Key("model".to_owned()),
                Segment::Key("seed".to_owned()),
            ])
        };
        let seed = match options.without_seed {
            Some(internal) => {
                if let Some(entry) = self.drawing_component(document) {
                    return Err(RunFailure::DrawsRandom {
                        path: entry.path().clone(),
                    });
                }
                if D::draws_random(document) {
                    return Err(RunFailure::DrawsRandom { path: seed_path() });
                }
                internal
            }
            None => match options.seed.or_else(|| document.model().seed()) {
                Some(seed) => seed,
                None => return Err(RunFailure::NoSeed { path: seed_path() }),
            },
        };
        let mut config = EngineConfig::new(seed);
        if !options.checks {
            config = config.without_checks();
        }
        let engine = self
            .build(document, config)
            .map_err(|e| RunFailure::Build(Box::new(e)))?;
        let mut header = Header::for_engine(
            &engine,
            ModelHash::from_bytes(*model.hash().as_bytes()),
            SystemHash::from_bytes(*model.system_hash().as_bytes()),
        );
        if options.without_seed.is_some() {
            header.seed = None;
        }
        Ok(Prepared {
            engine,
            header,
            until: SimTime::from_nanos(document.experiment().horizon().as_nanos()),
            max_events: options.max_events,
            cancel: options.cancel,
        })
    }
}

/// The ids next to each component.
type Neighbours<'a> = BTreeMap<&'a str, Vec<String>>;

/// Each component's neighbours in declared order: upstream and downstream.
fn neighbours(document: &Document) -> (Neighbours<'_>, Neighbours<'_>) {
    let mut upstream = Neighbours::new();
    let mut downstream = Neighbours::new();
    for edge in document.flow() {
        for from in edge.from() {
            for to in edge.to() {
                downstream
                    .entry(from.id())
                    .or_default()
                    .push(to.id().to_owned());
                upstream
                    .entry(to.id())
                    .or_default()
                    .push(from.id().to_owned());
            }
        }
    }
    (upstream, downstream)
}

/// How a run is asked for: everything except the model. The horizon is not
/// here: it is always the model's.
///
/// ```
/// use taktsim_components::RunOptions;
///
/// let options = RunOptions::new().seed(7).max_events(1000);
/// assert_eq!((options.seed, options.max_events, options.checks), (Some(7), Some(1000), true));
/// assert!(!options.without_checks().checks);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RunOptions<'a> {
    /// The seed; `None` takes the model's. Neither is an error, never entropy
    /// and never an unstated default.
    pub seed: Option<u64>,
    /// Whether invariants and conservation are checked after every event. On
    /// by default (spec §7 Layer 3).
    pub checks: bool,
    /// The most events the run processes.
    pub max_events: Option<u64>,
    /// Once set, the run stops, e.g. a wall-clock budget kept by the caller.
    pub cancel: Option<&'a AtomicBool>,
    /// A run with no seed: the engine's internal seed, which the header does not
    /// state. Set by [`no_seed`](Self::no_seed) and
    /// [`no_seed_check`](Self::no_seed_check).
    without_seed: Option<u64>,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> RunOptions<'a> {
    /// The model's seed, checks on, no limits.
    ///
    /// ```
    /// assert!(taktsim_components::RunOptions::new().checks);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            seed: None,
            checks: true,
            max_events: None,
            cancel: None,
            without_seed: None,
        }
    }

    /// With this seed instead of the model's.
    ///
    /// ```
    /// assert_eq!(taktsim_components::RunOptions::new().seed(3).seed, Some(3));
    /// ```
    #[must_use]
    pub const fn seed(self, seed: u64) -> Self {
        Self {
            seed: Some(seed),
            without_seed: None,
            ..self
        }
    }

    /// A run with no seed, for a model that draws nothing (ADR-0024 §5). The
    /// header states none and the engine has an internal seed of 0, which
    /// nothing may depend on. [`Registry::prepare`] refuses it for a model that
    /// draws.
    ///
    /// ```
    /// use taktsim_components::{RunFailure, RunOptions};
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let loader = Loader::new(registry.catalog());
    /// let text = |cycle: &str| format!(
    ///     "format: \"1.0\"\nmodel: {{name: m, seed: 42}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
    ///      components: [{{id: source, type: Source}}, {{id: cut, type: Machine, cycle_time: {cycle}}}, {{id: sink, type: Sink}}]\n\
    ///      flow: [[source, cut, sink]]\n"
    /// );
    /// let exact = loader.load_str(&text("1min"), Format::Yaml)?;
    /// let prepared = registry.prepare(&exact, RunOptions::new().no_seed())?;
    /// assert_eq!(prepared.header().seed, None);
    ///
    /// let drawn = loader.load_str(&text("{dist: exponential, mean: 1min}"), Format::Yaml)?;
    /// let refused = registry.prepare(&drawn, RunOptions::new().no_seed());
    /// assert!(matches!(refused, Err(RunFailure::DrawsRandom { .. })));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn no_seed(self) -> Self {
        Self {
            seed: None,
            without_seed: Some(0),
            ..self
        }
    }

    /// As [`no_seed`](Self::no_seed), with an internal seed of 1: for replaying a
    /// run with no seed under another seed, so a false claim of drawing nothing
    /// diverges (ADR-0024 §5).
    ///
    /// ```
    /// use taktsim_components::RunOptions;
    /// use taktsim_core::NoTrace;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, cut, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let (trace, _) = registry.prepare(&model, RunOptions::new().no_seed())?.run_traced(Vec::new())?;
    /// let report = registry.prepare(&model, RunOptions::new().no_seed_check())?.replay(trace.as_slice())?;
    /// assert!(report.events > 0);
    /// # let _ = NoTrace;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn no_seed_check(self) -> Self {
        Self {
            seed: None,
            without_seed: Some(1),
            ..self
        }
    }

    /// With checks off (`--fast`): only for calibration inner loops after one
    /// checked run.
    ///
    /// ```
    /// assert!(!taktsim_components::RunOptions::new().without_checks().checks);
    /// ```
    #[must_use]
    pub const fn without_checks(self) -> Self {
        Self {
            checks: false,
            ..self
        }
    }

    /// Stopping after `n` events.
    ///
    /// ```
    /// assert_eq!(taktsim_components::RunOptions::new().max_events(5).max_events, Some(5));
    /// ```
    #[must_use]
    pub const fn max_events(self, n: u64) -> Self {
        Self {
            max_events: Some(n),
            ..self
        }
    }

    /// Stopping as soon as `flag` is set.
    ///
    /// ```
    /// use std::sync::atomic::AtomicBool;
    ///
    /// let flag = AtomicBool::new(false);
    /// assert!(taktsim_components::RunOptions::new().cancel_on(&flag).cancel.is_some());
    /// ```
    #[must_use]
    pub const fn cancel_on(self, flag: &'a AtomicBool) -> Self {
        Self {
            cancel: Some(flag),
            ..self
        }
    }
}

/// A run that is ready to go.
///
/// ```
/// use taktsim_components::RunOptions;
/// use taktsim_core::NoTrace;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m, seed: 1}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
///      flow: [[source, sink]]\n",
///     Format::Yaml,
/// )?;
/// let report = registry.prepare(&model, RunOptions::new())?.run(&mut NoTrace)?;
/// assert_eq!(report.items.delivered.get(), 60);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Prepared<'a, D: Domain> {
    engine: Engine<D::Message>,
    header: Header,
    until: SimTime,
    max_events: Option<u64>,
    cancel: Option<&'a AtomicBool>,
}

impl<D: Domain> fmt::Debug for Prepared<'_, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("header", &self.header)
            .field("until", &self.until)
            .field("max_events", &self.max_events)
            .finish_non_exhaustive()
    }
}

impl<'a, D: Domain> Prepared<'a, D> {
    fn limits(&self) -> RunLimits<'a> {
        RunLimits {
            until: self.until,
            max_events: self.max_events,
            cancel: self.cancel,
        }
    }

    /// The trace header of this run, taken from its engine.
    ///
    /// ```
    /// use taktsim_components::RunOptions;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let prepared = registry.prepare(&model, RunOptions::new().seed(9))?;
    /// assert_eq!(prepared.header().seed, Some(9));
    /// assert_eq!(prepared.header().components, ["source", "sink"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Runs to the model's horizon, or a limit of the options, recording to
    /// `trace`.
    ///
    /// # Errors
    ///
    /// [`RunFailure::Run`] if the run fails.
    ///
    /// ```
    /// use taktsim_components::RunOptions;
    /// use taktsim_core::{NoTrace, RunOutcome};
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let report = registry.prepare(&model, RunOptions::new().seed(1).max_events(3))?.run(&mut NoTrace)?;
    /// assert_eq!(report.summary.outcome, RunOutcome::EventLimit);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn run(mut self, trace: &mut dyn TraceSink<D::Message>) -> Result<RunReport, RunFailure> {
        let limits = self.limits();
        let summary = self
            .engine
            .run(limits, trace)
            .map_err(|e| RunFailure::Run(Box::new(e)))?;
        Ok(RunReport {
            summary,
            items: self.engine.item_counts(),
            trace: None,
        })
    }

    /// Runs and writes the trace to `out`, always stating how the run ended
    /// (ADR-0022 §9). Returns `out` and the report.
    ///
    /// # Errors
    ///
    /// [`RunFailure::Write`] if the trace cannot be written, [`RunFailure::Run`]
    /// if the run fails.
    ///
    /// ```
    /// use taktsim_components::RunOptions;
    /// use taktsim_core::RunOutcome;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let (bytes, report) = registry.prepare(&model, RunOptions::new().seed(1))?.run_traced(Vec::new())?;
    /// let trace = report.trace.ok_or("a trace was written")?;
    /// assert_eq!(trace.bytes, bytes.len() as u64);
    /// assert_eq!(trace.end.map(|e| e.outcome), Some(RunOutcome::Horizon));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn run_traced<W: Write>(mut self, out: W) -> Result<(W, RunReport), RunFailure> {
        let mut writer = TraceWriter::<W, D::Message>::new(out, &self.header)
            .map_err(|e| RunFailure::Write(Box::new(e)))?;
        let limits = self.limits();
        let summary = self
            .engine
            .run(limits, &mut writer)
            .map_err(|e| RunFailure::Run(Box::new(e)))?;
        writer.run_ended(&summary);
        let (out, trace) = writer
            .finish()
            .map_err(|e| RunFailure::Write(Box::new(e)))?;
        Ok((
            out,
            RunReport {
                summary,
                items: self.engine.item_counts(),
                trace: Some(trace),
            },
        ))
    }
}

impl<D: Domain> Prepared<'_, D> {
    /// Re-executes the run recorded in `reader` with this engine and compares it
    /// at every event (ADR-0021), through the registry, so a caller needs no
    /// type of the domain. It compares the seed if the trace states one.
    ///
    /// # Errors
    ///
    /// [`RunFailure::Replay`] if the trace cannot be read or the replay differs.
    ///
    /// ```
    /// use taktsim_components::RunOptions;
    /// use taktsim_model::{Format, Loader};
    ///
    /// let registry = taktsim_components::standard()?;
    /// let model = Loader::new(registry.catalog()).load_str(
    ///     "format: \"1.0\"\nmodel: {name: m, seed: 1}\nexperiment: {type: terminating, horizon: 1h}\n\
    ///      components: [{id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}, {id: sink, type: Sink}]\n\
    ///      flow: [[source, sink]]\n",
    ///     Format::Yaml,
    /// )?;
    /// let (trace, written) = registry.prepare(&model, RunOptions::new())?.run_traced(Vec::new())?;
    /// let report = registry.prepare(&model, RunOptions::new())?.replay(trace.as_slice())?;
    /// assert_eq!(Some(report.hash), written.trace.map(|t| t.hash));
    /// assert!(registry.prepare(&model, RunOptions::new().seed(2))?.replay(trace.as_slice()).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn replay<R: Read>(mut self, reader: R) -> Result<ReplayReport, RunFailure> {
        let trace = TraceReader::<R, D::Message>::new(reader)
            .map_err(|e| RunFailure::Replay(Box::new(ReplayError::Trace(e))))?;
        replay(trace, &mut self.engine).map_err(|e| RunFailure::Replay(Box::new(e)))
    }
}

/// What a run did.
///
/// ```
/// use taktsim_components::RunReport;
/// use taktsim_core::{ItemCounts, RunOutcome, RunSummary};
///
/// let report = RunReport {
///     summary: RunSummary { outcome: RunOutcome::Idle, events: 0, now: "1h".parse()? },
///     items: ItemCounts::default(),
///     trace: None,
/// };
/// assert_eq!(report.items.delivered.get(), 0);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RunReport {
    /// Why it stopped, how many events, and the clock.
    pub summary: RunSummary,
    /// Items by where they are or how they left.
    pub items: ItemCounts,
    /// What the trace holds, if it was written by
    /// [`run_traced`](Prepared::run_traced).
    pub trace: Option<Summary>,
}

/// Why a document cannot become an engine. Each variant names the field path.
///
/// ```
/// use taktsim_components::BuildError;
/// use taktsim_model::{FieldPath, Segment};
///
/// let e = BuildError::UnknownType {
///     path: FieldPath::from(vec![Segment::Key("components".into()), Segment::Index(1)]),
///     type_name: "Conveyor".into(),
/// };
/// assert_eq!(e.path().map(ToString::to_string).as_deref(), Some("components[1]"));
/// assert_eq!(
///     e.to_string(),
///     "components[1]: this registry has no component type `Conveyor`; load the model with the registry that runs it"
/// );
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The document breaks what the domain checks: every error with its field
    /// path, what was found, what is allowed and an example.
    #[error("{0}")]
    Model(#[source] Box<LoadErrors>),
    /// The registry has no such component type: the document was loaded with
    /// another catalog.
    #[error(
        "{path}: this registry has no component type `{type_name}`; load the model with the registry that runs it"
    )]
    UnknownType {
        /// The entry.
        path: FieldPath,
        /// The type.
        type_name: String,
    },
    /// The entry's parameters are not this registry's type of that name: the
    /// document was loaded with another catalog.
    #[error(
        "{path}: the parameters of `{type_name}` are of another type than this registry's; load the model with the registry that runs it"
    )]
    WrongParameters {
        /// The entry.
        path: FieldPath,
        /// The type.
        type_name: String,
    },
    /// A distribution cannot be sampled. A distribution the loader read always
    /// can.
    #[error("{path}: {source}")]
    Distribution {
        /// The entry.
        path: FieldPath,
        /// Why.
        #[source]
        source: Box<DistributionError>,
    },
    /// A variant name the model's list does not hold. A check at load time
    /// finds every such name first.
    #[error(
        "{path}: no variant `{name}` in this model; declare it on a Source, e.g. `variants: {{{name}: 100%}}`"
    )]
    UnknownVariant {
        /// The entry.
        path: FieldPath,
        /// The name.
        name: String,
    },
    /// The engine refuses the component.
    #[error("{path}: {source}")]
    Engine {
        /// The entry.
        path: FieldPath,
        /// Why.
        #[source]
        source: Box<EngineError>,
    },
}

impl BuildError {
    /// The field path of what cannot be built: the first error's, for
    /// [`Model`](Self::Model).
    ///
    /// ```
    /// use taktsim_components::BuildError;
    /// use taktsim_model::FieldPath;
    ///
    /// let e = BuildError::WrongParameters { path: FieldPath::default(), type_name: "Sink".into() };
    /// assert_eq!(e.path().map(ToString::to_string).as_deref(), Some("(document)"));
    /// ```
    #[must_use]
    pub fn path(&self) -> Option<&FieldPath> {
        Some(match self {
            Self::Model(errors) => errors.first().path(),
            Self::UnknownType { path, .. }
            | Self::WrongParameters { path, .. }
            | Self::Distribution { path, .. }
            | Self::UnknownVariant { path, .. }
            | Self::Engine { path, .. } => path,
        })
    }
}

/// Why a run could not be done.
///
/// ```
/// use taktsim_components::RunFailure;
/// use taktsim_model::{FieldPath, Segment};
///
/// let e = RunFailure::NoSeed {
///     path: FieldPath::from(vec![Segment::Key("model".into()), Segment::Key("seed".into())]),
/// };
/// assert_eq!(
///     e.to_string(),
///     "model.seed: no seed: give one in the run options or in the model, e.g. `model: {name: line-a, seed: 42}`; a run never takes one from entropy"
/// );
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RunFailure {
    /// Neither the options nor the model give a seed.
    #[error(
        "{path}: no seed: give one in the run options or in the model, e.g. `model: {{name: line-a, seed: 42}}`; a run never takes one from entropy"
    )]
    NoSeed {
        /// Where the model would give it.
        path: FieldPath,
    },
    /// A run with no seed was asked for, and the model draws random numbers.
    #[error(
        "{path}: this model draws random numbers, so a run of it needs a seed, e.g. `model: {{name: line-a, seed: 42}}`"
    )]
    DrawsRandom {
        /// What draws: a component's entry, or `model.seed` if the domain does.
        path: FieldPath,
    },
    /// The document cannot become an engine.
    #[error(transparent)]
    Build(Box<BuildError>),
    /// The replay failed.
    #[error(transparent)]
    Replay(Box<ReplayError>),
    /// The run failed.
    #[error("the run failed: {0}")]
    Run(#[source] Box<RunError>),
    /// The trace could not be written.
    #[error(transparent)]
    Write(Box<WriteError>),
}
