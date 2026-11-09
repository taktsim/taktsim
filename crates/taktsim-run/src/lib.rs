//! Validate a model, run it into a run store and report a typed result: the logic
//! behind the `taktsim` CLI, which the Python bindings call too (ADR-0024 §1). It
//! is generic over a [`Domain`], so it names no factory word.
//!
//! ```
//! use taktsim_model::Format;
//! use taktsim_run::{ModelSource, RunRequest, Store, run};
//!
//! let registry = taktsim_components::standard()?;
//! let dir = std::env::temp_dir().join(format!("taktsim-doc-crate-{}", std::process::id()));
//! let store = Store::open(&dir)?;
//! let model = "format: \"1.0\"\nmodel: {name: line-a, seed: 42}\n\
//!              experiment: {type: terminating, horizon: 8h}\n\
//!              components:\n\
//!              - {id: source, type: Source}\n\
//!              - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}}\n\
//!              - {id: sink, type: Sink}\n\
//!              flow: [[source, cut, sink]]\n";
//!
//! let first = run(&registry, ModelSource::Text(model, Format::Yaml), &RunRequest::new(), &store)?;
//! assert!(first.result.complete);
//! assert_eq!(first.result.provenance.seeds, [42]);
//! assert!(first.result.metrics.iter().any(|m| m.name == "delivered(sink)"));
//!
//! // The same model and seed: returned from the store without running.
//! let second = run(&registry, ModelSource::Text(model, Format::Yaml), &RunRequest::new(), &store)?;
//! assert!(second.reused);
//! assert_eq!(second.result, first.result);
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod diagnostic;
mod ending;
mod experiment;
mod facts;
mod id;
mod memory;
mod metrics;
mod result;
mod schemas;
mod seeds;
mod store;

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use taktsim_components::{Domain, Registry, RunFailure, RunOptions, RunReport};
use taktsim_core::{ENGINE_NAME, ENGINE_VERSION, RunOutcome};
use taktsim_model::{Document, Encode, Format, LoadErrors, Loader, Model};
use taktsim_trace::{
    FORMAT_VERSION, RECORD_FORMAT, ReplayError, Summary, TraceHash, TraceMessage, TraceReader,
    WriteError,
};
use taktsim_units::SimDuration;

pub use diagnostic::{Diagnostic, DiagnosticKind, Failure, FailureClass};
pub use experiment::{Budget, Ended, Progress, Spent};
pub use id::{ExperimentId, ParseIdError, RunId, RunIdInputs};
pub use result::{
    Accuracy, AccuracyKind, AccuracyPlan, AccuracyTarget, Metric, MetricValue, Over, Provenance,
    RESULT_FORMAT, Replicated, ResultDocument, RunEntry, SamplingInterval, Scalar, TraceRef,
    Unreported, VALIDATION_FORMAT, ValidationReport, diagnostic_schema, result_schema,
    validation_schema,
};
pub use schemas::{SCHEMA_NAMES, schema_named, schema_names};
pub use seeds::{MAX_SEEDS, MAX_SHORT_LIST, ParseSeedError, SeedSpec, expand_seeds};
pub use store::Store;

use crate::ending::Ending;
use crate::experiment::SeedList;
use crate::facts::Places;
use crate::metrics::{Fold, FoldError};
use crate::seeds::Mode;
use crate::store::{
    Commit, MODEL_FILE, RESULT_FILE, StoredResult, TRACE_FILE, TempRun, io_failure,
};

/// Where a model comes from.
///
/// ```
/// use taktsim_model::Format;
/// use taktsim_run::ModelSource;
///
/// let text = ModelSource::Text("format: \"1.0\"", Format::Yaml);
/// assert!(matches!(text, ModelSource::Text(..)));
/// ```
#[derive(Debug, Clone, Copy)]
pub enum ModelSource<'a> {
    /// A file; its format is chosen by its extension, and `include:` is read from
    /// its directory.
    Path(&'a Path),
    /// Text in this format.
    Text(&'a str, Format),
}

impl ModelSource<'_> {
    fn load<D: Domain>(self, registry: &Registry<D>) -> Result<Model, LoadErrors> {
        let loader = Loader::new(registry.catalog());
        match self {
            Self::Path(path) => loader.load_file(path),
            Self::Text(text, format) => loader.load_str(text, format),
        }
    }
}

/// How a run is asked for: everything except the model and the store.
///
/// ```
/// let request = taktsim_run::RunRequest::new();
/// assert!(request.reuse);
/// assert!(request.seeds.is_empty());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RunRequest<'a> {
    /// The seeds; empty takes the model's (ADR-0024 §5, ADR-0025 §4). Two or more
    /// are a list; with `experiment.precision`, one is the first of a sequence.
    pub seeds: &'a [u64],
    /// What an experiment may spend.
    pub budget: Budget,
    /// The most replications at once.
    pub threads: Option<NonZeroUsize>,
    /// The memory replications at once may take, in MiB.
    pub max_memory_mib: Option<NonZeroU64>,
    /// Another horizon: a new version of the model, with its own hash.
    pub horizon: Option<SimDuration>,
    /// The most events the run processes. A run stopped by it is partial.
    pub max_events: Option<u64>,
    /// Whether a stored run is returned without running. `false` runs anyway and
    /// compares with the stored run, never overwriting.
    pub reuse: bool,
    /// Once set, the run stops and is partial.
    pub cancel: Option<&'a AtomicBool>,
}

