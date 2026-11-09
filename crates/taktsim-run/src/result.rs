//! The result document (ADR-0024 §3, §4): one document for every run, stored and
//! printed byte for byte the same. Readers ignore unknown keys, so later versions
//! can add fields; nothing here is shaped around a factory or a confidence
//! interval.

use std::collections::BTreeMap;

use taktsim_model::{
    Decode, Decoder, Encode, Fields, Json, ModelHash, Node, Record, Schemas, Segment, SystemHash,
    Value, record_schema,
};
use taktsim_stats::ConfidenceInterval;
use taktsim_trace::TraceHash;
use taktsim_units::SimDuration;

use crate::diagnostic::Diagnostic;
use crate::id::{ExperimentId, RunId};

/// The format of a [`ResultDocument`]. A new major number only for a breaking
/// change.
///
/// ```
/// assert_eq!(taktsim_run::RESULT_FORMAT, "taktsim-result/1");
/// ```
pub const RESULT_FORMAT: &str = "taktsim-result/1";

/// The format of a [`ValidationReport`].
///
/// ```
/// assert_eq!(taktsim_run::VALIDATION_FORMAT, "taktsim-validation/1");
/// ```
pub const VALIDATION_FORMAT: &str = "taktsim-validation/1";

/// A key whose value is one fixed word: a document's format.
macro_rules! format_word {
    ($name:ident, $word:expr) => {
        struct $name;

        impl Decode for $name {
            const EXPECTED: &'static str = $word;
            const EXAMPLE: &'static str = $word;

            fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
                if matches!(node.value(), Value::Str(text) if text == $word) {
                    Some(Self)
                } else {
                    cx.wrong_type::<Self>(node);
                    None
                }
            }

            fn schema(_: &mut Schemas) -> Json {
                Json::object([("const", Json::from($word))])
            }
        }
    };
}

format_word!(ResultFormat, RESULT_FORMAT);
format_word!(ValidationFormat, VALIDATION_FORMAT);

/// A list with at least one entry.
struct NonEmpty<T>(Vec<T>);

impl<T: Decode> Decode for NonEmpty<T> {
    const EXPECTED: &'static str = "a list with at least one entry";
    const EXAMPLE: &'static str = "[a]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let list = Vec::<T>::decode(node, cx)?;
        if list.is_empty() {
            cx.not_allowed(node, "an empty list", Self::EXPECTED, Self::EXAMPLE);
            return None;
        }
        Some(Self(list))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<T>()),
            ("minItems", Json::from(1)),
        ])
    }
}

/// A SHA-256 as 64 lowercase hex digits.
struct Sha256<T>(T);

impl<T: std::str::FromStr> Decode for Sha256<T> {
    const EXPECTED: &'static str = "a SHA-256 as 64 lowercase hex digits";
    const EXAMPLE: &'static str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SCHEMA_NAME: Option<&'static str> = Some("Sha256");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Str(text) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        if let Ok(hash) = text.parse() {
            Some(Self(hash))
        } else {
            cx.not_allowed(node, text.clone(), Self::EXPECTED, Self::EXAMPLE);
            None
        }
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("string")),
            ("pattern", Json::from("^[0-9a-f]{64}$")),
        ])
    }
}

/// The result of a run: what ran, how it ended, and what it measured. `complete`
/// is the only field a reader uses to decide; `stop_reason` only explains.
///
/// ```
/// use taktsim_model::{Format, decode_str};
/// use taktsim_run::ResultDocument;
///
/// let text = r#"{
///   "format": "taktsim-result/1", "complete": false, "stop_reason": "event_limit",
///   "provenance": {
///     "engine": "taktsim-core", "engine_version": "0.1.0",
///     "package": "taktsim-components", "package_version": "0.1.0",
///     "model_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
///     "system_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
///     "platform": "x86_64-linux", "seeds": [42]
///   },
///   "runs": [{"complete": false, "stop_reason": "event_limit", "end_time": "1min",
///             "events": 3, "records": 5, "seed": 42}],
///   "metrics": [], "unreported": [],
///   "a_field_of_a_later_version": {"ignored": true}
/// }"#;
/// let result: ResultDocument = decode_str(text, Format::Json)?;
/// assert!(!result.complete);
/// assert_eq!(result.experiment_id, None);
/// assert_eq!(result.runs[0].seed, Some(42));
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ResultDocument {
    /// Whether the run, or the experiment, finished what was asked. If not, the
    /// numbers hold only for the stated `over` and must not be compared with a
    /// complete result.
    pub complete: bool,
    /// Present exactly when every entry of `runs` has a `run_id`.
    pub experiment_id: Option<ExperimentId>,
    /// Why it stopped: an open set of words, for explanation only.
    pub stop_reason: String,
    /// What it came from.
    pub provenance: Provenance,
    /// At least one.
    pub runs: Vec<RunEntry>,
    /// What it measured.
    pub metrics: Vec<Metric>,
    /// What the model declares that this result does not hold.
    pub unreported: Vec<Unreported>,
}

