//! Experiments over several seeds (ADR-0025 §4, §5, §8, §9). An experiment is a prefix
//! of its seed order: runs enter the statistics and the stopping decision strictly in
//! that order, so the document is a function of the counted runs, whatever finished
//! first and whatever thread ran what.

use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use taktsim_components::{Domain, Kpi, Ports, Registry};
use taktsim_core::RunOutcome;
use taktsim_model::{Document, Model};
use taktsim_stats::{ReplicationControl, ReplicationPlan, ReplicationStatus, StatsError};
use taktsim_units::{Ratio, SimDuration};

use crate::diagnostic::{Diagnostic, Failure, FailureClass};
use crate::id::ExperimentId;
use crate::memory;
use crate::metrics::{self, KpiSpec, Labels, Targets};
use crate::result::{
    Accuracy, AccuracyKind, AccuracyPlan, Metric, MetricValue, Over, Provenance, ResultDocument,
    Scalar,
};
use crate::store::{Store, StoredResult};
use crate::{One, Ready, RunRequest, RunResponse, Worked};

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
            if let Some(step) = self.settle_lowest(index, outcome)? {
                return Ok(step);
            }
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

    /// After a clock stop or a signal with nothing committed, the lowest seed's own
    /// ending settles what is left (ADR-0025 §5). The cut reaches a run only while it
    /// processes events, so the lowest seed can still arrive finished, while it folds
    /// and writes its result: it is then the one run counted, since the document needs
    /// one and a finished run says more than a partial one. Its failure is the
    /// experiment's. Its event limit leaves the stop, and its partial document is what
    /// is left. `Some` if this commits it.
    fn settle_lowest(&mut self, index: u32, outcome: Outcome) -> Result<Option<Step>, StatsError> {
        let open = match self.stop {
            Some(Stop::Clock | Stop::Cancelled) => index == 0 && self.committed() == 0,
            Some(
                Stop::SeedsFinished
                | Stop::PrecisionReached
                | Stop::BudgetExhausted
                | Stop::EventLimit(_)
                | Stop::Failed(_),
            )
            | None => false,
        };
        if !open {
            return Ok(None);
        }
        match outcome {
            Outcome::Values(values) => {
                self.push_controls(&values)?;
                self.matrix.push(values);
                Ok(Some(Step {
                    committed: vec![0],
                    cancel_from: Some(1),
                }))
            }
            Outcome::Failed => {
                self.stop = Some(Stop::Failed(0));
                Ok(None)
            }
            Outcome::EventLimit => Ok(None),
        }
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

/// Which budget ended an experiment.
///
/// ```
/// use taktsim_run::Spent;
///
/// let flag = |spent: Spent| match spent {
///     Spent::MaxRuns => "--max-runs",
///     Spent::MaxSeconds => "--max-seconds",
/// };
/// assert_eq!(flag(Spent::MaxSeconds), "--max-seconds");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spent {
    /// `--max-runs`: every replication it allows ran, under the targets.
    MaxRuns,
    /// `--max-seconds`: the clock ran out.
    MaxSeconds,
}

/// What an experiment did beside its document, for the `progress` line (ADR-0025 §5,
/// §8). None of it is in the document, which stays a function of the counted runs.
///
/// ```
/// use std::num::NonZeroUsize;
/// use taktsim_run::{Progress, Spent};
///
/// let progress = Progress { counted: 14, planned: 100, at_once: NonZeroUsize::new(4), spent: Some(Spent::MaxSeconds) };
/// assert!(progress.counted < progress.planned);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Replications counted.
    pub counted: u32,
    /// The most there could have been: the list's length, or `--max-runs`.
    pub planned: u32,
    /// How many replications ran at once; `None` if every one came from the store.
    pub at_once: Option<NonZeroUsize>,
    /// Which budget ended it, if one did.
    pub spent: Option<Spent>,
}

/// What the replications of an experiment gave, in seed order.
struct Replications {
    stop: Stop,
    counted: Vec<One>,
    lowest_partial: Option<One>,
    failure: Option<Failure>,
    at_once: Option<NonZeroUsize>,
}

/// How often the coordinator looks at the signal flag and the clock.
const POLL: Duration = Duration::from_millis(100);

/// A seed that finished before the ones below it, waiting to be committed.
#[derive(Debug)]
enum Waiting {
    /// It was in the store.
    Stored(Box<One>),
    /// It ran, and waits in `tmp/`.
    Fresh(Box<Ready>),
}

/// What reached the coordinator for one seed.
#[derive(Debug)]
enum Arrived {
    /// It was in the store, and takes no thread.
    Stored(Box<One>),
    /// A worker ran it.
    Worked(Worked),
}

