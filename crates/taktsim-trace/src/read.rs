//! The streaming trace reader (ADR-0004, ADR-0021).

use std::fmt;
use std::io::{self, Read};
use std::marker::PhantomData;
use std::ops::Range;

use taktsim_core::digest::{Crc32, Sha256, crc32};

use crate::codec::{DecodeError, TraceMessage};
use crate::header::{
    CHUNK_CAPACITY, CHUNK_HEADER, CHUNK_TAG, END_SIZE, END_TAG, FORMAT_VERSION, Header, MAGIC,
    ModelHash, TraceHash,
};
use crate::record::{self, EVENT, Record};
use crate::write::Summary;

/// Reads a trace record by record, holding one chunk in memory.
///
/// Every chunk's CRC32 is checked before its records are returned, and the
/// end marker's counts and stream hash are checked against what was read,
/// so a damaged or cut-off trace is reported with a typed [`ReadError`],
/// never read as a shorter or different trace. Records before the damage
/// are returned first: a trace whose writer crashed is readable up to its
/// last complete chunk (ADR-0004).
///
/// As an [`Iterator`], it yields `Ok` records, then at most one `Err`, then
/// ends.
///
/// ```
/// use taktsim_core::{ComponentId, ItemId, TraceRecord, TraceSink};
/// use taktsim_trace::{Header, ModelHash, Record, TraceReader, TraceWriter};
/// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
/// # #[derive(Debug, PartialEq)] struct Tick;
/// # impl taktsim_core::Message for Tick {}
/// # impl TraceMessage for Tick {
/// #     const FORMAT: &'static str = "tick/1";
/// #     fn encode(&self, _: &mut Encoder<'_>) {}
/// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
/// # }
///
/// let header = Header::new::<Tick>(ModelHash::default(), 1, vec!["source".into()]);
/// let mut writer = TraceWriter::new(Vec::new(), &header)?;
/// let source = ComponentId::from_index(0);
/// writer.record(&TraceRecord::<Tick>::ItemCreated { time: "1s".parse()?, item: ItemId::from_raw(0), by: source });
/// let (bytes, written) = writer.finish()?;
///
/// let mut reader = TraceReader::<_, Tick>::new(bytes.as_slice())?;
/// assert_eq!(reader.header(), &header);
/// let records: Vec<Record<Tick>> = reader.by_ref().collect::<Result<_, _>>()?;
/// assert_eq!(records.len(), 1);
/// assert_eq!(reader.summary(), Some(written));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct TraceReader<R: Read, M> {
    source: R,
    header: Header,
    /// The records of the current chunk.
    chunk: Vec<u8>,
    /// Where the next record starts in `chunk`.
    pos: usize,
    /// Records of the current chunk not read yet.
    chunk_left: u32,
    /// The next record, read but not returned yet.
    peeked: Option<Raw>,
    records: u64,
    events: u64,
    chunks: u64,
    /// Bytes read from `source`.
    offset: u64,
    hasher: Sha256,
    state: State,
    _message: PhantomData<fn() -> M>,
}

/// A record located in the current chunk.
#[derive(Debug, Clone)]
pub(crate) struct Raw {
    pub(crate) index: u64,
    pub(crate) kind: u8,
    range: Range<usize>,
}

#[derive(Debug, Clone, Copy)]
enum State {
    Records,
    End(Summary),
    Failed,
}

impl<R: Read, M: TraceMessage> TraceReader<R, M> {
    /// Reads and checks the header.
    ///
    /// # Errors
    ///
    /// [`ReadError::NotATrace`], [`ReadError::UnsupportedVersion`],
    /// [`ReadError::MessageFormat`] if the trace's messages are not `M`'s,
    /// [`ReadError::Checksum`], [`ReadError::Invalid`],
    /// [`ReadError::Truncated`] or [`ReadError::Io`].
    ///
    /// ```
    /// use taktsim_trace::{ReadError, TraceReader};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    ///
    /// let result = TraceReader::<_, Tick>::new(&b"time,entity\n"[..]);
    /// assert!(matches!(result, Err(ReadError::NotATrace { .. })));
    /// ```
    pub fn new(source: R) -> Result<Self, ReadError> {
        let mut reader = Self {
            source,
            header: Header {
                engine_version: String::new(),
                message_format: String::new(),
                model_hash: ModelHash::default(),
                seed: 0,
                components: Vec::new(),
            },
            chunk: Vec::new(),
            pos: 0,
            chunk_left: 0,
            peeked: None,
            records: 0,
            events: 0,
            chunks: 0,
            offset: 0,
            hasher: Sha256::new(),
            state: State::Records,
            _message: PhantomData,
        };
        reader.header = reader.read_header()?;
        if reader.header.message_format != M::FORMAT {
            return Err(ReadError::MessageFormat {
                found: reader.header.message_format,
                expected: M::FORMAT,
            });
        }
        reader.chunk.reserve(CHUNK_CAPACITY);
        Ok(reader)
    }

