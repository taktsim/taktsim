//! Run ids and experiment ids (ADR-0024 §6): opaque, content-addressed names.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};
use taktsim_model::jcs::{CanonicalError, canonicalize};
use taktsim_model::{Decode, Decoder, Encode, Json, ModelHash, Node, Schemas, Value};

/// The hexadecimal SHA-256 of `canonical`.
fn sha256_hex(canonical: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(canonical.as_bytes());
    let mut text = String::with_capacity(64);
    for byte in digest {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Whether `text` is 64 lowercase hex digits.
fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// What a run id names: everything that decides the records of a run. The
/// horizon is inside the model hash (ADR-0022 §2).
///
/// ```
/// use taktsim_model::ModelHash;
/// use taktsim_run::{RunId, RunIdInputs};
///
/// let inputs = RunIdInputs {
///     engine: "taktsim-core",
///     engine_version: "0.1.0",
///     record_format: "taktsim-core/2",
///     package: "taktsim-components",
///     package_version: "0.1.0",
///     message_format: "taktsim-components/1",
///     model_hash: ModelHash::of(b"model"),
///     seed: Some(42),
/// };
/// assert!(RunId::of(&inputs)?.as_str().starts_with("taktsim-core-"));
/// # Ok::<(), taktsim_model::jcs::CanonicalError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunIdInputs<'a> {
    /// The engine's name.
    pub engine: &'a str,
    /// The engine's version.
    pub engine_version: &'a str,
    /// The record format of its traces.
    pub record_format: &'a str,
    /// The domain package's name.
    pub package: &'a str,
    /// The domain package's version.
    pub package_version: &'a str,
    /// The domain package's message format.
    pub message_format: &'a str,
    /// The model hash.
    pub model_hash: ModelHash,
    /// The seed that ran; `None` for a run with no seed.
    pub seed: Option<u64>,
}

/// The name of one finished run in a store: `<engine>-` and the SHA-256 of what
/// decides the run. Compare it whole and do not parse it.
///
/// ```
/// use taktsim_run::RunId;
///
/// let id: RunId = format!("taktsim-core-{}", "a".repeat(64)).parse()?;
/// assert_eq!(id.to_string().len(), "taktsim-core-".len() + 64);
/// assert!("../../etc".parse::<RunId>().is_err());
/// # Ok::<(), taktsim_run::ParseIdError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(String);

impl RunId {
    /// The id of a run with these inputs.
    ///
    /// # Errors
    ///
    /// A [`CanonicalError`] for a seed beyond 2^53 − 1, which has no canonical
    /// form. `taktsim_run::run` refuses such a seed first.
    ///
    /// ```
    /// use taktsim_model::ModelHash;
    /// use taktsim_run::{RunId, RunIdInputs};
    ///
    /// let inputs = RunIdInputs {
    ///     engine: "taktsim-core",
    ///     engine_version: "0.1.0",
    ///     record_format: "taktsim-core/2",
    ///     package: "taktsim-components",
    ///     package_version: "0.1.0",
    ///     message_format: "taktsim-components/1",
    ///     model_hash: ModelHash::of(b"model"),
    ///     seed: None,
    /// };
    /// let other = RunIdInputs { seed: Some(0), ..inputs };
    /// assert_ne!(RunId::of(&inputs)?, RunId::of(&other)?);
    /// # Ok::<(), taktsim_model::jcs::CanonicalError>(())
    /// ```
    pub fn of(inputs: &RunIdInputs<'_>) -> Result<Self, CanonicalError> {
        let RunIdInputs {
            engine,
            engine_version,
            record_format,
            package,
            package_version,
            message_format,
            model_hash,
            seed,
        } = *inputs;
        let canonical = canonicalize(&Json::object([
            ("engine", Json::from(engine)),
            ("engine_version", Json::from(engine_version)),
            ("message_format", Json::from(message_format)),
            ("model_hash", Json::from(model_hash.to_string())),
            ("package", Json::from(package)),
            ("package_version", Json::from(package_version)),
            ("record_format", Json::from(record_format)),
            ("run_id_format", Json::from(1)),
            ("seed", seed.map_or(Json::Null, Json::from)),
        ]))?;
        Ok(Self(format!("{engine}-{}", sha256_hex(&canonical))))
    }

    /// The id as text.
    ///
    /// ```
    /// let id: taktsim_run::RunId = format!("e-{}", "0".repeat(64)).parse()?;
    /// assert!(id.as_str().starts_with("e-0"));
    /// # Ok::<(), taktsim_run::ParseIdError>(())
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Text that is not an id.
///
/// ```
/// use taktsim_run::{ParseIdError, RunId};
///
/// assert_eq!("abc".parse::<RunId>(), Err(ParseIdError));
/// assert_eq!(ParseIdError.to_string(), "an id is a name, `-` and 64 lowercase hex digits");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an id is a name, `-` and 64 lowercase hex digits")]
pub struct ParseIdError;

/// Whether `name` could be the name before an id's hash. It stays in one path
/// segment, so an id read from a document or an argument cannot leave the store.
fn is_id_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl FromStr for RunId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.rsplit_once('-') {
            Some((name, hash)) if is_id_name(name) && is_sha256_hex(hash) => Ok(Self(s.to_owned())),
            Some(_) | None => Err(ParseIdError),
        }
    }
}