impl RunRequest<'_> {
    /// The model's seed and horizon, no limit, and a stored run reused.
    ///
    /// ```
    /// assert_eq!(taktsim_run::RunRequest::new().horizon, None);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            seeds: &[],
            budget: Budget::none(),
            threads: None,
            max_memory_mib: None,
            horizon: None,
            max_events: None,
            reuse: true,
            cancel: None,
        }
    }
}

impl Default for RunRequest<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// What [`run`] returns.
///
/// ```
/// use taktsim_run::{Ended, RunResponse};
///
/// fn exit_code(response: &RunResponse) -> u8 {
///     match response.ended {
///         Ended::Complete => 0,
///         Ended::BudgetExhausted | Ended::EventLimit => 3,
///         Ended::Cancelled => 4,
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RunResponse {
    /// The result.
    pub result: ResultDocument,
    /// The result as a document: the bytes in the store for a run that is kept or
    /// reused, including any key this build does not read (ADR-0024 §3).
    pub text: String,
    /// Whether it came from the store without running.
    pub reused: bool,
    /// What the caller should know, such as a seed that is not used.
    pub warnings: Vec<Diagnostic>,
    /// How it ended; the exit code follows from this, never from `stop_reason`.
    pub ended: Ended,
    /// What an experiment over several seeds did beside its document; `None` for one
    /// run.
    pub progress: Option<Progress>,
}

/// What [`replay_stored`] compared.
///
/// ```
/// use taktsim_run::ReplaySummary;
/// use taktsim_trace::TraceHash;
///
/// let summary = ReplaySummary { records: 3, events: 2, stream_hash: TraceHash::sha256(b"") };
/// assert_eq!(summary.events, 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplaySummary {
    /// Records compared.
    pub records: u64,
    /// Of these, events.
    pub events: u64,
    /// The trace's stream hash, checked against its records.
    pub stream_hash: TraceHash,
}

/// Checks a model without running it. Reads only.
///
/// ```
/// use taktsim_model::Format;
/// use taktsim_run::{ModelSource, validate};
///
/// let registry = taktsim_components::standard()?;
/// let report = validate(&registry, ModelSource::Text("{}", Format::Json));
/// assert!(!report.valid);
/// assert_eq!(report.errors[0].path.as_deref(), Some("format"));
/// assert_eq!(report.model_hash, None);
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
#[must_use]
pub fn validate<D: Domain>(registry: &Registry<D>, source: ModelSource<'_>) -> ValidationReport {
    match source.load(registry) {
        Ok(model) => ValidationReport {
            valid: true,
            errors: Vec::new(),
            model_hash: Some(model.hash()),
            system_hash: Some(model.system_hash()),
        },
        Err(errors) => ValidationReport {
            valid: false,
            errors: errors.iter().map(Diagnostic::from).collect(),
            model_hash: None,
            system_hash: None,
        },
    }
}

/// The inputs of the run id of `model` with `seed` under this registry's domain.
fn run_id<D: Domain>(model: &Model, seed: Option<u64>) -> Result<RunId, Failure> {
    let inputs = RunIdInputs {
        engine: ENGINE_NAME,
        engine_version: ENGINE_VERSION,
        record_format: RECORD_FORMAT,
        package: D::PACKAGE,
        package_version: D::PACKAGE_VERSION,
        message_format: <D::Message as TraceMessage>::FORMAT,
        model_hash: model.hash(),
        seed,
    };
    RunId::of(&inputs).map_err(|e| {
        Failure::one(
            FailureClass::Invalid,
            Diagnostic::error("run.seed_out_of_range", e.to_string()).at("--seed"),
        )
    })
}

/// A horizon the rules refuse came from the request, and the file's is fine, so the
/// error stands at the flag (ADR-0024 §9). Any other error is the model's.
fn horizon_refused(errors: &LoadErrors) -> Failure {
    let mut failure = Failure::from_load(errors);
    for line in &mut failure.diagnostics {
        if line.path.as_deref() == Some("experiment.horizon") {
            line.path = Some("--horizon".to_owned());
        }
    }
    failure
}

/// Where this binary runs: the only field of a result that differs across targets.
fn platform() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// Each declared observable that these metrics do not answer: only a value that does
/// not exist (ADR-0025 §2).
fn unreported(document: &Document, metrics: &[Metric]) -> Vec<Unreported> {
    metrics::declared(document)
        .into_iter()
        .filter(|entry| {
            !entry
                .metric_names()
                .iter()
                .all(|name| metrics.iter().any(|metric| metric.name == *name))
        })
        .map(|entry| Unreported {
            observable: entry.text,
            reason: "undefined".to_owned(),
        })
        .collect()
}

