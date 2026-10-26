//! Records: the owned form a reader returns, and their byte layout
//! (ADR-0021).
//!
//! Every record starts with a kind byte, then fixed-width little-endian
//! fields. An event's message comes last, after its length (`u16`), so a
//! record's end can be found without decoding the message.
//!
//! | Kind | Byte | Fields after the kind byte |
//! | --- | --- | --- |
//! | event | 1 | seq u64, time u64 (ns), target u32, from u32, has item u8 (0/1), item u64 (0 if none), message length u16, message |
//! | item created | 2 | time u64, item u64, by u32 |
//! | item disposed | 3 | time u64, item u64, by u32, how u8 (0 delivered, 1 scrapped, 2 consumed) |

use std::fmt;

use taktsim_core::{ComponentId, Disposal, ItemId, TraceRecord};
use taktsim_units::SimTime;

use crate::codec::{DecodeError, Decoder, EncodeError, Encoder, TraceMessage};

pub(crate) const EVENT: u8 = 1;
pub(crate) const ITEM_CREATED: u8 = 2;
pub(crate) const ITEM_DISPOSED: u8 = 3;

/// Size of an event record without its message.
pub(crate) const EVENT_FIXED: usize = 1 + 8 + 8 + 4 + 4 + 1 + 8 + 2;
const ITEM_CREATED_SIZE: usize = 1 + 8 + 8 + 4;
const ITEM_DISPOSED_SIZE: usize = ITEM_CREATED_SIZE + 1;

/// One record read back from a trace: the owned form of
/// [`TraceRecord`], with the message decoded.
///
/// ```
/// use taktsim_core::{ComponentId, ItemId, TraceRecord};
/// use taktsim_trace::Record;
///
/// let record: Record<()> = Record::ItemCreated {
///     time: "5s".parse()?,
///     item: ItemId::from_raw(0),
///     by: ComponentId::from_index(1),
/// };
/// assert!(matches!(record.as_trace(), TraceRecord::ItemCreated { .. }));
/// assert_eq!(record.time().to_string(), "5s");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Record<M> {
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
        message: M,
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
    /// `by` disposed of `item`.
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

impl<M> Record<M> {
    /// The record as the engine emits it, borrowing the message.
    ///
    /// ```
    /// use taktsim_core::{ComponentId, TraceRecord};
    /// use taktsim_trace::Record;
    ///
    /// let c = ComponentId::from_index(0);
    /// let record = Record::Event { seq: 1, time: "1s".parse()?, target: c, from: c, message: 'x', item: None };
    /// assert!(matches!(record.as_trace(), TraceRecord::Event { message: &'x', .. }));
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn as_trace(&self) -> TraceRecord<'_, M> {
        match self {
            Self::Event {
                seq,
                time,
                target,
                from,
                message,
                item,
            } => TraceRecord::Event {
                seq: *seq,
                time: *time,
                target: *target,
                from: *from,
                message,
                item: *item,
            },
            Self::ItemCreated { time, item, by } => TraceRecord::ItemCreated {
                time: *time,
                item: *item,
                by: *by,
            },
            Self::ItemDisposed {
                time,
                item,
                by,
                how,
            } => TraceRecord::ItemDisposed {
                time: *time,
                item: *item,
                by: *by,
                how: *how,
            },
        }
    }

    /// The simulation time of the record.
    ///
    /// ```
    /// use taktsim_core::{ComponentId, ItemId, Disposal};
    /// use taktsim_trace::Record;
    ///
    /// let r: Record<()> = Record::ItemDisposed {
    ///     time: "1min".parse()?, item: ItemId::from_raw(0), by: ComponentId::from_index(0), how: Disposal::Delivered,
    /// };
    /// assert_eq!(r.time().to_string(), "1min");
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn time(&self) -> SimTime {
        match self {
            Self::Event { time, .. }
            | Self::ItemCreated { time, .. }
            | Self::ItemDisposed { time, .. } => *time,
        }
    }

    /// The record as one line of text, with components named by
    /// `components` (the trace header's list), e.g.
    /// `12s event #4 source -> sink Arrive, item 0`. The message is shown
    /// with its `Debug` text. Used by text dumps and replay reports.
    ///
    /// ```
    /// use taktsim_core::{ComponentId, ItemId};
    /// use taktsim_trace::Record;
    ///
    /// let names = ["source".to_owned(), "sink".to_owned()];
    /// let r: Record<()> = Record::ItemCreated { time: "2s".parse()?, item: ItemId::from_raw(3), by: ComponentId::from_index(0) };
    /// assert_eq!(r.display(&names).to_string(), "2s item 3 created by source");
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    #[must_use]
    pub fn display<'a>(&'a self, components: &'a [String]) -> impl fmt::Display + 'a
    where
        M: fmt::Debug,
    {
        Shown {
            record: self.as_trace(),
            components,
        }
    }
}

