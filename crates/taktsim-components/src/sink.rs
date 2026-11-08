//! Sink: takes every part and delivers it (spec §4).

use taktsim_core::{Component, ComponentError, Context, Delivery, Disposal, Violation};
use taktsim_model::{ComponentParams, Encode, Fields, Json};

use crate::flow::Flow;
use crate::message::FlowMessage;
use crate::port::Inlet;
use crate::run::{BuildError, ComponentType, Ports, Wiring};

/// `type: Sink`: the end of a line. No parameters.
///
/// ```
/// use taktsim_components::Sink;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
///      flow: [[source, cut, sink]]\n",
///     Format::Yaml,
/// )?;
/// assert!(model.document().components()[2].params::<Sink>().is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sink;

impl Encode for Sink {
    fn encode(&self) -> Json {
        Json::object([])
    }
}

impl ComponentParams for Sink {
    const TYPE: &'static str = "Sink";
    const EXAMPLE: &'static str = "{id: sink, type: Sink}";

    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

impl ComponentType<Flow> for Sink {
    const PORTS: Ports = Ports::Inlet;

    fn draws_random(&self) -> bool {
        false
    }

    fn build(
        &self,
        _: &Wiring<'_, crate::flow::FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(SinkBehaviour::default()))
    }
}

/// A Sink at run time. Holds nothing: a part it receives is delivered within
/// the same event.
#[derive(Debug, Default)]
struct SinkBehaviour {
    inlet: Inlet,
    received: u64,
    delivered: u64,
}

impl Component<FlowMessage> for SinkBehaviour {
    fn handle(
        &mut self,
        d: Delivery<'_, FlowMessage>,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        match d.message {
            FlowMessage::Offer => {
                self.inlet.offered(d.from);
                self.inlet.accept(1, ctx)?;
            }
            FlowMessage::Withdraw => self.inlet.withdrawn(d.from),
            FlowMessage::Decline => self.inlet.release("Decline")?,
            FlowMessage::Arrive(_) => {
                self.inlet.release("Arrive")?;
                let Some(item) = d.item else {
                    return Err(unexpected(d.message));
                };
                ctx.receive(item)?;
                self.received += 1;
                ctx.dispose(item, Disposal::Delivered)?;
                self.delivered += 1;
            }
            message @ (FlowMessage::Failure
            | FlowMessage::Repaired
            | FlowMessage::CycleDone { .. }
            | FlowMessage::SetupDone
            | FlowMessage::NextArrival
            | FlowMessage::Accept
            | FlowMessage::MachineState(_)
            | FlowMessage::SourceState(_)
            | FlowMessage::VariantName { .. }) => return Err(unexpected(message)),
        }
        Ok(())
    }

    fn check_invariants(&self) -> Result<(), Violation> {
        if self.received != self.delivered {
            return Err(Violation {
                invariant: "units received = units delivered",
                detail: format!("{} received, {} delivered", self.received, self.delivered),
            });
        }
        if self.inlet.has_waiting() {
            return Err(Violation {
                invariant: "a sink always takes an offer",
                detail: "an offer waits".to_owned(),
            });
        }
        Ok(())
    }
}

fn unexpected(message: &FlowMessage) -> ComponentError {
    ComponentError::Unexpected {
        state: "sink",
        event: format!("{message:?}"),
    }
}

#[cfg(test)]
mod tests {
    use taktsim_core::ComponentId;

    use super::*;

    #[test]
    fn its_invariants_catch_what_they_say() {
        let mut sink = SinkBehaviour::default();
        assert!(sink.check_invariants().is_ok());
        sink.received = 1;
        let v = sink.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "units received = units delivered");
        sink.delivered = 1;
        sink.inlet.offered(ComponentId::from_index(0));
        let v = sink.check_invariants().unwrap_err();
        assert_eq!(v.invariant, "a sink always takes an offer");
    }
}
