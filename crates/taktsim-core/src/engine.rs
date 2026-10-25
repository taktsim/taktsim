//! The event loop: components, items, events and runtime checks
//! (ADR-0001, ADR-0018).

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use taktsim_units::{SimDuration, SimTime};

use crate::error::{ComponentError, EngineError, RecentEvent, RunError, Violation};
use crate::queue::{CancelError, EventQueue, Next, Pending, Scheduled};
use crate::random::{Stream, StreamKey};
use crate::trace::{TraceRecord, TraceSink};

/// How many events a run error report includes.
const RECENT_EVENTS: usize = 32;

/// A component of a run, numbered in the order components were added.
///
/// ```
/// use taktsim_core::ComponentId;
///
/// assert_eq!(ComponentId::from_index(3).index(), 3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentId(pub(crate) usize);

impl ComponentId {
    /// The id of the component added `index`-th (from 0). Ids come from
    /// [`Engine::add_component`]; this constructor exists for trace readers.
    ///
    /// ```
    /// use taktsim_core::ComponentId;
    ///
    /// assert_eq!(ComponentId::from_index(0), ComponentId::from_index(0));
    /// ```
    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        Self(index)
    }

    /// The position of the component in the order it was added.
    ///
    /// ```
    /// use taktsim_core::ComponentId;
    ///
    /// assert_eq!(ComponentId::from_index(7).index(), 7);
    /// ```
    #[must_use]
    pub const fn index(self) -> usize {
        self.0
    }
}

/// A scheduled event, e.g. to cancel a timer. Its number is the event's
/// sequence number, unique within a run.
///
/// ```
/// use taktsim_core::EventId;
///
/// assert_eq!(EventId::from_raw(12).to_string(), "event #12");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(pub(crate) u64);

impl EventId {
    /// The event with sequence number `seq`, e.g. read from a trace.
    ///
    /// ```
    /// use taktsim_core::EventId;
    ///
    /// assert_eq!(EventId::from_raw(3).as_u64(), 3);
    /// ```
    #[must_use]
    pub const fn from_raw(seq: u64) -> Self {
        Self(seq)
    }

    /// The sequence number.
    ///
    /// ```
    /// use taktsim_core::EventId;
    ///
    /// assert_eq!(EventId::from_raw(9).as_u64(), 9);
    /// ```
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event #{}", self.0)
    }
}

/// An item flowing through the model: what the spec calls a "unit" (a part,
/// a lot, an assembly). Named `Item` in code to keep it apart from units of
/// measure. Numbered in the order items are created.
///
/// ```
/// use taktsim_core::ItemId;
///
/// assert_eq!(ItemId::from_raw(4).to_string(), "item 4");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemId(pub(crate) u64);

impl ItemId {
    /// The item with number `raw`, e.g. read from a trace.
    ///
    /// ```
    /// use taktsim_core::ItemId;
    ///
    /// assert_eq!(ItemId::from_raw(4).as_u64(), 4);
    /// ```
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The item's number.
    ///
    /// ```
    /// use taktsim_core::ItemId;
    ///
    /// assert_eq!(ItemId::from_raw(8).as_u64(), 8);
    /// ```
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "item {}", self.0)
    }
}

/// How an item leaves the model (spec §4, conservation).
///
/// ```
/// use taktsim_core::Disposal;
///
/// assert_ne!(Disposal::Delivered, Disposal::Scrapped);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Disposal {
    /// Delivered to a sink.
    Delivered,
    /// Scrapped, e.g. by an inspection.
    Scrapped,
    /// Consumed, e.g. as an input of an assembly.
    Consumed,
}

/// The messages of a model: one type for all events, usually an enum
/// defined by the component package. The engine never looks inside.
///
/// ```
/// use taktsim_core::Message;
///
/// #[derive(Debug, Clone)]
/// enum Flow {
///     Failure,
///     CycleDone,
/// }
///
/// impl Message for Flow {
///     // A failure at the same instant as a cycle end is handled first.
///     fn priority(&self) -> u8 {
///         match self {
///             Flow::Failure => 0,
///             Flow::CycleDone => 1,
///         }
///     }
/// }
/// assert!(Flow::Failure.priority() < Flow::CycleDone.priority());
/// ```
pub trait Message: Clone + fmt::Debug + 'static {
    /// Order among events at the same instant: lower first, then in the
    /// order they were scheduled (ADR-0001). Part of the trace contract:
    /// changing a priority changes traces.
    fn priority(&self) -> u8 {
        0
    }
}

