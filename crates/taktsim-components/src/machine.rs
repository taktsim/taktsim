//! Machine: a station that works, fails and changes over (spec §4,
//! ADR-0022 §5, §6, §7).

use std::collections::{BTreeMap, VecDeque};

use taktsim_core::random::{DurationDistribution, Stream};
use taktsim_core::{Component, ComponentError, Context, Delivery, EventId, ItemId, Violation};
use taktsim_model::{
    ComponentParams, ComponentRef, Decode, Decoder, Distribution, Encode, Fields, Json,
    LoadErrorKind, Node, Record, Schemas, Segment, Value,
};
use taktsim_units::{Count, SimDuration, SimTime};

use crate::flow::{Flow, FlowShared, variant_id};
use crate::message::{FlowMessage, ItemAttrs, MachineStateNote, VariantId};
use crate::params::{CycleTime, Mtbf, Mttr, PerVariant, Units, VariantRef, draws};
use crate::port::{Inlet, Outlet};
use crate::run::{BuildError, ComponentType, Wiring};
use crate::state::MachineState;

/// One entry of a setup's `matrix`: the time of one change of variant.
#[derive(Debug, Clone)]
pub(crate) struct Change {
    pub(crate) from: ComponentRef,
    pub(crate) to: ComponentRef,
    time: Distribution,
}

impl Record for Change {
    const EXPECTED: &'static str =
        "a change of variant such as `{from: basic, to: premium, time: 6min}`";
    const EXAMPLE: &'static str = "{from: basic, to: premium, time: 6min}";

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let from = fields.required::<VariantRef>("from");
        let to = fields.required::<VariantRef>("to");
        let time = fields.required("time");
        let (from, to, time) = (from?.0, to?.0, time?);
        if from == to
            && let Some(node) = fields.node()
        {
            fields.decoder().not_allowed(
                node,
                format!("a change from `{from}` to `{to}`"),
                "a change between two different variants",
                <Self as Record>::EXAMPLE,
            );
            return None;
        }
        Some(Self { from, to, time })
    }
}

impl Encode for Change {
    fn encode(&self) -> Json {
        Json::object([])
            .with("from", &self.from)
            .with("time", &self.time)
            .with("to", &self.to)
    }
}

/// A setup's `matrix`: sorted by `(from, to)`, at least one entry, no pair
/// twice.
#[derive(Debug, Clone)]
struct Matrix(Vec<Change>);

impl Decode for Matrix {
    const EXPECTED: &'static str =
        "a list of changes such as `[{from: basic, to: premium, time: 6min}]`";
    const EXAMPLE: &'static str = "[{from: basic, to: premium, time: 6min}]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::List(items) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        if items.is_empty() {
            let kind = LoadErrorKind::TooFew {
                what: "a setup's matrix",
                min: 1,
                found: 0,
                example: Self::EXAMPLE,
            };
            cx.error(node.start(), kind);
            return None;
        }
        let mut changes = Vec::<Change>::decode(node, cx)?;
        let mut ok = true;
        for (i, change) in changes.iter().enumerate() {
            if changes[..i]
                .iter()
                .any(|c| c.from == change.from && c.to == change.to)
            {
                cx.within(Segment::Index(i), |cx| {
                    cx.not_allowed(
                        &items[i],
                        format!("the change from `{}` to `{}` twice", change.from, change.to),
                        "each change once",
                        Self::EXAMPLE,
                    );
                });
                ok = false;
            }
        }
        if !ok {
            return None;
        }
        changes.sort_by(|a, b| (a.from.id(), a.to.id()).cmp(&(b.from.id(), b.to.id())));
        Some(Self(changes))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<Change>()),
            ("minItems", Json::from(1)),
            (
                "description",
                Json::from(
                    "The changeover time of each listed change of variant. `0s` means the change \
                     costs nothing: no timer, no `setup` state.",
                ),
            ),
        ])
    }
}