/// The failure of a run that did not go through.
fn from_run_failure(failure: RunFailure, document: &Document) -> Failure {
    match failure {
        RunFailure::NoSeed { path } | RunFailure::DrawsRandom { path } => Failure::one(
            FailureClass::Invalid,
            Diagnostic::error("run.no_seed", "this model needs a seed")
                .at(path.to_string())
                .found("no seed")
                .allowed("`--seed` or `model.seed`")
                .example("--seed 42"),
        ),
        RunFailure::Build(error) => {
            let mut line = Diagnostic::error("run.build", error.to_string());
            line.path = error.path().map(ToString::to_string);
            Failure::one(FailureClass::Invalid, line)
        }
        RunFailure::Run(error) => Failure::one(
            FailureClass::RunFailed,
            diagnostic::from_run_error(&error, document),
        ),
        RunFailure::Write(error) => {
            if let WriteError::Io { .. } = *error {
                Failure::one(
                    FailureClass::Io,
                    Diagnostic::error("store.write", error.to_string()).at("--store"),
                )
            } else {
                Failure::one(
                    FailureClass::RunFailed,
                    Diagnostic::error("run.trace_format", error.to_string()),
                )
            }
        }
        RunFailure::Replay(error) => Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error("run.replay_differs", error.to_string()),
        ),
        other => Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error("run.engine", other.to_string()),
        ),
    }
}

/// A failed check on a finished run: exit code 2, with the last events.
fn run_failed(type_name: &str, message: String, fold: &Fold) -> Failure {
    let mut line = Diagnostic::error(type_name, message).at_time(fold.last_event);
    line.items_alive = Some(fold.items_alive());
    line.recent.clone_from(&fold.recent);
    Failure::one(FailureClass::RunFailed, line)
}

/// A trace at `path` that cannot be read: a file that failed (exit code 7), not a
/// run that diverged (exit code 2), since no run writes such a trace.
fn unreadable_trace(path: &Path, error: &dyn std::fmt::Display) -> Failure {
    let mut line =
        Diagnostic::error("store.read", format!("{}: {error}", path.display())).at("--store");
    line.file = Some(path.display().to_string());
    Failure::one(FailureClass::Io, line)
}

/// The failure of a trace that cannot be folded.
fn from_fold_error(
    error: FoldError,
    unreadable: impl Fn(&taktsim_trace::ReadError) -> Failure,
) -> Failure {
    match error {
        FoldError::Read(error) => unreadable(&error),
        FoldError::Kpi {
            type_name,
            message,
            last_event,
            recent,
        } => {
            let mut line = Diagnostic::error(type_name, message).at_time(last_event);
            line.recent = recent;
            Failure::one(FailureClass::RunFailed, line)
        }
    }
}

/// Folds the trace at `path`.
fn fold_trace<D: Domain>(path: &Path, places: &Places) -> Result<Fold, Failure> {
    let file = File::open(path).map_err(|e| io_failure("store.read", path, &e))?;
    let unreadable = |e: &taktsim_trace::ReadError| unreadable_trace(path, e);
    let reader =
        TraceReader::<_, D::Message>::new(BufReader::new(file)).map_err(|e| unreadable(&e))?;
    metrics::fold::<_, D>(reader, places).map_err(|e| from_fold_error(e, unreadable))
}

/// The metrics of a trace, however it was written: the fold of a run, with no engine
/// (ADR-0025 §1). A trace that states no seed says that the model draws nothing. The
/// second list is what the model declares and these metrics do not answer.
///
/// # Errors
///
/// A [`Failure`]: `Io` if the trace cannot be read, `RunFailed` if it states something
/// impossible (`run.kpi_conservation`), a state its type does not declare
/// (`run.kpi_undeclared_state`) or no end (`run.kpi_no_end`).
///
/// ```
/// use taktsim_components::RunOptions;
/// use taktsim_model::{Format, Loader};
///
/// let registry = taktsim_components::standard()?;
/// let model = Loader::new(registry.catalog()).load_str(
///     "format: \"1.0\"\nmodel: {name: m, seed: 1}\nexperiment: {type: terminating, horizon: 1h}\n\
///      components: [{id: source, type: Source, interarrival: 1min}, {id: sink, type: Sink}]\n\
///      flow: [[source, sink]]\n",
///     Format::Yaml,
/// )?;
/// let (trace, _) = registry.prepare(&model, RunOptions::new())?.run_traced(Vec::new())?;
/// let (metrics, unreported) = taktsim_run::metrics_of_trace(&registry, &model, trace.as_slice())?;
/// assert!(metrics.iter().any(|m| m.name == "throughput(sink)"));
/// assert!(unreported.is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn metrics_of_trace<D: Domain, R: Read>(
    registry: &Registry<D>,
    model: &Model,
    trace: R,
) -> Result<(Vec<Metric>, Vec<Unreported>), Failure> {
    let unreadable = |e: &taktsim_trace::ReadError| {
        Failure::one(
            FailureClass::Io,
            Diagnostic::error("store.read", format!("the trace: {e}")).at("<trace>"),
        )
    };
    let document = model.document();
    let reader = TraceReader::<_, D::Message>::new(trace).map_err(|e| unreadable(&e))?;
    let seed = reader.header().seed;
    let places = Places::of(registry, document);
    let fold =
        metrics::fold::<_, D>(reader, &places).map_err(|e| from_fold_error(e, unreadable))?;
    let end = SimDuration::from_nanos(fold.end_time().as_nanos());
    let metrics = run_metrics(registry, document, &fold, seed, end);
    let unreported = unreported(document, &metrics);
    Ok((metrics, unreported))
}

