//! The streaming trace writer (ADR-0004, ADR-0021).

use std::io::{self, Write};
use std::marker::PhantomData;

use sha2::{Digest, Sha256};
use taktsim_core::{TraceRecord, TraceSink};

use crate::codec::{EncodeError, TraceMessage};
use crate::header::{CHUNK_CAPACITY, CHUNK_TAG, END_TAG, Header, TraceHash};
use crate::record;

/// What a finished trace holds, from the writer or from a reader that read
/// it to the end.
///
/// ```
/// use taktsim_trace::{Summary, TraceHash};
///
/// let s = Summary { records: 0, events: 0, chunks: 0, bytes: 100, hash: TraceHash::sha256(b"") };
/// assert_eq!(s.records, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Records in the trace.
    pub records: u64,
    /// Of these, delivered events.
    pub events: u64,
    /// Chunks the records are stored in.
    pub chunks: u64,
    /// The size of the trace in bytes.
    pub bytes: u64,
    /// The SHA-256 of the record stream.
    pub hash: TraceHash,
}

/// Writes a trace as the engine runs: pass it to
/// [`Engine::run`](taktsim_core::Engine::run) as the trace sink, then call
/// [`finish`](Self::finish).
///
/// Records are collected into one chunk of at most
/// [`CHUNK_CAPACITY`] bytes, which is written out with its CRC32 when the
/// next record does not fit, so memory stays at one chunk whatever the
/// length of the run. Recording cannot fail the run (ADR-0018): the first
/// error is kept, later records are dropped, and `finish` returns it. A
/// trace whose writer failed or was dropped without `finish` has no end
/// marker, so readers report it as truncated.
///
/// ```
/// use taktsim_core::{Component, ComponentError, Context, Delivery, Engine, EngineConfig, Message, RunLimits};
/// use taktsim_trace::{DecodeError, Decoder, Encoder, Header, ModelHash, TraceMessage, TraceWriter};
///
/// #[derive(Debug)]
/// struct Tick;
/// impl Message for Tick {}
/// impl TraceMessage for Tick {
///     const FORMAT: &'static str = "tick/1";
///     fn encode(&self, _: &mut Encoder<'_>) {}
///     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
/// }
///
/// struct Clock;
/// impl Component<Tick> for Clock {
///     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
///         ctx.schedule("1s".parse()?, Tick)?;
///         Ok(())
///     }
///     fn handle(&mut self, _: Delivery<'_, Tick>, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
///         ctx.schedule("1s".parse()?, Tick)?;
///         Ok(())
///     }
/// }
///
/// let mut engine = Engine::new(EngineConfig::new(1));
/// engine.add_component("clock", Box::new(Clock))?;
/// let header = Header::for_engine(&engine, ModelHash::sha256(b"clock"));
/// // A `File` in practice; a `Vec` here.
/// let mut writer = TraceWriter::new(Vec::new(), &header)?;
/// engine.run(RunLimits::until("1min".parse()?), &mut writer)?;
/// let (bytes, summary) = writer.finish()?;
/// assert_eq!((summary.records, summary.events), (60, 60));
/// assert_eq!(summary.bytes, bytes.len() as u64);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct TraceWriter<W: Write, M> {
    out: W,
    /// The records of the open chunk.
    chunk: Vec<u8>,
    chunk_records: u32,
    /// Records in closed chunks.
    closed_records: u64,
    events: u64,
    chunks: u64,
    bytes: u64,
    components: usize,
    hasher: Sha256,
    error: Option<WriteError>,
    _message: PhantomData<fn(&M)>,
}

