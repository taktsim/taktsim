//! The golden-trace harness (spec §7 Layer 2, ADR-0021), behind the
//! `golden` feature: for tests of reference models, in any crate.
//!
//! [`record`] runs a model into a trace, reads it back, replays it with a
//! freshly built engine, and returns a [`Golden`] whose
//! [`snapshot`](Golden::snapshot) is compared with a committed `insta`
//! snapshot. The snapshot holds the stream hash, so any change to any
//! record fails the test, and a text dump of the records, so the diff
//! shows what changed. It leaves out the engine version and the file size,
//! so a release does not change every snapshot; replay refuses a trace of
//! another engine version anyway.
//!
//! If the environment variable [`HASHES_ENV`] names a directory, `record`
//! also writes the case's hashes there, one file per case. CI does this on
//! every tier-1 target and fails if any target's hashes differ (A4).
//!
//! ```
//! use taktsim_core::{Component, ComponentError, Context, Delivery, Engine, EngineConfig, Message};
//! use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
//!
//! #[derive(Debug)]
//! struct Tick;
//! impl Message for Tick {}
//! impl TraceMessage for Tick {
//!     const FORMAT: &'static str = "tick/1";
//!     fn encode(&self, _: &mut Encoder<'_>) {}
//!     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
//! }
//! struct Clock;
//! impl Component<Tick> for Clock {
//!     fn start(&mut self, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
//!         ctx.schedule("1min".parse()?, Tick)?;
//!         Ok(())
//!     }
//!     fn handle(&mut self, _: Delivery<'_, Tick>, ctx: &mut Context<'_, Tick>) -> Result<(), ComponentError> {
//!         ctx.schedule("1min".parse()?, Tick)?;
//!         Ok(())
//!     }
//! }
//!
//! let case = golden::Case {
//!     name: "doc_clock",
//!     model_hash: ModelHash::sha256(b"clock"),
//!     until: "3min".parse()?,
//! };
//! let golden = golden::record(&case, || {
//!     let mut engine = Engine::new(EngineConfig::new(1));
//!     engine.add_component("clock", Box::new(Clock))?;
//!     Ok(engine)
//! })?;
//! assert!(golden.snapshot().ends_with("1min event #0 clock -> clock Tick\n2min event #1 clock -> clock Tick\n3min event #2 clock -> clock Tick\n"));
//! // In a test: insta::assert_snapshot!(case.name, golden.snapshot());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use taktsim_core::{Engine, EngineError, RunError, RunLimits};
use taktsim_units::SimTime;

use crate::codec::TraceMessage;
use crate::header::{Header, ModelHash, TraceHash};
use crate::read::{ReadError, TraceReader};
use crate::replay::{ReplayError, replay};
use crate::write::{Summary, TraceWriter, WriteError};

/// The environment variable that names the directory for case hashes.
///
/// ```
/// assert_eq!(taktsim_trace::golden::HASHES_ENV, "TAKTSIM_TRACE_HASHES");
/// ```
pub const HASHES_ENV: &str = "TAKTSIM_TRACE_HASHES";

/// Records shown at the start of a dump.
const HEAD: usize = 400;
/// Records shown at the end of a dump.
const TAIL: usize = 100;

/// A golden-trace case: a model, run from time zero to `until`.
///
/// ```
/// let case = taktsim_trace::golden::Case {
///     name: "mm1_8h",
///     model_hash: taktsim_trace::ModelHash::default(),
///     until: "8h".parse()?,
/// };
/// assert_eq!(case.name, "mm1_8h");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Case<'a> {
    /// The case's name: lowercase letters, digits and `_`, unique across
    /// the workspace (it names the hash file and the snapshot).
    pub name: &'a str,
    /// The model's hash, written into the trace header.
    pub model_hash: ModelHash,
    /// The horizon.
    pub until: SimTime,
}

/// The result of a golden case.
///
/// ```
/// # use taktsim_core::{Component, ComponentError, Context, Delivery, Engine, EngineConfig, Message};
/// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
/// # #[derive(Debug)] struct Tick;
/// # impl Message for Tick {}
/// # impl TraceMessage for Tick {
/// #     const FORMAT: &'static str = "tick/1";
/// #     fn encode(&self, _: &mut Encoder<'_>) {}
/// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
/// # }
/// let case = golden::Case { name: "doc_empty", model_hash: ModelHash::default(), until: "1h".parse()? };
/// let golden = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
/// assert_eq!(golden.summary().records, 0);
/// assert_eq!(golden.header().seed, 5);
/// assert_ne!(golden.file_hash(), golden.summary().hash);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone)]
pub struct Golden {
    name: String,
    header: Header,
    summary: Summary,
    file_hash: TraceHash,
    head: Vec<String>,
    tail: VecDeque<String>,
}