impl ResultDocument {
    /// The document as stored and printed: keys sorted, `\n` line ends.
    ///
    /// ```
    /// # use taktsim_model::{Format, decode_str};
    /// # let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    /// # let text = format!(r#"{{"format": "taktsim-result/1", "complete": true, "stop_reason": "horizon",
    /// #   "provenance": {{"engine": "e", "engine_version": "1", "package": "p", "package_version": "1",
    /// #     "model_hash": "{hash}", "system_hash": "{hash}", "platform": "x", "seeds": []}},
    /// #   "runs": [{{"complete": true, "stop_reason": "horizon", "end_time": "1h", "events": 0, "records": 0}}],
    /// #   "metrics": [], "unreported": []}}"#);
    /// # let result: taktsim_run::ResultDocument = decode_str(&text, Format::Json)?;
    /// let text = result.to_text();
    /// assert!(text.starts_with("{\n  \"complete\": true,\n"));
    /// assert!(text.ends_with("}\n"));
    /// assert_eq!(decode_str::<taktsim_run::ResultDocument>(&text, Format::Json)?, result);
    /// # Ok::<(), taktsim_model::LoadErrors>(())
    /// ```
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = self.encode().to_pretty();
        text.push('\n');
        text
    }
}

impl Encode for ResultDocument {
    fn encode(&self) -> Json {
        Json::object([])
            .with("format", &RESULT_FORMAT)
            .with("complete", &self.complete)
            .with_optional("experiment_id", self.experiment_id.as_ref())
            .with("stop_reason", &self.stop_reason)
            .with("provenance", &self.provenance)
            .with("runs", &self.runs)
            .with("metrics", &self.metrics)
            .with("unreported", &self.unreported)
    }
}

impl Record for ResultDocument {
    const EXPECTED: &'static str = "a result document";
    const EXAMPLE: &'static str = "{format: taktsim-result/1, complete: true}";
    const DESCRIPTION: Option<&'static str> = Some(
        "The result of a run (ADR-0024 §3). Readers use `complete` to decide and ignore unknown keys.",
    );
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let format = fields.required::<ResultFormat>("format");
        let complete = fields.required("complete");
        let experiment_id = fields.optional("experiment_id");
        let stop_reason = fields.required("stop_reason");
        let provenance = fields.required("provenance");
        let runs = fields.required::<NonEmpty<RunEntry>>("runs");
        let metrics = fields.required("metrics");
        let unreported = fields.required("unreported");
        format?;
        Some(Self {
            complete: complete?,
            experiment_id: experiment_id?,
            stop_reason: stop_reason?,
            provenance: provenance?,
            runs: runs?.0,
            metrics: metrics?,
            unreported: unreported?,
        })
    }
}

/// What a result came from (spec §7 Layer 3). `platform` is the only field that
/// differs across targets.
///
/// ```
/// use taktsim_model::{Encode, ModelHash};
/// use taktsim_run::Provenance;
///
/// let provenance = Provenance {
///     engine: "taktsim-core".into(),
///     engine_version: "0.1.0".into(),
///     package: "taktsim-components".into(),
///     package_version: "0.1.0".into(),
///     model_hash: ModelHash::of(b"model"),
///     system_hash: "18c3cbaad4e836fd4c29a17ba0a88ff553dfb516a69886db10ed2efb09d89904".parse()?,
///     platform: "x86_64-linux".into(),
///     seeds: vec![42],
/// };
/// assert_eq!(provenance.encode()["seeds"][0], taktsim_model::Json::from(42));
/// # Ok::<(), taktsim_model::ParseHashError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The engine's name.
    pub engine: String,
    /// The engine's version.
    pub engine_version: String,
    /// The domain package's name.
    pub package: String,
    /// The domain package's version.
    pub package_version: String,
    /// The model hash.
    pub model_hash: ModelHash,
    /// The system hash.
    pub system_hash: SystemHash,
    /// Where it ran, such as `x86_64-linux`.
    pub platform: String,
    /// The seeds used; empty for a model that draws nothing.
    pub seeds: Vec<u64>,
}

impl Encode for Provenance {
    fn encode(&self) -> Json {
        Json::object([])
            .with("engine", &self.engine)
            .with("engine_version", &self.engine_version)
            .with("package", &self.package)
            .with("package_version", &self.package_version)
            .with("model_hash", &self.model_hash.to_string())
            .with("system_hash", &self.system_hash.to_string())
            .with("platform", &self.platform)
            .with("seeds", &self.seeds)
    }
}

