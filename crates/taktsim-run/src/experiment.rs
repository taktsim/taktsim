//! Experiments over several seeds (ADR-0025 §4, §5, §8, §9). An experiment is a prefix
//! of its seed order: runs enter the statistics and the stopping decision strictly in
//! that order, so the document is a function of the counted runs, whatever finished
//! first and whatever thread ran what.

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use taktsim_components::{Domain, Kpi, Ports, Registry};
use taktsim_core::RunOutcome;
use taktsim_model::{Document, Model};
use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus, StatsError};
use taktsim_units::{Ratio, SimDuration};

use crate::diagnostic::{Diagnostic, Failure, FailureClass};
use crate::id::ExperimentId;
use crate::metrics::{self, KpiSpec, Labels, Targets};
use crate::result::{
    Accuracy, AccuracyKind, AccuracyPlan, Metric, MetricValue, Over, Provenance, ResultDocument,
    Scalar,
};
use crate::store::Store;
use crate::{One, RunRequest, RunResponse};

/// What a long command may spend (ADR-0019 item 4, ADR-0025 §4). Each is a flag; which
/// are required depends on what is asked, so each may be absent here.
///
/// ```
/// use std::num::{NonZeroU32, NonZeroU64};
/// use taktsim_run::Budget;
///
/// let budget = Budget { max_runs: NonZeroU32::new(100), max_seconds: NonZeroU64::new(600) };
/// assert_eq!(budget.max_runs.map(NonZeroU32::get), Some(100));
/// assert_eq!(Budget::none().max_seconds, None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Budget {
    /// The most replications.
    pub max_runs: Option<NonZeroU32>,
    /// The most seconds of wall-clock time. Only the length of the counted prefix
    /// depends on it, never its content.
    pub max_seconds: Option<NonZeroU64>,
}

impl Budget {
    /// No budget.
    ///
    /// ```
    /// assert_eq!(taktsim_run::Budget::none().max_runs, None);
    /// ```
    #[must_use]
    pub const fn none() -> Self {
        Self {
            max_runs: None,
            max_seconds: None,
        }
    }
}

/// How a command ended, typed, so an exit code never comes from the open word
/// `stop_reason` (ADR-0025 §5).
///
/// ```
/// use taktsim_run::Ended;
///
/// fn exit_code(ended: Ended) -> u8 {
///     match ended {
///         Ended::Complete => 0,
///         Ended::BudgetExhausted | Ended::EventLimit => 3,
///         Ended::Cancelled => 4,
///     }
/// }
/// assert_eq!(exit_code(Ended::Cancelled), 4);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Everything asked for finished: the list, or every target.
    Complete,
    /// `--max-runs` or `--max-seconds` was used up first.
    BudgetExhausted,
    /// A replication was stopped by `--max-events`.
    EventLimit,
    /// A signal, or the caller's flag.
    Cancelled,
}

/// How one replication ended, as the sequencer takes it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    /// It finished: its value on each target row, `None` where undefined.
    Values(Vec<Option<f64>>),
    /// `--max-events` stopped it.
    EventLimit,
    /// It failed.
    Failed,
}

/// Why an experiment stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Every seed of a list finished.
    SeedsFinished,
    /// Every target row reached its target.
    PrecisionReached,
    /// `--max-runs` ran under targets.
    BudgetExhausted,
    /// `--max-seconds` ran out.
    Clock,
    /// The replication at this index hit `--max-events`.
    EventLimit(u32),
    /// The replication at this index failed.
    Failed(u32),
    /// A signal.
    Cancelled,
}

/// What the sequencer says after a result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Step {
    /// The indices committed by this result, in order.
    pub(crate) committed: Vec<u32>,
    /// Every replication from this index on is not needed and may be cut.
    pub(crate) cancel_from: Option<u32>,
}

/// One target row's control. A row undefined in some replication never reaches its
/// target (ADR-0025 §5).
#[derive(Debug, Clone, PartialEq)]
struct RowControl {
    control: ReplicationControl,
    spoiled: bool,
}

