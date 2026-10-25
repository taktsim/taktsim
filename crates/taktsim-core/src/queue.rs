//! The future-event list (ADR-0001): a binary heap ordered by
//! `(time, priority, sequence)`, with lazy cancellation.
//!
//! The sequence number is unique and increases with every scheduled event,
//! so the order is total: two events at the same instant and priority pop
//! in the order they were scheduled, on every platform.

use std::cmp::Ordering;
use std::collections::binary_heap::PeekMut;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use taktsim_units::SimTime;

use crate::engine::{ComponentId, EventId, ItemId};

/// The order of events: earlier time first, then lower priority, then
/// earlier scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub(crate) time: SimTime,
    pub(crate) priority: u8,
    pub(crate) seq: u64,
}

/// An event waiting in the queue.
#[derive(Debug)]
pub(crate) struct Scheduled<M> {
    pub(crate) key: Key,
    pub(crate) target: ComponentId,
    pub(crate) from: ComponentId,
    pub(crate) message: M,
    /// The item this event carries to `target`, if any.
    pub(crate) item: Option<ItemId>,
    /// Whether this is a cancellable timer (tracked in `timers`).
    pub(crate) timer: bool,
}

// `BinaryHeap` is a max-heap; reversing the key makes it pop the smallest.
impl<M> PartialEq for Scheduled<M> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<M> Eq for Scheduled<M> {}

impl<M> PartialOrd for Scheduled<M> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<M> Ord for Scheduled<M> {
    fn cmp(&self, other: &Self) -> Ordering {
        other.key.cmp(&self.key)
    }
}

/// Why an event could not be cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelError {
    /// Not a pending timer: already delivered, cancelled, or never a timer.
    NotPending,
    /// A pending timer of another component.
    NotOwner(ComponentId),
}

/// The future-event list.
#[derive(Debug)]
pub(crate) struct EventQueue<M> {
    heap: BinaryHeap<Scheduled<M>>,
    next_seq: u64,
    /// Pending cancellable events (timers) and the component that owns each.
    timers: BTreeMap<u64, ComponentId>,
    /// Cancelled events that are still in the heap.
    cancelled: BTreeSet<u64>,
}

impl<M> EventQueue<M> {
    pub(crate) fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
            timers: BTreeMap::new(),
            cancelled: BTreeSet::new(),
        }
    }

    /// Schedules an event; `timer` events can later be cancelled by
    /// `target`, which owns them.
    pub(crate) fn push(&mut self, event: Pending<M>) -> EventId {
        let seq = self.next_seq;
        // 2^64 scheduled events cannot occur in a run.
        self.next_seq = self.next_seq.wrapping_add(1);
        if event.timer {
            self.timers.insert(seq, event.target);
        }
        self.heap.push(Scheduled {
            key: Key {
                time: event.time,
                priority: event.priority,
                seq,
            },
            target: event.target,
            from: event.from,
            message: event.message,
            item: event.item,
            timer: event.timer,
        });
        EventId(seq)
    }

    /// Cancels the pending timer `id`, which `by` must own.
    pub(crate) fn cancel(&mut self, id: EventId, by: ComponentId) -> Result<(), CancelError> {
        let owner = *self.timers.get(&id.0).ok_or(CancelError::NotPending)?;
        if owner != by {
            return Err(CancelError::NotOwner(owner));
        }
        self.timers.remove(&id.0);
        self.cancelled.insert(id.0);
        // Rebuild once stale entries make up more than half of the heap.
        if self.cancelled.len() * 2 > self.heap.len() {
            let cancelled = std::mem::take(&mut self.cancelled);
            self.heap
                .retain(|event| !cancelled.contains(&event.key.seq));
        }
        Ok(())
    }

    /// Removes and returns the next live event if it is due by `until`
    /// and `take` is true; otherwise says why not.
    pub(crate) fn pop_until(&mut self, until: SimTime, take: bool) -> Next<M> {
        self.drop_cancelled();
        let Some(top) = self.heap.peek_mut() else {
            return Next::Empty;
        };
        if top.key.time > until {
            return Next::Later;
        }
        if !take {
            return Next::Waiting;
        }
        let event = PeekMut::pop(top);
        if event.timer {
            self.timers.remove(&event.key.seq);
        }
        Next::Due(event)
    }

    /// The number of live events.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.heap.len() - self.cancelled.len()
    }

    fn drop_cancelled(&mut self) {
        while let Some(top) = self.heap.peek() {
            if !self.cancelled.remove(&top.key.seq) {
                break;
            }
            self.heap.pop();
        }
    }
}