impl Golden {
    /// The trace's header.
    ///
    /// ```
    /// # use taktsim_core::{Engine, EngineConfig};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    /// let case = golden::Case { name: "doc_header", model_hash: ModelHash::sha256(b"m"), until: "1h".parse()? };
    /// let golden = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
    /// assert_eq!(golden.header().model_hash, ModelHash::sha256(b"m"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The trace's summary, with the stream hash.
    ///
    /// ```
    /// # use taktsim_core::{Engine, EngineConfig};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceHash, TraceMessage, golden};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    /// let case = golden::Case { name: "doc_summary", model_hash: ModelHash::default(), until: "1h".parse()? };
    /// let golden = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
    /// // No records: the hash of nothing.
    /// assert_eq!(golden.summary().hash, TraceHash::sha256(b""));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn summary(&self) -> Summary {
        self.summary
    }

    /// The SHA-256 of the whole trace file, header and framing included.
    /// Unlike the stream hash, it changes with the engine version.
    ///
    /// ```
    /// # use taktsim_core::{Engine, EngineConfig};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    /// let case = golden::Case { name: "doc_file_hash", model_hash: ModelHash::default(), until: "1h".parse()? };
    /// let a = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
    /// let b = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(6))))?;
    /// // The seed is in the header, not in the records.
    /// assert_eq!(a.summary().hash, b.summary().hash);
    /// assert_ne!(a.file_hash(), b.file_hash());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn file_hash(&self) -> TraceHash {
        self.file_hash
    }

    /// The text compared with the committed snapshot: provenance without
    /// the engine version, counts, the stream hash, then one line per
    /// record (the first 400 and the last 100 of a longer trace).
    ///
    /// ```
    /// # use taktsim_core::{Engine, EngineConfig};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    /// let case = golden::Case { name: "doc_snapshot", model_hash: ModelHash::default(), until: "1h".parse()? };
    /// let golden = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
    /// assert!(golden.snapshot().starts_with("case: doc_snapshot\nmessages: tick/1\n"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn snapshot(&self) -> String {
        let h = &self.header;
        let s = &self.summary;
        let mut text = String::new();
        // Writing to a String cannot fail.
        let _ = write!(
            text,
            "case: {}\nmessages: {}\nmodel hash: {}\nseed: {}\ncomponents: {}\n\
             records: {} ({} events) in {} chunks\nstream sha256: {}\n---\n",
            self.name,
            h.message_format,
            h.model_hash,
            h.seed,
            h.components.join(", "),
            s.records,
            s.events,
            s.chunks,
            s.hash,
        );
        for line in &self.head {
            text.push_str(line);
            text.push('\n');
        }
        let shown = (self.head.len() + self.tail.len()) as u64;
        if shown < s.records {
            let _ = writeln!(text, "... {} records not shown ...", s.records - shown);
        }
        for line in &self.tail {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// The line written to the hash directory: name, records, stream and
    /// file hash.
    ///
    /// ```
    /// # use taktsim_core::{Engine, EngineConfig};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    /// let case = golden::Case { name: "doc_line", model_hash: ModelHash::default(), until: "1h".parse()? };
    /// let golden = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
    /// assert!(golden.hash_line().starts_with("doc_line records=0 stream="));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn hash_line(&self) -> String {
        format!(
            "{} records={} stream={} file={}\n",
            self.name, self.summary.records, self.summary.hash, self.file_hash
        )
    }

    /// Writes [`hash_line`](Self::hash_line) to `<dir>/<name>.txt`,
    /// creating `dir` if needed. [`record`] does this when [`HASHES_ENV`]
    /// is set.
    ///
    /// # Errors
    ///
    /// [`GoldenError::Hashes`] if the file cannot be written.
    ///
    /// ```
    /// # use taktsim_core::{Engine, EngineConfig};
    /// # use taktsim_trace::{DecodeError, Decoder, Encoder, ModelHash, TraceMessage, golden};
    /// # #[derive(Debug)] struct Tick;
    /// # impl taktsim_core::Message for Tick {}
    /// # impl TraceMessage for Tick {
    /// #     const FORMAT: &'static str = "tick/1";
    /// #     fn encode(&self, _: &mut Encoder<'_>) {}
    /// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
    /// # }
    /// let case = golden::Case { name: "doc_write", model_hash: ModelHash::default(), until: "1h".parse()? };
    /// let golden = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(5))))?;
    /// let dir = std::env::temp_dir().join(format!("taktsim-doc-{}", std::process::id()));
    /// golden.write_hashes(&dir)?;
    /// assert_eq!(std::fs::read_to_string(dir.join("doc_write.txt"))?, golden.hash_line());
    /// std::fs::remove_dir_all(&dir)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn write_hashes(&self, dir: &Path) -> Result<(), GoldenError> {
        let path = dir.join(format!("{}.txt", self.name));
        std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(&path, self.hash_line()))
            .map_err(|source| GoldenError::Hashes { path, source })
    }
}