impl<W: Write, M: TraceMessage> TraceWriter<W, M> {
    /// A writer that writes `header` to `out` at once. `out` should not
    /// buffer: the writer writes whole chunks.
    ///
    /// # Errors
    ///
    /// [`WriteError::Header`] if a text in the header is longer than 65,535
    /// bytes, [`WriteError::MessageFormat`] if the header names another
    /// message format than `M`'s, [`WriteError::Io`] if `out` fails.
    ///
    /// ```
    /// use taktsim_trace::{Header, ModelHash, TraceWriter, WriteError};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    ///
    /// let header = Header::new::<Tick>(ModelHash::default(), 1, vec!["x".repeat(70_000)]);
    /// let result = TraceWriter::<_, Tick>::new(Vec::new(), &header);
    /// assert!(matches!(result, Err(WriteError::Header(_))));
    /// ```
    pub fn new(mut out: W, header: &Header) -> Result<Self, WriteError> {
        if header.message_format != M::FORMAT {
            return Err(WriteError::MessageFormat {
                header: header.message_format.clone(),
                messages: M::FORMAT,
            });
        }
        let bytes = header.encode().map_err(WriteError::Header)?;
        out.write_all(&bytes)
            .map_err(|source| WriteError::Io { offset: 0, source })?;
        Ok(Self {
            out,
            chunk: Vec::with_capacity(CHUNK_CAPACITY),
            chunk_records: 0,
            closed_records: 0,
            events: 0,
            chunks: 0,
            bytes: bytes.len() as u64,
            components: header.components.len(),
            hasher: Sha256::new(),
            error: None,
            _message: PhantomData,
        })
    }