/// A Machine's `setup`: at least one of `per_variant` and `matrix`. The first
/// unit costs no setup. A unit of another variant needs a changeover: `matrix`
/// if it has the pair, otherwise `per_variant`, otherwise none.
///
/// ```
/// use taktsim_components::Machine;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     r#"
/// format: "1.0"
/// model: {name: m}
/// experiment: {type: terminating, horizon: 1h}
/// components:
///   - {id: source, type: Source, variants: {basic: 80%, premium: 20%}}
///   - id: weld
///     type: Machine
///     cycle_time: 45s
///     setup:
///       per_variant: 4min
///       matrix:
///         - {from: premium, to: basic, time: 0s}
///         - {from: basic, to: premium, time: 6min}
///   - {id: sink, type: Sink}
/// flow: [[source, weld, sink]]
/// "#,
///     Format::Yaml,
/// )?;
/// // The matrix is sorted in the normalized form.
/// assert!(model.canonical_json().contains(
///     r#""matrix":[{"from":"basic","time":"6min","to":"premium"},{"from":"premium","time":"0s","to":"basic"}]"#
/// ));
/// assert!(model.document().components()[1].params::<Machine>().is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Setup {
    per_variant: Option<Distribution>,
    matrix: Option<Matrix>,
}

impl Setup {
    /// Whether any of its times is drawn.
    fn draws_random(&self) -> bool {
        let Self {
            per_variant,
            matrix,
        } = self;
        per_variant.as_ref().is_some_and(draws)
            || matrix.as_ref().is_some_and(|Matrix(changes)| {
                changes.iter().any(|change| {
                    let Change {
                        from: _,
                        to: _,
                        time,
                    } = change;
                    draws(time)
                })
            })
    }

    /// The listed changes, sorted by `(from, to)`.
    pub(crate) fn changes(&self) -> &[Change] {
        self.matrix.as_ref().map_or(&[], |m| m.0.as_slice())
    }
}

impl Record for Setup {
    const EXPECTED: &'static str = "a setup such as `{per_variant: 4min}`";
    const EXAMPLE: &'static str = "{per_variant: 4min}";

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let per_variant = fields.optional::<PerVariant>("per_variant");
        let matrix = fields.optional("matrix");
        let (per_variant, matrix) = (per_variant?.map(|p| p.0), matrix?);
        if per_variant.is_none() && matrix.is_none() {
            // With other keys in it, they are reported as unknown instead.
            if let Some(node) = fields.node()
                && matches!(node.value(), Value::Map(entries) if entries.is_empty())
            {
                fields.decoder().not_allowed(
                    node,
                    "a setup without times",
                    "at least one of `per_variant` and `matrix`; leave out `setup` if variant \
                     changes cost nothing",
                    <Self as Record>::EXAMPLE,
                );
            }
            return None;
        }
        Some(Self {
            per_variant,
            matrix,
        })
    }
}

/// `setup` as a value, for the schema's rule that at least one key is there.
#[derive(Debug, Clone)]
struct SetupEntry(Setup);

impl Decode for SetupEntry {
    const EXPECTED: &'static str = <Setup as Record>::EXPECTED;
    const EXAMPLE: &'static str = <Setup as Record>::EXAMPLE;

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        Setup::decode(node, cx).map(Self)
    }

    fn schema(schemas: &mut Schemas) -> Json {
        let required = |key: &str| Json::object([("required", Json::Array(vec![Json::from(key)]))]);
        schemas
            .of::<Setup>()
            .with(
                "anyOf",
                &Json::Array(vec![required("per_variant"), required("matrix")]),
            )
            .with(
                "description",
                &"Changeover times. The first unit costs none; a unit of another variant takes \
                  the `matrix` entry for that change, otherwise `per_variant`, otherwise none.",
            )
    }
}

impl Encode for Setup {
    fn encode(&self) -> Json {
        let value = Json::object([]).with_optional("per_variant", self.per_variant.as_ref());
        match &self.matrix {
            Some(matrix) => value.with("matrix", &matrix.0),
            None => value,
        }
    }
}

/// `type: Machine`: `cycle_time`, required; `mtbf` and `mttr`, both or neither;
/// `setup`; and `capacity`, default `1 parts`.
///
/// ```
/// use taktsim_components::Machine;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components:\n\
///      - {id: source, type: Source}\n\
///      - {id: cut, type: Machine, cycle_time: 42s, mtbf: {dist: exponential, mean: 6h}, mttr: 12min, capacity: 2 parts}\n\
///      - {id: sink, type: Sink}\n\
///      flow: [[source, cut, sink]]\n",
///     Format::Yaml,
/// )?;
/// assert!(model.document().components()[1].params::<Machine>().is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Machine {
    cycle_time: Distribution,
    mtbf: Option<Distribution>,
    mttr: Option<Distribution>,
    setup: Option<Setup>,
    capacity: Option<Count>,
}