impl Record for Provenance {
    const EXPECTED: &'static str = "the provenance of a result";
    const EXAMPLE: &'static str = "{engine: taktsim-core, engine_version: 0.1.0}";
    const SCHEMA_NAME: Option<&'static str> = Some("Provenance");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let engine = fields.required("engine");
        let engine_version = fields.required("engine_version");
        let package = fields.required("package");
        let package_version = fields.required("package_version");
        let model_hash = fields.required::<Sha256<ModelHash>>("model_hash");
        let system_hash = fields.required::<Sha256<SystemHash>>("system_hash");
        let platform = fields.required("platform");
        let seeds = fields.required("seeds");
        Some(Self {
            engine: engine?,
            engine_version: engine_version?,
            package: package?,
            package_version: package_version?,
            model_hash: model_hash?.0,
            system_hash: system_hash?.0,
            platform: platform?,
            seeds: seeds?,
        })
    }
}

/// One run of a result. `run_id` and `trace` are present exactly when the run
/// reached its end and is in the store.
///
/// ```
/// use taktsim_model::Encode;
/// use taktsim_run::RunEntry;
///
/// let partial = RunEntry {
///     complete: false,
///     stop_reason: "event_limit".into(),
///     end_time: "1min".parse()?,
///     events: 3,
///     records: 5,
///     run_id: None,
///     seed: Some(42),
///     trace: None,
/// };
/// assert_eq!(partial.encode()["run_id"], taktsim_model::Json::Null);
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEntry {
    /// Whether this run finished what was asked.
    pub complete: bool,
    /// Why it stopped.
    pub stop_reason: String,
    /// The simulation time at its end.
    pub end_time: SimDuration,
    /// Events delivered.
    pub events: u64,
    /// Records in its trace.
    pub records: u64,
    /// Its name in the store, if it is kept.
    pub run_id: Option<RunId>,
    /// Its seed; absent if it has none.
    pub seed: Option<u64>,
    /// Its trace, if it is kept.
    pub trace: Option<TraceRef>,
}

impl Encode for RunEntry {
    fn encode(&self) -> Json {
        Json::object([])
            .with("complete", &self.complete)
            .with("stop_reason", &self.stop_reason)
            .with("end_time", &self.end_time)
            .with("events", &self.events)
            .with("records", &self.records)
            .with_optional("run_id", self.run_id.as_ref())
            .with_optional("seed", self.seed.as_ref())
            .with_optional("trace", self.trace.as_ref())
    }
}

impl Record for RunEntry {
    const EXPECTED: &'static str = "one run of a result";
    const EXAMPLE: &'static str = "{complete: true, stop_reason: horizon, end_time: 8h}";
    const SCHEMA_NAME: Option<&'static str> = Some("RunEntry");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let complete = fields.required("complete");
        let stop_reason = fields.required("stop_reason");
        let end_time = fields.required("end_time");
        let events = fields.required("events");
        let records = fields.required("records");
        let run_id = fields.optional("run_id");
        let seed = fields.optional("seed");
        let trace = fields.optional("trace");
        Some(Self {
            complete: complete?,
            stop_reason: stop_reason?,
            end_time: end_time?,
            events: events?,
            records: records?,
            run_id: run_id?,
            seed: seed?,
            trace: trace?,
        })
    }
}

/// Where a run's trace is and what it holds. `path` is relative to the store and
/// uses `/` on every target.
///
/// ```
/// use taktsim_run::TraceRef;
/// use taktsim_trace::TraceHash;
///
/// let trace = TraceRef {
///     path: "runs/id/trace.takt".into(),
///     bytes: 100,
///     trace_format: 3,
///     record_format: "taktsim-core/2".into(),
///     message_format: "taktsim-components/1".into(),
///     stream_hash: TraceHash::sha256(b""),
/// };
/// assert_eq!(trace.trace_format, u32::from(taktsim_trace::FORMAT_VERSION));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRef {
    /// Where it is, below the store.
    pub path: String,
    /// Its size.
    pub bytes: u64,
    /// The file format.
    pub trace_format: u32,
    /// The record format.
    pub record_format: String,
    /// The message format.
    pub message_format: String,
    /// The SHA-256 of its records.
    pub stream_hash: TraceHash,
}

impl Encode for TraceRef {
    fn encode(&self) -> Json {
        Json::object([])
            .with("path", &self.path)
            .with("bytes", &self.bytes)
            .with("trace_format", &self.trace_format)
            .with("record_format", &self.record_format)
            .with("message_format", &self.message_format)
            .with("stream_hash", &self.stream_hash.to_string())
    }
}

impl Record for TraceRef {
    const EXPECTED: &'static str = "a reference to a trace";
    const EXAMPLE: &'static str = "{path: runs/id/trace.takt, bytes: 100}";
    const SCHEMA_NAME: Option<&'static str> = Some("TraceRef");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let path = fields.required("path");
        let bytes = fields.required("bytes");
        let trace_format = fields.required("trace_format");
        let record_format = fields.required("record_format");
        let message_format = fields.required("message_format");
        let stream_hash = fields.required::<Sha256<ModelHash>>("stream_hash");
        Some(Self {
            path: path?,
            bytes: bytes?,
            trace_format: trace_format?,
            record_format: record_format?,
            message_format: message_format?,
            stream_hash: TraceHash::from_bytes(*stream_hash?.0.as_bytes()),
        })
    }
}

