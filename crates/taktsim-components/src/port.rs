//! The protocol between neighbours (ADR-0022 §4), shared by every component:
//! an [`Outlet`] offers parts downstream, an [`Inlet`] takes offers from
//! upstream and reserves places.
//!
//! - An `Accept` always gets exactly one reply at the same instant, so a
//!   reservation never lasts longer than an instant.
//! - A holder calls the helpers within every event: a free place and a waiting
//!   offer never coexist, nor a part to give and no offer outstanding, so a
//!   wake-up cannot be lost.
//! - Between instants, a target holds an entry exactly when its upstream has an
//!   offer outstanding.
//!
//! An `Accept` can cross a `Withdraw` at one instant. Offers and withdrawals
//! keep their order (one priority), and `Accept` carries no number, so the
//! outlet cannot tell which offer an `Accept` answers once it has withdrawn
//! one this instant. It answers the `Accept` as usual and sends `Withdraw`
//! again: the last message on its way to that target then says what the
//! outlet believes, so both sides agree when the instant ends.

use std::collections::VecDeque;

use taktsim_core::{ComponentError, ComponentId, Context};
use taktsim_units::SimTime;

use crate::message::FlowMessage;

/// One downstream neighbour.
#[derive(Debug)]
struct Target {
    id: ComponentId,
    /// An offer is outstanding.
    offered: bool,
    /// When a withdrawal was last sent: at that instant an `Accept` can cross
    /// it.
    withdrawn_at: Option<SimTime>,
}

/// The upstream end of the protocol: at most one outstanding offer per
/// neighbour, in declared order.
#[derive(Debug, Default)]
pub(crate) struct Outlet {
    targets: Vec<Target>,
}

impl Outlet {
    /// Looks up the targets, in declared order.
    pub(crate) fn connect(
        &mut self,
        names: &[String],
        ctx: &Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        for name in names {
            self.targets.push(Target {
                id: ctx.lookup(name)?,
                offered: false,
                withdrawn_at: None,
            });
        }
        Ok(())
    }

    /// Offers to every target without an outstanding offer, in declared order.
    pub(crate) fn offer(
        &mut self,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        for target in &mut self.targets {
            if !target.offered {
                target.offered = true;
                ctx.send(target.id, FlowMessage::Offer)?;
            }
        }
        Ok(())
    }

    /// Withdraws every outstanding offer: there is nothing left to give.
    pub(crate) fn withdraw(
        &mut self,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let now = ctx.now();
        for target in &mut self.targets {
            if target.offered {
                target.offered = false;
                target.withdrawn_at = Some(now);
                ctx.send(target.id, FlowMessage::Withdraw)?;
            }
        }
        Ok(())
    }

    /// Takes in an `Accept` from `from`, which the caller answers with an
    /// `Arrive` or a `Decline`. If a withdrawal to `from` was sent at this
    /// instant and an offer stands, that offer may be on its way, so it is
    /// withdrawn too (module docs).
    pub(crate) fn accepted(
        &mut self,
        from: ComponentId,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        let now = ctx.now();
        let Some(target) = self.targets.iter_mut().find(|t| t.id == from) else {
            return Err(ComponentError::Unexpected {
                state: "an accept from a component that is not downstream",
                event: format!("Accept from component #{}", from.index()),
            });
        };
        let stood = target.offered;
        target.offered = false;
        if stood && target.withdrawn_at == Some(now) {
            ctx.send(from, FlowMessage::Withdraw)?;
        }
        Ok(())
    }

    /// Whether every target has an offer outstanding.
    pub(crate) fn all_offered(&self) -> bool {
        self.targets.iter().all(|t| t.offered)
    }
}

/// The downstream end of the protocol: offers waiting in the order they
/// arrived, and places reserved.
#[derive(Debug, Default)]
pub(crate) struct Inlet {
    waiting: VecDeque<ComponentId>,
    reserved: u64,
}

impl Inlet {
    /// An offer from `from` joins the queue.
    pub(crate) fn offered(&mut self, from: ComponentId) {
        self.waiting.push_back(from);
    }

    /// The offer from `from` leaves the queue. No entry, no effect.
    pub(crate) fn withdrawn(&mut self, from: ComponentId) {
        if let Some(position) = self.waiting.iter().position(|&u| u == from) {
            self.waiting.remove(position);
        }
    }

    /// Accepts the oldest waiting offers, as far as `free` places go.
    pub(crate) fn accept(
        &mut self,
        mut free: u64,
        ctx: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        while free > 0 {
            let Some(upstream) = self.waiting.pop_front() else {
                break;
            };
            self.reserved += 1;
            free -= 1;
            ctx.send(upstream, FlowMessage::Accept)?;
        }
        Ok(())
    }

    /// A reservation ended: a part arrived, or a `Decline` released it.
    pub(crate) fn release(&mut self, event: &'static str) -> Result<(), ComponentError> {
        self.reserved = self
            .reserved
            .checked_sub(1)
            .ok_or_else(|| ComponentError::Unexpected {
                state: "no place reserved",
                event: event.to_owned(),
            })?;
        Ok(())
    }

    /// Places reserved.
    pub(crate) const fn reserved(&self) -> u64 {
        self.reserved
    }

    /// Whether an offer is waiting.
    pub(crate) fn has_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}

#[cfg(test)]
impl Outlet {
    /// An outlet with one target and no offer to it.
    pub(crate) fn with_unoffered_target() -> Self {
        Self {
            targets: vec![Target {
                id: ComponentId::from_index(0),
                offered: false,
                withdrawn_at: None,
            }],
        }
    }
}
