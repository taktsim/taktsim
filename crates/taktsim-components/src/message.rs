//! The messages of a flow model: what components say to each other, and what
//! they write in the trace (ADR-0022 §4, §8, §9).

use taktsim_core::{ItemId, Message};
use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
use taktsim_units::SimTime;

use crate::state::{MachineState, SourceState};

/// An index into a model's sorted variant names, `default` included if some
/// Source declares no variants. A `VariantName` note gives each id its name
/// in the trace.
///
/// ```
/// use taktsim_components::VariantId;
///
/// assert_eq!(VariantId::new(3).get(), 3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VariantId(u16);

impl VariantId {
    /// The variant at position `index` of the sorted names.
    ///
    /// ```
    /// use taktsim_components::VariantId;
    ///
    /// assert_eq!(VariantId::new(0), VariantId::new(0));
    /// ```
    #[must_use]
    pub const fn new(index: u16) -> Self {
        Self(index)
    }

    /// The position in the sorted names.
    ///
    /// ```
    /// use taktsim_components::VariantId;
    ///
    /// assert_eq!(VariantId::new(7).get(), 7);
    /// ```
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// What travels with an item on [`FlowMessage::Arrive`]: holders keep it beside
/// the `ItemId`, and the trace carries it (ADR-0022 §7). `#[non_exhaustive]`,
/// so a new attribute breaks no caller; each new one changes `Arrive`'s bytes,
/// so it is a new [`FlowMessage::FORMAT`](TraceMessage::FORMAT).
///
/// ```
/// use taktsim_components::{ItemAttrs, VariantId};
///
/// let attrs = ItemAttrs::new(VariantId::new(1), 5);
/// assert_eq!((attrs.variant.get(), attrs.priority, attrs.due), (1, 5, None));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ItemAttrs {
    /// The variant.
    pub variant: VariantId,
    /// The priority: higher goes first, absent in the model is 0. Unrelated to
    /// a message's priority, where lower goes first.
    pub priority: u8,
    /// The due date. Always absent in M5; carrying it spares M6 a format
    /// change.
    pub due: Option<SimTime>,
}

impl ItemAttrs {
    /// A unit of `variant` with `priority` and no due date.
    ///
    /// ```
    /// use taktsim_components::{ItemAttrs, VariantId};
    ///
    /// assert_eq!(ItemAttrs::new(VariantId::new(0), 0).priority, 0);
    /// ```
    #[must_use]
    pub const fn new(variant: VariantId, priority: u8) -> Self {
        Self {
            variant,
            priority,
            due: None,
        }
    }
}

/// A machine's state as its note states it (ADR-0022 §9).
///
/// ```
/// use taktsim_components::{MachineState, MachineStateNote};
///
/// let note = MachineStateNote { state: MachineState::Busy, units_in_cycle: 2, setup_variant: None };
/// assert_eq!(note.units_in_cycle, 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineStateNote {
    /// The state.
    pub state: MachineState,
    /// Units in their cycle: 0 while down, since a paused cycle is not
    /// running.
    pub units_in_cycle: u32,
    /// In `setup`, the variant being set up for.
    pub setup_variant: Option<VariantId>,
}

/// A Source's state as its note states it (ADR-0022 §7).
///
/// ```
/// use taktsim_components::{SourceState, SourceStateNote};
///
/// let note = SourceStateNote { state: SourceState::Blocked, held: 2 };
/// assert_eq!(note.held, 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceStateNote {
    /// The state.
    pub state: SourceState,
    /// Parts created and not yet released.
    pub held: u32,
}

/// The messages of a flow model. Timers and protocol messages are delivered;
/// the notes only go to the trace. The enum is closed: a new one changes this
/// crate and its [`FORMAT`](TraceMessage::FORMAT) (ADR-0022 §11).
///
/// ```
/// use taktsim_components::FlowMessage;
/// use taktsim_core::Message;
///
/// // Finishing and landing come before asking.
/// assert!(FlowMessage::Failure.priority() < FlowMessage::Repaired.priority());
/// assert!(FlowMessage::Repaired.priority() < FlowMessage::SetupDone.priority());
/// assert!(FlowMessage::SetupDone.priority() < FlowMessage::NextArrival.priority());
/// assert!(FlowMessage::Accept.priority() < FlowMessage::Offer.priority());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowMessage {
    /// Timer: a machine fails.
    Failure,
    /// Timer: a repair ends.
    Repaired,
    /// Timer: `item`'s cycle ends.
    CycleDone {
        /// The unit.
        item: ItemId,
    },
    /// Timer: a changeover ends. One delivered per finished changeover.
    SetupDone,
    /// Timer: a Source's interval ends.
    NextArrival,
    /// Upstream → downstream, with the item: the part moves, and the
    /// reservation becomes the part.
    Arrive(ItemAttrs),
    /// Downstream → upstream: a place is reserved for you.
    Accept,
    /// Upstream → downstream: I have nothing for you any more. The answer to
    /// an `Accept`; the reservation is released.
    Decline,
    /// Upstream → downstream: I have a part for you.
    Offer,
    /// Upstream → downstream: forget my offer. No entry, no effect.
    Withdraw,
    /// Note: a machine's state or its count of units in a cycle changed.
    MachineState(MachineStateNote),
    /// Note: a Source's state or the parts it holds changed.
    SourceState(SourceStateNote),
    /// Note: the name of a variant, so a trace names its variants on its own.
    VariantName {
        /// The id.
        id: VariantId,
        /// The name.
        name: String,
    },
}