/// An event as a component receives it.
///
/// ```
/// use taktsim_core::{ComponentId, Delivery, EventId};
///
/// let d = Delivery {
///     id: EventId::from_raw(0),
///     from: ComponentId::from_index(1),
///     message: "Arrive",
///     item: None,
/// };
/// assert_eq!(d.message, "Arrive");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery<M> {
    /// The event.
    pub id: EventId,
    /// The sender (the receiving component itself for a timer).
    pub from: ComponentId,
    /// The message.
    pub message: M,
    /// The item the event carries, which the component must
    /// [`receive`](Context::receive) while handling it.
    pub item: Option<ItemId>,
}

/// A building block of a model: a typed state machine over engine events
/// with runtime invariants (spec §4, "Extensibility").
///
/// The engine calls [`start`](Self::start) once at time zero, then
/// [`handle`](Self::handle) for every event addressed to the component.
/// After each call, if checks are on, it calls
/// [`check_invariants`](Self::check_invariants) and compares
/// [`held_items`](Self::held_items) with its own records (conservation).
///
/// ```
/// use taktsim_core::{Component, ComponentError, Context, Delivery, Disposal, ItemId};
///
/// #[derive(Debug, Clone)]
/// struct Arrive;
/// impl taktsim_core::Message for Arrive {}
///
/// /// Disposes of every item it receives as delivered.
/// struct Sink;
///
/// impl Component<Arrive> for Sink {
///     fn handle(&mut self, d: Delivery<Arrive>, ctx: &mut Context<'_, Arrive>) -> Result<(), ComponentError> {
///         if let Some(item) = d.item {
///             ctx.receive(item)?;
///             ctx.dispose(item, Disposal::Delivered)?;
///         }
///         Ok(())
///     }
/// }
/// ```
pub trait Component<M: Message> {
    /// Called once at time zero, before the first event: look up
    /// neighbours, take random streams, schedule the first events.
    ///
    /// # Errors
    ///
    /// Any [`ComponentError`]; the run stops with it.
    fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
        let _ = ctx;
        Ok(())
    }

    /// Handles one event.
    ///
    /// # Errors
    ///
    /// Any [`ComponentError`]; the run stops with it.
    fn handle(
        &mut self,
        delivery: Delivery<M>,
        ctx: &mut Context<'_, M>,
    ) -> Result<(), ComponentError>;

    /// Calls `visit` once for every item the component holds, from its own
    /// data structures. The default holds none.
    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        let _ = visit;
    }

    /// Checks the component's invariants. The default has none.
    ///
    /// The engine checks a component only after it started or handled an
    /// event, which is when its state can change (ADR-0018). Invariants must
    /// therefore depend only on the component's own state.
    ///
    /// # Errors
    ///
    /// The first [`Violation`] found.
    fn check_invariants(&self) -> Result<(), Violation> {
        Ok(())
    }
}

/// Where a live item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    Held(ComponentId),
    InTransit { to: ComponentId },
}

/// Counts of items by where they are or how they left (spec §4).
///
/// ```
/// use taktsim_core::ItemCounts;
///
/// let c = ItemCounts::default();
/// assert_eq!(c.created, c.held + c.in_transit + c.delivered + c.scrapped + c.consumed);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ItemCounts {
    /// Items created so far.
    pub created: u64,
    /// Items held by a component.
    pub held: u64,
    /// Items moving between components (always zero between events).
    pub in_transit: u64,
    /// Items delivered to a sink.
    pub delivered: u64,
    /// Items scrapped.
    pub scrapped: u64,
    /// Items consumed.
    pub consumed: u64,
}

/// Every live item and the counts conservation is checked against.
#[derive(Debug, Default)]
struct Items {
    live: BTreeMap<ItemId, Location>,
    held_by: Vec<u64>,
    counts: ItemCounts,
}

/// The state components can act on, separate from the components
/// themselves so a component and its context can be borrowed together.
struct State<M> {
    seed: u64,
    now: SimTime,
    queue: EventQueue<M>,
    items: Items,
    names: Vec<String>,
    ids: BTreeMap<String, ComponentId>,
    streams: BTreeMap<StreamKey, (ComponentId, String)>,
    recent: VecDeque<RecentRaw<M>>,
}

/// An event as kept for error reports, rendered only if one is needed.
struct RecentRaw<M> {
    seq: u64,
    time: SimTime,
    target: ComponentId,
    from: ComponentId,
    message: M,
    item: Option<ItemId>,
}

/// What a component can do while it starts or handles an event.
///
/// Every operation is checked: a component cannot move an item it does not
/// hold, cancel another component's timer or schedule beyond simulation
/// time.
pub struct Context<'a, M: Message> {
    me: ComponentId,
    /// The item carried by the event being handled, until received.
    arriving: Option<ItemId>,
    state: &'a mut State<M>,
    trace: &'a mut dyn TraceSink<M>,
}