/// What can be said of a run with this seed (ADR-0024 §4).
fn run_accuracy(seed: Option<u64>) -> Accuracy {
    if seed.is_some() {
        Accuracy::none(
            "single_replication",
            "one replication has no interval; run several seeds",
        )
    } else {
        Accuracy::none(
            "no_random_draws",
            "the model draws nothing, so the number is exact for this model; it says nothing about how well the model fits reality",
        )
    }
}

/// The metrics of one fold.
fn run_metrics<D: Domain>(
    registry: &Registry<D>,
    document: &Document,
    fold: &Fold,
    seed: Option<u64>,
    end: SimDuration,
) -> Vec<Metric> {
    let accuracy = run_accuracy(seed);
    let specs = metrics::kpi_specs(registry, document);
    let targets = metrics::targets(document);
    // With no seed there is no interval to compare: the target is stated, and `reached`
    // left out (ADR-0025 §5).
    let stated = seed.is_none().then_some(&targets);
    fold.metrics(document, registry.roles(), &specs, end, (&accuracy, stated))
}

/// Runs into the temporary directory, returning the report and its trace's summary.
fn write_trace<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    options: RunOptions<'_>,
    temp: &TempRun,
) -> Result<(RunReport, Summary), Failure> {
    let document = model.document();
    let prepared = registry
        .prepare(model, options)
        .map_err(|e| from_run_failure(e, document))?;
    let path = temp.file(TRACE_FILE);
    let file = File::create(&path).map_err(|e| io_failure("store.write", &path, &e))?;
    let (mut out, report) = prepared
        .run_traced(BufWriter::new(file))
        .map_err(|e| from_run_failure(e, document))?;
    out.flush()
        .map_err(|e| io_failure("store.write", &path, &e))?;
    let written = report.trace.ok_or_else(|| {
        Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error("internal.no_trace", "a traced run reported no trace"),
        )
    })?;
    Ok((report, written))
}

/// The check run (ADR-0024 §5): a run that claims to draw nothing and reached its
/// end is replayed against an engine with a different internal seed, before it is
/// stored. It asks what matters: whether the trace depends on the seed.
fn check_no_draw_claim<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    trace: &Path,
    fold: &Fold,
) -> Result<(), Failure> {
    let file = File::open(trace).map_err(|e| io_failure("store.read", trace, &e))?;
    let checked = registry
        .prepare(model, RunOptions::new().no_seed_check())
        .and_then(|prepared| prepared.replay(BufReader::new(file)));
    match checked {
        Ok(_) => Ok(()),
        Err(RunFailure::Replay(error)) if matches!(*error, ReplayError::Trace(_)) => {
            Err(unreadable_trace(trace, &error))
        }
        Err(error) => Err(run_failed(
            "run.no_draw_claim_false",
            format!(
                "this model claims to draw nothing, but another seed gives another run: {error}"
            ),
            fold,
        )),
    }
}

/// Everything about a finished run that goes into its result.
struct Finished<'a> {
    id: Option<&'a RunId>,
    seed: Option<u64>,
    stop_reason: &'static str,
    complete: bool,
    report: &'a RunReport,
    written: &'a Summary,
    fold: &'a Fold,
}

/// The result of a run.
fn result_document<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    finished: &Finished<'_>,
) -> Result<ResultDocument, Failure> {
    let Finished {
        id,
        seed,
        stop_reason,
        complete,
        report,
        written,
        fold,
    } = finished;
    let document = model.document();
    let end = SimDuration::from_nanos(report.summary.now.as_nanos());
    let experiment_id = id
        .map(|id| ExperimentId::over([(*id).clone()]))
        .transpose()
        .map_err(|e| {
            Failure::one(
                FailureClass::RunFailed,
                Diagnostic::error("internal.experiment_id", e.to_string()),
            )
        })?;
    let trace = id.map(|id| TraceRef {
        path: format!("runs/{id}/{TRACE_FILE}"),
        bytes: written.bytes,
        trace_format: u32::from(FORMAT_VERSION),
        record_format: RECORD_FORMAT.to_owned(),
        message_format: <D::Message as TraceMessage>::FORMAT.to_owned(),
        stream_hash: written.hash,
    });
    let metrics = run_metrics(registry, document, fold, *seed, end);
    Ok(ResultDocument {
        complete: *complete,
        experiment_id,
        stop_reason: (*stop_reason).to_owned(),
        provenance: Provenance {
            engine: ENGINE_NAME.to_owned(),
            engine_version: ENGINE_VERSION.to_owned(),
            package: D::PACKAGE.to_owned(),
            package_version: D::PACKAGE_VERSION.to_owned(),
            model_hash: model.hash(),
            system_hash: model.system_hash(),
            platform: platform(),
            seeds: seed.iter().copied().collect(),
        },
        runs: vec![RunEntry {
            complete: *complete,
            stop_reason: (*stop_reason).to_owned(),
            end_time: end,
            events: written.events,
            records: written.records,
            run_id: id.map(|id| (*id).clone()),
            seed: *seed,
            trace,
        }],
        unreported: unreported(document, &metrics),
        metrics,
    })
}

