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
mod id;
mod metrics;
mod result;
mod schemas;
mod seeds;
mod store;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;

use taktsim_components::{Domain, Registry, RunFailure, RunOptions, RunReport};
use taktsim_core::{ENGINE_NAME, ENGINE_VERSION};
use taktsim_model::{Document, Format, LoadErrors, Loader, Model};
use taktsim_trace::{
    FORMAT_VERSION, RECORD_FORMAT, ReplayError, Summary, TraceHash, TraceMessage, TraceReader,
    WriteError,
};
use taktsim_units::SimDuration;

pub use diagnostic::{Diagnostic, DiagnosticKind, Failure, FailureClass};
pub use id::{ExperimentId, ParseIdError, RunId, RunIdInputs};
pub use result::{
    Accuracy, AccuracyKind, AccuracyTarget, Metric, MetricValue, Over, Provenance, RESULT_FORMAT,
    ResultDocument, RunEntry, SamplingInterval, Scalar, TraceRef, Unreported, VALIDATION_FORMAT,
    ValidationReport, diagnostic_schema, result_schema, validation_schema,
};
pub use schemas::{SCHEMA_NAMES, schema_named, schema_names};
pub use store::Store;

use crate::ending::Ending;
use crate::metrics::Fold;
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
/// assert_eq!(request.seed, None);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RunRequest<'a> {
    /// The seed; `None` takes the model's (ADR-0024 §5).
    pub seed: Option<u64>,
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
            seed: None,
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
/// fn exit_code(response: &taktsim_run::RunResponse) -> u8 {
///     if response.result.complete { 0 } else { 3 }
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

/// Each declared observable that this result does not hold: M7 computes none.
fn unreported(document: &Document) -> Vec<Unreported> {
    let declared: BTreeSet<&str> = document
        .observe()
        .iter()
        .map(taktsim_model::Observable::text)
        .chain(
            document
                .experiment()
                .precision()
                .iter()
                .map(|p| p.observable().text()),
        )
        .collect();
    declared
        .into_iter()
        .map(|observable| Unreported {
            observable: observable.to_owned(),
            reason: "not_computed".to_owned(),
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

/// Folds the trace at `path`.
fn fold_trace<D: Domain>(path: &Path) -> Result<Fold, Failure> {
    let file = File::open(path).map_err(|e| io_failure("store.read", path, &e))?;
    let unreadable = |e: taktsim_trace::ReadError| {
        Failure::one(
            FailureClass::Io,
            Diagnostic::error("store.read", format!("{}: {e}", path.display())).at("--store"),
        )
    };
    let reader = TraceReader::<_, D::Message>::new(BufReader::new(file)).map_err(unreadable)?;
    metrics::fold(reader).map_err(unreadable)
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
            Err(Failure::one(
                FailureClass::Io,
                Diagnostic::error("store.read", error.to_string()).at("--store"),
            ))
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
    let accuracy = if seed.is_some() {
        Accuracy::none(
            "single_replication",
            "one replication has no interval; run several seeds",
        )
    } else {
        Accuracy::none(
            "no_random_draws",
            "the model draws nothing, so the number is exact for this model; it says nothing about how well the model fits reality",
        )
    };
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
        metrics: fold.metrics(document, registry.roles(), end, &accuracy),
        unreported: unreported(document),
    })
}

/// Runs a model into the store and returns its result (ADR-0024 §3, §8).
///
/// A run that reached its end is checked, kept and named by its run id. A run
/// stopped by a limit has `complete: false`, is not kept and has no id; it is still
/// returned. A stored run is returned without running, unless the request says not
/// to reuse it; then the new trace is compared with it, and it is never overwritten.
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
    let seeding = seeds::effective(
        registry.draws_random(document),
        request.seed,
        document.model().seed(),
    )?;
    let id = run_id::<D>(&model, seeding.seed)?;
    if request.reuse
        && let Some(stored) = store.read_result(&id)?
    {
        check_filed_under(&stored.document, &id)?;
        return Ok(RunResponse {
            result: stored.document,
            text: stored.text,
            reused: true,
            warnings: seeding.warnings,
        });
    }

    let temp = store.begin()?;
    let model_path = temp.file(MODEL_FILE);
    std::fs::write(&model_path, model.canonical_json())
        .map_err(|e| io_failure("store.write", &model_path, &e))?;
    let (report, written) = write_trace(registry, &model, options(request, seeding.seed), &temp)?;
    let trace_path = temp.file(TRACE_FILE);
    let fold = fold_trace::<D>(&trace_path)?;
    if let Some((what, sum, counter)) = fold.differs_from(&report.items) {
        return Err(run_failed(
            "run.metrics_differ",
            format!("the trace says {sum} items {what}, the engine counted {counter}"),
            &fold,
        ));
    }
    let (stop_reason, complete) = match ending::ending(report.summary.outcome, fold.items_alive()) {
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
        seed: seeding.seed,
        stop_reason,
        complete,
        report: &report,
        written: &written,
        fold: &fold,
    };
    if !complete {
        let result = result_document(registry, &model, &finished(None))?;
        return Ok(RunResponse {
            text: result.to_text(),
            result,
            reused: false,
            warnings: seeding.warnings,
        });
    }
    if seeding.seed.is_none() {
        check_no_draw_claim(registry, &model, &trace_path, &fold)?;
    }
    let result = result_document(registry, &model, &finished(Some(&id)))?;
    let text = result.to_text();
    let result_path = temp.file(RESULT_FILE);
    std::fs::write(&result_path, &text).map_err(|e| io_failure("store.write", &result_path, &e))?;
    let stored = match store.commit(&temp, &id)? {
        Commit::Stored => StoredResult {
            document: result,
            text,
        },
        Commit::AlreadyThere => compare_with_stored(store, &id, &written)?,
    };
    Ok(RunResponse {
        result: stored.document,
        text: stored.text,
        reused: false,
        warnings: seeding.warnings,
    })
}