/// The stopping decision, with no threads and no clock (ADR-0025 §8). It keeps the
/// value matrix, one `Option<f64>` per committed seed and target row; the controls are
/// an incremental form of a function of that matrix, which [`rebuild`](Self::rebuild)
/// replays.
#[derive(Debug, Clone)]
pub(crate) struct Sequencer {
    total: u32,
    sequential: bool,
    plans: Vec<ReplicationPlan>,
    min_replications: u32,
    matrix: Vec<Vec<Option<f64>>>,
    controls: Vec<RowControl>,
    pending: BTreeMap<u32, Outcome>,
    lowest_failure: Option<u32>,
    stop: Option<Stop>,
}

impl Sequencer {
    /// A sequencer of `total` seeds; with `sequential`, one control per target.
    pub(crate) fn new(
        targets: &[Ratio],
        total: u32,
        sequential: bool,
        min_replications: u32,
    ) -> Result<Self, StatsError> {
        let plans = if sequential {
            targets
                .iter()
                .map(|target| ReplicationPlan::new(*target, min_replications, total))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let mut sequencer = Self {
            total,
            sequential,
            plans,
            min_replications,
            matrix: Vec::new(),
            controls: Vec::new(),
            pending: BTreeMap::new(),
            lowest_failure: None,
            stop: None,
        };
        sequencer.rebuild_controls()?;
        Ok(sequencer)
    }

    /// Why it stopped, once it has.
    pub(crate) const fn stop(&self) -> Option<Stop> {
        self.stop
    }

    /// How many seeds are committed.
    pub(crate) fn committed(&self) -> u32 {
        u32::try_from(self.matrix.len()).unwrap_or(u32::MAX)
    }

    /// The committed values.
    pub(crate) fn matrix(&self) -> &[Vec<Option<f64>>] {
        &self.matrix
    }

    fn rebuild_controls(&mut self) -> Result<(), StatsError> {
        self.controls = self
            .plans
            .iter()
            .map(|plan| RowControl {
                control: ReplicationControl::new(*plan),
                spoiled: false,
            })
            .collect();
        let matrix = std::mem::take(&mut self.matrix);
        for row in &matrix {
            self.push_controls(row)?;
        }
        self.matrix = matrix;
        Ok(())
    }

    /// Replaces the matrix, as M9 does when a warm-up moves, and replays the controls.
    pub(crate) fn rebuild(&mut self, matrix: &[Vec<Option<f64>>]) -> Result<(), StatsError> {
        self.matrix = matrix.to_vec();
        self.rebuild_controls()?;
        self.stop = self.stop.or_else(|| self.decide());
        Ok(())
    }

    fn push_controls(&mut self, values: &[Option<f64>]) -> Result<(), StatsError> {
        for (row, control) in self.controls.iter_mut().enumerate() {
            match values.get(row).copied().flatten() {
                Some(value) => {
                    control.control.push(value)?;
                }
                None => control.spoiled = true,
            }
        }
        Ok(())
    }

    /// The decision after the committed seeds.
    fn decide(&self) -> Option<Stop> {
        let n = self.committed();
        if !self.sequential {
            return (n >= self.total).then_some(Stop::SeedsFinished);
        }
        let reached = n >= self.min_replications
            && self.controls.iter().all(|row| {
                !row.spoiled && row.control.status() == ReplicationStatus::PrecisionReached
            });
        if reached {
            Some(Stop::PrecisionReached)
        } else if n >= self.total {
            Some(Stop::BudgetExhausted)
        } else {
            None
        }
    }

    /// The replication at `index` finished with `outcome`.
    pub(crate) fn on_finished(&mut self, index: u32, outcome: Outcome) -> Result<Step, StatsError> {
        if self.stop.is_some() || index < self.committed() || index >= self.total {
            return Ok(Step {
                committed: Vec::new(),
                cancel_from: Some(self.committed()),
            });
        }
        if matches!(outcome, Outcome::EventLimit | Outcome::Failed) {
            self.lowest_failure = Some(self.lowest_failure.map_or(index, |low| low.min(index)));
        }
        self.pending.insert(index, outcome);
        let mut committed = Vec::new();
        while self.stop.is_none() {
            let next = self.committed();
            let Some(outcome) = self.pending.remove(&next) else {
                break;
            };
            match outcome {
                Outcome::Values(values) => {
                    self.push_controls(&values)?;
                    self.matrix.push(values);
                    committed.push(next);
                    self.stop = self.decide();
                }
                Outcome::EventLimit => self.stop = Some(Stop::EventLimit(next)),
                Outcome::Failed => self.stop = Some(Stop::Failed(next)),
            }
        }
        let cancel_from = if self.stop.is_some() {
            self.pending.clear();
            Some(self.committed())
        } else {
            self.lowest_failure.map(|low| low + 1)
        };
        Ok(Step {
            committed,
            cancel_from,
        })
    }

    /// The clock ran out: the committed prefix is the experiment.
    pub(crate) fn on_deadline(&mut self) {
        if self.stop.is_none() {
            self.stop = Some(Stop::Clock);
            self.pending.clear();
        }
    }

    /// A signal: the committed prefix is what is left.
    pub(crate) fn on_cancel(&mut self) {
        if self.stop.is_none() {
            self.stop = Some(Stop::Cancelled);
            self.pending.clear();
        }
    }
}

/// The seeds of an experiment.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SeedList<'a> {
    /// A sorted list.
    List(&'a [u64]),
    /// `first, first + 1, …`, at most `count`.
    Sequence { first: u64, count: u32 },
}

impl SeedList<'_> {
    fn len(&self) -> u32 {
        match self {
            Self::List(seeds) => u32::try_from(seeds.len()).unwrap_or(u32::MAX),
            Self::Sequence { count, .. } => *count,
        }
    }

