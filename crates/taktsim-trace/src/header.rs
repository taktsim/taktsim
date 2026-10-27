//! The trace header, hashes and the file layout constants (ADR-0021).

use std::fmt;

use taktsim_core::digest::{crc32, sha256};
use taktsim_core::{ComponentId, ENGINE_VERSION, Engine};

use crate::codec::{EncodeError, Encoder, TraceMessage};

/// The first bytes of every trace: `TAKT`, then CR LF, SUB, LF, so a
/// conversion of line endings or a text-mode transfer is detected (as in
/// PNG).
///
/// ```
/// assert_eq!(&taktsim_trace::MAGIC[..4], b"TAKT");
/// ```
pub const MAGIC: [u8; 8] = *b"TAKT\r\n\x1a\n";

/// The trace format version this build writes and reads. A reader refuses
/// every other version (ADR-0021).
///
/// ```
/// assert_eq!(taktsim_trace::FORMAT_VERSION, 1);
/// ```
pub const FORMAT_VERSION: u16 = 1;

/// The most record bytes in one chunk: 64 KiB (ADR-0004). A writer holds at
/// most one chunk in memory, and a reader too.
///
/// ```
/// assert_eq!(taktsim_trace::CHUNK_CAPACITY, 65_536);
/// ```
pub const CHUNK_CAPACITY: usize = 64 * 1024;

/// Starts a chunk.
pub(crate) const CHUNK_TAG: u8 = b'C';
/// Starts the end marker.
pub(crate) const END_TAG: u8 = b'E';
/// A chunk's header after its tag: payload length u32, records u32, index
/// of the first record u64, CRC32 u32.
pub(crate) const CHUNK_HEADER: usize = 4 + 4 + 8 + 4;
/// The end marker after its tag: records u64, events u64, chunks u64, the
/// stream hash, CRC32 u32.
pub(crate) const END_SIZE: usize = 8 + 8 + 8 + 32 + 4;

/// A SHA-256 digest, written as 64 lowercase hex digits.
fn hex(bytes: &[u8; 32], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    bytes.iter().try_for_each(|b| write!(f, "{b:02x}"))
}

/// The hash of the model a trace was produced from (ADR-0002: SHA-256 of
/// the canonical model).
///
/// ```
/// use taktsim_trace::ModelHash;
///
/// let hash = ModelHash::sha256(b"abc");
/// assert_eq!(hash.to_string(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
/// assert_eq!(ModelHash::from_bytes(*hash.as_bytes()), hash);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ModelHash([u8; 32]);

impl ModelHash {
    /// A hash computed elsewhere, e.g. by the model loader.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::ModelHash::from_bytes([0; 32]), Default::default());
    /// ```
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The SHA-256 of `data`, e.g. a canonical model text.
    ///
    /// ```
    /// let a = taktsim_trace::ModelHash::sha256(b"line: 1");
    /// assert_ne!(a, taktsim_trace::ModelHash::sha256(b"line: 2"));
    /// ```
    #[must_use]
    pub fn sha256(data: &[u8]) -> Self {
        Self(sha256(data))
    }

    /// The 32 bytes.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::ModelHash::default().as_bytes(), &[0; 32]);
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ModelHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex(&self.0, f)
    }
}

/// The SHA-256 of a trace's record stream: every record's bytes, in order,
/// without the header and the chunk framing (ADR-0021). Two runs with the
/// same hash produced the same records; it does not depend on the engine
/// version written in the header or on chunk boundaries.
///
/// ```
/// use taktsim_trace::TraceHash;
///
/// let empty = TraceHash::sha256(b"");
/// assert!(empty.to_string().starts_with("e3b0c442"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TraceHash([u8; 32]);

impl TraceHash {
    /// A hash read from somewhere else.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::TraceHash::from_bytes([0; 32]), Default::default());
    /// ```
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The SHA-256 of `data`, e.g. of a whole trace file.
    ///
    /// ```
    /// let h = taktsim_trace::TraceHash::sha256(b"abc");
    /// assert!(h.to_string().starts_with("ba7816bf"));
    /// ```
    #[must_use]
    pub fn sha256(data: &[u8]) -> Self {
        Self(sha256(data))
    }