/// Runs a model into the store and returns its result (ADR-0024 §3, §8).
///
/// A run that reached its end is checked, kept and named by its run id. A run
/// stopped by a limit has `complete: false`, is not kept and has no id; it is still
/// returned. A stored run is returned without running, unless the request says not
/// to reuse it; then the new run's trace and result are compared with it, it is
/// never overwritten, and on a match its stored bytes are returned.
///
/// # Errors
///
/// A [`Failure`]: `Invalid` if the model does not load or has no seed it needs,
/// `RunFailed` if the run breaks an invariant, stalls, differs from the stored run
/// or its claim of drawing nothing is false, `Io` if a file or the store fails.
/// Nothing is stored after any of these.
///
/// ```
/// use taktsim_model::Format;
/// use taktsim_run::{ModelSource, RunRequest, Store, run};
///
/// let registry = taktsim_components::standard()?;
/// let dir = std::env::temp_dir().join(format!("taktsim-doc-run-{}", std::process::id()));
/// let store = Store::open(&dir)?;
/// let model = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
///              components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
///              flow: [[source, cut, sink]]\n";
/// let source = ModelSource::Text(model, Format::Yaml);
///
/// // Draws nothing, so no seed.
/// let exact = run(&registry, source, &RunRequest::new(), &store)?;
/// assert!(exact.result.provenance.seeds.is_empty());
/// assert!(exact.result.runs[0].run_id.is_some());
///
/// // Stopped by a limit: partial, not kept, no id.
/// let mut limited = RunRequest::new();
/// limited.max_events = Some(1);
/// limited.reuse = false;
/// let partial = run(&registry, source, &limited, &store)?;
/// assert!(!partial.result.complete);
/// assert_eq!(partial.result.stop_reason, "event_limit");
/// assert_eq!(partial.result.experiment_id, None);
/// # std::fs::remove_dir_all(&dir)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn run<D: Domain>(
    registry: &Registry<D>,
    source: ModelSource<'_>,
    request: &RunRequest<'_>,
    store: &Store,
) -> Result<RunResponse, Failure> {
    let model = source
        .load(registry)
        .map_err(|errors| Failure::from_load(&errors))?;
    let model = match request.horizon {
        Some(horizon) => model
            .with_horizon(horizon)
            .map_err(|errors| horizon_refused(&errors))?,
        None => model,
    };
    let document = model.document();
    let mode = seeds::plan(registry.draws_random(document), request, document)?;
    match mode {
        Mode::One(seeding) => {
            let one = run_one(registry, &model, seeding.seed, request, store)?;
            let ended = match one.partial {
                None | Some(RunOutcome::Horizon | RunOutcome::Idle) => Ended::Complete,
                Some(RunOutcome::EventLimit) => Ended::EventLimit,
                Some(RunOutcome::Cancelled) => Ended::Cancelled,
            };
            Ok(RunResponse {
                result: one.result,
                reused: one.reused,
                warnings: seeding.warnings,
                ended,
                progress: None,
            })
        }
        Mode::List { seeds } => {
            experiment::run_experiment(registry, &model, SeedList::List(&seeds), request, store)
        }
        Mode::Sequence { first, max_runs } => experiment::run_experiment(
            registry,
            &model,
            SeedList::Sequence {
                first,
                count: max_runs,
            },
            request,
            store,
        ),
    }
}

/// One run's result, and how it ended.
#[derive(Debug)]
pub(crate) struct One {
    pub(crate) result: ResultDocument,
    /// The result as a document: the bytes in the store for a run that is kept or
    /// reused (ADR-0024 §3).
    pub(crate) text: String,
    pub(crate) reused: bool,
    /// How a partial run stopped; `None` for one that reached its end.
    pub(crate) partial: Option<RunOutcome>,
}

/// A run that reached its end and waits in `tmp/` to be committed (ADR-0025 §8).
/// Dropped, it leaves nothing behind.
#[derive(Debug)]
pub(crate) struct Ready {
    temp: TempRun,
    id: RunId,
    written: Summary,
    pub(crate) result: ResultDocument,
    /// What one replication like this takes in memory, in bytes.
    pub(crate) memory: u64,
}

/// What running one replication gave, before anything is committed.
#[derive(Debug)]
pub(crate) enum Worked {
    /// It reached its end: commit it to keep it.
    Ready(Box<Ready>),
    /// A limit or the cancel flag stopped it: it is not kept.
    Partial(Box<One>),
}

/// The stored run of `model` with `seed`, if the request reuses one.
fn stored<D: Domain>(
    model: &Model,
    seed: Option<u64>,
    request: &RunRequest<'_>,
    store: &Store,
) -> Result<Option<One>, Failure> {
    if !request.reuse {
        return Ok(None);
    }
    let id = run_id::<D>(model, seed)?;
    Ok(store.read_result(&id)?.map(|result| One {
        result,
        reused: true,
        partial: None,
    }))
}

/// One run of `model` with `seed`, from the store or run and kept (ADR-0024 §3, §8).
fn run_one<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    seed: Option<u64>,
    request: &RunRequest<'_>,
    store: &Store,
) -> Result<One, Failure> {
    if let Some(one) = stored::<D>(model, seed, request, store)? {
        return Ok(one);
    }
    match work_one(registry, model, seed, request, request.cancel, store)? {
        Worked::Partial(one) => Ok(*one),
        Worked::Ready(ready) => Ok(One {
            result: commit(store, *ready)?,
            reused: false,
            partial: None,
        }),
    }
}

