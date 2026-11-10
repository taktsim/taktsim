//! Buffer: places between stations, with a discipline (spec §4, ADR-0022 §7).

use std::collections::BTreeMap;

use taktsim_core::{Component, ComponentError, Context, Delivery, ItemId, Violation};
use taktsim_model::{
    ComponentParams, Decode, Decoder, Encode, Fields, Json, LoadErrorKind, Location, Node, Schemas,
};
use taktsim_units::Count;

use crate::flow::{Flow, FlowShared};
use crate::message::{FlowMessage, ItemAttrs};
use crate::params::Places;
use crate::port::{Inlet, Outlet};
use crate::run::{BuildError, ComponentType, Wiring};

/// Which part a Buffer gives when a place opens downstream. It chooses when an
/// `Accept` arrives, not when it offers. The order is entry into this buffer,
/// never `ItemId`: an id follows creation, and parts overtake.
///
/// ```
/// use taktsim_components::Discipline;
///
/// assert_eq!(Discipline::Priority.as_str(), "priority");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discipline {
    /// First into this buffer.
    Fifo,
    /// Last into this buffer.
    Lifo,
    /// Highest priority, then first into this buffer.
    Priority,
}

impl Discipline {
    /// The word in a model.
    ///
    /// ```
    /// assert_eq!(taktsim_components::Discipline::Fifo.as_str(), "fifo");
    /// ```
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fifo => "fifo",
            Self::Lifo => "lifo",
            Self::Priority => "priority",
        }
    }
}

impl Encode for Discipline {
    fn encode(&self) -> Json {
        self.as_str().into()
    }
}

impl Decode for Discipline {
    const EXPECTED: &'static str = "fifo, lifo or priority";
    const EXAMPLE: &'static str = "fifo";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let text = String::decode(node, cx)?;
        let words = [Self::Fifo, Self::Lifo, Self::Priority];
        if let Some(discipline) = words.into_iter().find(|d| d.as_str() == text) {
            return Some(discipline);
        }
        let kind = LoadErrorKind::UnknownValue {
            what: "discipline",
            suggestion: None,
            found: text,
            allowed: words.iter().map(|d| d.as_str().to_owned()).collect(),
        };
        cx.error(node.start(), kind);
        None
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            (
                "enum",
                Json::Array(vec!["fifo".into(), "lifo".into(), "priority".into()]),
            ),
            (
                "description",
                Json::from(
                    "Which part leaves first: first into the buffer, last into it, or highest \
                     priority, then first into it. Default fifo.",
                ),
            ),
        ])
    }
}

/// `type: Buffer`: `capacity`, required and above zero, and `discipline`,
/// default `fifo`.
///
/// ```
/// use taktsim_components::Buffer;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components:\n\
///      - {id: source, type: Source, interarrival: 1min}\n\
///      - {id: buf1, type: Buffer, capacity: 20 parts, discipline: lifo}\n\
///      - {id: sink, type: Sink}\n\
///      flow: [[source, buf1, sink]]\n",
///     Format::Yaml,
/// )?;
/// assert!(model.document().components()[1].params::<Buffer>().is_some());
/// assert!(model.canonical_json().contains(r#""discipline":"lifo""#));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Buffer {
    capacity: Count,
    /// Where `capacity` is written; not part of the model.
    capacity_at: Location,
    discipline: Option<Discipline>,
}

impl Buffer {
    /// Its places, and where `capacity` is written.
    pub(crate) const fn capacity(&self) -> (Count, Location) {
        (self.capacity, self.capacity_at)
    }
}

impl Encode for Buffer {
    fn encode(&self) -> Json {
        Json::object([])
            .with("capacity", &self.capacity)
            .with_optional("discipline", self.discipline.as_ref())
    }
}

impl ComponentParams for Buffer {
    const TYPE: &'static str = "Buffer";
    const EXAMPLE: &'static str = "{id: buf1, type: Buffer, capacity: 20 parts}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let capacity = fields.required::<Places>("capacity");
        let discipline = fields.optional("discipline");
        let Places(capacity, capacity_at) = capacity?;
        Some(Self {
            capacity,
            capacity_at,
            discipline: discipline?,
        })
    }
}

impl ComponentType<Flow> for Buffer {
    fn build(
        &self,
        wiring: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(BufferBehaviour {
            capacity: self.capacity.get(),
            discipline: self.discipline.unwrap_or(Discipline::Fifo),
            downstream: wiring.downstream().to_vec(),
            inlet: Inlet::default(),
            outlet: Outlet::default(),
            parts: BTreeMap::new(),
            entered: 0,
        }))
    }
}

/// A Buffer at run time.
#[derive(Debug)]
struct BufferBehaviour {
    capacity: u64,
    discipline: Discipline,
    downstream: Vec<String>,
    inlet: Inlet,
    outlet: Outlet,
    /// The parts, in the order they leave.
    parts: BTreeMap<(u16, u64), (ItemId, ItemAttrs)>,
    /// How many parts entered: the entry order.
    entered: u64,
}

impl BufferBehaviour {
    /// The place in the leaving order of the part entering as number `entry`.
    fn order(&self, attrs: ItemAttrs, entry: u64) -> (u16, u64) {
        match self.discipline {
            Discipline::Fifo => (0, entry),
            Discipline::Lifo => (0, u64::MAX - entry),
            Discipline::Priority => (u16::from(u8::MAX - attrs.priority), entry),
        }
    }