impl<M: Message> Context<'_, M> {
    /// The current simulation time.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct Tick;
    /// # impl Message for Tick {}
    /// struct Clock(Vec<taktsim_units::SimTime>);
    /// impl Component<Tick> for Clock {
    ///     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
    ///         ctx.schedule("5s".parse()?, Tick)?;
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<Tick>, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
    ///         assert_eq!(ctx.now().to_string(), "5s");
    ///         Ok(())
    ///     }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("clock", Box::new(Clock(vec![])))?;
    /// engine.run(RunLimits::until("1min".parse()?), &mut NoTrace)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn now(&self) -> SimTime {
        self.state.now
    }

    /// The id of the component this context belongs to.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// struct Me;
    /// impl Component<M> for Me {
    ///     fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
    ///         assert_eq!(ctx.me(), ctx.lookup("me")?);
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("me", Box::new(Me))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn me(&self) -> ComponentId {
        self.me
    }

    /// The component with model id `name`, e.g. a downstream neighbour.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownComponent`].
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// struct Lookup;
    /// impl Component<M> for Lookup {
    ///     fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
    ///         assert!(ctx.lookup("nowhere").is_err());
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("a", Box::new(Lookup))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn lookup(&self, name: &str) -> Result<ComponentId, EngineError> {
        self.state
            .ids
            .get(name)
            .copied()
            .ok_or_else(|| EngineError::UnknownComponent {
                name: name.to_owned(),
            })
    }

    /// Schedules `message` to this component after `after`; returns the
    /// event so the timer can be [cancelled](Self::cancel).
    ///
    /// # Errors
    ///
    /// [`EngineError::Time`] if the time would pass the end of simulation
    /// time (about 584 years).
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct Tick;
    /// # impl Message for Tick {}
    /// struct Ticker(u32);
    /// impl Component<Tick> for Ticker {
    ///     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
    ///         ctx.schedule("1s".parse()?, Tick)?;
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<Tick>, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
    ///         self.0 += 1;
    ///         ctx.schedule("1s".parse()?, Tick)?;
    ///         Ok(())
    ///     }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("ticker", Box::new(Ticker(0)))?;
    /// let summary = engine.run(RunLimits::until("10s".parse()?), &mut NoTrace)?;
    /// assert_eq!(summary.events, 10);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn schedule(&mut self, after: SimDuration, message: M) -> Result<EventId, EngineError> {
        let time = self.state.now.checked_add(after)?;
        Ok(self.state.queue.push(Pending {
            time,
            priority: message.priority(),
            target: self.me,
            from: self.me,
            message,
            item: None,
            timer: true,
        }))
    }

    /// Cancels a pending timer this component scheduled.
    ///
    /// # Errors
    ///
    /// [`EngineError::NotPending`] if the event was already delivered or
    /// cancelled, or is a message; [`EngineError::NotOwner`] if it is
    /// another component's timer.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct Tick;
    /// # impl Message for Tick {}
    /// struct Changes;
    /// impl Component<Tick> for Changes {
    ///     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
    ///         let first = ctx.schedule("1s".parse()?, Tick)?;
    ///         ctx.cancel(first)?;
    ///         assert!(ctx.cancel(first).is_err());
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<Tick>, _: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
    ///         unreachable!("the only timer was cancelled")
    ///     }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("c", Box::new(Changes))?;
    /// let summary = engine.run(RunLimits::until("1min".parse()?), &mut NoTrace)?;
    /// assert_eq!(summary.events, 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn cancel(&mut self, event: EventId) -> Result<(), EngineError> {
        self.state
            .queue
            .cancel(event, self.me)
            .map_err(|e| match e {
                CancelError::NotPending => EngineError::NotPending { event },
                CancelError::NotOwner(owner) => EngineError::NotOwner {
                    event,
                    by: self.state.names[self.me.0].clone(),
                    owner: self.state.names[owner.0].clone(),
                },
            })
    }

    /// Sends `message` to component `to` at the current time. Messages are
    /// delivered in the order sent (among the same priority) and cannot be
    /// cancelled.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidComponent`] if `to` is not a component of this
    /// run.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct Ping;
    /// # impl Message for Ping {}
    /// struct Sender;
    /// impl Component<Ping> for Sender {
    ///     fn start(&mut self, ctx: &mut Context<'_, Ping>) -> Result<(), ComponentError> {
    ///         let to = ctx.lookup("receiver")?;
    ///         ctx.send(to, Ping)?;
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<Ping>, _: &mut Context<'_, Ping>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// struct Receiver(u32);
    /// impl Component<Ping> for Receiver {
    ///     fn handle(&mut self, _: Delivery<Ping>, _: &mut Context<'_, Ping>) -> Result<(), ComponentError> {
    ///         self.0 += 1;
    ///         Ok(())
    ///     }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("sender", Box::new(Sender))?;
    /// engine.add_component("receiver", Box::new(Receiver(0)))?;
    /// assert_eq!(engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?.events, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn send(&mut self, to: ComponentId, message: M) -> Result<(), EngineError> {
        self.push_now(to, message, None)
    }

    /// Creates a new item, held by this component.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// struct Maker(Vec<ItemId>);
    /// impl Component<M> for Maker {
    ///     fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
    ///         self.0.push(ctx.create_item());
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    ///     fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
    ///         self.0.iter().copied().for_each(visit);
    ///     }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("maker", Box::new(Maker(vec![])))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// assert_eq!(engine.item_counts().held, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn create_item(&mut self) -> ItemId {
        let items = &mut self.state.items;
        let item = ItemId(items.counts.created);
        items.counts.created += 1;
        items.counts.held += 1;
        items.held_by[self.me.0] += 1;
        items.live.insert(item, Location::Held(self.me));
        self.trace.record(&TraceRecord::ItemCreated {
            time: self.state.now,
            item,
            by: self.me,
        });
        item
    }

    /// Sends `item`, held by this component, to component `to` with
    /// `message`, at the current time. Until `to` handles that event and
    /// [receives](Self::receive) the item, it is in transit.
    ///
    /// # Errors
    ///
    /// [`EngineError::ItemNotHeld`] if this component does not hold `item`;
    /// [`EngineError::InvalidComponent`] if `to` is not a component of this
    /// run.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct Arrive;
    /// # impl Message for Arrive {}
    /// struct Source;
    /// impl Component<Arrive> for Source {
    ///     fn start(&mut self, ctx: &mut Context<'_, Arrive>) -> Result<(), ComponentError> {
    ///         let item = ctx.create_item();
    ///         let sink = ctx.lookup("sink")?;
    ///         ctx.transfer(item, sink, Arrive)?;
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<Arrive>, _: &mut Context<'_, Arrive>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// struct Sink;
    /// impl Component<Arrive> for Sink {
    ///     fn handle(&mut self, d: Delivery<Arrive>, ctx: &mut Context<'_, Arrive>) -> Result<(), ComponentError> {
    ///         if let Some(item) = d.item {
    ///             ctx.receive(item)?;
    ///             ctx.dispose(item, Disposal::Delivered)?;
    ///         }
    ///         Ok(())
    ///     }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("source", Box::new(Source))?;
    /// engine.add_component("sink", Box::new(Sink))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// assert_eq!(engine.item_counts().delivered, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn transfer(
        &mut self,
        item: ItemId,
        to: ComponentId,
        message: M,
    ) -> Result<(), EngineError> {
        self.check_held(item)?;
        self.push_now(to, message, Some(item))?;
        let items = &mut self.state.items;
        items.live.insert(item, Location::InTransit { to });
        items.held_by[self.me.0] -= 1;
        items.counts.held -= 1;
        items.counts.in_transit += 1;
        Ok(())
    }

    /// Takes `item`, which the event being handled carries, into this
    /// component.
    ///
    /// # Errors
    ///
    /// [`EngineError::ItemNotArriving`] if the current event does not carry
    /// `item` (or it was already received).
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// struct Picky;
    /// impl Component<M> for Picky {
    ///     fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
    ///         // Nothing is arriving while starting.
    ///         assert!(ctx.receive(ItemId::from_raw(0)).is_err());
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("p", Box::new(Picky))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn receive(&mut self, item: ItemId) -> Result<(), EngineError> {
        if self.arriving != Some(item) {
            return Err(EngineError::ItemNotArriving {
                item,
                by: self.state.names[self.me.0].clone(),
            });
        }
        self.arriving = None;
        let items = &mut self.state.items;
        items.live.insert(item, Location::Held(self.me));
        items.held_by[self.me.0] += 1;
        items.counts.held += 1;
        items.counts.in_transit -= 1;
        Ok(())
    }

    /// Removes `item`, held by this component, from the model.
    ///
    /// # Errors
    ///
    /// [`EngineError::ItemNotHeld`] if this component does not hold `item`.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// struct Scrapper;
    /// impl Component<M> for Scrapper {
    ///     fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
    ///         let item = ctx.create_item();
    ///         ctx.dispose(item, Disposal::Scrapped)?;
    ///         assert!(ctx.dispose(item, Disposal::Scrapped).is_err());
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("qc", Box::new(Scrapper))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// assert_eq!(engine.item_counts().scrapped, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn dispose(&mut self, item: ItemId, how: Disposal) -> Result<(), EngineError> {
        self.check_held(item)?;
        let items = &mut self.state.items;
        items.live.remove(&item);
        items.held_by[self.me.0] -= 1;
        items.counts.held -= 1;
        match how {
            Disposal::Delivered => items.counts.delivered += 1,
            Disposal::Scrapped => items.counts.scrapped += 1,
            Disposal::Consumed => items.counts.consumed += 1,
        }
        self.trace.record(&TraceRecord::ItemDisposed {
            time: self.state.now,
            item,
            by: self.me,
            how,
        });
        Ok(())
    }

    /// A random stream for `purpose` of this component (ADR-0013), e.g.
    /// `"cycle_time"`. Each purpose can be taken once per run.
    ///
    /// # Errors
    ///
    /// [`EngineError::StreamConflict`] if this component already took a
    /// stream for `purpose` (or, very unlikely, another stream has the same
    /// key).
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// struct Random(Option<random::Stream>);
    /// impl Component<M> for Random {
    ///     fn start(&mut self, ctx: &mut Context<'_, M>) -> Result<(), ComponentError> {
    ///         self.0 = Some(ctx.stream("cycle_time")?);
    ///         assert!(ctx.stream("cycle_time").is_err());
    ///         Ok(())
    ///     }
    ///     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    /// }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// engine.add_component("cut", Box::new(Random(None)))?;
    /// engine.run(RunLimits::until("1s".parse()?), &mut NoTrace)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stream(&mut self, purpose: &str) -> Result<Stream, EngineError> {
        let name = &self.state.names[self.me.0];
        let key = StreamKey::new(name, purpose);
        if let Some((owner, existing)) = self.state.streams.get(&key) {
            return Err(EngineError::StreamConflict {
                component: name.clone(),
                purpose: purpose.to_owned(),
                existing_component: self.state.names[owner.0].clone(),
                existing_purpose: existing.clone(),
            });
        }
        self.state
            .streams
            .insert(key, (self.me, purpose.to_owned()));
        Ok(Stream::new(self.state.seed, key))
    }

    /// Fails unless this component holds `item`.
    fn check_held(&self, item: ItemId) -> Result<(), EngineError> {
        if self.state.items.live.get(&item) == Some(&Location::Held(self.me)) {
            return Ok(());
        }
        Err(EngineError::ItemNotHeld {
            item,
            by: self.state.names[self.me.0].clone(),
        })
    }

    /// Schedules a message from this component to `to` at the current time.
    fn push_now(
        &mut self,
        to: ComponentId,
        message: M,
        item: Option<ItemId>,
    ) -> Result<(), EngineError> {
        if to.0 >= self.state.names.len() {
            return Err(EngineError::InvalidComponent { index: to.0 });
        }
        self.state.queue.push(Pending {
            time: self.state.now,
            priority: message.priority(),
            target: to,
            from: self.me,
            message,
            item,
            timer: false,
        });
        Ok(())
    }
}