/// Runs `model` with `seed` into `tmp/`, folds it and writes its result there; a
/// worker's part (ADR-0025 §8). `cancel` cuts it.
fn work_one<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    seed: Option<u64>,
    request: &RunRequest<'_>,
    cancel: Option<&AtomicBool>,
    store: &Store,
) -> Result<Worked, Failure> {
    let document = model.document();
    let id = run_id::<D>(model, seed)?;
    let temp = store.begin()?;
    let model_path = temp.file(MODEL_FILE);
    std::fs::write(&model_path, model.canonical_json())
        .map_err(|e| io_failure("store.write", &model_path, &e))?;
    let (report, written) = write_trace(registry, model, options(request, seed, cancel), &temp)?;
    let trace_path = temp.file(TRACE_FILE);
    let places = Places::of(registry, document);
    let fold = fold_trace::<D>(&trace_path, &places)?;
    if let Some((what, sum, counter)) = fold.differs_from(&report.items) {
        return Err(run_failed(
            "run.metrics_differ",
            format!("the trace says {sum} items {what}, the engine counted {counter}"),
            &fold,
        ));
    }
    let (stop_reason, complete) = match ending::ending(
        report.summary.outcome,
        fold.items_alive(),
        report.wait_never_ends,
    ) {
        Ending::Ended {
            stop_reason,
            complete,
        } => (stop_reason, complete),
        Ending::Stalled => {
            return Err(run_failed(
                "run.stalled",
                format!(
                    "no event is left and {} items are still in the system: a lost wake-up, or a broken component type",
                    fold.items_alive()
                ),
                &fold,
            ));
        }
    };
    let finished = |id| Finished {
        id,
        seed,
        stop_reason,
        complete,
        report: &report,
        written: &written,
        fold: &fold,
    };
    if !complete {
        return Ok(Worked::Partial(Box::new(One {
            result: result_document(registry, model, &finished(None))?,
            reused: false,
            partial: Some(report.summary.outcome),
        })));
    }
    if seed.is_none() {
        check_no_draw_claim(registry, model, &trace_path, &fold)?;
    }
    let result = result_document(registry, model, &finished(Some(&id)))?;
    let text = result.to_text();
    let result_path = temp.file(RESULT_FILE);
    std::fs::write(&result_path, &text).map_err(|e| io_failure("store.write", &result_path, &e))?;
    Ok(Worked::Ready(Box::new(Ready {
        memory: fold.memory_estimate(),
        temp,
        id,
        written,
        result,
    })))
}

/// Renames a finished run into the store: the coordinator's part. A run with its id
/// already there is compared and never overwritten.
fn commit(store: &Store, ready: Ready) -> Result<ResultDocument, Failure> {
    match store.commit(&ready.temp, &ready.id)? {
        Commit::Stored => Ok(ready.result),
        Commit::AlreadyThere => compare_with_stored(store, &ready.id, &ready.written),
    }
}

/// The options of a run with this seed, cut by `cancel`.
fn options<'a>(
    request: &RunRequest<'_>,
    seed: Option<u64>,
    cancel: Option<&'a AtomicBool>,
) -> RunOptions<'a> {
    let mut options = match seed {
        Some(seed) => RunOptions::new().seed(seed),
        None => RunOptions::new().no_seed(),
    };
    if let Some(max_events) = request.max_events {
        options = options.max_events(max_events);
    }
    if let Some(cancel) = cancel {
        options = options.cancel_on(cancel);
    }
    options
}

/// A failure of a stored run whose files disagree: exit code 2.
fn stored_run_differs(what: String) -> Failure {
    Failure::one(
        FailureClass::RunFailed,
        Diagnostic::error("run.stored_run_differs", what).at("--store"),
    )
}

/// A run's id is computed from what it ran, so its result can be checked against the
/// directory it is filed under (spec §5, ADR-0024 §6). A result that says or computes
/// to another id, states hashes other than `model`'s, names a trace other than the
/// one filed beside it, or holds more than its one run, is not this run's, and must
/// not be returned as this run.
fn check_filed_under(result: &ResultDocument, id: &RunId, model: &Model) -> Result<(), Failure> {
    let disagrees = |what: &str| {
        stored_run_differs(format!(
            "the result filed under run {id} is not that run's: {what}"
        ))
    };
    let provenance = &result.provenance;
    let [run] = result.runs.as_slice() else {
        return Err(disagrees(if result.runs.is_empty() {
            "it names no run"
        } else {
            "it states more than its one run"
        }));
    };
    if run.run_id.as_ref() != Some(id) {
        return Err(disagrees("it states another run id"));
    }
    // A partial run is never stored (ADR-0024 §8).
    if !result.complete || !run.complete {
        return Err(disagrees("it says it is partial"));
    }
    let experiment = ExperimentId::over([id.clone()]).map_err(|e| {
        Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error("internal.experiment_id", e.to_string()),
        )
    })?;
    if result.experiment_id.as_ref() != Some(&experiment) {
        return Err(disagrees("it states another experiment id than its run's"));
    }
    if provenance.seeds.as_slice() != run.seed.as_slice() {
        return Err(disagrees("its provenance states another seed than its run"));
    }
    if provenance.model_hash != model.hash() || provenance.system_hash != model.system_hash() {
        return Err(disagrees("it does not state the hashes of its model"));
    }
    let Some(trace) = &run.trace else {
        return Err(disagrees("it names no trace"));
    };
    if trace.path != format!("runs/{id}/{TRACE_FILE}") {
        return Err(disagrees(
            "it names another trace than the one filed with it",
        ));
    }
    let inputs = RunIdInputs {
        engine: &provenance.engine,
        engine_version: &provenance.engine_version,
        record_format: &trace.record_format,
        package: &provenance.package,
        package_version: &provenance.package_version,
        message_format: &trace.message_format,
        model_hash: provenance.model_hash,
        seed: run.seed,
    };
    if RunId::of(&inputs).as_ref() != Ok(id) {
        return Err(disagrees("what it states computes to another id"));
    }
    Ok(())
}