/// A measured quantity, over a window or at a time. `name` and `unit` are open
/// strings; `values` is one entry for a scalar and several, told apart by their
/// labels, for a table.
///
/// ```
/// use taktsim_model::Encode;
/// use taktsim_run::{Accuracy, Metric, MetricValue, Over, Scalar};
///
/// let metric = Metric {
///     name: "delivered(sink)".into(),
///     unit: "items".into(),
///     over: Over::Window { from: "0s".parse()?, to: "8h".parse()? },
///     observable: None,
///     values: vec![MetricValue {
///         value: Scalar::Integer(639),
///         labels: None,
///         accuracy: vec![Accuracy::none("single_replication", "one replication has no interval")],
///     }],
/// };
/// assert_eq!(metric.encode()["values"][0]["value"], taktsim_model::Json::from(639));
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    /// Its name.
    pub name: String,
    /// Its one declared unit (ADR-0014 §6).
    pub unit: String,
    /// What span it covers.
    pub over: Over,
    /// The declared text this metric answers, if any.
    pub observable: Option<String>,
    /// Its values, each with its own accuracy.
    pub values: Vec<MetricValue>,
}

impl Encode for Metric {
    fn encode(&self) -> Json {
        Json::object([])
            .with("name", &self.name)
            .with("unit", &self.unit)
            .with("over", &self.over)
            .with_optional("observable", self.observable.as_ref())
            .with("values", &self.values)
    }
}

impl Record for Metric {
    const EXPECTED: &'static str = "a metric";
    const EXAMPLE: &'static str = "{name: delivered(sink), unit: items}";
    const SCHEMA_NAME: Option<&'static str> = Some("Metric");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let name = fields.required("name");
        let unit = fields.required("unit");
        let over = fields.required("over");
        let observable = fields.optional("observable");
        let values = fields.required::<NonEmpty<MetricValue>>("values");
        Some(Self {
            name: name?,
            unit: unit?,
            over: over?,
            observable: observable?,
            values: values?.0,
        })
    }
}

/// What span a metric covers.
///
/// ```
/// use taktsim_model::Encode;
/// use taktsim_run::Over;
///
/// let at = Over::At { at: "8h".parse()? };
/// assert_eq!(at.encode()["at"], "8h");
/// # Ok::<(), taktsim_units::UnitError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Over {
    /// A window; a later warm-up is a `from` above `0s`.
    Window {
        /// Its start.
        from: SimDuration,
        /// Its end.
        to: SimDuration,
    },
    /// A reading at one time.
    At {
        /// The time.
        at: SimDuration,
    },
}

impl Encode for Over {
    fn encode(&self) -> Json {
        match self {
            Self::Window { from, to } => Json::object([]).with("from", from).with("to", to),
            Self::At { at } => Json::object([]).with("at", at),
        }
    }
}

/// The keys of an [`Over`].
struct OverKeys {
    from: Option<SimDuration>,
    to: Option<SimDuration>,
    at: Option<SimDuration>,
}

impl Record for OverKeys {
    const EXPECTED: &'static str = "`{from: 0s, to: 8h}` or `{at: 8h}`";
    const EXAMPLE: &'static str = "{from: 0s, to: 8h}";
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let from = fields.optional("from");
        let to = fields.optional("to");
        let at = fields.optional("at");
        Some(Self {
            from: from?,
            to: to?,
            at: at?,
        })
    }
}

impl Decode for Over {
    const EXPECTED: &'static str = <OverKeys as Record>::EXPECTED;
    const EXAMPLE: &'static str = <OverKeys as Record>::EXAMPLE;
    const SCHEMA_NAME: Option<&'static str> = Some("Over");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let keys = OverKeys::decode(node, cx)?;
        match (keys.from, keys.to, keys.at) {
            (Some(from), Some(to), None) => Some(Self::Window { from, to }),
            (None, None, Some(at)) => Some(Self::At { at }),
            (Some(_) | None, Some(_) | None, Some(_) | None) => {
                cx.not_allowed(
                    node,
                    "another mix of `from`, `to` and `at`",
                    Self::EXPECTED,
                    Self::EXAMPLE,
                );
                None
            }
        }
    }

    fn schema(schemas: &mut Schemas) -> Json {
        let duration = schemas.of::<SimDuration>();
        // Other keys stay open (`Record::OPEN`), but the keys of the other shape
        // are refused, as `decode` refuses any other mix of the three.
        let shape = |keys: &[&str], refused: &[&str]| {
            Json::object([
                ("type", Json::from("object")),
                (
                    "required",
                    Json::Array(keys.iter().map(|k| Json::from(*k)).collect()),
                ),
                (
                    "properties",
                    Json::Object(
                        keys.iter()
                            .map(|k| ((*k).to_owned(), duration.clone()))
                            .chain(refused.iter().map(|k| ((*k).to_owned(), Json::from(false))))
                            .collect(),
                    ),
                ),
            ])
        };
        Json::object([(
            "oneOf",
            Json::Array(vec![
                shape(&["from", "to"], &["at"]),
                shape(&["at"], &["from", "to"]),
            ]),
        )])
    }
}