    fn seed(&self, index: u32) -> u64 {
        match self {
            Self::List(seeds) => seeds.get(index as usize).copied().unwrap_or(0),
            Self::Sequence { first, .. } => first + u64::from(index),
        }
    }

    const fn sequential(&self) -> bool {
        match self {
            Self::List(_) => false,
            Self::Sequence { .. } => true,
        }
    }
}

/// What a row's value is in a run that does not have it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// A count or a rate: 0, a true value.
    Zero,
    /// Undefined.
    Undefined,
}

/// A metric of the experiment: its name, what it is, and the rows the model
/// determines, in order.
#[derive(Debug, Clone)]
struct PlanMetric {
    name: String,
    unit: &'static str,
    at_end: bool,
    kind: RowKind,
    required: bool,
    fixed: Vec<Option<Labels>>,
    spec: Option<KpiSpec>,
}

/// The rows of an experiment, from the model and the registry, so neither the order
/// nor the set depends on what the lowest seed happened to do (ADR-0025 §5).
#[derive(Debug, Clone)]
pub(crate) struct RowPlan {
    metrics: Vec<PlanMetric>,
    targets: Targets,
}

/// The unit of M7's counters.
const ITEMS: &str = "items";

impl RowPlan {
    pub(crate) fn of<D: Domain>(registry: &Registry<D>, document: &Document) -> Self {
        let roles = registry.roles();
        let mut metrics = Vec::new();
        let count = |name: String, required: bool| PlanMetric {
            name,
            unit: ITEMS,
            at_end: false,
            kind: RowKind::Zero,
            required,
            fixed: vec![None],
            spec: None,
        };
        for entry in document.components() {
            let ports = roles.ports(entry.type_name());
            let id = entry.id();
            metrics.push(count(
                format!("created({id})"),
                ports == Some(Ports::Outlet),
            ));
            metrics.push(count(
                format!("delivered({id})"),
                ports == Some(Ports::Inlet),
            ));
            metrics.push(count(format!("scrapped({id})"), false));
            metrics.push(count(format!("consumed({id})"), false));
        }
        metrics.push(PlanMetric {
            at_end: true,
            ..count("items_alive".to_owned(), true)
        });
        for spec in metrics::kpi_specs(registry, document) {
            let kind = match spec.kpi {
                Kpi::Deliveries => RowKind::Zero,
                Kpi::Occupancy
                | Kpi::StateShares
                | Kpi::InProgress
                | Kpi::AwaitingRelease
                | Kpi::Transit => RowKind::Undefined,
            };
            metrics.push(PlanMetric {
                name: spec.metric.clone(),
                unit: spec.unit(),
                at_end: false,
                kind,
                required: true,
                fixed: spec.fixed_rows(),
                spec: Some(spec),
            });
        }
        Self {
            metrics,
            targets: metrics::targets(document),
        }
    }