/// How a run is set up.
///
/// ```
/// use taktsim_core::EngineConfig;
///
/// let config = EngineConfig::new(42);
/// assert!(config.checks);
/// assert!(!config.without_checks().checks);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    /// The seed of every random stream of the run.
    pub seed: u64,
    /// Whether invariants and conservation are checked after every event
    /// (spec §7 Layer 3). On by default; turn off only for calibration inner
    /// loops after one checked run.
    pub checks: bool,
}

impl EngineConfig {
    /// A run with `seed` and checks on.
    ///
    /// ```
    /// use taktsim_core::EngineConfig;
    ///
    /// assert_eq!(EngineConfig::new(7).seed, 7);
    /// ```
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { seed, checks: true }
    }

    /// The same configuration with checks off (`--fast`).
    ///
    /// ```
    /// use taktsim_core::EngineConfig;
    ///
    /// assert!(!EngineConfig::new(7).without_checks().checks);
    /// ```
    #[must_use]
    pub const fn without_checks(self) -> Self {
        Self {
            checks: false,
            ..self
        }
    }
}

/// When a call to [`Engine::run`] stops.
///
/// ```
/// use std::sync::atomic::AtomicBool;
/// use taktsim_core::RunLimits;
///
/// let stop = AtomicBool::new(false);
/// let limits = RunLimits::until("8h".parse()?).max_events(1_000_000).cancel_on(&stop);
/// assert_eq!(limits.max_events, Some(1_000_000));
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RunLimits<'a> {
    /// Events up to and including this time are processed.
    pub until: SimTime,
    /// The most events this call processes.
    pub max_events: Option<u64>,
    /// Checked before every event; once `true`, the run stops (e.g. a
    /// wall-clock budget enforced by the caller).
    pub cancel: Option<&'a AtomicBool>,
}