impl Machine {
    /// The setup, if the machine has one.
    pub(crate) const fn setup(&self) -> Option<&Setup> {
        self.setup.as_ref()
    }
}

impl Encode for Machine {
    fn encode(&self) -> Json {
        Json::object([])
            .with("cycle_time", &self.cycle_time)
            .with_optional("mtbf", self.mtbf.as_ref())
            .with_optional("mttr", self.mttr.as_ref())
            .with_optional("setup", self.setup.as_ref())
            .with_optional("capacity", self.capacity.as_ref())
    }
}

impl ComponentParams for Machine {
    const TYPE: &'static str = "Machine";
    const EXAMPLE: &'static str = "{id: cut, type: Machine, cycle_time: 42s}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let cycle_time = fields.required::<CycleTime>("cycle_time");
        let mtbf = fields.optional::<Mtbf>("mtbf");
        let mttr = fields.optional::<Mttr>("mttr");
        let setup = fields.optional::<SetupEntry>("setup");
        let capacity = fields.optional::<Units>("capacity");
        let lone = match (fields.entry("mtbf"), fields.entry("mttr")) {
            (Some(entry), None) => Some(("mtbf", entry)),
            (None, Some(entry)) => Some(("mttr", entry)),
            (Some(_), Some(_)) | (None, None) => None,
        };
        if let Some((key, entry)) = lone {
            fields.decoder().within(Segment::Key(key.to_owned()), |cx| {
                cx.not_allowed(
                    entry.value(),
                    format!("`{key}` alone"),
                    "`mtbf` and `mttr` together, or neither",
                    "{mtbf: {dist: exponential, mean: 6h}, mttr: 12min}",
                );
            });
            return None;
        }
        Some(Self {
            cycle_time: cycle_time?.0,
            mtbf: mtbf?.map(|m| m.0),
            mttr: mttr?.map(|m| m.0),
            setup: setup?.map(|s| s.0),
            capacity: capacity?.map(|c| c.0),
        })
    }
}

impl ComponentType<Flow> for Machine {
    const STATION: bool = true;

    fn draws_random(&self) -> bool {
        let Self {
            cycle_time,
            mtbf,
            mttr,
            setup,
            capacity: _,
        } = self;
        draws(cycle_time)
            || mtbf.as_ref().is_some_and(draws)
            || mttr.as_ref().is_some_and(draws)
            || setup.as_ref().is_some_and(Setup::draws_random)
    }

    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        let path = wiring.path();
        let sampler = |d: &Distribution| {
            d.sampler().map_err(|source| BuildError::Distribution {
                path: path.clone(),
                source: Box::new(source),
            })
        };
        let failures = match (&self.mtbf, &self.mttr) {
            (Some(mtbf), Some(mttr)) => Some(FailureProcess {
                up: sampler(mtbf)?,
                repair: sampler(mttr)?,
                streams: None,
            }),
            (Some(_) | None, None) | (None, Some(_)) => None,
        };
        let mut matrix = BTreeMap::new();
        let mut per_variant = None;
        if let Some(setup) = &self.setup {
            per_variant = setup.per_variant.as_ref().map(sampler).transpose()?;
            for change in setup.changes() {
                let pair = (
                    variant_id(wiring.shared(), change.from.id(), path)?,
                    variant_id(wiring.shared(), change.to.id(), path)?,
                );
                let time = if change.time == Distribution::Constant(SimDuration::ZERO) {
                    None
                } else {
                    Some(sampler(&change.time)?)
                };
                matrix.insert(pair, time);
            }
        }
        Ok(Box::new(MachineBehaviour {
            capacity: self.capacity.map_or(1, Count::get),
            cycle_time: sampler(&self.cycle_time)?,
            cycle_stream: None,
            failures,
            per_variant,
            matrix,
            setup_stream: None,
            has_setup: self.setup.is_some(),
            source_near: wiring.shared().is_idle_when_empty(wiring.id()),
            downstream: wiring.downstream().to_vec(),
            inlet: Inlet::default(),
            outlet: Outlet::default(),
            waiting: VecDeque::new(),
            cycles: BTreeMap::new(),
            finished: VecDeque::new(),
            changeover: None,
            ready_for: None,
            down: false,
            noted: None,
            bad_transition: None,
        }))
    }
}