    /// Writes the last chunk and the end marker, flushes `out` and returns
    /// it with the trace's summary.
    ///
    /// # Errors
    ///
    /// The first error while recording or writing. The trace then has no
    /// end marker.
    ///
    /// ```
    /// use taktsim_trace::{Header, ModelHash, TraceWriter};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    ///
    /// let header = Header::new::<Tick>(ModelHash::default(), 1, vec![]);
    /// let writer = TraceWriter::<_, Tick>::new(Vec::new(), &header)?;
    /// let (bytes, summary) = writer.finish()?;
    /// assert_eq!((summary.records, summary.chunks), (0, 0));
    /// assert_eq!(summary.bytes, bytes.len() as u64);
    /// # Ok::<(), taktsim_trace::WriteError>(())
    /// ```
    pub fn finish(mut self) -> Result<(W, Summary), WriteError> {
        if self.error.is_none() && !self.chunk.is_empty() {
            self.close_chunk();
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        let records = self.closed_records;
        let hash = TraceHash::from_bytes(self.hasher.finalize().into());
        let mut end = Vec::with_capacity(1 + crate::header::END_SIZE);
        end.push(END_TAG);
        end.extend_from_slice(&records.to_le_bytes());
        end.extend_from_slice(&self.events.to_le_bytes());
        end.extend_from_slice(&self.chunks.to_le_bytes());
        end.extend_from_slice(hash.as_bytes());
        let crc = crc32fast::hash(&end);
        end.extend_from_slice(&crc.to_le_bytes());
        let offset = self.bytes;
        self.out
            .write_all(&end)
            .and_then(|()| self.out.flush())
            .map_err(|source| WriteError::Io { offset, source })?;
        let summary = Summary {
            records,
            events: self.events,
            chunks: self.chunks,
            bytes: self.bytes + end.len() as u64,
            hash,
        };
        Ok((self.out, summary))
    }

    /// Writes the open chunk: tag, length, records, first record, CRC32,
    /// then the records.
    fn close_chunk(&mut self) {
        self.close_chunk_at(self.chunk.len());
    }

    /// Writes the first `len` bytes of the open chunk as a chunk and keeps
    /// the rest (one record that did not fit) as the start of the next.
    fn close_chunk_at(&mut self, len: usize) {
        let records = if len == self.chunk.len() {
            self.chunk_records
        } else {
            self.chunk_records - 1
        };
        // At most CHUNK_CAPACITY, which fits a u32 (checked below).
        let len32 = u32::try_from(len).unwrap_or(u32::MAX);
        let payload = &self.chunk[..len];
        let mut head = [0; 1 + crate::header::CHUNK_HEADER];
        head[0] = CHUNK_TAG;
        head[1..5].copy_from_slice(&len32.to_le_bytes());
        head[5..9].copy_from_slice(&records.to_le_bytes());
        head[9..17].copy_from_slice(&self.closed_records.to_le_bytes());
        let mut crc = crc32fast::Hasher::new();
        crc.update(&head[..17]);
        crc.update(payload);
        head[17..21].copy_from_slice(&crc.finalize().to_le_bytes());
        self.hasher.update(payload);
        let offset = self.bytes;
        if let Err(source) = self
            .out
            .write_all(&head)
            .and_then(|()| self.out.write_all(payload))
        {
            self.error = Some(WriteError::Io { offset, source });
            return;
        }
        self.bytes += (head.len() + len) as u64;
        self.chunks += 1;
        self.closed_records += u64::from(records);
        self.chunk.drain(..len);
        self.chunk_records -= records;
    }
}

impl<W: Write, M: TraceMessage> TraceSink<M> for TraceWriter<W, M> {
    fn record(&mut self, trace_record: &TraceRecord<'_, M>) {
        if self.error.is_some() {
            return;
        }
        let index = self.closed_records + u64::from(self.chunk_records);
        let start = self.chunk.len();
        if let Err(source) = record::encode(trace_record, &mut self.chunk) {
            self.error = Some(WriteError::Encode {
                record: index,
                source,
            });
            return;
        }
        let component = highest_component(trace_record).index();
        if component >= self.components {
            self.chunk.truncate(start);
            self.error = Some(WriteError::UnknownComponent {
                record: index,
                component,
                components: self.components,
            });
            return;
        }
        if let TraceRecord::Event { .. } = trace_record {
            self.events += 1;
        }
        self.chunk_records += 1;
        if self.chunk.len() > CHUNK_CAPACITY {
            self.close_chunk_at(start);
        } else if self.chunk.len() == CHUNK_CAPACITY {
            self.close_chunk();
        }
    }
}

// Chunk lengths are written as u32.
const _: () = assert!(CHUNK_CAPACITY <= u32::MAX as usize);

/// The highest-numbered component a record names.
fn highest_component<M>(record: &TraceRecord<'_, M>) -> taktsim_core::ComponentId {
    match record {
        TraceRecord::Event { target, from, .. } => *target.max(from),
        TraceRecord::ItemCreated { by, .. } | TraceRecord::ItemDisposed { by, .. } => *by,
    }
}

/// Why a trace could not be written.
///
/// ```
/// use taktsim_trace::WriteError;
///
/// let e = WriteError::UnknownComponent { record: 4, component: 9, components: 3 };
/// assert_eq!(
///     e.to_string(),
///     "record 4 names component #9, but the header lists 3 components"
/// );
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// Writing to the output failed.
    #[error("writing the trace failed at byte {offset}: {source}")]
    Io {
        /// Bytes written before the failure.
        offset: u64,
        /// The error of the output.
        #[source]
        source: io::Error,
    },
    /// A value in the header does not fit the format.
    #[error("the trace header does not fit the format: {0}")]
    Header(#[source] EncodeError),
    /// The header names a different message format than the writer's
    /// message type.
    #[error("the header says messages are `{header}`, but this writer writes `{messages}`")]
    MessageFormat {
        /// The format in the header.
        header: String,
        /// The message type's format.
        messages: &'static str,
    },
    /// A record does not fit the format.
    #[error("record {record} does not fit the format: {source}")]
    Encode {
        /// The record's position in the trace, from 0.
        record: u64,
        /// What does not fit.
        #[source]
        source: EncodeError,
    },
    /// A record names a component that is not in the header.
    #[error(
        "record {record} names component #{component}, but the header lists {components} components"
    )]
    UnknownComponent {
        /// The record's position in the trace, from 0.
        record: u64,
        /// The component's number.
        component: usize,
        /// The components in the header.
        components: usize,
    },
}