/// The one thread that commits in seed order (ADR-0025 §8). Workers run into `tmp/`;
/// this feeds the sequencer, renames what it commits into the store, and cuts what it
/// no longer needs.
struct Coordinator<'r> {
    sequencer: Sequencer,
    plan: &'r RowPlan,
    /// What a committed run is checked against (ADR-0024 §6, §8).
    model: &'r Model,
    store: &'r Store,
    /// One cancel flag per replication.
    flags: &'r [AtomicBool],
    /// Nothing at or above this index is started, and what runs there is cut.
    limit: u32,
    waiting: BTreeMap<u32, Waiting>,
    counted: Vec<One>,
    lowest_partial: Option<One>,
    failures: BTreeMap<u32, Failure>,
}

fn internal_line(message: &str) -> Failure {
    Failure::one(
        FailureClass::RunFailed,
        Diagnostic::error("internal.experiment", message.to_owned()),
    )
}

impl Coordinator<'_> {
    /// Cuts every replication from `from` on, and drops what waits there.
    fn cut(&mut self, from: u32) {
        self.limit = self.limit.min(from);
        for flag in self.flags.iter().skip(from as usize) {
            flag.store(true, Ordering::Relaxed);
        }
        self.waiting.retain(|index, _| *index < from);
    }

    /// The seed at `index` finished: feed the sequencer, commit what it commits in
    /// order, and cut what it no longer needs.
    fn finished(&mut self, index: u32, arrived: Result<Arrived, Failure>) -> Result<(), Failure> {
        let outcome = match arrived {
            Ok(Arrived::Stored(one)) => {
                let values = self.plan.target_values(&one.result);
                self.waiting.insert(index, Waiting::Stored(one));
                Outcome::Values(values)
            }
            Ok(Arrived::Worked(Worked::Ready(ready))) => {
                let values = self.plan.target_values(&ready.result);
                self.waiting.insert(index, Waiting::Fresh(ready));
                Outcome::Values(values)
            }
            Ok(Arrived::Worked(Worked::Partial(one))) => {
                // Only this coordinator sets a replication's flag, and only once it
                // has decided: a cut replication tells the sequencer nothing.
                let outcome = match one.partial {
                    Some(RunOutcome::Cancelled) => None,
                    Some(RunOutcome::EventLimit | RunOutcome::Horizon | RunOutcome::Idle)
                    | None => Some(Outcome::EventLimit),
                };
                if index == 0 {
                    self.lowest_partial = Some(*one);
                }
                let Some(outcome) = outcome else {
                    return Ok(());
                };
                outcome
            }
            Err(failure) => {
                self.failures.insert(index, failure);
                Outcome::Failed
            }
        };
        // Past the limit only the lowest seed still counts, while nothing is committed:
        // the sequencer decides whether it settles the stop.
        let lowest = index == 0 && self.sequencer.committed() == 0;
        if index >= self.limit && !lowest {
            self.waiting.remove(&index);
            return Ok(());
        }
        let step = self
            .sequencer
            .on_finished(index, outcome)
            .map_err(|e| internal(&e))?;
        for committed in step.committed {
            let one = match self.waiting.remove(&committed) {
                Some(Waiting::Stored(one)) => *one,
                Some(Waiting::Fresh(ready)) => {
                    let id = ready.id.clone();
                    crate::commit(self.store, self.model, *ready)?;
                    // Each counted run is read back from the store, fresh or reused,
                    // so both follow one path (ADR-0025 §5), and both are checked
                    // against the id they are filed under (ADR-0024 §8).
                    let stored = crate::read_filed(self.store, &id, self.model)?
                        .ok_or_else(|| internal_line("a committed run is not in the store"))?;
                    One {
                        result: stored.document,
                        text: stored.text,
                        reused: false,
                        partial: None,
                    }
                }
                None => return Err(internal_line("a committed seed has no result")),
            };
            self.counted.push(one);
        }
        if let Some(from) = step.cancel_from {
            self.cut(from);
        }
        Ok(())
    }
}

/// What a worker tells the coordinator about its replication.
#[derive(Debug)]
enum Report {
    /// It returned.
    Done(Result<Worked, Failure>),
    /// It panicked. rayon raises the panic again when the scope ends, so it reaches
    /// ADR-0007's path once the coordinator stops waiting and cuts the others.
    Panicked,
}

/// A worker's way back to the coordinator. Dropped unsent, as a panicking worker
/// unwinds, it says `Panicked`, so the coordinator never waits for a report that will
/// not come.
struct Reporter {
    sender: Option<mpsc::Sender<(u32, Report)>>,
    index: u32,
}