/// A run with this id is in the store already: it must be the run just made. Its
/// stream must be the same, or the engine changed or determinism broke; and so must
/// everything its result states but `provenance.platform`, or the stored numbers
/// are not this run's (ADR-0024 §8). The stored bytes are returned, so a key this
/// build does not read is kept.
fn compare_with_stored(
    store: &Store,
    id: &RunId,
    model: &Model,
    result: &ResultDocument,
    written: &Summary,
) -> Result<StoredResult, Failure> {
    let stored = store.read_result(id)?;
    if let Some(stored) = &stored {
        check_filed_under(&stored.document, id, model)?;
    }
    let stored_hash = stored
        .iter()
        .flat_map(|stored| &stored.document.runs)
        .find_map(|run| run.trace.as_ref())
        .map(|trace| trace.stream_hash);
    let stored = match stored {
        Some(stored) if stored_hash == Some(written.hash) => stored,
        Some(_) | None => {
            return Err(Failure::one(
                FailureClass::RunFailed,
                Diagnostic::error(
                    "run.store_differs",
                    format!(
                        "run {id} is in the store with another trace: the engine changed or determinism broke"
                    ),
                )
                .at("--no-reuse")
                .found(written.hash.to_string())
                .allowed(
                    stored_hash.map_or_else(|| "no stored hash".to_owned(), |h| h.to_string()),
                ),
            ));
        }
    };
    let mut made = result.clone();
    made.provenance
        .platform
        .clone_from(&stored.document.provenance.platform);
    let (made, kept) = (made.encode(), stored.document.encode());
    let differs = made
        .as_object()
        .into_iter()
        .flatten()
        .find(|(key, value)| kept[key.as_str()] != **value);
    if let Some((key, value)) = differs {
        return Err(Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error(
                "run.store_differs",
                format!(
                    "run {id} is in the store with the same trace but another `{key}`: the stored result is not what this run gives"
                ),
            )
            .at("--no-reuse")
            .found(value.to_string())
            .allowed(kept[key.as_str()].to_string()),
        ));
    }
    Ok(stored)
}

/// The header of the stored trace at `path` must name the stored model and the
/// result's seed (ADR-0024 §1). Replay compares the records, which two models
/// that differ only in, say, their name share, and it skips the seed of a trace
/// that states none; so a trace copied from another run would replay.
fn check_trace_header<D: Domain>(
    path: &Path,
    model: &Model,
    seed: Option<u64>,
    id: &RunId,
) -> Result<(), Failure> {
    let file = File::open(path).map_err(|e| io_failure("store.read", path, &e))?;
    let reader = TraceReader::<_, D::Message>::new(BufReader::new(file))
        .map_err(|e| unreadable_trace(path, &e))?;
    let header = reader.header();
    let seed_text =
        |seed: Option<u64>| seed.map_or_else(|| "no seed".to_owned(), |s| s.to_string());
    let differs = if header.model_hash.as_bytes() != model.hash().as_bytes() {
        Some((
            "model hash",
            header.model_hash.to_string(),
            model.hash().to_string(),
        ))
    } else if header.system_hash.as_bytes() != model.system_hash().as_bytes() {
        Some((
            "system hash",
            header.system_hash.to_string(),
            model.system_hash().to_string(),
        ))
    } else if header.seed != seed {
        Some(("seed", seed_text(header.seed), seed_text(seed)))
    } else {
        None
    };
    let Some((what, found, allowed)) = differs else {
        return Ok(());
    };
    Err(Failure::one(
        FailureClass::RunFailed,
        Diagnostic::error(
            "run.stored_run_differs",
            format!(
                "the trace of run {id} states another {what} than its stored model and result: it is another run's trace"
            ),
        )
        .at("--store")
        .found(found)
        .allowed(allowed),
    ))
}