impl<'a> RunLimits<'a> {
    /// Run until `until`, without other limits.
    ///
    /// ```
    /// use taktsim_core::RunLimits;
    ///
    /// assert!(RunLimits::until("8h".parse()?).max_events.is_none());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn until(until: SimTime) -> Self {
        Self {
            until,
            max_events: None,
            cancel: None,
        }
    }

    /// Stop after `n` events.
    ///
    /// ```
    /// use taktsim_core::RunLimits;
    ///
    /// assert_eq!(RunLimits::until("8h".parse()?).max_events(5).max_events, Some(5));
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn max_events(self, n: u64) -> Self {
        Self {
            max_events: Some(n),
            ..self
        }
    }

    /// Stop as soon as `flag` is set.
    ///
    /// ```
    /// use std::sync::atomic::AtomicBool;
    /// use taktsim_core::RunLimits;
    ///
    /// let flag = AtomicBool::new(false);
    /// assert!(RunLimits::until("8h".parse()?).cancel_on(&flag).cancel.is_some());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub const fn cancel_on(self, flag: &'a AtomicBool) -> Self {
        Self {
            cancel: Some(flag),
            ..self
        }
    }
}

/// Why a call to [`Engine::run`] returned.
///
/// ```
/// use taktsim_core::RunOutcome;
///
/// assert_ne!(RunOutcome::Horizon, RunOutcome::Cancelled);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// The next event is after the horizon; the clock is at the horizon.
    Horizon,
    /// No events are left; the clock is at the horizon.
    Idle,
    /// `max_events` events were processed.
    EventLimit,
    /// The cancel flag was set.
    Cancelled,
}