impl Reporter {
    fn send(mut self, worked: Result<Worked, Failure>) {
        if let Some(sender) = self.sender.take() {
            // The coordinator holds the receiver until every worker has reported; a
            // failed send drops the run, and its temp directory.
            let _ = sender.send((self.index, Report::Done(worked)));
        }
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send((self.index, Report::Panicked));
        }
    }
}

/// Where the workers run and what they share.
struct Work<'r, D: Domain> {
    registry: &'r Registry<D>,
    model: &'r Model,
    seeds: SeedList<'r>,
    request: &'r RunRequest<'r>,
    store: &'r Store,
    cores: NonZeroUsize,
}

impl<'r, D: Domain> Work<'r, D> {
    /// Whether the caller's flag is set.
    fn signalled(&self) -> bool {
        self.request
            .cancel
            .is_some_and(|c| c.load(Ordering::Relaxed))
    }

    /// Runs the replication of `seed` on the pool, cut by `flag`; it reports through
    /// `reporter`, even when it panics.
    fn spawn(
        &self,
        scope: &rayon::Scope<'r>,
        reporter: Reporter,
        seed: Option<u64>,
        flag: Option<&'r AtomicBool>,
    ) {
        let Work {
            registry,
            model,
            request,
            store,
            ..
        } = *self;
        scope.spawn(move |_| {
            let reporter = reporter;
            let worked = crate::work_one(registry, model, seed, request, flag, store);
            reporter.send(worked);
        });
    }

    /// Starts replications while the window allows, waits for one, polls the signal
    /// and the clock, until the sequencer has decided and nothing runs. At most
    /// `at_once` seeds run past the decision point, so waste is bounded.
    fn drive(
        &self,
        scope: &rayon::Scope<'r>,
        coordinator: &mut Coordinator<'r>,
    ) -> Result<Option<NonZeroUsize>, Failure> {
        let (sender, receiver) = mpsc::channel::<(u32, Report)>();
        let started = Instant::now();
        let deadline = self
            .request
            .budget
            .max_seconds
            .map(|s| Duration::from_secs(s.get()));
        // Unknown until the first replication that runs has gone alone (ADR-0010).
        let mut at_once: Option<NonZeroUsize> = None;
        let mut next = 0_u32;
        let mut running = 0_usize;
        // Once seen, the caller's flag stays seen, even if the caller clears it.
        let mut cancelled = false;
        loop {
            while coordinator.sequencer.stop().is_none() && next < coordinator.limit {
                cancelled |= self.signalled();
                if cancelled && next > 0 {
                    // Nothing more starts. What runs reports, cut, before the poll
                    // below decides; with nothing running it decides now.
                    if running == 0 {
                        coordinator.sequencer.on_cancel();
                    }
                    break;
                }
                let window = at_once.map_or(1, NonZeroUsize::get);
                let ahead = (next - coordinator.sequencer.committed()) as usize;
                if ahead >= window || running >= window {
                    break;
                }
                let index = next;
                next += 1;
                let seed = Some(self.seeds.seed(index));
                match crate::stored::<D>(self.model, seed, self.request, self.store) {
                    Ok(Some(one)) => {
                        coordinator.finished(index, Ok(Arrived::Stored(Box::new(one))))?;
                    }
                    Ok(None) => {
                        running += 1;
                        let reporter = Reporter {
                            sender: Some(sender.clone()),
                            index,
                        };
                        let flag = coordinator.flags.get(index as usize);
                        if cancelled {
                            // Signalled before anything ran: the lowest seed starts cut,
                            // so its partial document is what is left (ADR-0025 §5),
                            // as one run's is.
                            if let Some(flag) = flag {
                                flag.store(true, Ordering::Relaxed);
                            }
                        }
                        self.spawn(scope, reporter, seed, flag);
                    }
                    Err(failure) => coordinator.finished(index, Err(failure))?,
                }
            }
            if running == 0 {
                if coordinator.sequencer.stop().is_some() {
                    return Ok(at_once);
                }
                return Err(internal_line(
                    "an experiment waits for a replication that does not run",
                ));
            }
            match receiver.recv_timeout(POLL) {
                Ok((index, Report::Panicked)) => {
                    // The caller cuts every other replication, and the scope then
                    // raises the panic.
                    return Err(internal_line(&format!(
                        "the replication at index {index} panicked"
                    )));
                }
                Ok((index, Report::Done(worked))) => {
                    running -= 1;
                    if let (None, Ok(Worked::Ready(ready))) = (at_once, &worked) {
                        at_once = Some(memory::parallel(
                            self.cores,
                            self.request.threads,
                            self.request.max_memory_mib,
                            ready.memory,
                        ));
                    }
                    coordinator.finished(index, worked.map(Arrived::Worked))?;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(internal_line("the replications stopped reporting"));
                }
            }
            if coordinator.sequencer.stop().is_none() {
                cancelled |= self.signalled();
                if cancelled {
                    coordinator.sequencer.on_cancel();
                } else if deadline.is_some_and(|d| started.elapsed() >= d) {
                    coordinator.sequencer.on_deadline();
                }
            }
            if coordinator.sequencer.stop().is_some() {
                let committed = coordinator.sequencer.committed();
                coordinator.cut(committed);
            }
        }
    }
}