    /// The target rows, in plan order: a metric, a row, and its target.
    fn target_rows(&self) -> Vec<(usize, Option<Labels>, Ratio)> {
        let mut rows = Vec::new();
        for (index, metric) in self.metrics.iter().enumerate() {
            let (Some(spec), Some(target)) = (&metric.spec, self.targets.get(&metric.name)) else {
                continue;
            };
            for labels in &metric.fixed {
                if spec.is_target_row(labels.as_ref()) {
                    rows.push((index, labels.clone(), *target));
                }
            }
        }
        rows
    }

    /// The targets of the target rows, for the sequencer.
    pub(crate) fn targets(&self) -> Vec<Ratio> {
        self.target_rows().into_iter().map(|(_, _, t)| t).collect()
    }

    /// A run's value on one row.
    fn value_of(
        metric: &PlanMetric,
        labels: Option<&Labels>,
        result: &ResultDocument,
    ) -> Option<f64> {
        let found = result
            .metrics
            .iter()
            .find(|m| m.name == metric.name)
            .and_then(|m| m.values.iter().find(|v| v.labels.as_ref() == labels))
            .and_then(|v| v.value.as_f64());
        match (found, metric.kind) {
            (Some(value), RowKind::Zero | RowKind::Undefined) => Some(value),
            (None, RowKind::Zero) => Some(0.0),
            (None, RowKind::Undefined) => None,
        }
    }

    /// A run's values on the target rows.
    pub(crate) fn target_values(&self, result: &ResultDocument) -> Vec<Option<f64>> {
        self.target_rows()
            .iter()
            .map(|(index, labels, _)| {
                Self::value_of(&self.metrics[*index], labels.as_ref(), result)
            })
            .collect()
    }

    /// The metrics over these counted runs.
    pub(crate) fn metrics(
        &self,
        runs: &[&ResultDocument],
        sequential: bool,
        end: SimDuration,
    ) -> Result<Vec<Metric>, StatsError> {
        let mut out = Vec::new();
        for metric in &self.metrics {
            let seen: Vec<&Metric> = runs
                .iter()
                .filter_map(|run| run.metrics.iter().find(|m| m.name == metric.name))
                .collect();
            if !metric.required && seen.is_empty() {
                continue;
            }
            let mut labels = metric.fixed.clone();
            let extra: std::collections::BTreeSet<Option<Labels>> = seen
                .iter()
                .flat_map(|m| m.values.iter().map(|v| v.labels.clone()))
                .filter(|l| !metric.fixed.contains(l))
                .collect();
            labels.extend(extra);
            let mut values = Vec::new();
            for label in labels {
                let column: Vec<Option<f64>> = runs
                    .iter()
                    .map(|run| Self::value_of(metric, label.as_ref(), run))
                    .collect();
                let mut plan = if sequential {
                    AccuracyPlan::sequential()
                } else {
                    AccuracyPlan::fixed()
                };
                if let (Some(spec), Some(target)) = (&metric.spec, self.targets.get(&metric.name))
                    && spec.is_target_row(label.as_ref())
                {
                    plan = plan.with_target(*target);
                }
                let Some(row) = Accuracy::from_replications(&column, &plan)? else {
                    continue;
                };
                let mut accuracy = row.accuracy;
                if let Some(inside) = censored_mean(metric, label.as_ref(), runs) {
                    accuracy.insert(1, Accuracy::censored(inside));
                }
                values.push(MetricValue {
                    value: Scalar::measured(row.value),
                    labels: label,
                    accuracy,
                });
            }
            if values.is_empty() {
                continue;
            }
            out.push(Metric {
                name: metric.name.clone(),
                unit: metric.unit.to_owned(),
                over: if metric.at_end {
                    Over::At { at: end }
                } else {
                    Over::Window {
                        from: SimDuration::ZERO,
                        to: end,
                    }
                },
                observables: metric
                    .spec
                    .as_ref()
                    .map(|spec| spec.observables.clone())
                    .unwrap_or_default(),
                values,
            });
        }
        Ok(out)
    }
}