/// What a call to [`Engine::run`] did.
///
/// ```
/// use taktsim_core::{RunOutcome, RunSummary};
///
/// let s = RunSummary { outcome: RunOutcome::Idle, events: 0, now: taktsim_units::SimTime::START };
/// assert_eq!(s.events, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSummary {
    /// Why the call returned.
    pub outcome: RunOutcome,
    /// Events processed by this call.
    pub events: u64,
    /// The simulation time afterwards.
    pub now: SimTime,
}

/// A simulation run: components, the event queue, items and the clock.
///
/// Add components, then [`run`](Self::run) (repeatedly, if wanted, to
/// advance in steps). The same components, seed and engine version give the
/// same events, byte for byte.
///
/// ```
/// use taktsim_core::{Component, ComponentError, Context, Delivery, Engine, EngineConfig, Message, NoTrace, RunLimits, RunOutcome};
///
/// #[derive(Debug, Clone)]
/// struct Tick;
/// impl Message for Tick {}
///
/// struct Ticker;
/// impl Component<Tick> for Ticker {
///     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
///         ctx.schedule("1s".parse()?, Tick)?;
///         Ok(())
///     }
///     fn handle(&mut self, _: Delivery<Tick>, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
///         ctx.schedule("1s".parse()?, Tick)?;
///         Ok(())
///     }
/// }
///
/// let mut engine = Engine::new(EngineConfig::new(42));
/// engine.add_component("ticker", Box::new(Ticker))?;
/// let first = engine.run(RunLimits::until("1min".parse()?), &mut NoTrace)?;
/// assert_eq!((first.outcome, first.events), (RunOutcome::Horizon, 60));
/// let second = engine.run(RunLimits::until("2min".parse()?), &mut NoTrace)?;
/// assert_eq!(second.events, 60);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Engine<M: Message> {
    config: EngineConfig,
    components: Vec<Box<dyn Component<M>>>,
    state: State<M>,
    started: bool,
}

impl<M: Message> Engine<M> {
    /// An empty run.
    ///
    /// ```
    /// use taktsim_core::{Engine, EngineConfig, Message};
    ///
    /// #[derive(Debug, Clone)]
    /// struct Tick;
    /// impl Message for Tick {}
    ///
    /// let engine: Engine<Tick> = Engine::new(EngineConfig::new(1));
    /// assert_eq!(engine.now(), taktsim_units::SimTime::START);
    /// ```
    #[must_use]
    pub fn new(config: EngineConfig) -> Self {
        Self {
            config,
            components: Vec::new(),
            state: State {
                seed: config.seed,
                now: SimTime::START,
                queue: EventQueue::new(),
                items: Items::default(),
                names: Vec::new(),
                ids: BTreeMap::new(),
                streams: BTreeMap::new(),
                recent: VecDeque::with_capacity(RECENT_EVENTS),
            },
            started: false,
        }
    }