/// When the machine fails and how long a repair takes: one machine, one
/// failure process. The clock is the calendar: it runs while up, whatever the
/// machine does, and pauses while down (ADR-0022 §6). Later an operating-time
/// clock can pause this timer with the same cancellation.
struct FailureProcess {
    up: DurationDistribution,
    repair: DurationDistribution,
    /// The `failures` and `repairs` streams, once started.
    streams: Option<(Stream, Stream)>,
}

impl FailureProcess {
    /// Takes the streams and arms the first failure.
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.streams = Some((ctx.stream("failures")?, ctx.stream("repairs")?));
        self.arm(ctx)
    }

    /// Draws the next up time and schedules the failure.
    fn arm(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if let Some((failures, _)) = &mut self.streams {
            let up = self.up.sample(failures)?;
            ctx.schedule(up, FlowMessage::Failure)?;
        }
        Ok(())
    }

    /// Draws the repair time and schedules its end.
    fn repair(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if let Some((_, repairs)) = &mut self.streams {
            let time = self.repair.sample(repairs)?;
            ctx.schedule(time, FlowMessage::Repaired)?;
        }
        Ok(())
    }
}

/// A running or paused timer: it knows its end while it runs, and what remains
/// while paused.
#[derive(Debug, Clone, Copy)]
enum Timer {
    Running { event: EventId, ends: SimTime },
    Paused { remaining: SimDuration },
}

impl Timer {
    /// Starts a timer of `after` with `message`.
    fn start(
        after: SimDuration,
        message: FlowMessage,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<Self, ComponentError> {
        let ends = ctx.now().checked_add(after)?;
        let event = ctx.schedule(after, message)?;
        Ok(Self::Running { event, ends })
    }

    /// Cancels the timer and keeps what remains.
    fn pause(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if let Self::Running { event, ends } = *self {
            ctx.cancel(event)?;
            *self = Self::Paused {
                remaining: ends.checked_duration_since(ctx.now())?,
            };
        }
        Ok(())
    }

    /// Schedules what remains again with `message`.
    fn resume(
        &mut self,
        message: FlowMessage,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        if let Self::Paused { remaining } = *self {
            *self = Self::start(remaining, message, ctx)?;
        }
        Ok(())
    }
}

/// A changeover: to which variant, and its timer.
struct Changeover {
    to: VariantId,
    timer: Timer,
}

/// A Machine at run time.
struct MachineBehaviour {
    capacity: u64,
    cycle_time: DurationDistribution,
    cycle_stream: Option<Stream>,
    failures: Option<FailureProcess>,
    per_variant: Option<DurationDistribution>,
    /// Listed changes; `None` for a `0s` entry, which is no changeover.
    matrix: BTreeMap<(VariantId, VariantId), Option<DurationDistribution>>,
    setup_stream: Option<Stream>,
    has_setup: bool,
    /// When empty it is `idle`, not `starved`: a Source reaches it through
    /// non-stations only (ADR-0022 §5).
    source_near: bool,
    downstream: Vec<String>,
    inlet: Inlet,
    outlet: Outlet,
    /// Arrived and not started, in arrival order.
    waiting: VecDeque<(ItemId, ItemAttrs)>,
    /// Each unit in its cycle, found by id for its timer only: no order a part
    /// sees follows from this id.
    cycles: BTreeMap<ItemId, (ItemAttrs, Timer)>,
    /// Finished, in the order they finished; they keep their places.
    finished: VecDeque<(ItemId, ItemAttrs)>,
    changeover: Option<Changeover>,
    /// The variant it is set up for; `None` until the first unit, which costs
    /// no setup.
    ready_for: Option<VariantId>,
    down: bool,
    noted: Option<MachineStateNote>,
    /// A change of state the table of ADR-0022 §5 does not allow.
    bad_transition: Option<(MachineState, MachineState)>,
}

impl MachineBehaviour {
    /// The state, from what the machine holds. The first line that holds
    /// wins.
    fn state(&self) -> MachineState {
        if self.down {
            MachineState::Down
        } else if self.changeover.is_some() {
            MachineState::Setup
        } else if !self.cycles.is_empty() {
            MachineState::Busy
        } else if !self.finished.is_empty() {
            MachineState::Blocked
        } else if self.source_near {
            MachineState::Idle
        } else {
            MachineState::Starved
        }
    }