    /// Free places.
    fn free(&self) -> u64 {
        self.capacity
            .saturating_sub(self.fill() + self.inlet.reserved())
    }

    fn fill(&self) -> u64 {
        self.parts.len() as u64
    }

    /// Takes what is waiting and offers what it has, or withdraws.
    fn settle(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.inlet.accept(self.free(), ctx)?;
        if self.parts.is_empty() {
            self.outlet.withdraw(ctx)
        } else {
            self.outlet.offer(ctx)
        }
    }
}

impl Component<FlowMessage> for BufferBehaviour {
    fn start(&mut self, ctx: &mut Context<'_, FlowMessage>) -> Result<(), ComponentError> {
        self.outlet.connect(&self.downstream, ctx)
    }

    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        match d.message {
            FlowMessage::Offer => self.inlet.offered(d.from),
            FlowMessage::Withdraw => self.inlet.withdrawn(d.from),
            FlowMessage::Decline => self.inlet.release("Decline")?,
            FlowMessage::Arrive(attrs) => {
                self.inlet.release("Arrive")?;
                let Some(item) = d.item else {
                    return Err(unexpected(d.message));
                };
                ctx.receive(item)?;
                let order = self.order(*attrs, self.entered);
                self.entered += 1;
                self.parts.insert(order, (item, *attrs));
            }
            FlowMessage::Accept => {
                self.outlet.accepted(d.from, ctx)?;
                match self.parts.pop_first() {
                    Some((_, (item, attrs))) => {
                        ctx.transfer(item, d.from, FlowMessage::Arrive(attrs))?;
                    }
                    None => ctx.send(d.from, FlowMessage::Decline)?,
                }
            }
            message @ (FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::CycleDone { .. }
            | FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }) => return Err(unexpected(message)),
        }
        self.settle(ctx)
    }

    fn held_items(&self, visit: &mut dyn FnMut(ItemId)) {
        for (item, _) in self.parts.values() {
            visit(*item);
        }
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        let (fill, reserved) = (self.fill(), self.inlet.reserved());
        if fill + reserved > self.capacity {
            return Err(Violation {
                invariant: "fill + reserved <= capacity",
                detail: format!(
                    "fill {fill}, reserved {reserved}, capacity {}",
                    self.capacity
                ),
            });
        }
        if fill + reserved < self.capacity && self.inlet.has_waiting() {
            return Err(Violation {
                invariant: "a free place and a waiting offer never coexist",
                detail: format!(
                    "fill {fill}, reserved {reserved}, capacity {}",
                    self.capacity
                ),
            });
        }
        if fill > 0 && !self.outlet.all_offered() {
            return Err(Violation {
                invariant: "a unit to give and no outstanding offer never coexist",
                detail: format!("fill {fill}"),
            });
        }
        Ok(())
    }
}

fn unexpected(message: &FlowMessage) -> ComponentError {
    ComponentError::Unexpected {
        state: "buffer",
        event: format!("{message:?}"),
    }
}

#[cfg(test)]
mod tests {
    use taktsim_core::ComponentId;

    use super::*;
    use crate::message::VariantId;

    fn buffer(capacity: u64) -> BufferBehaviour {
        BufferBehaviour {
            capacity,
            discipline: Discipline::Fifo,
            downstream: Vec::new(),
            inlet: Inlet::default(),
            outlet: Outlet::default(),
            parts: BTreeMap::new(),
            entered: 0,
        }
    }

    fn add(buffer: &mut BufferBehaviour, n: u64) {
        let attrs = ItemAttrs::new(VariantId::new(0), 0);
        buffer.parts.insert((0, n), (ItemId::from_raw(n), attrs));
    }

    #[test]
    fn its_invariants_catch_what_they_say() {
        let mut b = buffer(1);
        assert!(b.check_invariants().is_ok());
        add(&mut b, 0);
        add(&mut b, 1);
        let v = b.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "fill + reserved <= capacity");

        let mut b = buffer(1);
        b.inlet.offered(ComponentId::from_index(0));
        let v = b.check_invariants().unwrap_err();
        assert_eq!(
            v.invariant,
            "a free place and a waiting offer never coexist"
        );

        let mut b = buffer(1);
        add(&mut b, 0);
        b.outlet = Outlet::with_unoffered_target();
        let v = b.check_invariants().unwrap_err();
        assert_eq!(
            v.invariant,
            "a unit to give and no outstanding offer never coexist"
        );
    }

    #[test]
    fn the_leaving_order_is_entry_order_never_the_id() {
        let attrs = |priority| ItemAttrs::new(VariantId::new(0), priority);
        let mut b = buffer(9);
        assert!(b.order(attrs(9), 5) < b.order(attrs(0), 6));
        b.discipline = Discipline::Lifo;
        assert!(b.order(attrs(0), 6) < b.order(attrs(9), 5));
        b.discipline = Discipline::Priority;
        // Higher goes first, then earliest into the buffer.
        assert!(b.order(attrs(1), 6) < b.order(attrs(0), 5));
        assert!(b.order(attrs(1), 5) < b.order(attrs(1), 6));
        assert!(b.order(attrs(255), 0) < b.order(attrs(0), 0));
    }
}