impl Message for FlowMessage {
    /// ADR-0022 §8. Lower goes first. Part of the trace contract.
    fn priority(&self) -> u8 {
        match self {
            Self::Failure => 0,
            Self::Repaired => 1,
            Self::CycleDone { .. } | Self::SetupDone => 2,
            Self::NextArrival => 3,
            Self::Arrive(_) => 4,
            Self::Accept => 5,
            Self::Decline => 6,
            Self::Offer | Self::Withdraw => 7,
            Self::MachineState(_) | Self::SourceState(_) | Self::VariantName { .. } => 8,
        }
    }
}

const FAILURE: u8 = 0;
const REPAIRED: u8 = 1;
const CYCLE_DONE: u8 = 2;
const SETUP_DONE: u8 = 3;
const NEXT_ARRIVAL: u8 = 4;
const ARRIVE: u8 = 5;
const ACCEPT: u8 = 6;
const DECLINE: u8 = 7;
const OFFER: u8 = 8;
const MACHINE_STATE: u8 = 9;
const SOURCE_STATE: u8 = 10;
const VARIANT_NAME: u8 = 11;
const WITHDRAW: u8 = 12;

impl TraceMessage for FlowMessage {
    /// The encoding of ADR-0022 §9: a tag `u8`, then its fields.
    const FORMAT: &'static str = "taktsim-components/1";

    fn encode(&self, out: &mut Encoder<'_>) {
        match self {
            Self::Failure => out.u8(FAILURE),
            Self::Repaired => out.u8(REPAIRED),
            Self::CycleDone { item } => {
                out.u8(CYCLE_DONE);
                out.item(*item);
            }
            Self::SetupDone => out.u8(SETUP_DONE),
            Self::NextArrival => out.u8(NEXT_ARRIVAL),
            Self::Arrive(attrs) => {
                out.u8(ARRIVE);
                out.u16(attrs.variant.get());
                out.u8(attrs.priority);
                out.bool(attrs.due.is_some());
                out.time(attrs.due.unwrap_or(SimTime::START));
            }
            Self::Accept => out.u8(ACCEPT),
            Self::Decline => out.u8(DECLINE),
            Self::Offer => out.u8(OFFER),
            Self::Withdraw => out.u8(WITHDRAW),
            Self::MachineState(note) => {
                out.u8(MACHINE_STATE);
                out.u8(note.state.code());
                out.u32(note.units_in_cycle);
                out.bool(note.setup_variant.is_some());
                out.u16(note.setup_variant.map_or(0, VariantId::get));
            }
            Self::SourceState(note) => {
                out.u8(SOURCE_STATE);
                out.u8(note.state.code());
                out.u32(note.held);
            }
            Self::VariantName { id, name } => {
                out.u8(VARIANT_NAME);
                out.u16(id.get());
                out.str(name);
            }
        }
    }

    fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let tag = input.u8()?;
        Ok(match tag {
            FAILURE => Self::Failure,
            REPAIRED => Self::Repaired,
            CYCLE_DONE => Self::CycleDone {
                item: input.item()?,
            },
            SETUP_DONE => Self::SetupDone,
            NEXT_ARRIVAL => Self::NextArrival,
            ARRIVE => {
                let variant = VariantId::new(input.u16()?);
                let priority = input.u8()?;
                let has_due = input.bool()?;
                let raw = input.time()?;
                let due = if has_due {
                    Some(raw)
                } else if raw == SimTime::START {
                    None
                } else {
                    return Err(input.invalid("due date of a part without one", raw.as_nanos(), 0));
                };
                Self::Arrive(ItemAttrs {
                    variant,
                    priority,
                    due,
                })
            }
            ACCEPT => Self::Accept,
            DECLINE => Self::Decline,
            OFFER => Self::Offer,
            WITHDRAW => Self::Withdraw,
            MACHINE_STATE => {
                let code = input.u8()?;
                let Some(state) = MachineState::from_code(code) else {
                    return Err(input.invalid("machine state", code.into(), "0 to 5"));
                };
                let units_in_cycle = input.u32()?;
                let has_variant = input.bool()?;
                let raw = input.u16()?;
                let setup_variant = if has_variant {
                    Some(VariantId::new(raw))
                } else if raw == 0 {
                    None
                } else {
                    return Err(input.invalid(
                        "setup variant of a note without one",
                        raw.into(),
                        0,
                    ));
                };
                Self::MachineState(MachineStateNote {
                    state,
                    units_in_cycle,
                    setup_variant,
                })
            }
            SOURCE_STATE => {
                let code = input.u8()?;
                let Some(state) = SourceState::from_code(code) else {
                    return Err(input.invalid("source state", code.into(), "0 or 1"));
                };
                Self::SourceState(SourceStateNote {
                    state,
                    held: input.u32()?,
                })
            }
            VARIANT_NAME => Self::VariantName {
                id: VariantId::new(input.u16()?),
                name: input.str()?.to_owned(),
            },
            other => return Err(input.invalid("FlowMessage tag", other.into(), "0 to 12")),
        })
    }
}
