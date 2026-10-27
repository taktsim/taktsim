//! Trace records: what happened in a run, in order (ADR-0004).
//!
//! The engine emits a record for every delivered event and every item
//! created or disposed of. It writes them to a [`TraceSink`] the caller
//! provides; the core itself does no I/O. Storage and replay live in
//! `taktsim-trace` (ADR-0004, ADR-0021).

use taktsim_units::SimTime;

use crate::engine::{ComponentId, Disposal, ItemId};

/// One thing that happened in a run.
///
/// ```
/// use taktsim_core::{ComponentId, TraceRecord};
///
/// let record: TraceRecord<'_, &str> = TraceRecord::Event {
///     seq: 0,
///     time: "1s".parse()?,
///     target: ComponentId::from_index(0),
///     from: ComponentId::from_index(0),
///     message: &"Tick",
///     item: None,
/// };
/// assert!(matches!(record, TraceRecord::Event { seq: 0, .. }));
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
///
/// Not `#[non_exhaustive]`: a trace writer must encode every kind, so a new
/// kind has to fail to compile there instead of being dropped by a wildcard
/// arm (ADR-0021).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceRecord<'a, M> {
    /// An event was delivered to `target`.
    Event {
        /// The event's sequence number, unique within the run.
        seq: u64,
        /// The simulation time of delivery.
        time: SimTime,
        /// The component that handled it.
        target: ComponentId,
        /// The component that sent it (the target itself for a timer).
        from: ComponentId,
        /// The message.
        message: &'a M,
        /// The item it carried, if any.
        item: Option<ItemId>,
    },
    /// `by` created `item`.
    ItemCreated {
        /// The simulation time.
        time: SimTime,
        /// The new item.
        item: ItemId,
        /// The component that created it.
        by: ComponentId,
    },
    /// `by` disposed of `item`: delivered, scrapped or consumed.
    ItemDisposed {
        /// The simulation time.
        time: SimTime,
        /// The item.
        item: ItemId,
        /// The component that disposed of it.
        by: ComponentId,
        /// How the item left the model.
        how: Disposal,
    },
}

/// Receives trace records as they happen.
///
/// ```
/// use taktsim_core::{TraceRecord, TraceSink};
///
/// /// Counts events.
/// struct Counter(u64);
///
/// impl TraceSink<&'static str> for Counter {
///     fn record(&mut self, record: &TraceRecord<'_, &'static str>) {
///         if let TraceRecord::Event { .. } = record {
///             self.0 += 1;
///         }
///     }
/// }
/// ```
pub trait TraceSink<M> {
    /// Called once per record, in the order things happen. Recording cannot
    /// fail the run: a sink that writes somewhere reports its own errors
    /// when it is finished.
    fn record(&mut self, record: &TraceRecord<'_, M>);
}

/// A sink that discards every record.
///
/// ```
/// use taktsim_core::{NoTrace, TraceRecord, TraceSink};
///
/// let mut sink = NoTrace;
/// sink.record(&TraceRecord::<u8>::ItemCreated {
///     time: taktsim_units::SimTime::START,
///     item: taktsim_core::ItemId::from_raw(0),
///     by: taktsim_core::ComponentId::from_index(0),
/// });
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTrace;

impl<M> TraceSink<M> for NoTrace {
    fn record(&mut self, _record: &TraceRecord<'_, M>) {}
}