/// A metric's number: an exact count, a measured number, or a yes or no
/// (ADR-0015 lets an observable be declared as one).
///
/// ```
/// use taktsim_model::{Encode, Json};
/// use taktsim_run::Scalar;
///
/// assert_eq!(Scalar::Integer(3).encode(), Json::from(3));
/// assert_eq!(Scalar::Flag(true).encode(), Json::from(true));
/// assert_eq!(Scalar::Number(0.5).encode(), Json::from(0.5));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar {
    /// A count.
    Integer(u64),
    /// A number with a fraction.
    Number(f64),
    /// A yes or no.
    Flag(bool),
}

impl Encode for Scalar {
    fn encode(&self) -> Json {
        match self {
            Self::Integer(n) => n.encode(),
            Self::Number(x) => x.encode(),
            Self::Flag(b) => b.encode(),
        }
    }
}

impl Decode for Scalar {
    const EXPECTED: &'static str = "a number or a boolean";
    const EXAMPLE: &'static str = "42";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        match node.value() {
            Value::Int(n) => u64::try_from(*n).map_or(
                // A negative number is a measurement, not a count.
                f64::decode(node, cx).map(Self::Number),
                |n| Some(Self::Integer(n)),
            ),
            Value::Float(x) => Some(Self::Number(*x)),
            Value::Bool(b) => Some(Self::Flag(*b)),
            Value::Null | Value::Str(_) | Value::List(_) | Value::Map(_) => {
                cx.wrong_type::<Self>(node);
                None
            }
        }
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([(
            "anyOf",
            Json::Array(vec![
                Json::object([("type", Json::from("number"))]),
                Json::object([("type", Json::from("boolean"))]),
            ]),
        )])
    }
}

/// The labels that tell values of a table apart, so no reader splits a string.
struct Labels(BTreeMap<String, String>);

impl Decode for Labels {
    const EXPECTED: &'static str = "a mapping of text, such as `{resource: cut, state: busy}`";
    const EXAMPLE: &'static str = "{resource: cut, state: busy}";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let Value::Map(entries) = node.value() else {
            cx.wrong_type::<Self>(node);
            return None;
        };
        let mut labels = BTreeMap::new();
        let mut valid = true;
        for entry in entries {
            let value = cx.within(Segment::Key(entry.key().to_owned()), |cx| {
                String::decode(entry.value(), cx)
            });
            match value {
                Some(value) => {
                    labels.insert(entry.key().to_owned(), value);
                }
                None => valid = false,
            }
        }
        valid.then_some(Self(labels))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("object")),
            (
                "additionalProperties",
                Json::object([("type", Json::from("string"))]),
            ),
        ])
    }
}

/// One value of a metric, with its accuracy: at least one statement, which may be
/// that there is no error bound.
///
/// ```
/// use taktsim_run::{Accuracy, MetricValue, Scalar};
///
/// let value = MetricValue {
///     value: Scalar::Number(0.5),
///     labels: Some([("state".to_owned(), "busy".to_owned())].into()),
///     accuracy: vec![Accuracy::none("no_random_draws", "the model draws nothing")],
/// };
/// assert_eq!(value.accuracy.len(), 1);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct MetricValue {
    /// The number.
    pub value: Scalar,
    /// What tells it apart from the other values of its metric.
    pub labels: Option<BTreeMap<String, String>>,
    /// What can be said about its error.
    pub accuracy: Vec<Accuracy>,
}

impl Encode for MetricValue {
    fn encode(&self) -> Json {
        let value = Json::object([])
            .with("value", &self.value)
            .with("accuracy", &self.accuracy);
        match &self.labels {
            Some(labels) => value.with(
                "labels",
                &Json::Object(
                    labels
                        .iter()
                        .map(|(k, v)| (k.clone(), v.encode()))
                        .collect(),
                ),
            ),
            None => value,
        }
    }
}

impl Record for MetricValue {
    const EXPECTED: &'static str = "a value with its accuracy";
    const EXAMPLE: &'static str = "{value: 42, accuracy: [{kind: none, reason: r, detail: d}]}";
    const SCHEMA_NAME: Option<&'static str> = Some("MetricValue");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let value = fields.required("value");
        let labels = fields.optional::<Labels>("labels");
        let accuracy = fields.required::<NonEmpty<Accuracy>>("accuracy");
        Some(Self {
            value: value?,
            labels: labels?.map(|l| l.0),
            accuracy: accuracy?.0,
        })
    }
}