/// The mean of the units inside at the end over the runs, for a lead time row, if any
/// run had some.
fn censored_mean(
    metric: &PlanMetric,
    labels: Option<&Labels>,
    runs: &[&ResultDocument],
) -> Option<f64> {
    metric
        .spec
        .as_ref()
        .filter(|spec| spec.kpi == Kpi::Transit)?;
    let inside: Vec<f64> = runs
        .iter()
        .map(|run| {
            run.metrics
                .iter()
                .find(|m| m.name == metric.name)
                .and_then(|m| m.values.iter().find(|v| v.labels.as_ref() == labels))
                .and_then(|v| {
                    v.accuracy.iter().find_map(|a| match &a.kind {
                        AccuracyKind::Censored { units_inside, .. } => Some(*units_inside),
                        AccuracyKind::Sampling(_)
                        | AccuracyKind::None { .. }
                        | AccuracyKind::PartlyUndefined { .. }
                        | AccuracyKind::Unknown { .. } => None,
                    })
                })
                .unwrap_or(0.0)
        })
        .collect();
    let sum: f64 = inside.iter().sum();
    #[allow(clippy::cast_precision_loss)] // At most 100,000 runs.
    let n = inside.len() as f64;
    (sum > 0.0).then(|| sum / n)
}

fn internal(error: &StatsError) -> Failure {
    Failure::one(
        FailureClass::RunFailed,
        Diagnostic::error("internal.stats", error.to_string()),
    )
}

/// The words of a stop, and what follows from it.
const fn ending(stop: Stop) -> (Ended, &'static str) {
    match stop {
        Stop::SeedsFinished => (Ended::Complete, "seeds_finished"),
        Stop::PrecisionReached => (Ended::Complete, "precision_reached"),
        Stop::BudgetExhausted | Stop::Clock => (Ended::BudgetExhausted, "budget_exhausted"),
        Stop::EventLimit(_) | Stop::Failed(_) => (Ended::EventLimit, "event_limit"),
        Stop::Cancelled => (Ended::Cancelled, "cancelled"),
    }
}

/// What the replications of an experiment gave, in seed order.
struct Replications {
    stop: Stop,
    counted: Vec<One>,
    lowest_partial: Option<One>,
    failure: Option<Failure>,
}

/// Runs the replications in seed order until the sequencer stops.
fn replicate<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    seeds: SeedList<'_>,
    request: &RunRequest<'_>,
    store: &Store,
    plan: &RowPlan,
) -> Result<Replications, Failure> {
    let n0 = ReplicationPlan::DEFAULT_MIN_REPLICATIONS;
    let mut sequencer = Sequencer::new(&plan.targets(), seeds.len(), seeds.sequential(), n0)
        .map_err(|e| internal(&e))?;
    let started = Instant::now();
    let deadline = request
        .budget
        .max_seconds
        .map(|s| Duration::from_secs(s.get()));
    let mut counted: Vec<One> = Vec::new();
    let mut lowest_partial: Option<One> = None;
    let mut failure: Option<Failure> = None;
    let mut index = 0;
    while sequencer.stop().is_none() && index < seeds.len() {
        // The first replication always runs, alone: ADR-0010's checked first run.
        if index > 0 {
            if deadline.is_some_and(|d| started.elapsed() >= d) {
                sequencer.on_deadline();
                break;
            }
            if request.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                sequencer.on_cancel();
                break;
            }
        }
        let seed = Some(seeds.seed(index));
        let outcome = match crate::run_one(registry, model, seed, request, store) {
            Ok(one) => match one.partial {
                None => {
                    let values = plan.target_values(&one.result);
                    counted.push(one);
                    Outcome::Values(values)
                }
                Some(RunOutcome::Cancelled) => {
                    if index == 0 {
                        lowest_partial = Some(one);
                    }
                    sequencer.on_cancel();
                    break;
                }
                Some(RunOutcome::EventLimit | RunOutcome::Horizon | RunOutcome::Idle) => {
                    if index == 0 {
                        lowest_partial = Some(one);
                    }
                    Outcome::EventLimit
                }
            },
            Err(error) => {
                failure.get_or_insert(error);
                Outcome::Failed
            }
        };
        sequencer
            .on_finished(index, outcome)
            .map_err(|e| internal(&e))?;
        index += 1;
    }
    // A runtime invariant (spec §7 Layer 3): the incremental decision is the decision
    // over the matrix.
    let mut check = sequencer.clone();
    check
        .rebuild(sequencer.matrix())
        .map_err(|e| internal(&e))?;
    if check.controls != sequencer.controls {
        return Err(Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error(
                "internal.sequencer",
                "the replication controls differ from a replay of the value matrix",
            ),
        ));
    }
    let Some(stop) = sequencer.stop() else {
        return Err(Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error(
                "internal.experiment",
                "an experiment ended without a decision",
            ),
        ));
    };
    counted.truncate(sequencer.committed() as usize);
    Ok(Replications {
        stop,
        counted,
        lowest_partial,
        failure,
    })
}