/// Runs `case`, checks that its trace reads back and replays, and, if
/// [`HASHES_ENV`] is set, writes its hashes there. `build` must build the
/// same engine every time it is called (it is called twice).
///
/// # Errors
///
/// A [`GoldenError`] if the case name is not allowed, building or running
/// the model fails, the trace cannot be written or read, the replay
/// diverges, or the hash file cannot be written.
///
/// ```
/// use taktsim_core::{Engine, EngineConfig};
/// use taktsim_trace::{golden, golden::GoldenError, ModelHash};
/// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
/// # #[derive(Debug)] struct Tick;
/// # impl taktsim_core::Message for Tick {}
/// # impl TraceMessage for Tick {
/// #     const FORMAT: &'static str = "tick/1";
/// #     fn encode(&self, _: &mut Encoder<'_>) {}
/// #     fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick) }
/// # }
///
/// let case = golden::Case { name: "Not/Allowed", model_hash: ModelHash::default(), until: "1h".parse()? };
/// let result = golden::record(&case, || Ok(Engine::<Tick>::new(EngineConfig::new(1))));
/// assert!(matches!(result, Err(GoldenError::Name { .. })));
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
pub fn record<M: TraceMessage>(
    case: &Case<'_>,
    build: impl Fn() -> Result<Engine<M>, EngineError>,
) -> Result<Golden, GoldenError> {
    let valid = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_';
    if case.name.is_empty() || !case.name.chars().all(valid) {
        return Err(GoldenError::Name {
            name: case.name.to_owned(),
        });
    }
    let mut engine = build()?;
    let header = Header::for_engine(&engine, case.model_hash);
    let mut writer = TraceWriter::new(Vec::new(), &header)?;
    engine
        .run(RunLimits::until(case.until), &mut writer)
        .map_err(Box::new)?;
    let (bytes, summary) = writer.finish()?;
    let mut reader = TraceReader::<_, M>::new(bytes.as_slice())?;
    let mut head = Vec::new();
    let mut tail = VecDeque::new();
    while let Some(record) = reader.next_record()? {
        let line = record.display(&header.components).to_string();
        if head.len() < HEAD {
            head.push(line);
        } else {
            if tail.len() == TAIL {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    }
    replay(TraceReader::new(bytes.as_slice())?, &mut build()?)?;
    let golden = Golden {
        name: case.name.to_owned(),
        header,
        summary,
        file_hash: TraceHash::sha256(&bytes),
        head,
        tail,
    };
    if let Some(dir) = std::env::var_os(HASHES_ENV) {
        golden.write_hashes(Path::new(&dir))?;
    }
    Ok(golden)
}

/// Why a golden case failed.
///
/// ```
/// let e = taktsim_trace::golden::GoldenError::Name { name: "A b".into() };
/// assert_eq!(e.to_string(), "golden case name `A b` is not allowed: use lowercase letters, digits and `_`, e.g. `mm1_8h`");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GoldenError {
    /// The case name is not allowed.
    #[error(
        "golden case name `{name}` is not allowed: use lowercase letters, digits and `_`, e.g. `mm1_8h`"
    )]
    Name {
        /// The name.
        name: String,
    },
    /// Building the model failed.
    #[error("building the model failed: {0}")]
    Build(#[from] EngineError),
    /// The run failed.
    #[error("the run failed: {0}")]
    Run(#[from] Box<RunError>),
    /// Writing the trace failed.
    #[error(transparent)]
    Write(#[from] WriteError),
    /// Reading the trace back failed.
    #[error(transparent)]
    Read(#[from] ReadError),
    /// The replay failed.
    #[error(transparent)]
    Replay(#[from] ReplayError),
    /// The hash file could not be written.
    #[error("writing {} failed: {source}", path.display())]
    Hashes {
        /// The file.
        path: PathBuf,
        /// The error.
        #[source]
        source: io::Error,
    },
}