/// What can be said about a value's error (ADR-0024 §4).
///
/// ```
/// use taktsim_run::{Accuracy, AccuracyKind};
///
/// let none = Accuracy::none("single_replication", "one replication has no interval");
/// assert!(matches!(none.kind, AccuracyKind::None { .. }));
/// assert_eq!(none.target, None);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Accuracy {
    /// The statement.
    pub kind: AccuracyKind,
    /// The precision that was asked for.
    pub target: Option<AccuracyTarget>,
    /// Whether it was reached.
    pub reached: Option<bool>,
}

impl Accuracy {
    /// That there is no statement, and why.
    ///
    /// ```
    /// use taktsim_model::Encode;
    ///
    /// let none = taktsim_run::Accuracy::none("single_replication", "one replication has no interval");
    /// assert_eq!(none.encode()["kind"], "none");
    /// assert_eq!(none.encode()["reason"], "single_replication");
    /// ```
    #[must_use]
    pub fn none(reason: &str, detail: &str) -> Self {
        Self {
            kind: AccuracyKind::None {
                reason: reason.to_owned(),
                detail: detail.to_owned(),
            },
            target: None,
            reached: None,
        }
    }
}

/// The statement of an [`Accuracy`]. A kind this build does not know reads as no
/// statement, never as an error.
///
/// ```
/// use taktsim_model::{Format, decode_str};
/// use taktsim_run::{Accuracy, AccuracyKind};
///
/// let later: Accuracy = decode_str("{kind: discretization, order: 2}", Format::Yaml)?;
/// assert_eq!(later.kind, AccuracyKind::Unknown { kind: "discretization".into() });
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum AccuracyKind {
    /// A confidence interval over independent replications.
    Sampling(SamplingInterval),
    /// No statement, and why.
    None {
        /// Why: an open set of words.
        reason: String,
        /// The same for a person.
        detail: String,
    },
    /// A kind this build does not know.
    Unknown {
        /// Its word.
        kind: String,
    },
}

/// A confidence interval as a result states it.
///
/// ```
/// use taktsim_run::SamplingInterval;
/// use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};
///
/// let ci = ConfidenceInterval::from_values(&[98.0, 100.0, 102.0], ConfidenceLevel::DEFAULT)?;
/// let interval = SamplingInterval::from(&ci);
/// assert_eq!(interval.replications, 3);
/// assert_eq!(interval.mean, 100.0);
/// # Ok::<(), taktsim_stats::StatsError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplingInterval {
    /// The mean.
    pub mean: f64,
    /// The half-width.
    pub half_width: f64,
    /// The standard deviation of the replications.
    pub std_dev: f64,
    /// How many replications.
    pub replications: u32,
    /// The confidence level, such as 0.95.
    pub level: f64,
}

impl From<&ConfidenceInterval> for SamplingInterval {
    fn from(ci: &ConfidenceInterval) -> Self {
        Self {
            mean: ci.mean(),
            half_width: ci.half_width(),
            std_dev: ci.std_dev(),
            replications: ci.replications(),
            level: ci.level().level(),
        }
    }
}

impl From<&ConfidenceInterval> for Accuracy {
    fn from(ci: &ConfidenceInterval) -> Self {
        Self {
            kind: AccuracyKind::Sampling(ci.into()),
            target: None,
            reached: None,
        }
    }
}

impl Encode for SamplingInterval {
    fn encode(&self) -> Json {
        Json::object([])
            .with("mean", &self.mean)
            .with("half_width", &self.half_width)
            .with("std_dev", &self.std_dev)
            .with("replications", &self.replications)
            .with("level", &self.level)
    }
}

impl Record for SamplingInterval {
    const EXPECTED: &'static str = "a confidence interval";
    const EXAMPLE: &'static str =
        "{mean: 100.0, half_width: 2.0, std_dev: 1.5, replications: 20, level: 0.95}";
    const SCHEMA_NAME: Option<&'static str> = Some("SamplingInterval");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let mean = fields.required("mean");
        let half_width = fields.required("half_width");
        let std_dev = fields.required("std_dev");
        let replications = fields.required("replications");
        let level = fields.required("level");
        Some(Self {
            mean: mean?,
            half_width: half_width?,
            std_dev: std_dev?,
            replications: replications?,
            level: level?,
        })
    }
}

/// The precision that was asked for: an object, so an absolute target can be added
/// without changing a reader of `relative`.
///
/// ```
/// use taktsim_model::Encode;
///
/// let target = taktsim_run::AccuracyTarget { relative: Some(0.02) };
/// assert_eq!(target.encode()["relative"], taktsim_model::Json::from(0.02));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccuracyTarget {
    /// The half-width as a fraction of the mean.
    pub relative: Option<f64>,
}

impl Encode for AccuracyTarget {
    fn encode(&self) -> Json {
        Json::object([]).with_optional("relative", self.relative.as_ref())
    }
}

impl Record for AccuracyTarget {
    const EXPECTED: &'static str = "a target such as `{relative: 0.02}`";
    const EXAMPLE: &'static str = "{relative: 0.02}";
    const SCHEMA_NAME: Option<&'static str> = Some("AccuracyTarget");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            relative: fields.optional("relative")?,
        })
    }
}

