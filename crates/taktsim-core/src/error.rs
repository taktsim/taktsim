//! Errors of the engine.

use std::fmt;

use taktsim_units::{SimTime, UnitError};

use crate::engine::{EventId, ItemId};

/// A misuse of the engine by a component or by the code that builds a run.
///
/// Every variant names the components involved by their model ids.
///
/// ```
/// use taktsim_core::EngineError;
///
/// let e = EngineError::UnknownComponent { name: "buf2".into() };
/// assert_eq!(e.to_string(), "no component with id `buf2`");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// A component id is used twice.
    #[error("component id `{name}` is used twice")]
    DuplicateComponent {
        /// The id.
        name: String,
    },
    /// A component was added after the run started.
    #[error("component `{name}` added after the run started")]
    AlreadyStarted {
        /// The id of the component.
        name: String,
    },
    /// No component has this id.
    #[error("no component with id `{name}`")]
    UnknownComponent {
        /// The id looked up.
        name: String,
    },
    /// A component id from a different engine.
    #[error("component number {index} does not exist in this run")]
    InvalidComponent {
        /// The index of the id.
        index: usize,
    },
    /// Scheduling beyond the end of simulation time (about 584 years).
    #[error(transparent)]
    Time(#[from] UnitError),
    /// A cancelled event is not a pending timer: it was already delivered or
    /// cancelled, or it was a message, which cannot be cancelled.
    #[error("{event} is not a pending timer")]
    NotPending {
        /// The event.
        event: EventId,
    },
    /// A component cancelled another component's timer.
    #[error("`{by}` cannot cancel {event}, a timer of `{owner}`")]
    NotOwner {
        /// The event.
        event: EventId,
        /// The component that tried.
        by: String,
        /// The component the timer belongs to.
        owner: String,
    },
    /// A component moved or disposed of an item it does not hold.
    #[error("`{by}` does not hold {item}")]
    ItemNotHeld {
        /// The item.
        item: ItemId,
        /// The component that tried.
        by: String,
    },
    /// A component received an item that the event it is handling does not
    /// carry.
    #[error("`{by}` cannot receive {item}: the current event does not carry it")]
    ItemNotArriving {
        /// The item.
        item: ItemId,
        /// The component that tried.
        by: String,
    },
    /// Two random streams of a run would have the same key: the same
    /// component and purpose twice, or (very unlikely) a hash collision.
    #[error(
        "stream `{purpose}` of `{component}` conflicts with stream `{existing_purpose}` of `{existing_component}`"
    )]
    StreamConflict {
        /// The component asking for the stream.
        component: String,
        /// Its purpose.
        purpose: String,
        /// The component that already has a stream with this key.
        existing_component: String,
        /// That stream's purpose.
        existing_purpose: String,
    },
}

/// Why a component could not handle an event.
///
/// ```
/// use taktsim_core::ComponentError;
///
/// let e = ComponentError::Unexpected { state: "down", event: "CycleDone".into() };
/// assert_eq!(e.to_string(), "unexpected event CycleDone in state `down`");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ComponentError {
    /// An engine operation failed.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// A quantity computation failed, e.g. a sampled duration beyond
    /// simulation time.
    #[error(transparent)]
    Unit(#[from] UnitError),
    /// The event does not fit the component's current state.
    #[error("unexpected event {event} in state `{state}`")]
    Unexpected {
        /// The state the component was in.
        state: &'static str,
        /// The event, as text.
        event: String,
    },
}

/// A broken runtime invariant of a component (spec §4), e.g. a buffer
/// holding more than its capacity.
///
/// ```
/// use taktsim_core::Violation;
///
/// let v = Violation { invariant: "0 <= fill <= capacity", detail: "fill 21, capacity 20".into() };
/// assert_eq!(v.to_string(), "invariant `0 <= fill <= capacity` violated: fill 21, capacity 20");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invariant `{invariant}` violated: {detail}")]
pub struct Violation {
    /// The invariant as stated in the component's documentation.
    pub invariant: &'static str,
    /// What was found.
    pub detail: String,
}

/// One of the events before a failed run, for its error report.
///
/// ```
/// use taktsim_core::RecentEvent;
///
/// let e = RecentEvent {
///     seq: 7,
///     time: "42s".parse()?,
///     target: "weld".into(),
///     from: "cut".into(),
///     message: "Arrive".into(),
///     item: None,
/// };
/// assert_eq!(e.to_string(), "#7 at 42s: cut -> weld Arrive");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentEvent {
    /// The event's sequence number.
    pub seq: u64,
    /// When it was delivered.
    pub time: SimTime,
    /// The receiving component.
    pub target: String,
    /// The sending component (the target itself for a timer).
    pub from: String,
    /// The message, as text.
    pub message: String,
    /// The item it carried, if any.
    pub item: Option<ItemId>,
}

impl fmt::Display for RecentEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#{} at {}: {} -> {} {}",
            self.seq, self.time, self.from, self.target, self.message
        )?;
        match self.item {
            Some(item) => write!(f, " with {item}"),
            None => Ok(()),
        }
    }
}

/// Why a run stopped with an error. Every variant names the component and
/// the time, and carries the last events before the failure (spec §4: "a
/// violation aborts the run with the event trace that led to it").
///
/// ```
/// use taktsim_core::{RunError, Violation};
///
/// let e = RunError::Invariant {
///     component: "buf1".into(),
///     time: "1h".parse()?,
///     violation: Violation { invariant: "0 <= fill <= capacity", detail: "fill 21".into() },
///     recent: vec![],
/// };
/// assert_eq!(
///     e.to_string(),
///     "at 1h, `buf1`: invariant `0 <= fill <= capacity` violated: fill 21"
/// );
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RunError {
    /// A component failed while handling an event (or starting).
    #[error("at {time}, `{component}`: {source}")]
    Component {
        /// The component.
        component: String,
        /// The simulation time.
        time: SimTime,
        /// What went wrong (boxed to keep `RunError` small).
        source: Box<ComponentError>,
        /// The last events before the failure, oldest first.
        recent: Vec<RecentEvent>,
    },
    /// A component broke one of its invariants.
    #[error("at {time}, `{component}`: {violation}")]
    Invariant {
        /// The component.
        component: String,
        /// The simulation time.
        time: SimTime,
        /// The broken invariant.
        violation: Violation,
        /// The last events before the failure, oldest first.
        recent: Vec<RecentEvent>,
    },
    /// Conservation failed: the items a component reports holding differ
    /// from the items the engine has recorded for it.
    #[error("at {time}, `{component}`: conservation violated: {detail}")]
    Conservation {
        /// The component.
        component: String,
        /// The simulation time.
        time: SimTime,
        /// What differs.
        detail: String,
        /// The last events before the failure, oldest first.
        recent: Vec<RecentEvent>,
    },
    /// An event carried an item to a component that did not receive it.
    #[error("at {time}, `{component}` did not receive {item}, which the event carried")]
    NotReceived {
        /// The component.
        component: String,
        /// The simulation time.
        time: SimTime,
        /// The item left in transit.
        item: ItemId,
        /// The last events before the failure, oldest first.
        recent: Vec<RecentEvent>,
    },
    /// A run was asked to end before the current simulation time.
    #[error("cannot run until {until}: the simulation is already at {now}")]
    HorizonInPast {
        /// The current simulation time.
        now: SimTime,
        /// The requested end.
        until: SimTime,
    },
}