/// Runs the replications on a pool of an explicit size, committing in seed order until
/// the sequencer stops (ADR-0025 §8).
fn replicate<D: Domain>(
    registry: &Registry<D>,
    model: &Model,
    seeds: SeedList<'_>,
    request: &RunRequest<'_>,
    store: &Store,
    plan: &RowPlan,
) -> Result<Replications, Failure> {
    let n0 = ReplicationPlan::DEFAULT_MIN_REPLICATIONS;
    let sequencer = Sequencer::new(&plan.targets(), seeds.len(), seeds.sequential(), n0)
        .map_err(|e| internal(&e))?;
    // Only speed depends on the cores: no thread count reaches a byte.
    let cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(memory::pool_size(cores, request.threads).get())
        .thread_name(|i| format!("taktsim-replication-{i}"))
        .build()
        .map_err(|e| {
            Failure::one(
                FailureClass::RunFailed,
                Diagnostic::error(
                    "run.threads",
                    format!("the replication threads cannot start: {e}"),
                ),
            )
        })?;
    let flags: Vec<AtomicBool> = (0..seeds.len()).map(|_| AtomicBool::new(false)).collect();
    let mut coordinator = Coordinator {
        sequencer,
        plan,
        model,
        store,
        flags: &flags,
        limit: seeds.len(),
        waiting: BTreeMap::new(),
        counted: Vec::new(),
        lowest_partial: None,
        failures: BTreeMap::new(),
    };
    let work = Work {
        registry,
        model,
        seeds,
        request,
        store,
        cores,
    };
    let driven = pool.in_place_scope(|scope| {
        let driven = work.drive(scope, &mut coordinator);
        if driven.is_err() {
            // Nothing more is needed: cut every worker, so the scope ends soon.
            coordinator.cut(0);
        }
        driven
    });
    let at_once = driven?;
    let Coordinator {
        sequencer,
        counted,
        lowest_partial,
        mut failures,
        ..
    } = coordinator;
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
        return Err(internal_line("an experiment ended without a decision"));
    };
    if counted.len() != sequencer.committed() as usize {
        return Err(internal_line(
            "the counted runs differ from the committed seeds",
        ));
    }
    let failure = match stop {
        Stop::Failed(index) => failures.remove(&index),
        Stop::SeedsFinished
        | Stop::PrecisionReached
        | Stop::BudgetExhausted
        | Stop::Clock
        | Stop::EventLimit(_)
        | Stop::Cancelled => None,
    };
    Ok(Replications {
        stop,
        counted,
        lowest_partial,
        failure,
        at_once,
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
        at_once,
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
    let progress = Some(Progress {
        counted: u32::try_from(counted.len()).unwrap_or(u32::MAX),
        planned: seeds.len(),
        at_once,
        spent: match stop {
            Stop::BudgetExhausted => Some(Spent::MaxRuns),
            Stop::Clock => Some(Spent::MaxSeconds),
            Stop::SeedsFinished
            | Stop::PrecisionReached
            | Stop::EventLimit(_)
            | Stop::Failed(_)
            | Stop::Cancelled => None,
        },
    });
    if counted.is_empty() {
        // No finished replication: the lowest seed's partial document (ADR-0025 §5).
        let Some(partial) = lowest_partial else {
            return Err(Failure::one(
                FailureClass::RunFailed,
                Diagnostic::error("internal.experiment", "no replication ran"),
            ));
        };
        // Its own word says how the run stopped; the top one, why the experiment did,
        // as the exit code does.
        let mut result = partial.result;
        stop_reason.clone_into(&mut result.stop_reason);
        return Ok(RunResponse {
            text: result.to_text(),
            result,
            reused: false,
            warnings: Vec::new(),
            ended,
            progress,
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
    let kept = match (result.experiment_id.clone(), determined) {
        (Some(id), true) => store.keep_experiment(&id, &result, !request.reuse, |stored| {
            crate::check_experiment_filed_under(stored, &id, &result)
        })?,
        (Some(_) | None, true | false) => StoredResult {
            text: result.to_text(),
            document: result,
        },
    };
    Ok(RunResponse {
        result: kept.document,
        text: kept.text,
        reused,
        warnings: Vec::new(),
        ended,
        progress,
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

    /// After a clock stop or a signal with nothing committed, the lowest seed's own
    /// ending settles what is left: a run that finished all the same is the one
    /// counted, and a failure is the experiment's. Nothing above it is.
    #[test]
    fn the_lowest_seed_settles_a_stop_with_nothing_committed() {
        for stop in [Sequencer::on_deadline, Sequencer::on_cancel] {
            let mut sequencer = Sequencer::new(&[target("5%")], 3, true, 2).unwrap();
            stop(&mut sequencer);
            let before = sequencer.stop();
            let step = sequencer
                .on_finished(1, Outcome::Values(vec![Some(1.0)]))
                .unwrap();
            assert_eq!(step.committed, Vec::<u32>::new());
            let step = sequencer
                .on_finished(0, Outcome::Values(vec![Some(2.0)]))
                .unwrap();
            assert_eq!(step.committed, [0]);
            assert_eq!(step.cancel_from, Some(1));
            assert_eq!(sequencer.stop(), before, "the stop stays");
            assert_eq!(sequencer.matrix(), [vec![Some(2.0)]]);
            let step = sequencer
                .on_finished(0, Outcome::Values(vec![Some(3.0)]))
                .unwrap();
            assert_eq!(step.committed, Vec::<u32>::new(), "once only");

            let mut failed = Sequencer::new(&[], 3, false, 10).unwrap();
            stop(&mut failed);
            failed.on_finished(0, Outcome::Failed).unwrap();
            assert_eq!(failed.stop(), Some(Stop::Failed(0)));
            assert_eq!(failed.committed(), 0);

            let mut limited = Sequencer::new(&[], 3, false, 10).unwrap();
            stop(&mut limited);
            limited.on_finished(0, Outcome::EventLimit).unwrap();
            assert_eq!(
                limited.stop(),
                before,
                "its partial document is what is left"
            );
        }
        // A stop the sequencer decided is not reopened.
        let mut finished = Sequencer::new(&[], 1, false, 10).unwrap();
        finished.on_finished(0, Outcome::Values(vec![])).unwrap();
        let step = finished.on_finished(0, Outcome::Values(vec![])).unwrap();
        assert_eq!(step.committed, Vec::<u32>::new());
    }

    /// The clock runs out while the lowest seed folds and writes its result, after its
    /// last event, so the cut cannot stop it: it arrives finished, and is the one run
    /// counted, not dropped for an internal error.
    #[test]
    fn a_lowest_seed_that_finishes_after_the_clock_is_counted() {
        let registry = taktsim_components::standard().unwrap();
        let model = taktsim_model::Loader::new(registry.catalog())
            .load_str(
                "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                 components: [{id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}, {id: sink, type: Sink}]\n\
                 flow: [[source, sink]]\n",
                taktsim_model::Format::Yaml,
            )
            .unwrap();
        let dir =
            std::env::temp_dir().join(format!("taktsim-run-late-lowest-{}", std::process::id()));
        let store = Store::open(&dir).unwrap();
        let request = RunRequest::new();
        let worked = crate::work_one(&registry, &model, Some(1), &request, None, &store).unwrap();
        assert!(matches!(worked, Worked::Ready(_)));
        let plan = RowPlan::of(&registry, model.document());
        let flags: Vec<AtomicBool> = (0..3).map(|_| AtomicBool::new(false)).collect();
        let mut coordinator = Coordinator {
            sequencer: Sequencer::new(&plan.targets(), 3, false, 10).unwrap(),
            plan: &plan,
            model: &model,
            store: &store,
            flags: &flags,
            limit: 3,
            waiting: BTreeMap::new(),
            counted: Vec::new(),
            lowest_partial: None,
            failures: BTreeMap::new(),
        };
        coordinator.sequencer.on_deadline();
        coordinator.cut(0);
        coordinator
            .finished(0, Ok(Arrived::Worked(worked)))
            .unwrap();
        let counted = coordinator.counted.len();
        let stop = coordinator.sequencer.stop();
        let cut = flags.iter().all(|flag| flag.load(Ordering::Relaxed));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(counted, 1);
        assert_eq!(stop, Some(Stop::Clock));
        assert!(cut);
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