/// One record as a line of text (see [`Record::display`]).
pub(crate) fn line<M: fmt::Debug>(record: &TraceRecord<'_, M>, components: &[String]) -> String {
    Line { record, components }.to_string()
}

/// A record shown by [`Record::display`].
struct Shown<'a, M> {
    record: TraceRecord<'a, M>,
    components: &'a [String],
}

impl<M: fmt::Debug> fmt::Display for Shown<'_, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Line {
            record: &self.record,
            components: self.components,
        }
        .fmt(f)
    }
}

struct Line<'b, 'a, M> {
    record: &'b TraceRecord<'a, M>,
    components: &'b [String],
}

impl<M: fmt::Debug> fmt::Display for Line<'_, '_, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |id: ComponentId| Name(self.components.get(id.index()).map(String::as_str), id);
        match *self.record {
            TraceRecord::Event {
                seq,
                time,
                target,
                from,
                message,
                item,
            } => {
                write!(
                    f,
                    "{time} event #{seq} {} -> {} {message:?}",
                    name(from),
                    name(target)
                )?;
                if let Some(item) = item {
                    write!(f, ", item {}", item.as_u64())?;
                }
                Ok(())
            }
            TraceRecord::ItemCreated { time, item, by } => {
                write!(f, "{time} item {} created by {}", item.as_u64(), name(by))
            }
            TraceRecord::ItemDisposed {
                time,
                item,
                by,
                how,
            } => {
                let how = match how {
                    Disposal::Delivered => "delivered",
                    Disposal::Scrapped => "scrapped",
                    Disposal::Consumed => "consumed",
                };
                write!(f, "{time} item {} {how} by {}", item.as_u64(), name(by))
            }
        }
    }
}

/// A component by name, or by number if the name is unknown.
struct Name<'a>(Option<&'a str>, ComponentId);

impl fmt::Display for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(name) => f.write_str(name),
            None => write!(f, "#{}", self.1.index()),
        }
    }
}

/// The largest record: one fits in a chunk (ADR-0021).
pub(crate) const MAX_RECORD: usize = crate::CHUNK_CAPACITY;

/// Appends `record` to `out`; on an error, `out` is left as it was.
pub(crate) fn encode<M: TraceMessage>(
    record: &TraceRecord<'_, M>,
    out: &mut Vec<u8>,
) -> Result<(), EncodeError> {
    let start = out.len();
    let mut encoder = Encoder::new(out);
    match record {
        TraceRecord::Event {
            seq,
            time,
            target,
            from,
            message,
            item,
        } => {
            encoder.u8(EVENT);
            encoder.u64(*seq);
            encoder.time(*time);
            encoder.component(*target);
            encoder.component(*from);
            encoder.bool(item.is_some());
            encoder.u64(item.map_or(0, ItemId::as_u64));
            // The message length, filled in below.
            encoder.u16(0);
            message.encode(&mut encoder);
        }
        TraceRecord::ItemCreated { time, item, by } => {
            encoder.u8(ITEM_CREATED);
            encoder.time(*time);
            encoder.item(*item);
            encoder.component(*by);
        }
        TraceRecord::ItemDisposed {
            time,
            item,
            by,
            how,
        } => {
            encoder.u8(ITEM_DISPOSED);
            encoder.time(*time);
            encoder.item(*item);
            encoder.component(*by);
            encoder.u8(match how {
                Disposal::Delivered => 0,
                Disposal::Scrapped => 1,
                Disposal::Consumed => 2,
            });
        }
    }
    let result = encoder.finish().and_then(|()| {
        let bytes = out.len() - start;
        let too_large = EncodeError::RecordTooLarge {
            bytes,
            max: MAX_RECORD,
        };
        if bytes > MAX_RECORD {
            return Err(too_large);
        }
        if out[start] == EVENT {
            // At most MAX_RECORD - EVENT_FIXED, so it fits a u16.
            let length = u16::try_from(bytes - EVENT_FIXED).map_err(|_| too_large)?;
            out[start + EVENT_FIXED - 2..start + EVENT_FIXED]
                .copy_from_slice(&length.to_le_bytes());
        }
        Ok(())
    });
    if result.is_err() {
        out.truncate(start);
    }
    result
}