impl Encode for Accuracy {
    fn encode(&self) -> Json {
        let statement = match &self.kind {
            AccuracyKind::Sampling(interval) => Json::object([])
                .with("kind", &"sampling")
                .with("interval", interval),
            AccuracyKind::None { reason, detail } => Json::object([])
                .with("kind", &"none")
                .with("reason", reason)
                .with("detail", detail),
            AccuracyKind::Unknown { kind } => Json::object([]).with("kind", kind),
        };
        statement
            .with_optional("target", self.target.as_ref())
            .with_optional("reached", self.reached.as_ref())
    }
}

/// The keys of an [`Accuracy`].
struct AccuracyKeys {
    kind: String,
    interval: Option<SamplingInterval>,
    reason: Option<String>,
    detail: Option<String>,
    target: Option<AccuracyTarget>,
    reached: Option<bool>,
}

impl Record for AccuracyKeys {
    const EXPECTED: &'static str = "an accuracy statement";
    const EXAMPLE: &'static str = "{kind: none, reason: single_replication, detail: one run}";
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let kind = fields.required("kind");
        let interval = fields.optional("interval");
        let reason = fields.optional("reason");
        let detail = fields.optional("detail");
        let target = fields.optional("target");
        let reached = fields.optional("reached");
        Some(Self {
            kind: kind?,
            interval: interval?,
            reason: reason?,
            detail: detail?,
            target: target?,
            reached: reached?,
        })
    }
}

impl Decode for Accuracy {
    const EXPECTED: &'static str = <AccuracyKeys as Record>::EXPECTED;
    const EXAMPLE: &'static str = <AccuracyKeys as Record>::EXAMPLE;
    const SCHEMA_NAME: Option<&'static str> = Some("Accuracy");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let keys = AccuracyKeys::decode(node, cx)?;
        let kind = match keys.kind.as_str() {
            "sampling" => keys.interval.map(AccuracyKind::Sampling),
            "none" => keys
                .reason
                .zip(keys.detail)
                .map(|(reason, detail)| AccuracyKind::None { reason, detail }),
            _ => Some(AccuracyKind::Unknown {
                kind: keys.kind.clone(),
            }),
        };
        let Some(kind) = kind else {
            cx.not_allowed(
                node,
                format!("`kind: {}` without its keys", keys.kind),
                "`interval` with `sampling`, `reason` and `detail` with `none`",
                Self::EXAMPLE,
            );
            return None;
        };
        Some(Self {
            kind,
            target: keys.target,
            reached: keys.reached,
        })
    }

    fn schema(schemas: &mut Schemas) -> Json {
        let when = |word: &str, then: &[&str]| {
            Json::object([
                (
                    "if",
                    Json::object([
                        ("required", Json::Array(vec![Json::from("kind")])),
                        (
                            "properties",
                            Json::object([("kind", Json::object([("const", Json::from(word))]))]),
                        ),
                    ]),
                ),
                (
                    "then",
                    Json::object([(
                        "required",
                        Json::Array(then.iter().map(|k| Json::from(*k)).collect()),
                    )]),
                ),
            ])
        };
        <AccuracyKeys as Decode>::schema(schemas)
            .with(
                "description",
                &"What can be said about a value's error. Read a kind you do not know as no statement, never as an error.",
            )
            .with(
                "allOf",
                &Json::Array(vec![
                    when("sampling", &["interval"]),
                    when("none", &["reason", "detail"]),
                ]),
            )
    }
}

/// What the model declares in `observe` or `experiment.precision` that a result
/// does not hold, stated in the result, where an agent reads it.
///
/// ```
/// use taktsim_model::Encode;
///
/// let missing = taktsim_run::Unreported { observable: "throughput(sink)".into(), reason: "not_computed".into() };
/// assert_eq!(missing.encode()["reason"], "not_computed");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreported {
    /// The declared text.
    pub observable: String,
    /// Why: an open set of words.
    pub reason: String,
}

impl Encode for Unreported {
    fn encode(&self) -> Json {
        Json::object([])
            .with("observable", &self.observable)
            .with("reason", &self.reason)
    }
}

impl Record for Unreported {
    const EXPECTED: &'static str = "a declared observable with a reason";
    const EXAMPLE: &'static str = "{observable: throughput(sink), reason: not_computed}";
    const SCHEMA_NAME: Option<&'static str> = Some("Unreported");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let observable = fields.required("observable");
        let reason = fields.required("reason");
        Some(Self {
            observable: observable?,
            reason: reason?,
        })
    }
}

/// What `validate` found (ADR-0024 §9). The hashes are present only when the model
/// is valid.
///
/// ```
/// use taktsim_model::Encode;
/// use taktsim_run::ValidationReport;
///
/// let report = ValidationReport { valid: false, errors: vec![], model_hash: None, system_hash: None };
/// assert_eq!(report.encode()["format"], "taktsim-validation/1");
/// assert_eq!(report.failure_class(), None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    /// Whether the model is valid.
    pub valid: bool,
    /// Every problem found, in reading order.
    pub errors: Vec<Diagnostic>,
    /// The model hash of a valid model.
    pub model_hash: Option<ModelHash>,
    /// The system hash of a valid model.
    pub system_hash: Option<SystemHash>,
}