    /// Places taken: by units and by reservations.
    fn taken(&self) -> u64 {
        (self.waiting.len() + self.cycles.len() + self.finished.len()) as u64
            + self.inlet.reserved()
    }

    /// Starts every unit that can start: in arrival order, and a unit whose
    /// change costs time waits at the front until no unit is in a cycle, then
    /// the changeover runs, without waiting for finished units to leave.
    fn start_units(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        while self.changeover.is_none() {
            let Some(&(item, attrs)) = self.waiting.front() else {
                break;
            };
            let variant = attrs.variant;
            let change = self
                .ready_for
                .filter(|ready| *ready != variant)
                .and_then(|ready| match self.matrix.get(&(ready, variant)) {
                    Some(time) => time.as_ref(),
                    None => self.per_variant.as_ref(),
                });
            if let Some(time) = change {
                if !self.cycles.is_empty() {
                    break;
                }
                let Some(stream) = self.setup_stream.as_mut() else {
                    return Err(ComponentError::Unexpected {
                        state: "a changeover without a setup stream",
                        event: "start".to_owned(),
                    });
                };
                let after = time.sample(stream)?;
                self.changeover = Some(Changeover {
                    to: variant,
                    timer: Timer::start(after, FlowMessage::SetupDone, ctx)?,
                });
                break;
            }
            self.ready_for = Some(variant);
            self.waiting.pop_front();
            let Some(stream) = self.cycle_stream.as_mut() else {
                return Err(ComponentError::Unexpected {
                    state: "a cycle before the machine started",
                    event: "start".to_owned(),
                });
            };
            let after = self.cycle_time.sample(stream)?;
            let timer = Timer::start(after, FlowMessage::CycleDone { item }, ctx)?;
            self.cycles.insert(item, (attrs, timer));
        }
        Ok(())
    }

    /// Does what can be done now: a down machine moves nothing. Then writes a
    /// note if the state or the count of units in a cycle changed.
    fn settle(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        if !self.down {
            self.start_units(ctx)?;
            self.inlet
                .accept(self.capacity.saturating_sub(self.taken()), ctx)?;
            if self.finished.is_empty() {
                self.outlet.withdraw(ctx)?;
            } else {
                self.outlet.offer(ctx)?;
            }
        }
        self.write_note(ctx);
        Ok(())
    }

    /// One note after an event, if anything changed.
    fn write_note(&mut self, ctx: &mut Context<'_, FlowMessage>) {
        let state = self.state();
        // At most 65,535 units (`capacity`).
        let running = u32::try_from(self.cycles.len()).unwrap_or(u32::MAX);
        let note = MachineStateNote {
            state,
            units_in_cycle: if self.down { 0 } else { running },
            setup_variant: match state {
                MachineState::Setup => self.changeover.as_ref().map(|c| c.to),
                MachineState::Idle
                | MachineState::Busy
                | MachineState::Blocked
                | MachineState::Starved
                | MachineState::Down => None,
            },
        };
        if self.noted == Some(note) {
            return;
        }
        if let Some(before) = self.noted
            && before.state != state
            && !before.state.successors().contains(&state)
        {
            self.bad_transition.get_or_insert((before.state, state));
        }
        self.noted = Some(note);
        ctx.note(&FlowMessage::MachineState(note));
    }

    /// A failure: cancel each timer and keep what remains (ADR-0001, ADR-0018).
    fn fail(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.down = true;
        for (_, timer) in self.cycles.values_mut() {
            timer.pause(ctx)?;
        }
        if let Some(changeover) = &mut self.changeover {
            changeover.timer.pause(ctx)?;
        }
        if let Some(process) = &mut self.failures {
            process.repair(ctx)?;
        }
        Ok(())
    }