/// Runs an experiment over `seeds`, in seed order, and returns its document
/// (ADR-0025 §5). Each counted run is stored, and so is the document when the model and
/// the run set determine it (§10).
pub(crate) fn run_experiment<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    seeds: SeedList<'_>,
    request: &RunRequest<'_>,
    store: &Store,
) -> Result<RunResponse, Failure> {
    let document = model.document();
    let plan = RowPlan::of(registry, document);
    let Replications {
        stop,
        counted,
        lowest_partial,
        failure,
    } = replicate(registry, model, seeds, request, store, &plan)?;
    if let Stop::Failed(_) = stop {
        return Err(failure.unwrap_or_else(|| {
            Failure::one(
                FailureClass::RunFailed,
                Diagnostic::error(
                    "internal.experiment",
                    "a replication failed without a reason",
                ),
            )
        }));
    }
    let (ended, stop_reason) = ending(stop);
    if counted.is_empty() {
        // No finished replication: the lowest seed's partial document (ADR-0025 §5).
        let Some(partial) = lowest_partial else {
            return Err(Failure::one(
                FailureClass::RunFailed,
                Diagnostic::error("internal.experiment", "no replication ran"),
            ));
        };
        return Ok(RunResponse {
            result: partial.result,
            reused: false,
            warnings: Vec::new(),
            ended,
        });
    }
    let result = experiment_document(
        document,
        &plan,
        &counted,
        seeds.sequential(),
        (ended, stop_reason),
    )?;
    let reused = counted.iter().all(|one| one.reused);
    let determined = match stop {
        Stop::SeedsFinished | Stop::PrecisionReached | Stop::BudgetExhausted => true,
        Stop::Clock => seeds.sequential(),
        Stop::EventLimit(_) | Stop::Failed(_) | Stop::Cancelled => false,
    };
    let result = match (result.experiment_id.clone(), determined) {
        (Some(id), true) => store.keep_experiment(&id, &result, !request.reuse)?,
        (Some(_) | None, true | false) => result,
    };
    Ok(RunResponse {
        result,
        reused,
        warnings: Vec::new(),
        ended,
    })
}