/// The kind and length of the record that starts `bytes`, found without
/// decoding its message.
pub(crate) fn frame(bytes: &[u8]) -> Result<(u8, usize), DecodeError> {
    let mut input = Decoder::new(bytes);
    let kind = input.u8()?;
    let fixed = match kind {
        EVENT => EVENT_FIXED,
        ITEM_CREATED => ITEM_CREATED_SIZE,
        ITEM_DISPOSED => ITEM_DISPOSED_SIZE,
        kind => return Err(input.invalid("record kind", kind.into(), "1, 2 or 3")),
    };
    input.slice(fixed - 1)?;
    let message = if kind == EVENT {
        let length = u16::from_le_bytes([bytes[EVENT_FIXED - 2], bytes[EVENT_FIXED - 1]]);
        input.slice(length.into())?;
        usize::from(length)
    } else {
        0
    };
    Ok((kind, fixed + message))
}

/// The time of an event record, at a fixed offset.
pub(crate) fn event_time(bytes: &[u8]) -> SimTime {
    let mut time = [0; 8];
    time.copy_from_slice(&bytes[9..17]);
    SimTime::from_nanos(u64::from_le_bytes(time))
}

/// Decodes one record that fills `bytes`, checking that it names only the
/// trace's `components`.
pub(crate) fn decode<M: TraceMessage>(
    bytes: &[u8],
    components: usize,
) -> Result<Record<M>, DecodeError> {
    let mut input = Decoder::new(bytes);
    let record = match input.u8()? {
        EVENT => {
            let seq = input.u64()?;
            let time = input.time()?;
            let target = component(&mut input, components)?;
            let from = component(&mut input, components)?;
            let has_item = input.bool()?;
            let raw = input.u64()?;
            let item = if has_item {
                Some(ItemId::from_raw(raw))
            } else if raw == 0 {
                None
            } else {
                return Err(input.invalid("item number of an event without an item", raw, 0));
            };
            let length = input.u16()?;
            let message = crate::codec::decode_message(input.slice(length.into())?)?;
            Record::Event {
                seq,
                time,
                target,
                from,
                message,
                item,
            }
        }
        ITEM_CREATED => Record::ItemCreated {
            time: input.time()?,
            item: input.item()?,
            by: component(&mut input, components)?,
        },
        ITEM_DISPOSED => Record::ItemDisposed {
            time: input.time()?,
            item: input.item()?,
            by: component(&mut input, components)?,
            how: match input.u8()? {
                0 => Disposal::Delivered,
                1 => Disposal::Scrapped,
                2 => Disposal::Consumed,
                how => {
                    return Err(input.invalid(
                        "disposal",
                        how.into(),
                        "0 (delivered), 1 (scrapped) or 2 (consumed)",
                    ));
                }
            },
        },
        kind => return Err(input.invalid("record kind", kind.into(), "1, 2 or 3")),
    };
    input.finish()?;
    Ok(record)
}

/// Reads a component number and checks it against the header.
fn component(input: &mut Decoder<'_>, components: usize) -> Result<ComponentId, DecodeError> {
    let id = input.component()?;
    if id.index() < components {
        Ok(id)
    } else {
        Err(input.invalid(
            "component",
            id.index() as u64,
            format_args!("below {components}, the number of components in the header"),
        ))
    }
}