impl Encode for RunId {
    fn encode(&self) -> Json {
        self.0.as_str().into()
    }
}

/// Reads an id through its `FromStr`.
fn decode_id<T: Decode + FromStr>(node: &Node, cx: &mut Decoder) -> Option<T> {
    let Value::Str(text) = node.value() else {
        cx.wrong_type::<T>(node);
        return None;
    };
    let parsed = text.parse::<T>().ok();
    if parsed.is_none() {
        cx.not_allowed(node, text.clone(), T::EXPECTED, T::EXAMPLE);
    }
    parsed
}

/// A string with this pattern.
fn pattern(pattern: &str) -> Json {
    Json::object([
        ("type", Json::from("string")),
        ("pattern", Json::from(pattern)),
    ])
}

impl Decode for RunId {
    const EXPECTED: &'static str = "a run id: `<engine>-` and 64 hex digits";
    const EXAMPLE: &'static str =
        "taktsim-core-0000000000000000000000000000000000000000000000000000000000000000";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        decode_id(node, cx)
    }

    fn schema(_: &mut Schemas) -> Json {
        pattern("^[a-z0-9-]+-[0-9a-f]{64}$")
    }
}

/// The name of a group of runs: `exp-` and the SHA-256 over its sorted run ids,
/// so members may differ in more than the seed, and order does not matter.
///
/// ```
/// use taktsim_run::{ExperimentId, RunId};
///
/// let a: RunId = format!("e-{}", "a".repeat(64)).parse()?;
/// let b: RunId = format!("e-{}", "b".repeat(64)).parse()?;
/// let forward = ExperimentId::over([a.clone(), b.clone()])?;
/// assert_eq!(forward, ExperimentId::over([b, a])?);
/// assert!(forward.as_str().starts_with("exp-"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExperimentId(String);

impl ExperimentId {
    /// The id over `runs`.
    ///
    /// # Errors
    ///
    /// A [`CanonicalError`] only if a text had no canonical form, which a
    /// [`RunId`] always has.
    ///
    /// ```
    /// use taktsim_run::{ExperimentId, RunId};
    ///
    /// let a: RunId = format!("e-{}", "a".repeat(64)).parse()?;
    /// assert_eq!(ExperimentId::over([a])?.as_str().len(), "exp-".len() + 64);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn over(runs: impl IntoIterator<Item = RunId>) -> Result<Self, CanonicalError> {
        let mut runs: Vec<RunId> = runs.into_iter().collect();
        runs.sort();
        let canonical = canonicalize(&Json::object([
            ("experiment_id_format", Json::from(1)),
            ("runs", runs.encode()),
        ]))?;
        Ok(Self(format!("exp-{}", sha256_hex(&canonical))))
    }

    /// The id as text.
    ///
    /// ```
    /// use taktsim_run::ExperimentId;
    ///
    /// let id: ExperimentId = format!("exp-{}", "0".repeat(64)).parse()?;
    /// assert!(id.as_str().starts_with("exp-0"));
    /// # Ok::<(), taktsim_run::ParseIdError>(())
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExperimentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ExperimentId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.strip_prefix("exp-") {
            Some(hash) if is_sha256_hex(hash) => Ok(Self(s.to_owned())),
            Some(_) | None => Err(ParseIdError),
        }
    }
}

impl Encode for ExperimentId {
    fn encode(&self) -> Json {
        self.0.as_str().into()
    }
}

impl Decode for ExperimentId {
    const EXPECTED: &'static str = "an experiment id: `exp-` and 64 hex digits";
    const EXAMPLE: &'static str =
        "exp-0000000000000000000000000000000000000000000000000000000000000000";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        decode_id(node, cx)
    }

    fn schema(_: &mut Schemas) -> Json {
        pattern("^exp-[0-9a-f]{64}$")
    }
}