    /// A repair ended: everything resumes, in `ItemId` order, and the next up
    /// time is drawn.
    fn repaired(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.down = false;
        for (item, (_, timer)) in &mut self.cycles {
            timer.resume(FlowMessage::CycleDone { item: *item }, ctx)?;
        }
        if let Some(changeover) = &mut self.changeover {
            changeover.timer.resume(FlowMessage::SetupDone, ctx)?;
        }
        if let Some(process) = &mut self.failures {
            process.arm(ctx)?;
        }
        Ok(())
    }
}

impl Component<FlowMessage> for MachineBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.outlet.connect(&self.downstream, ctx)?;
        self.cycle_stream = Some(ctx.stream("cycle_time")?);
        if self.has_setup {
            self.setup_stream = Some(ctx.stream("setup")?);
        }
        if let Some(process) = &mut self.failures {
            process.start(ctx)?;
        }
        self.write_note(ctx);
        Ok(())
    }

    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        match d.message {
            FlowMessage::Failure => self.fail(ctx)?,
            FlowMessage::Repaired => self.repaired(ctx)?,
            FlowMessage::CycleDone { item } => {
                let Some((attrs, _)) = self.cycles.remove(item) else {
                    return Err(self.unexpected(d.message));
                };
                self.finished.push_back((*item, attrs));
            }
            FlowMessage::SetupDone => {
                let Some(changeover) = self.changeover.take() else {
                    return Err(self.unexpected(d.message));
                };
                self.ready_for = Some(changeover.to);
            }
            FlowMessage::Offer => self.inlet.offered(d.from),
            FlowMessage::Withdraw => self.inlet.withdrawn(d.from),
            FlowMessage::Decline => self.inlet.release("Decline")?,
            FlowMessage::Arrive(attrs) => {
                self.inlet.release("Arrive")?;
                let Some(item) = d.item else {
                    return Err(self.unexpected(d.message));
                };
                ctx.receive(item)?;
                self.waiting.push_back((item, *attrs));
            }
            FlowMessage::Accept => {
                self.outlet.accepted(d.from, ctx)?;
                let finished = if self.down {
                    None
                } else {
                    self.finished.pop_front()
                };
                match finished {
                    Some((item, attrs)) => {
                        ctx.transfer(item, d.from, FlowMessage::Arrive(attrs))?;
                    }
                    None => ctx.send(d.from, FlowMessage::Decline)?,
                }
            }
            message @ (FlowMessage::NextArrival
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }) => return Err(self.unexpected(message)),
        }
        self.settle(ctx)
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        for (item, _) in &self.waiting {
            visit(*item);
        }
        for item in self.cycles.keys() {
            visit(*item);
        }
        for (item, _) in &self.finished {
            visit(*item);
        }
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        let taken = self.taken();
        if taken > self.capacity {
            return Err(Violation {
                invariant: "units in cycle + waiting to start + finished + reserved places <= capacity",
                detail: format!("{taken} places taken, capacity {}", self.capacity),
            });
        }
        if let Some((from, to)) = self.bad_transition {
            return Err(Violation {
                invariant: "state changes follow the table of ADR-0022 §5",
                detail: format!("{} to {}", from.as_str(), to.as_str()),
            });
        }
        if self.noted.map(|n| n.state) != Some(self.state()) {
            return Err(Violation {
                invariant: "the noted state is what the machine holds implies",
                detail: format!("{} from what it holds", self.state().as_str()),
            });
        }
        if self.changeover.is_some() && !self.cycles.is_empty() {
            return Err(Violation {
                invariant: "no changeover runs while a unit is in its cycle",
                detail: format!("{} units in a cycle", self.cycles.len()),
            });
        }
        if !self.down {
            if taken < self.capacity && self.inlet.has_waiting() {
                return Err(Violation {
                    invariant: "unless down, a free place and a waiting offer never coexist",
                    detail: format!("{taken} places taken, capacity {}", self.capacity),
                });
            }
            if !self.finished.is_empty() && !self.outlet.all_offered() {
                return Err(Violation {
                    invariant: "unless down, a finished unit and no outstanding offer never coexist",
                    detail: format!("{} finished", self.finished.len()),
                });
            }
            if !self.waiting.is_empty() && self.cycles.is_empty() && self.changeover.is_none() {
                return Err(Violation {
                    invariant: "unless down, a unit waits to start only behind a cycle or a changeover",
                    detail: format!("{} waiting", self.waiting.len()),
                });
            }
        }
        Ok(())
    }
}