/// The document of the counted runs.
fn experiment_document(
    document: &Document,
    plan: &RowPlan,
    counted: &[One],
    sequential: bool,
    (ended, stop_reason): (Ended, &str),
) -> Result<ResultDocument, Failure> {
    let end = counted[0].result.runs[0].end_time;
    if let Some(other) = counted
        .iter()
        .find(|one| one.result.runs[0].end_time != end)
    {
        return Err(Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error(
                "run.windows_differ",
                format!(
                    "replications ended at {end} and at {}; an experiment averages over one window",
                    other.result.runs[0].end_time
                ),
            ),
        ));
    }
    let documents: Vec<&ResultDocument> = counted.iter().map(|one| &one.result).collect();
    let metrics = plan
        .metrics(&documents, sequential, end)
        .map_err(|e| internal(&e))?;
    let runs: Vec<_> = counted
        .iter()
        .map(|one| one.result.runs[0].clone())
        .collect();
    let ids: Option<Vec<_>> = runs.iter().map(|run| run.run_id.clone()).collect();
    let experiment_id = ids.map(ExperimentId::over).transpose().map_err(|e| {
        Failure::one(
            FailureClass::RunFailed,
            Diagnostic::error("internal.experiment_id", e.to_string()),
        )
    })?;
    let first = &counted[0].result.provenance;
    Ok(ResultDocument {
        complete: ended == Ended::Complete,
        experiment_id,
        stop_reason: stop_reason.to_owned(),
        provenance: Provenance {
            seeds: runs.iter().filter_map(|run| run.seed).collect(),
            platform: crate::platform(),
            ..first.clone()
        },
        unreported: crate::unreported(document, &metrics),
        metrics,
        runs,
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn target(text: &str) -> Ratio {
        text.parse().unwrap()
    }

    /// Feeds outcomes in seed order.
    fn in_order(sequencer: &mut Sequencer, outcomes: &[Outcome]) {
        for (index, outcome) in outcomes.iter().enumerate() {
            if sequencer.stop().is_some() {
                break;
            }
            sequencer
                .on_finished(u32::try_from(index).unwrap(), outcome.clone())
                .unwrap();
        }
    }

    fn outcome() -> impl Strategy<Value = Outcome> {
        prop_oneof![
            18 => prop::collection::vec(prop::option::weighted(0.97, 90.0_f64..110.0), 2)
                .prop_map(Outcome::Values),
            1 => Just(Outcome::EventLimit),
            1 => Just(Outcome::Failed),
        ]
    }

    proptest! {
        /// Whatever order replications finish in, one decision and one committed set.
        #[test]
        fn sequencer_any_finishing_order(
            outcomes in prop::collection::vec(outcome(), 2..40),
            sequential in any::<bool>(),
            keys in prop::collection::vec(any::<u32>(), 40),
        ) {
            let total = u32::try_from(outcomes.len()).unwrap();
            let targets = [target("3%"), target("5%")];
            let make = || Sequencer::new(&targets, total, sequential, 2).unwrap();
            let mut reference = make();
            in_order(&mut reference, &outcomes);
            prop_assert!(reference.stop().is_some());

            let mut order: Vec<u32> = (0..total).collect();
            order.sort_by_key(|i| keys[*i as usize]);
            let mut shuffled = make();
            let mut cancelled_from = u32::MAX;
            for index in order {
                // A cut replication never reports.
                if index >= cancelled_from {
                    continue;
                }
                let step = shuffled.on_finished(index, outcomes[index as usize].clone()).unwrap();
                if let Some(from) = step.cancel_from {
                    cancelled_from = cancelled_from.min(from);
                }
            }
            prop_assert_eq!(shuffled.stop(), reference.stop());
            prop_assert_eq!(shuffled.matrix(), reference.matrix());
        }

        /// The controls are an incremental form of a function of the matrix.
        #[test]
        fn rebuild_equals_incremental(
            rows in prop::collection::vec(
                prop::collection::vec(prop::option::weighted(0.95, -50.0_f64..50.0), 3), 0..30),
        ) {
            let targets = [target("2%"), target("10%"), target("50%")];
            let mut incremental = Sequencer::new(&targets, 30, true, 2).unwrap();
            let outcomes: Vec<Outcome> = rows.iter().cloned().map(Outcome::Values).collect();
            in_order(&mut incremental, &outcomes);
            let mut rebuilt = Sequencer::new(&targets, 30, true, 2).unwrap();
            rebuilt.rebuild(incremental.matrix()).unwrap();
            prop_assert_eq!(&rebuilt.controls, &incremental.controls);
            prop_assert_eq!(rebuilt.stop(), incremental.stop());
        }
    }

    #[test]
    fn a_list_finishes_with_its_seeds() {
        let mut sequencer = Sequencer::new(&[], 3, false, 10).unwrap();
        for index in 0..3 {
            assert_eq!(sequencer.stop(), None);
            let step = sequencer
                .on_finished(index, Outcome::Values(vec![]))
                .unwrap();
            assert_eq!(step.committed, [index]);
        }
        assert_eq!(sequencer.stop(), Some(Stop::SeedsFinished));
    }

    #[test]
    fn a_later_seed_waits_for_the_ones_before() {
        let mut sequencer = Sequencer::new(&[], 3, false, 10).unwrap();
        let step = sequencer.on_finished(2, Outcome::Values(vec![])).unwrap();
        assert_eq!(step.committed, Vec::<u32>::new());
        let step = sequencer.on_finished(0, Outcome::Values(vec![])).unwrap();
        assert_eq!(step.committed, [0]);
        let step = sequencer.on_finished(1, Outcome::Values(vec![])).unwrap();
        assert_eq!(step.committed, [1, 2]);
        assert_eq!(sequencer.stop(), Some(Stop::SeedsFinished));
    }

    /// A failure at seed k cuts only above k, and a lower failure found later wins.
    #[test]
    fn lowest_failure_wins() {
        let mut sequencer = Sequencer::new(&[], 10, false, 10).unwrap();
        let step = sequencer.on_finished(6, Outcome::Failed).unwrap();
        assert_eq!(step.cancel_from, Some(7));
        let step = sequencer.on_finished(3, Outcome::EventLimit).unwrap();
        assert_eq!(step.cancel_from, Some(4));
        for index in 0..3 {
            sequencer
                .on_finished(index, Outcome::Values(vec![]))
                .unwrap();
        }
        assert_eq!(sequencer.stop(), Some(Stop::EventLimit(3)));
        assert_eq!(sequencer.committed(), 3);
    }

    /// Under targets, a clock stop after 14 results gives what `--max-runs 14` gives.
    #[test]
    fn clock_stop_equals_max_runs() {
        let values: Vec<Outcome> = (0..14)
            .map(|i| Outcome::Values(vec![Some(if i % 2 == 0 { 50.0 } else { 150.0 })]))
            .collect();
        let mut clock = Sequencer::new(&[target("1%")], 100, true, 10).unwrap();
        in_order(&mut clock, &values);
        clock.on_deadline();
        let mut runs = Sequencer::new(&[target("1%")], 14, true, 10).unwrap();
        in_order(&mut runs, &values);
        assert_eq!(clock.matrix(), runs.matrix());
        assert_eq!(ending(clock.stop().unwrap()), ending(runs.stop().unwrap()));
        assert_eq!(ending(runs.stop().unwrap()).0, Ended::BudgetExhausted);
    }

    #[test]
    fn precision_stops_and_an_undefined_row_never_reaches() {
        let same = |n: usize, value: Option<f64>| -> Vec<Outcome> {
            (0..n).map(|_| Outcome::Values(vec![value])).collect()
        };
        let mut reached = Sequencer::new(&[target("5%")], 50, true, 10).unwrap();
        in_order(&mut reached, &same(10, Some(4.0)));
        assert_eq!(reached.stop(), Some(Stop::PrecisionReached));
        assert_eq!(reached.committed(), 10);

        let mut spoiled = Sequencer::new(&[target("5%")], 12, true, 10).unwrap();
        let mut outcomes = same(12, Some(4.0));
        outcomes[0] = Outcome::Values(vec![None]);
        in_order(&mut spoiled, &outcomes);
        assert_eq!(spoiled.stop(), Some(Stop::BudgetExhausted));
    }

    #[test]
    fn a_signal_keeps_the_prefix() {
        let mut sequencer = Sequencer::new(&[], 5, false, 10).unwrap();
        sequencer.on_finished(0, Outcome::Values(vec![])).unwrap();
        sequencer.on_finished(2, Outcome::Values(vec![])).unwrap();
        sequencer.on_cancel();
        assert_eq!(sequencer.stop(), Some(Stop::Cancelled));
        assert_eq!(sequencer.committed(), 1);
        let step = sequencer.on_finished(1, Outcome::Values(vec![])).unwrap();
        assert_eq!(step.committed, Vec::<u32>::new());
        sequencer.on_deadline();
        assert_eq!(
            sequencer.stop(),
            Some(Stop::Cancelled),
            "the first stop stays"
        );
    }

    #[test]
    fn the_endings_table() {
        assert_eq!(
            ending(Stop::SeedsFinished),
            (Ended::Complete, "seeds_finished")
        );
        assert_eq!(
            ending(Stop::PrecisionReached),
            (Ended::Complete, "precision_reached")
        );
        assert_eq!(
            ending(Stop::BudgetExhausted),
            (Ended::BudgetExhausted, "budget_exhausted")
        );
        assert_eq!(
            ending(Stop::Clock),
            (Ended::BudgetExhausted, "budget_exhausted")
        );
        assert_eq!(
            ending(Stop::EventLimit(3)),
            (Ended::EventLimit, "event_limit")
        );
        assert_eq!(ending(Stop::Cancelled), (Ended::Cancelled, "cancelled"));
    }
}