/// The result of [`EventQueue::pop_until`].
#[derive(Debug)]
pub(crate) enum Next<M> {
    /// The next event, due by the limit.
    Due(Scheduled<M>),
    /// An event is due, but the caller did not want to take it.
    Waiting,
    /// The next event is after the limit; it stays in the queue.
    Later,
    /// No events are left.
    Empty,
}

/// An event to schedule.
pub(crate) struct Pending<M> {
    pub(crate) time: SimTime,
    pub(crate) priority: u8,
    pub(crate) target: ComponentId,
    pub(crate) from: ComponentId,
    pub(crate) message: M,
    pub(crate) item: Option<ItemId>,
    pub(crate) timer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: ComponentId = ComponentId(0);
    const B: ComponentId = ComponentId(1);

    fn at(nanos: u64, priority: u8, timer: bool) -> Pending<u64> {
        Pending {
            time: SimTime::from_nanos(nanos),
            priority,
            target: A,
            from: A,
            message: nanos,
            item: None,
            timer,
        }
    }

    fn drain(queue: &mut EventQueue<u64>) -> Vec<(u64, u8, u64)> {
        let end = SimTime::from_nanos(u64::MAX);
        std::iter::from_fn(|| match queue.pop_until(end, true) {
            Next::Due(e) => Some((e.key.time.as_nanos(), e.key.priority, e.key.seq)),
            Next::Waiting | Next::Later | Next::Empty => None,
        })
        .collect()
    }

    #[test]
    fn pops_by_time_then_priority_then_scheduling_order() {
        let mut q = EventQueue::new();
        q.push(at(5, 0, false));
        q.push(at(3, 1, false));
        q.push(at(3, 0, false));
        q.push(at(3, 1, false));
        assert!(matches!(
            q.pop_until(SimTime::from_nanos(2), true),
            Next::Later
        ));
        assert!(matches!(
            q.pop_until(SimTime::from_nanos(3), false),
            Next::Waiting
        ));
        assert_eq!(drain(&mut q), [(3, 0, 2), (3, 1, 1), (3, 1, 3), (5, 0, 0)]);
        assert!(matches!(
            q.pop_until(SimTime::from_nanos(9), true),
            Next::Empty
        ));
    }

    #[test]
    fn cancelled_timers_never_pop() {
        let mut q = EventQueue::new();
        let first = q.push(at(1, 0, true));
        q.push(at(2, 0, true));
        q.push(at(3, 0, true));
        q.push(at(4, 0, true));
        q.cancel(first, A).unwrap();
        assert_eq!(q.len(), 3);
        assert_eq!(drain(&mut q), [(2, 0, 1), (3, 0, 2), (4, 0, 3)]);
    }

    #[test]
    fn many_cancellations_compact_the_heap() {
        let mut q = EventQueue::new();
        let ids: Vec<EventId> = (0..10).map(|t| q.push(at(t, 0, true))).collect();
        for id in &ids[..5] {
            q.cancel(*id, A).unwrap();
        }
        // Up to half of the heap may be stale: no compaction yet.
        assert_eq!((q.cancelled.len(), q.heap.len()), (5, 10));
        q.cancel(ids[5], A).unwrap();
        // The sixth cancellation crossed the half-way mark and compacted.
        assert!(q.cancelled.is_empty());
        assert_eq!(q.heap.len(), 4);
        assert_eq!(drain(&mut q), [(6, 0, 6), (7, 0, 7), (8, 0, 8), (9, 0, 9)]);
    }

    #[test]
    fn events_are_equal_exactly_when_their_keys_are() {
        let event = |seq, message| Scheduled {
            key: Key {
                time: SimTime::from_nanos(1),
                priority: 0,
                seq,
            },
            target: A,
            from: A,
            message,
            item: None,
            timer: false,
        };
        // Equal keys, different messages: equal. Different keys: not.
        assert_eq!(event(0, 1_u64), event(0, 99));
        assert_ne!(event(0, 1), event(1, 1));
        assert_eq!(event(0, 1).cmp(&event(0, 99)), Ordering::Equal);
    }

    #[test]
    fn only_the_owner_cancels_a_pending_timer() {
        let mut q = EventQueue::new();
        let timer = q.push(at(1, 0, true));
        let message = q.push(at(1, 0, false));
        assert_eq!(q.cancel(timer, B), Err(CancelError::NotOwner(A)));
        assert_eq!(q.cancel(message, A), Err(CancelError::NotPending));
        q.cancel(timer, A).unwrap();
        assert_eq!(q.cancel(timer, A), Err(CancelError::NotPending));
        assert_eq!(drain(&mut q), [(1, 0, 1)]);
        // Delivered timers are no longer pending.
        let later = q.push(at(2, 0, true));
        assert_eq!(drain(&mut q), [(2, 0, 2)]);
        assert_eq!(q.cancel(later, A), Err(CancelError::NotPending));
    }
}