impl MachineBehaviour {
    fn unexpected(&self, message: &FlowMessage) -> ComponentError {
        ComponentError::Unexpected {
            state: self.state().as_str(),
            event: format!("{message:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use taktsim_core::ComponentId;

    use super::*;

    fn machine(capacity: u64) -> MachineBehaviour {
        let mut m = MachineBehaviour {
            capacity,
            cycle_time: DurationDistribution::constant(SimDuration::ZERO),
            cycle_stream: None,
            failures: None,
            per_variant: None,
            matrix: BTreeMap::new(),
            setup_stream: None,
            has_setup: false,
            source_near: true,
            downstream: Vec::new(),
            inlet: Inlet::default(),
            outlet: Outlet::default(),
            waiting: VecDeque::new(),
            cycles: BTreeMap::new(),
            finished: VecDeque::new(),
            changeover: None,
            ready_for: None,
            down: false,
            noted: None,
            bad_transition: None,
        };
        note(&mut m);
        m
    }

    /// Says the note matches, as after an event.
    fn note(m: &mut MachineBehaviour) {
        m.noted = Some(MachineStateNote {
            state: m.state(),
            units_in_cycle: 0,
            setup_variant: None,
        });
    }

    fn unit(n: u64) -> (ItemId, ItemAttrs) {
        (ItemId::from_raw(n), ItemAttrs::new(VariantId::new(0), 0))
    }

    fn running(m: &mut MachineBehaviour, n: u64) {
        let (item, attrs) = unit(n);
        let timer = Timer::Paused {
            remaining: SimDuration::ZERO,
        };
        m.cycles.insert(item, (attrs, timer));
    }

    fn violated(m: &MachineBehaviour) -> &'static str {
        m.check_invariants().unwrap_err().invariant
    }

    #[test]
    fn its_invariants_catch_what_they_say() {
        assert!(machine(1).check_invariants().is_ok());

        let mut m = machine(1);
        m.finished.push_back(unit(0));
        m.finished.push_back(unit(1));
        note(&mut m);
        assert_eq!(
            violated(&m),
            "units in cycle + waiting to start + finished + reserved places <= capacity"
        );

        let mut m = machine(1);
        m.bad_transition = Some((MachineState::Busy, MachineState::Idle));
        assert_eq!(
            violated(&m),
            "state changes follow the table of ADR-0022 §5"
        );

        let mut m = machine(1);
        running(&mut m, 0);
        assert_eq!(
            violated(&m),
            "the noted state is what the machine holds implies"
        );

        let mut m = machine(2);
        running(&mut m, 0);
        m.changeover = Some(Changeover {
            to: VariantId::new(1),
            timer: Timer::Paused {
                remaining: SimDuration::ZERO,
            },
        });
        note(&mut m);
        assert_eq!(
            violated(&m),
            "no changeover runs while a unit is in its cycle"
        );

        let mut m = machine(1);
        m.inlet.offered(ComponentId::from_index(0));
        assert_eq!(
            violated(&m),
            "unless down, a free place and a waiting offer never coexist"
        );
        // A down machine queues offers without accepting.
        m.down = true;
        note(&mut m);
        assert!(m.check_invariants().is_ok());

        let mut m = machine(1);
        m.finished.push_back(unit(0));
        m.outlet = Outlet::with_unoffered_target();
        note(&mut m);
        assert_eq!(
            violated(&m),
            "unless down, a finished unit and no outstanding offer never coexist"
        );

        let mut m = machine(1);
        m.waiting.push_back(unit(0));
        note(&mut m);
        assert_eq!(
            violated(&m),
            "unless down, a unit waits to start only behind a cycle or a changeover"
        );
    }

    #[test]
    fn the_state_is_what_it_holds_implies_the_first_line_wins() {
        let mut m = machine(4);
        assert_eq!(m.state(), MachineState::Idle);
        m.source_near = false;
        assert_eq!(m.state(), MachineState::Starved);
        m.finished.push_back(unit(0));
        assert_eq!(m.state(), MachineState::Blocked);
        running(&mut m, 1);
        assert_eq!(m.state(), MachineState::Busy);
        m.changeover = Some(Changeover {
            to: VariantId::new(1),
            timer: Timer::Paused {
                remaining: SimDuration::ZERO,
            },
        });
        assert_eq!(m.state(), MachineState::Setup);
        m.down = true;
        assert_eq!(m.state(), MachineState::Down);
    }
}