impl ValidationReport {
    /// The kind of failure, if there are errors: a file that cannot be read is an
    /// I/O failure, everything else invalid input.
    ///
    /// ```
    /// use taktsim_run::{Diagnostic, FailureClass, ValidationReport};
    ///
    /// let report = ValidationReport {
    ///     valid: false,
    ///     errors: vec![Diagnostic::error("model.io", "cannot read the file")],
    ///     model_hash: None,
    ///     system_hash: None,
    /// };
    /// assert_eq!(report.failure_class(), Some(FailureClass::Io));
    /// ```
    #[must_use]
    pub fn failure_class(&self) -> Option<crate::FailureClass> {
        if self.errors.is_empty() {
            None
        } else if self.errors.iter().any(|e| e.type_name == "model.io") {
            Some(crate::FailureClass::Io)
        } else {
            Some(crate::FailureClass::Invalid)
        }
    }

    /// The report as printed: keys sorted, `\n` line ends.
    ///
    /// ```
    /// let report = taktsim_run::ValidationReport { valid: true, errors: vec![], model_hash: None, system_hash: None };
    /// assert!(report.to_text().ends_with("}\n"));
    /// ```
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = self.encode().to_pretty();
        text.push('\n');
        text
    }
}

impl Encode for ValidationReport {
    fn encode(&self) -> Json {
        Json::object([])
            .with("format", &VALIDATION_FORMAT)
            .with("valid", &self.valid)
            .with("errors", &self.errors)
            .with_optional(
                "model_hash",
                self.model_hash.map(|h| h.to_string()).as_ref(),
            )
            .with_optional(
                "system_hash",
                self.system_hash.map(|h| h.to_string()).as_ref(),
            )
    }
}

impl Record for ValidationReport {
    const EXPECTED: &'static str = "a validation report";
    const EXAMPLE: &'static str = "{format: taktsim-validation/1, valid: true, errors: []}";
    const DESCRIPTION: Option<&'static str> =
        Some("What `taktsim validate` found (ADR-0024 §9). Readers ignore unknown keys.");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let format = fields.required::<ValidationFormat>("format");
        let valid = fields.required("valid");
        let errors = fields.required("errors");
        let model_hash = fields.optional::<Sha256<ModelHash>>("model_hash");
        let system_hash = fields.optional::<Sha256<SystemHash>>("system_hash");
        format?;
        Some(Self {
            valid: valid?,
            errors: errors?,
            model_hash: model_hash?.map(|h| h.0),
            system_hash: system_hash?.map(|h| h.0),
        })
    }
}

/// The JSON Schema of a [`ResultDocument`].
///
/// ```
/// let schema = taktsim_run::result_schema();
/// assert_eq!(schema["properties"]["format"]["const"], "taktsim-result/1");
/// assert_eq!(schema["additionalProperties"], taktsim_model::Json::Null);
/// ```
#[must_use]
pub fn result_schema() -> Json {
    record_schema::<ResultDocument>().with("title", &RESULT_FORMAT)
}

/// The JSON Schema of a [`ValidationReport`].
///
/// ```
/// let schema = taktsim_run::validation_schema();
/// assert_eq!(schema["properties"]["format"]["const"], "taktsim-validation/1");
/// ```
#[must_use]
pub fn validation_schema() -> Json {
    record_schema::<ValidationReport>().with("title", &VALIDATION_FORMAT)
}

/// The JSON Schema of one line of stderr: a [`Diagnostic`] with what its type
/// promises. An `args.*` error always has a path, what was found, what is allowed
/// and an example; a `model.*` error a path, what was found and what is allowed.
///
/// ```
/// let schema = taktsim_run::diagnostic_schema();
/// assert_eq!(schema["properties"]["kind"]["enum"][0], "error");
/// assert_eq!(schema["required"][1], "type");
/// ```
#[must_use]
pub fn diagnostic_schema() -> Json {
    let when = |pattern: &str, then: &[&str]| {
        Json::object([
            (
                "if",
                Json::object([(
                    "properties",
                    Json::object([("type", Json::object([("pattern", Json::from(pattern))]))]),
                )]),
            ),
            (
                "then",
                Json::object([(
                    "required",
                    Json::Array(then.iter().map(|k| Json::from(*k)).collect()),
                )]),
            ),
        ])
    };
    record_schema::<Diagnostic>()
        .with("title", &"taktsim error line")
        .with(
            "allOf",
            &Json::Array(vec![
                when("^args\\.", &["path", "found", "allowed", "example"]),
                when("^model\\.", &["path", "found", "allowed"]),
            ]),
        )
}