/// Replays the stored run `id` against this registry, after checking the stored
/// model against the trace header's hashes and the result's seed, the stored model
/// against the result's hashes, the running package against the result's
/// provenance and the result against the id it is filed under; the replay's stream
/// hash, records and events must then be what the result states. A run with no seed
/// is replayed under a different internal seed, so a false claim of drawing nothing
/// diverges (ADR-0024 §1, §5).
///
/// # Errors
///
/// A [`Failure`]: `Invalid` for an id that is not in the store, `Io` if a stored
/// file cannot be read, or does not read as what it is (a damaged trace, a model
/// this build does not load), `RunFailed` if the stored files disagree, the package
/// differs or the replay diverges.
///
/// ```
/// use taktsim_model::Format;
/// use taktsim_run::{ModelSource, RunRequest, Store, replay_stored, run};
///
/// let registry = taktsim_components::standard()?;
/// let dir = std::env::temp_dir().join(format!("taktsim-doc-replay-{}", std::process::id()));
/// let store = Store::open(&dir)?;
/// let model = "format: \"1.0\"\nmodel: {name: m, seed: 1}\nexperiment: {type: terminating, horizon: 1h}\n\
///              components: [{id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}, {id: sink, type: Sink}]\n\
///              flow: [[source, sink]]\n";
/// let response = run(&registry, ModelSource::Text(model, Format::Yaml), &RunRequest::new(), &store)?;
/// let id = response.result.runs[0].run_id.as_ref().ok_or("kept")?;
/// let replayed = replay_stored(&registry, &store, id)?;
/// assert_eq!(Some(replayed.stream_hash), response.result.runs[0].trace.as_ref().map(|t| t.stream_hash));
/// # std::fs::remove_dir_all(&dir)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn replay_stored<D: Domain>(
    registry: &Registry<D>,
    store: &Store,
    id: &RunId,
) -> Result<ReplaySummary, Failure> {
    let Some(StoredResult {
        document: result, ..
    }) = store.read_result(id)?
    else {
        return Err(Failure::one(
            FailureClass::Invalid,
            Diagnostic::error("run.unknown_id", format!("no run {id} in the store"))
                .at("<run-id>")
                .found(id.to_string())
                .allowed("the id of a run in the store")
                .example(<RunId as taktsim_model::Decode>::EXAMPLE),
        ));
    };
    let provenance = &result.provenance;
    if provenance.package != D::PACKAGE || provenance.package_version != D::PACKAGE_VERSION {
        return Err(stored_run_differs(format!(
            "run {id} came from package {} {}, this is {} {}; replay it with the package that wrote it",
            provenance.package,
            provenance.package_version,
            D::PACKAGE,
            D::PACKAGE_VERSION
        )));
    }
    let model_path = store.run_file(id, MODEL_FILE);
    let text = std::fs::read_to_string(&model_path)
        .map_err(|e| io_failure("store.read", &model_path, &e))?;
    let model = Loader::new(registry.catalog())
        .load_str(&text, Format::Json)
        .map_err(|errors| {
            // The caller gave an id, not this file: the fault is in the store.
            let mut line = Diagnostic::error(
                "store.read",
                format!("{} is not a model this build loads", model_path.display()),
            )
            .at("--store")
            .found(errors.first().to_string())
            .allowed("the canonical model this build stored with the run");
            line.file = Some(model_path.display().to_string());
            Failure::one(FailureClass::Io, line)
        })?;
    check_filed_under(&result, id, &model)?;
    let trace_path = store.run_file(id, TRACE_FILE);
    let seed = result.runs.first().and_then(|run| run.seed);
    let document = model.document();
    check_trace_header::<D>(&trace_path, &model, seed, id)?;
    let file = File::open(&trace_path).map_err(|e| io_failure("store.read", &trace_path, &e))?;
    let options = seed.map_or_else(
        || RunOptions::new().no_seed_check(),
        |seed| RunOptions::new().seed(seed),
    );
    let replayed = registry
        .prepare(&model, options)
        .and_then(|prepared| prepared.replay(BufReader::new(file)))
        .map_err(|e| {
            if let RunFailure::Replay(error) = &e
                && let ReplayError::Trace(read) = &**error
            {
                return unreadable_trace(&trace_path, read);
            }
            from_run_failure(e, document)
        })?;
    let stated = result.runs.first().and_then(|run| {
        let trace = run.trace.as_ref()?;
        Some((trace.stream_hash, run.records, run.events))
    });
    if stated != Some((replayed.hash, replayed.records, replayed.events)) {
        return Err(stored_run_differs(format!(
            "the result of run {id} does not state the stream hash, records and events of its trace"
        )));
    }
    Ok(ReplaySummary {
        records: replayed.records,
        events: replayed.events,
        stream_hash: replayed.hash,
    })
}

#[cfg(test)]
mod tests {
    use taktsim_components::Flow;

    use super::*;

    /// A check run that cannot read the trace is a file that failed (exit 7), not a
    /// model that lies about drawing nothing (exit 2): only a damaged trace reaches
    /// that arm, and no run writes one.
    #[test]
    fn a_trace_the_check_run_cannot_read_is_an_io_failure() {
        let registry = taktsim_components::standard().unwrap();
        let model = Loader::new(registry.catalog())
            .load_str(
                "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                 components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
                 flow: [[source, cut, sink]]\n",
                Format::Yaml,
            )
            .unwrap();
        let (trace, _) = registry
            .prepare(&model, RunOptions::new().no_seed())
            .unwrap()
            .run_traced(Vec::new())
            .unwrap();
        let dir =
            std::env::temp_dir().join(format!("taktsim-run-check-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(TRACE_FILE);
        std::fs::write(&path, &trace).unwrap();
        let fold = fold_trace::<Flow>(&path).unwrap();
        assert!(check_no_draw_claim(&registry, &model, &path, &fold).is_ok());

        std::fs::write(&path, &trace[..trace.len() / 2]).unwrap();
        let failure = check_no_draw_claim(&registry, &model, &path, &fold).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(failure.class, FailureClass::Io);
        assert_eq!(failure.diagnostics[0].type_name, "store.read");
        assert_eq!(failure.diagnostics[0].path.as_deref(), Some("--store"));
    }
}