    /// The trace's header.
    ///
    /// ```
    /// use taktsim_trace::{Header, ModelHash, TraceReader, TraceWriter};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    ///
    /// let header = Header::new::<Tick>(ModelHash::sha256(b"m"), 42, vec![]);
    /// let (bytes, _) = TraceWriter::<_, Tick>::new(Vec::new(), &header)?.finish()?;
    /// assert_eq!(TraceReader::<_, Tick>::new(bytes.as_slice())?.header().seed, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The trace's summary, once the end marker has been read and checked.
    ///
    /// ```
    /// use taktsim_trace::{Header, ModelHash, TraceReader, TraceWriter};
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
    /// let (bytes, written) = TraceWriter::<_, Tick>::new(Vec::new(), &header)?.finish()?;
    /// let mut reader = TraceReader::<_, Tick>::new(bytes.as_slice())?;
    /// assert_eq!(reader.summary(), None);
    /// assert!(reader.next_record()?.is_none());
    /// assert_eq!(reader.summary(), Some(written));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn summary(&self) -> Option<Summary> {
        match self.state {
            State::End(summary) => Some(summary),
            State::Records | State::Failed => None,
        }
    }

    /// The next record, or `None` after the last one, once the end marker
    /// has been checked.
    ///
    /// # Errors
    ///
    /// A [`ReadError`] if the trace is damaged or cut off, or the source
    /// fails. After an error, every call returns
    /// [`ReadError::AlreadyFailed`].
    ///
    /// ```
    /// use taktsim_trace::{Header, ModelHash, ReadError, TraceReader, TraceWriter};
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
    /// let (bytes, _) = TraceWriter::<_, Tick>::new(Vec::new(), &header)?.finish()?;
    /// // Cut off the end marker.
    /// let cut = &bytes[..bytes.len() - 10];
    /// let mut reader = TraceReader::<_, Tick>::new(cut)?;
    /// assert!(matches!(reader.next_record(), Err(ReadError::Truncated { .. })));
    /// assert!(matches!(reader.next_record(), Err(ReadError::AlreadyFailed)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn next_record(&mut self) -> Result<Option<Record<M>>, ReadError> {
        let Some(raw) = self.peek_raw()? else {
            return Ok(None);
        };
        let index = raw.index;
        let result = record::decode(&self.chunk[raw.range.clone()], self.header.components.len());
        self.peeked = None;
        result.map(Some).map_err(|source| {
            self.state = State::Failed;
            ReadError::Record {
                record: index,
                source,
            }
        })
    }

    /// The next record's position and bytes, without consuming it.
    pub(crate) fn peek_raw(&mut self) -> Result<Option<Raw>, ReadError> {
        if self.peeked.is_none() {
            self.peeked = match self.state {
                State::Failed => return Err(ReadError::AlreadyFailed),
                State::End(_) => return Ok(None),
                State::Records => self.advance().inspect_err(|_| self.state = State::Failed)?,
            };
        }
        Ok(self.peeked.clone())
    }

    /// The bytes of a record returned by [`peek_raw`](Self::peek_raw).
    pub(crate) fn bytes(&self, raw: &Raw) -> &[u8] {
        &self.chunk[raw.range.clone()]
    }

    /// Consumes the record returned by [`peek_raw`](Self::peek_raw).
    pub(crate) fn consume(&mut self) {
        self.peeked = None;
    }

    /// Decodes record bytes of this trace, e.g. for a report.
    pub(crate) fn decode(&self, bytes: &[u8]) -> Result<Record<M>, DecodeError> {
        record::decode(bytes, self.header.components.len())
    }

    /// Finds the next record, reading the next chunk or the end marker
    /// when the current chunk is used up.
    fn advance(&mut self) -> Result<Option<Raw>, ReadError> {
        if self.pos == self.chunk.len() && !self.next_block()? {
            return Ok(None);
        }
        let index = self.records;
        let (kind, len) =
            record::frame(&self.chunk[self.pos..]).map_err(|source| ReadError::Record {
                record: index,
                source,
            })?;
        let range = self.pos..self.pos + len;
        self.pos += len;
        self.chunk_left -= 1;
        let ends = self.pos == self.chunk.len();
        if ends != (self.chunk_left == 0) {
            return Err(ReadError::Invalid {
                offset: self.offset,
                what: "chunk",
                found: if ends {
                    format!("{} records fewer than its header says", self.chunk_left)
                } else {
                    "more records than its header says".to_owned()
                },
                allowed: "exactly the records its header counts".to_owned(),
            });
        }
        self.records += 1;
        if kind == EVENT {
            self.events += 1;
        }
        Ok(Some(Raw { index, kind, range }))
    }

    /// Reads the next chunk (`true`) or the end marker (`false`).
    fn next_block(&mut self) -> Result<bool, ReadError> {
        let start = self.offset;
        let mut tag = [0];
        self.read_exact(&mut tag, start)?;
        match tag[0] {
            CHUNK_TAG => self.read_chunk(start).map(|()| true),
            END_TAG => self.read_end(start).map(|()| false),
            other => Err(ReadError::Invalid {
                offset: start,
                what: "block tag",
                found: format!("{other:#04x}"),
                allowed: format!("{CHUNK_TAG:#04x} (chunk) or {END_TAG:#04x} (end marker)"),
            }),
        }
    }

    fn read_chunk(&mut self, start: u64) -> Result<(), ReadError> {
        let mut head = [0; 1 + CHUNK_HEADER];
        head[0] = CHUNK_TAG;
        self.read_exact(&mut head[1..], start)?;
        let len = le_u32(&head[1..5]);
        let records = le_u32(&head[5..9]);
        let first = le_u64(&head[9..17]);
        let stored = le_u32(&head[17..21]);
        let len_usize = usize::try_from(len).unwrap_or(usize::MAX);
        if len_usize == 0 || len_usize > CHUNK_CAPACITY {
            return Err(ReadError::Invalid {
                offset: start + 1,
                what: "chunk length",
                found: len.to_string(),
                allowed: format!("1 to {CHUNK_CAPACITY} bytes"),
            });
        }
        let mut chunk = std::mem::take(&mut self.chunk);
        chunk.clear();
        chunk.resize(len_usize, 0);
        let read = self.read_exact(&mut chunk, start);
        self.chunk = chunk;
        read?;
        let mut crc = Crc32::new();
        crc.update(&head[..17]);
        crc.update(&self.chunk);
        let computed = crc.finalize();
        if computed != stored {
            self.chunk.clear();
            return Err(ReadError::Checksum {
                block: Block::Chunk(self.chunks),
                offset: start,
                stored,
                computed,
            });
        }
        if first != self.records || records == 0 {
            self.chunk.clear();
            return Err(ReadError::OutOfSequence {
                chunk: self.chunks,
                offset: start,
                expected: self.records,
                found: first,
                records,
            });
        }
        self.hasher.update(&self.chunk);
        self.chunks += 1;
        self.pos = 0;
        self.chunk_left = records;
        Ok(())
    }

    fn read_end(&mut self, start: u64) -> Result<(), ReadError> {
        let mut end = [0; 1 + END_SIZE];
        end[0] = END_TAG;
        self.read_exact(&mut end[1..], start)?;
        let stored = le_u32(&end[57..61]);
        let computed = crc32(&end[..57]);
        if computed != stored {
            return Err(ReadError::Checksum {
                block: Block::End,
                offset: start,
                stored,
                computed,
            });
        }
        let mut hash = [0; 32];
        hash.copy_from_slice(&end[25..57]);
        let hash = TraceHash::from_bytes(hash);
        let counted = TraceHash::from_bytes(self.hasher.clone().finalize());
        let checks = [
            ("records", le_u64(&end[1..9]), self.records),
            ("events", le_u64(&end[9..17]), self.events),
            ("chunks", le_u64(&end[17..25]), self.chunks),
        ];
        if let Some((what, stated, counted)) = checks.into_iter().find(|(_, s, c)| s != c) {
            return Err(ReadError::EndMismatch {
                what,
                stated: stated.to_string(),
                counted: counted.to_string(),
            });
        }
        if hash != counted {
            return Err(ReadError::EndMismatch {
                what: "stream hash",
                stated: hash.to_string(),
                counted: counted.to_string(),
            });
        }
        let after = self.offset;
        let mut extra = [0];
        loop {
            match self.source.read(&mut extra) {
                Ok(0) => break,
                Ok(_) => return Err(ReadError::TrailingData { offset: after }),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => {
                    return Err(ReadError::Io {
                        offset: after,
                        source,
                    });
                }
            }
        }
        self.state = State::End(Summary {
            records: self.records,
            events: self.events,
            chunks: self.chunks,
            bytes: self.offset,
            hash,
        });
        Ok(())
    }

    fn read_header(&mut self) -> Result<Header, ReadError> {
        let mut bytes = Vec::new();
        let mut magic = [0; 8];
        let got = self.read_up_to(&mut magic)?;
        if magic[..got] != MAGIC[..got] {
            return Err(ReadError::NotATrace {
                found: magic[..got].to_vec(),
            });
        }
        if got < magic.len() {
            return Err(ReadError::Truncated {
                offset: 0,
                records: 0,
            });
        }
        bytes.extend_from_slice(&magic);
        let version = u16::from_le_bytes(self.header_bytes::<2>(&mut bytes)?);
        if version != FORMAT_VERSION {
            return Err(ReadError::UnsupportedVersion {
                found: version,
                supported: FORMAT_VERSION,
            });
        }
        let engine_version = self.header_text(&mut bytes)?;
        let message_format = self.header_text(&mut bytes)?;
        let model_hash = self.header_bytes::<32>(&mut bytes)?;
        let seed = u64::from_le_bytes(self.header_bytes::<8>(&mut bytes)?);
        let count = u32::from_le_bytes(self.header_bytes::<4>(&mut bytes)?);
        // Not `with_capacity(count)`: the count is not checked yet.
        let mut components = Vec::new();
        for _ in 0..count {
            components.push(self.header_text(&mut bytes)?);
        }
        let stored = u32::from_le_bytes(self.header_bytes::<4>(&mut Vec::new())?);
        let computed = crc32(&bytes);
        if computed != stored {
            return Err(ReadError::Checksum {
                block: Block::Header,
                offset: 0,
                stored,
                computed,
            });
        }
        let text = |range: Range<usize>, what: &'static str| {
            String::from_utf8(bytes[range.clone()].to_vec()).map_err(|_| ReadError::Invalid {
                offset: range.start as u64,
                what,
                found: "bytes that are not UTF-8".to_owned(),
                allowed: "UTF-8 text".to_owned(),
            })
        };
        Ok(Header {
            engine_version: text(engine_version, "engine version")?,
            message_format: text(message_format, "message format")?,
            model_hash: ModelHash::from_bytes(model_hash),
            seed,
            components: components
                .into_iter()
                .map(|range| text(range, "component id"))
                .collect::<Result<_, _>>()?,
        })
    }

    /// Reads `N` header bytes, appending them to `bytes` for the CRC.
    fn header_bytes<const N: usize>(&mut self, bytes: &mut Vec<u8>) -> Result<[u8; N], ReadError> {
        let mut array = [0; N];
        self.read_exact(&mut array, 0)?;
        bytes.extend_from_slice(&array);
        Ok(array)
    }

    /// Reads a header text (u16 length, bytes); returns where it is in
    /// `bytes`.
    fn header_text(&mut self, bytes: &mut Vec<u8>) -> Result<Range<usize>, ReadError> {
        let len = usize::from(u16::from_le_bytes(self.header_bytes::<2>(bytes)?));
        let start = bytes.len();
        bytes.resize(start + len, 0);
        self.read_exact(&mut bytes[start..], 0)
            .inspect_err(|_| bytes.truncate(start))?;
        Ok(start..start + len)
    }

    /// Fills `buf`; a short read means the trace is cut off inside the
    /// block that starts at `block`.
    fn read_exact(&mut self, buf: &mut [u8], block: u64) -> Result<(), ReadError> {
        match self.source.read_exact(buf) {
            Ok(()) => {
                self.offset += buf.len() as u64;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(ReadError::Truncated {
                offset: block,
                records: self.records,
            }),
            Err(source) => Err(ReadError::Io {
                offset: self.offset,
                source,
            }),
        }
    }

    /// Reads as many bytes as are there, up to `buf.len()`.
    fn read_up_to(&mut self, buf: &mut [u8]) -> Result<usize, ReadError> {
        let mut got = 0;
        while got < buf.len() {
            match self.source.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => {
                    return Err(ReadError::Io {
                        offset: got as u64,
                        source,
                    });
                }
            }
        }
        self.offset += got as u64;
        Ok(got)
    }
}