    /// The 32 bytes.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::TraceHash::default().as_bytes(), &[0; 32]);
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for TraceHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex(&self.0, f)
    }
}

/// What a trace was produced from: its provenance (spec §7 Layer 3). Written
/// once, at the start of the trace.
///
/// ```
/// use taktsim_core::{Component, ComponentError, Context, Delivery, Engine, EngineConfig, Message};
/// use taktsim_trace::{DecodeError, Decoder, Encoder, Header, ModelHash, TraceMessage};
///
/// #[derive(Debug)]
/// struct Tick;
/// impl Message for Tick {}
/// impl TraceMessage for Tick {
///     const FORMAT: &'static str = "tick/1";
///     fn encode(&self, _: &mut Encoder<'_>) {}
///     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
/// }
/// struct Idle;
/// impl Component<Tick> for Idle {
///     fn handle(&mut self, _: Delivery<'_, Tick>, _: &mut Context<'_, Tick>) -> Result<(), ComponentError> { Ok(()) }
/// }
///
/// let mut engine = Engine::new(EngineConfig::new(42));
/// engine.add_component("m1", Box::new(Idle))?;
/// let header = Header::for_engine(&engine, ModelHash::sha256(b"model"));
/// assert_eq!(header.seed, 42);
/// assert_eq!(header.components, ["m1"]);
/// assert_eq!(header.message_format, "tick/1");
/// assert_eq!(header.engine_version, taktsim_core::ENGINE_VERSION);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The engine version that wrote the trace
    /// ([`ENGINE_VERSION`]).
    pub engine_version: String,
    /// How messages are encoded ([`TraceMessage::FORMAT`]).
    pub message_format: String,
    /// The model the run came from.
    pub model_hash: ModelHash,
    /// The run's seed.
    pub seed: u64,
    /// The components' model ids, in the order they were added: records
    /// refer to components by position in this list.
    pub components: Vec<String>,
}

impl Header {
    /// A header for messages of type `M`, written by this engine version.
    ///
    /// ```
    /// use taktsim_trace::{Header, ModelHash};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    ///
    /// let header = Header::new::<Tick>(ModelHash::default(), 7, vec!["m1".into()]);
    /// assert_eq!(header.message_format, "tick/1");
    /// ```
    #[must_use]
    pub fn new<M: TraceMessage>(model_hash: ModelHash, seed: u64, components: Vec<String>) -> Self {
        Self {
            engine_version: ENGINE_VERSION.to_owned(),
            message_format: M::FORMAT.to_owned(),
            model_hash,
            seed,
            components,
        }
    }

    /// The header for a run of `engine`: its seed and components. Build it
    /// after adding the components.
    ///
    /// ```
    /// use taktsim_core::{Engine, EngineConfig};
    /// use taktsim_trace::{Header, ModelHash};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    ///
    /// let engine: Engine<Tick> = Engine::new(EngineConfig::new(9));
    /// let header = Header::for_engine(&engine, ModelHash::default());
    /// assert_eq!((header.seed, header.components.len()), (9, 0));
    /// ```
    #[must_use]
    pub fn for_engine<M: TraceMessage>(engine: &Engine<M>, model_hash: ModelHash) -> Self {
        Self::new::<M>(model_hash, engine.config().seed, component_names(engine))
    }

    /// The header's bytes, CRC included.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut bytes = Vec::new();
        let mut out = Encoder::new(&mut bytes);
        for b in MAGIC {
            out.u8(b);
        }
        out.u16(FORMAT_VERSION);
        out.str(&self.engine_version);
        out.str(&self.message_format);
        for b in self.model_hash.as_bytes() {
            out.u8(*b);
        }
        out.u64(self.seed);
        // The count is a component number too: at most u32::MAX.
        out.component(ComponentId::from_index(self.components.len()));
        for name in &self.components {
            out.str(name);
        }
        out.finish()?;
        let crc = crc32(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        Ok(bytes)
    }
}

/// The model ids of `engine`'s components, in order.
pub(crate) fn component_names<M: TraceMessage>(engine: &Engine<M>) -> Vec<String> {
    (0..)
        .map_while(|index| engine.component_name(ComponentId::from_index(index)))
        .map(str::to_owned)
        .collect()
}