    /// Adds a component with model id `name`.
    ///
    /// # Errors
    ///
    /// [`EngineError::DuplicateComponent`] if `name` is taken;
    /// [`EngineError::AlreadyStarted`] after the first [`run`](Self::run).
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// # struct Idle;
    /// # impl Component<M> for Idle {
    /// #     fn handle(&mut self, _: Delivery<M>, _: &mut Context<'_, M>) -> Result<(), ComponentError> { Ok(()) }
    /// # }
    /// let mut engine = Engine::new(EngineConfig::new(1));
    /// let id = engine.add_component("buf1", Box::new(Idle))?;
    /// assert_eq!(engine.component_name(id), Some("buf1"));
    /// assert!(engine.add_component("buf1", Box::new(Idle)).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn add_component(
        &mut self,
        name: &str,
        component: Box<dyn Component<M>>,
    ) -> Result<ComponentId, EngineError> {
        if self.started {
            return Err(EngineError::AlreadyStarted {
                name: name.to_owned(),
            });
        }
        if self.state.ids.contains_key(name) {
            return Err(EngineError::DuplicateComponent {
                name: name.to_owned(),
            });
        }
        let id = ComponentId(self.components.len());
        self.components.push(component);
        self.state.names.push(name.to_owned());
        self.state.ids.insert(name.to_owned(), id);
        self.state.items.held_by.push(0);
        Ok(id)
    }

    /// The model id of component `id`.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// let engine: Engine<M> = Engine::new(EngineConfig::new(1));
    /// assert_eq!(engine.component_name(ComponentId::from_index(0)), None);
    /// ```
    #[must_use]
    pub fn component_name(&self, id: ComponentId) -> Option<&str> {
        self.state.names.get(id.0).map(String::as_str)
    }

    /// The current simulation time.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// let engine: Engine<M> = Engine::new(EngineConfig::new(1));
    /// assert_eq!(engine.now(), taktsim_units::SimTime::START);
    /// ```
    #[must_use]
    pub fn now(&self) -> SimTime {
        self.state.now
    }

    /// Item counts by where items are or how they left.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// let engine: Engine<M> = Engine::new(EngineConfig::new(1));
    /// assert_eq!(engine.item_counts(), ItemCounts::default());
    /// ```
    #[must_use]
    pub fn item_counts(&self) -> ItemCounts {
        self.state.items.counts
    }

    /// Processes events until a limit is reached. The first call starts
    /// every component, in the order they were added, at time zero.
    /// Events at exactly `limits.until` are processed.
    ///
    /// # Errors
    ///
    /// [`RunError`] if a component fails, an invariant or conservation is
    /// violated (with checks on), an item is not received, or `until` is
    /// before the current time. The run cannot continue after an error.
    ///
    /// ```
    /// # use taktsim_core::*;
    /// # #[derive(Debug, Clone)] struct M;
    /// # impl Message for M {}
    /// let mut engine: Engine<M> = Engine::new(EngineConfig::new(1));
    /// let summary = engine.run(RunLimits::until("8h".parse()?), &mut NoTrace)?;
    /// assert_eq!(summary.outcome, RunOutcome::Idle);
    /// assert_eq!(summary.now.to_string(), "8h");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn run(
        &mut self,
        limits: RunLimits<'_>,
        trace: &mut dyn TraceSink<M>,
    ) -> Result<RunSummary, RunError> {
        if limits.until < self.state.now {
            return Err(RunError::HorizonInPast {
                now: self.state.now,
                until: limits.until,
            });
        }
        if !self.started {
            self.started = true;
            for index in 0..self.components.len() {
                self.start_component(ComponentId(index), trace)?;
            }
        }
        let mut events = 0;
        let outcome = loop {
            if limits
                .cancel
                .is_some_and(|flag| flag.load(Ordering::Relaxed))
            {
                break RunOutcome::Cancelled;
            }
            if limits.max_events == Some(events) {
                break RunOutcome::EventLimit;
            }
            let event = match self.state.queue.pop_until(limits.until) {
                Next::Due(event) => event,
                Next::Later => {
                    self.state.now = limits.until;
                    break RunOutcome::Horizon;
                }
                Next::Empty => {
                    self.state.now = limits.until;
                    break RunOutcome::Idle;
                }
            };
            self.deliver(event, trace)?;
            events += 1;
        };
        Ok(RunSummary {
            outcome,
            events,
            now: self.state.now,
        })
    }

    fn start_component(
        &mut self,
        id: ComponentId,
        trace: &mut dyn TraceSink<M>,
    ) -> Result<(), RunError> {
        let mut ctx = Context {
            me: id,
            arriving: None,
            state: &mut self.state,
            trace,
        };
        let result = self.components[id.0].start(&mut ctx);
        result.map_err(|source| self.component_error(id, source))?;
        self.check(id)
    }

    /// Delivers `event` to its target, then checks the target.
    fn deliver(
        &mut self,
        event: Scheduled<M>,
        trace: &mut dyn TraceSink<M>,
    ) -> Result<(), RunError> {
        self.state.now = event.key.time;
        let target = event.target;
        trace.record(&TraceRecord::Event {
            seq: event.key.seq,
            time: event.key.time,
            target,
            from: event.from,
            message: &event.message,
            item: event.item,
        });
        if self.state.recent.len() == RECENT_EVENTS {
            self.state.recent.pop_front();
        }
        self.state.recent.push_back(RecentRaw {
            seq: event.key.seq,
            time: event.key.time,
            target,
            from: event.from,
            message: event.message.clone(),
            item: event.item,
        });
        let delivery = Delivery {
            id: EventId(event.key.seq),
            from: event.from,
            message: event.message,
            item: event.item,
        };
        let mut ctx = Context {
            me: target,
            arriving: event.item,
            state: &mut self.state,
            trace,
        };
        let result = self.components[target.0].handle(delivery, &mut ctx);
        let unreceived = ctx.arriving;
        result.map_err(|source| self.component_error(target, source))?;
        if let Some(item) = unreceived {
            return Err(RunError::NotReceived {
                component: self.state.names[target.0].clone(),
                time: self.state.now,
                item,
                recent: self.recent(),
            });
        }
        self.check(target)
    }

    /// After `id` started or handled an event: its invariants and
    /// conservation (spec §4, §7 Layer 3). Only `id` can have changed, so
    /// checking it is checking every component.
    fn check(&self, id: ComponentId) -> Result<(), RunError> {
        if !self.config.checks {
            return Ok(());
        }
        let component = &self.components[id.0];
        if let Err(violation) = component.check_invariants() {
            return Err(RunError::Invariant {
                component: self.state.names[id.0].clone(),
                time: self.state.now,
                violation,
                recent: self.recent(),
            });
        }
        let mut reported = Vec::new();
        component.held_items(&mut |item| reported.push(item));
        reported.sort_unstable();
        let items = &self.state.items;
        let duplicate = reported
            .windows(2)
            .find(|pair| pair.first() == pair.last())
            .and_then(<[ItemId]>::first);
        let problem = if let Some(item) = duplicate {
            Some(format!("it reports {item} twice"))
        } else if let Some(item) = reported
            .iter()
            .find(|item| items.live.get(item) != Some(&Location::Held(id)))
        {
            Some(format!("it reports {item}, which it does not hold"))
        } else if reported.len() as u64 != items.held_by[id.0] {
            Some(format!(
                "it reports {} items, the engine records {}",
                reported.len(),
                items.held_by[id.0]
            ))
        } else {
            let c = items.counts;
            let accounted = c.held + c.in_transit + c.delivered + c.scrapped + c.consumed;
            (c.created != accounted).then(|| {
                format!(
                    "{} items created, {accounted} held, in transit or disposed of",
                    c.created
                )
            })
        };
        match problem {
            Some(detail) => Err(RunError::Conservation {
                component: self.state.names[id.0].clone(),
                time: self.state.now,
                detail,
                recent: self.recent(),
            }),
            None => Ok(()),
        }
    }

    fn component_error(&self, id: ComponentId, source: ComponentError) -> RunError {
        RunError::Component {
            component: self.state.names[id.0].clone(),
            time: self.state.now,
            source: Box::new(source),
            recent: self.recent(),
        }
    }

    /// The last events, oldest first, rendered for an error report.
    fn recent(&self) -> Vec<RecentEvent> {
        let names = &self.state.names;
        self.state
            .recent
            .iter()
            .map(|e| RecentEvent {
                seq: e.seq,
                time: e.time,
                target: names[e.target.0].clone(),
                from: names[e.from.0].clone(),
                message: format!("{:?}", e.message),
                item: e.item,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Message for u8 {}

    struct Idle;

    impl Component<u8> for Idle {
        fn handle(
            &mut self,
            _: Delivery<u8>,
            _: &mut Context<'_, u8>,
        ) -> Result<(), ComponentError> {
            Ok(())
        }
    }

    #[test]
    fn engine_bookkeeping_errors_are_conservation_errors() {
        // The identity created = held + in transit + disposed can only fail
        // through an engine bug; corrupt a counter to show it is checked.
        let mut engine = Engine::new(EngineConfig::new(1));
        engine.add_component("idle", Box::new(Idle)).unwrap();
        engine.state.items.counts.created = 5;
        let err = engine.check(ComponentId(0)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "at 0s, `idle`: conservation violated: 5 items created, 0 held, in transit or disposed of"
        );
    }
}