impl<R: Read, M: TraceMessage> Iterator for TraceReader<R, M> {
    type Item = Result<Record<M>, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_record() {
            Ok(record) => record.map(Ok),
            Err(ReadError::AlreadyFailed) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

fn le_u32(bytes: &[u8]) -> u32 {
    let mut array = [0; 4];
    array.copy_from_slice(bytes);
    u32::from_le_bytes(array)
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut array = [0; 8];
    array.copy_from_slice(bytes);
    u64::from_le_bytes(array)
}

/// A part of a trace protected by its own CRC32.
///
/// ```
/// use taktsim_trace::Block;
///
/// assert_eq!(Block::Chunk(3).to_string(), "chunk 3");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// The header.
    Header,
    /// The chunk with this number, from 0.
    Chunk(u64),
    /// The end marker.
    End,
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header => f.write_str("the header"),
            Self::Chunk(n) => write!(f, "chunk {n}"),
            Self::End => f.write_str("the end marker"),
        }
    }
}

/// Why a trace could not be read. Offsets are in bytes from the start of
/// the trace.
///
/// ```
/// use taktsim_trace::ReadError;
///
/// let e = ReadError::UnsupportedVersion { found: 2, supported: 1 };
/// assert_eq!(
///     e.to_string(),
///     "trace format version 2 is not supported: this build reads version 1 only; \
///      read the trace with the taktsim release that wrote it"
/// );
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    /// Reading from the source failed.
    #[error("reading the trace failed at byte {offset}: {source}")]
    Io {
        /// Bytes read before the failure.
        offset: u64,
        /// The error of the source.
        #[source]
        source: io::Error,
    },
    /// The data does not start like a trace.
    #[error(
        "not a taktsim trace: it starts with {found:02x?}, a trace starts with {MAGIC:02x?} (\"TAKT\\r\\n\\x1a\\n\")"
    )]
    NotATrace {
        /// Up to the first 8 bytes.
        found: Vec<u8>,
    },
    /// The trace was written in another format version.
    #[error(
        "trace format version {found} is not supported: this build reads version {supported} only; \
         read the trace with the taktsim release that wrote it"
    )]
    UnsupportedVersion {
        /// The version in the trace.
        found: u16,
        /// The version this build reads.
        supported: u16,
    },
    /// The trace's messages are of another type.
    #[error(
        "the trace's messages are encoded as `{found}`, but they are read as `{expected}`; \
         read it with the message type of the component package that wrote it"
    )]
    MessageFormat {
        /// The format in the header.
        found: String,
        /// The reader's message format.
        expected: &'static str,
    },
    /// The trace ends before its end marker: the writer did not finish,
    /// e.g. after a crash or with a full disk, or the file was cut.
    #[error(
        "the trace is cut off in the block at byte {offset}, after {records} complete records, \
         and has no end marker; the records before it are intact"
    )]
    Truncated {
        /// Where the incomplete block starts.
        offset: u64,
        /// Complete records before it.
        records: u64,
    },
    /// A block's bytes do not match its CRC32: the trace is damaged.
    #[error(
        "{block} at byte {offset} is damaged: its CRC32 is {stored:#010x}, its bytes give {computed:#010x}"
    )]
    Checksum {
        /// The damaged block.
        block: Block,
        /// Where it starts.
        offset: u64,
        /// The CRC32 stored in the trace.
        stored: u32,
        /// The CRC32 of the bytes read.
        computed: u32,
    },
    /// A value that the format does not allow.
    #[error("invalid {what} at byte {offset}: found {found}, allowed {allowed}")]
    Invalid {
        /// Where it is.
        offset: u64,
        /// What was read.
        what: &'static str,
        /// What was found.
        found: String,
        /// What is allowed.
        allowed: String,
    },
    /// A chunk is not the next one: a chunk is missing, repeated or empty.
    #[error(
        "chunk {chunk} at byte {offset} is out of sequence: it starts at record {found} with {records} records, \
         the next record is {expected}"
    )]
    OutOfSequence {
        /// The chunk's number, from 0.
        chunk: u64,
        /// Where it starts.
        offset: u64,
        /// The next record's position.
        expected: u64,
        /// The position of the chunk's first record.
        found: u64,
        /// The records the chunk says it holds.
        records: u32,
    },
    /// The end marker does not match what was read.
    #[error("the end marker states {stated} {what}, the trace has {counted}")]
    EndMismatch {
        /// What differs.
        what: &'static str,
        /// What the end marker states.
        stated: String,
        /// What was read.
        counted: String,
    },
    /// Bytes follow the end marker.
    #[error("unexpected data after the end marker at byte {offset}")]
    TrailingData {
        /// Where the data starts.
        offset: u64,
    },
    /// A record in an intact chunk cannot be decoded: written by a faulty
    /// writer or message encoding.
    #[error("record {record} is invalid: {source}")]
    Record {
        /// The record's position in the trace, from 0.
        record: u64,
        /// What is wrong.
        #[source]
        source: DecodeError,
    },
    /// The reader already returned an error.
    #[error("the trace reader already failed")]
    AlreadyFailed,
}