/// The options of a run with this seed.
fn options<'a>(request: &RunRequest<'a>, seed: Option<u64>) -> RunOptions<'a> {
    let mut options = match seed {
        Some(seed) => RunOptions::new().seed(seed),
        None => RunOptions::new().no_seed(),
    };
    if let Some(max_events) = request.max_events {
        options = options.max_events(max_events);
    }
    if let Some(cancel) = request.cancel {
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
/// to another id is not this run's, and must not be returned as this run.
fn check_filed_under(result: &ResultDocument, id: &RunId) -> Result<(), Failure> {
    let disagrees = |what: &str| {
        stored_run_differs(format!(
            "the result filed under run {id} is not that run's: {what}"
        ))
    };
    let provenance = &result.provenance;
    let Some(run) = result.runs.first() else {
        return Err(disagrees("it names no run"));
    };
    if run.run_id.as_ref() != Some(id) {
        return Err(disagrees("it states another run id"));
    }
    if provenance.seeds.as_slice() != run.seed.as_slice() {
        return Err(disagrees("its provenance states another seed than its run"));
    }
    let Some(trace) = &run.trace else {
        return Err(disagrees("it names no trace"));
    };
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

/// A run with this id is in the store already: its stream must be the same. A
/// difference means the engine changed or determinism broke.
fn compare_with_stored(
    store: &Store,
    id: &RunId,
    written: &Summary,
) -> Result<StoredResult, Failure> {
    let stored = store.read_result(id)?;
    if let Some(stored) = &stored {
        check_filed_under(&stored.document, id)?;
    }
    let stored_hash = stored
        .iter()
        .flat_map(|stored| &stored.document.runs)
        .find_map(|run| run.trace.as_ref())
        .map(|trace| trace.stream_hash);
    match stored {
        Some(stored) if stored_hash == Some(written.hash) => Ok(stored),
        Some(_) | None => Err(Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error(
                "run.store_differs",
                format!(
                    "run {id} is in the store with another trace: the engine changed or determinism broke"
                ),
            )
            .at("--no-reuse")
            .found(written.hash.to_string())
            .allowed(stored_hash.map_or_else(|| "no stored hash".to_owned(), |h| h.to_string())),
        )),
    }
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
    let reader = TraceReader::<_, D::Message>::new(BufReader::new(file)).map_err(|e| {
        from_run_failure(
            RunFailure::Replay(Box::new(ReplayError::Trace(e))),
            model.document(),
        )
    })?;
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
/// file cannot be read, `RunFailed` if the stored files disagree, the package
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
        .map_err(|errors| Failure::from_load(&errors))?;
    if model.hash() != provenance.model_hash || model.system_hash() != provenance.system_hash {
        return Err(stored_run_differs(format!(
            "the stored model of run {id} does not have the hashes its result states"
        )));
    }
    check_filed_under(&result, id)?;
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
        .map_err(|e| from_run_failure(e, document))?;
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
