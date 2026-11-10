//! What a result reports: one fold over core's records of a trace (ADR-0024 §3,
//! ADR-0025 §2). The counts are keyed on each record's `by` and match `Disposal`
//! exhaustively, so any component may create or dispose of items. `Ports` only chooses
//! which zero rows are shown. The KPI rows take their names from the registry and match
//! [`Kpi`] exhaustively, so nothing here names a throughput or a buffer.

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;

use taktsim_components::{Domain, Kpi, Ports, Registry, Roles};
use taktsim_core::{Disposal, ItemCounts};
use taktsim_model::Document;
use taktsim_model::expr::ExprKind;
use taktsim_stats::{LeadTimes, PlaceKpis};
use taktsim_trace::{ReadError, Record, TraceReader};
use taktsim_units::{Ratio, SimDuration, SimTime};

use crate::facts::{Bridge, BridgeError, KpiFacts, Places};
use crate::result::{Accuracy, AccuracyTarget, Metric, MetricValue, Over, Scalar};

/// The events an error shows.
const RECENT: usize = 32;
/// The unit of every metric here: the engine's items, in any domain. A lot of a
/// continuous material is one item (spec §4); a quantity in a material's own unit
/// is a declared KPI.
const ITEMS: &str = "items";
/// The unit of a rate of delivery.
const ITEMS_PER_SECOND: &str = "items/s";
/// The unit of a share of the window.
const SHARE: &str = "share";
/// The unit of a lead time.
const SECONDS: &str = "s";
/// The label of a lead time's statistic.
const STATISTIC: &str = "statistic";
/// The statistic a target on a lead time is about.
const MEAN: &str = "mean";

/// What one component did.
#[derive(Debug, Clone, Copy, Default)]
struct Row {
    created: u64,
    delivered: u64,
    scrapped: u64,
    consumed: u64,
}

/// The fold of a trace.
#[derive(Debug)]
pub(crate) struct Fold {
    rows: Vec<Row>,
    /// The time of the last event.
    pub(crate) last_event: SimTime,
    /// The last records, as text, oldest first.
    pub(crate) recent: Vec<String>,
    /// What the records say about the KPIs.
    kpis: KpiFacts,
    /// The most items alive at once.
    peak_alive: u64,
}

impl Fold {
    /// Where the trace says the run ended.
    pub(crate) const fn end_time(&self) -> SimTime {
        self.kpis.report.end()
    }
}

/// Why a trace cannot be folded.
#[derive(Debug)]
pub(crate) enum FoldError {
    /// It cannot be read.
    Read(ReadError),
    /// It reads, and states something impossible.
    Kpi {
        /// The word on the error line.
        type_name: &'static str,
        /// What is wrong.
        message: String,
        /// The time of the last event before it.
        last_event: SimTime,
        /// The last records, as text.
        recent: Vec<String>,
    },
}

/// The type of an error line for what the bridge found.
const fn bridge_type(error: &BridgeError) -> &'static str {
    match error {
        BridgeError::Stats(_) => "run.kpi_conservation",
        BridgeError::UndeclaredState { .. } => "run.kpi_undeclared_state",
    }
}

/// Folds every record of `reader`, with the components `places`, up to the end its
/// marker states. A trace with no end marker is refused (ADR-0022 §9).
pub(crate) fn fold<R: Read, D: Domain>(
    mut reader: TraceReader<R, D::Message>,
    places: &Places,
) -> Result<Fold, FoldError> {
    let names = reader.header().components.clone();
    let mut rows = vec![Row::default(); names.len()];
    let mut last_event = SimTime::START;
    let mut recent = VecDeque::with_capacity(RECENT);
    let mut bridge = Bridge::new(places);
    let (mut alive, mut peak_alive) = (0_u64, 0_u64);
    let as_text = |recent: &VecDeque<Record<D::Message>>| -> Vec<String> {
        recent
            .iter()
            .map(|record| record.display(&names).to_string())
            .collect()
    };
    while let Some(record) = reader.next_record().map_err(FoldError::Read)? {
        match &record {
            Record::Event { time, .. } => last_event = *time,
            Record::ItemCreated { by, .. } => {
                if let Some(row) = rows.get_mut(by.index()) {
                    row.created += 1;
                }
                alive = alive.saturating_add(1);
                peak_alive = peak_alive.max(alive);
            }
            Record::ItemDisposed { by, how, .. } => {
                alive = alive.saturating_sub(1);
                if let Some(row) = rows.get_mut(by.index()) {
                    match how {
                        Disposal::Delivered => row.delivered += 1,
                        Disposal::Scrapped => row.scrapped += 1,
                        Disposal::Consumed => row.consumed += 1,
                    }
                }
            }
            Record::Note { .. } => {}
        }
        let pushed = bridge.push::<D>(&record);
        if recent.len() == RECENT {
            recent.pop_front();
        }
        recent.push_back(record);
        if let Err(error) = pushed {
            return Err(FoldError::Kpi {
                type_name: bridge_type(&error),
                message: error.to_string(),
                last_event,
                recent: as_text(&recent),
            });
        }
    }
    let recent_text = as_text(&recent);
    let Some(end) = reader.summary().and_then(|summary| summary.end) else {
        return Err(FoldError::Kpi {
            type_name: "run.kpi_no_end",
            message:
                "the trace does not state how the run ended, so a time-weighted value has no window"
                    .to_owned(),
            last_event,
            recent: recent_text,
        });
    };
    let kpis = bridge.finish(end.time).map_err(|error| FoldError::Kpi {
        type_name: bridge_type(&error),
        message: error.to_string(),
        last_event,
        recent: recent_text.clone(),
    })?;
    Ok(Fold {
        rows,
        last_event,
        recent: recent_text,
        kpis,
        peak_alive,
    })
}

impl Fold {
    fn total(&self, of: impl Fn(&Row) -> u64) -> u64 {
        self.rows.iter().map(of).sum()
    }

    /// Items created and not disposed of. Not WIP: it includes parts a blocked
    /// Source holds, which ADR-0022 §7 keeps out of WIP.
    pub(crate) fn items_alive(&self) -> u64 {
        let disposed =
            self.total(|r| r.delivered) + self.total(|r| r.scrapped) + self.total(|r| r.consumed);
        self.total(|r| r.created).saturating_sub(disposed)
    }

    /// The units inside the system at the end, released and not disposed of: what a
    /// lead time row's `censored` entry states (ADR-0025 §3). A part held at a Source
    /// is not one.
    pub(crate) fn wip_end(&self) -> u64 {
        self.kpis.report.wip_end()
    }

    /// What one replication like this one takes in memory, in bytes (ADR-0025 §8).
    pub(crate) fn memory_estimate(&self) -> u64 {
        crate::memory::estimate(self.total(|r| r.delivered), self.peak_alive)
    }

    /// The first of the four sums that differs from the engine's counter: the
    /// name, the sum and the counter.
    pub(crate) fn differs_from(&self, counts: &ItemCounts) -> Option<(&'static str, u64, u64)> {
        [
            ("created", self.total(|r| r.created), counts.created.get()),
            (
                "delivered",
                self.total(|r| r.delivered),
                counts.delivered.get(),
            ),
            (
                "scrapped",
                self.total(|r| r.scrapped),
                counts.scrapped.get(),
            ),
            (
                "consumed",
                self.total(|r| r.consumed),
                counts.consumed.get(),
            ),
        ]
        .into_iter()
        .find(|(_, sum, counter)| sum != counter)
    }

    /// The metrics of the run: each component's rows in document order, then
    /// `items_alive`, then the KPIs. `accuracy` is what can be said of each value,
    /// and `targets` the precision to state on the rows it covers: a model that draws
    /// nothing has no interval to compare, so it states a target and no `reached`.
    pub(crate) fn metrics(
        &self,
        document: &Document,
        roles: &Roles,
        kpis: &[KpiSpec],
        end: SimDuration,
        (accuracy, targets): (&Accuracy, Option<&Targets>),
    ) -> Vec<Metric> {
        let window = Over::Window {
            from: SimDuration::ZERO,
            to: end,
        };
        let metric = |name: String, over: Over, count: u64| Metric {
            name,
            unit: ITEMS.to_owned(),
            over,
            observables: Vec::new(),
            values: vec![MetricValue {
                value: Scalar::Integer(count),
                labels: None,
                accuracy: vec![accuracy.clone()],
            }],
        };
        let mut metrics = Vec::new();
        for (entry, row) in document.components().iter().zip(&self.rows) {
            let ports = roles.ports(entry.type_name());
            let id = entry.id();
            if row.created > 0 || ports == Some(Ports::Outlet) {
                metrics.push(metric(format!("created({id})"), window, row.created));
            }
            if row.delivered > 0 || ports == Some(Ports::Inlet) {
                metrics.push(metric(format!("delivered({id})"), window, row.delivered));
            }
            if row.scrapped > 0 {
                metrics.push(metric(format!("scrapped({id})"), window, row.scrapped));
            }
            if row.consumed > 0 {
                metrics.push(metric(format!("consumed({id})"), window, row.consumed));
            }
        }
        metrics.push(metric(
            "items_alive".to_owned(),
            Over::At { at: end },
            self.items_alive(),
        ));
        for spec in kpis {
            let values = self.kpi_values(spec, accuracy, targets);
            if !values.is_empty() {
                metrics.push(Metric {
                    name: spec.metric.clone(),
                    unit: spec.unit().to_owned(),
                    over: window,
                    observables: spec.observables.clone(),
                    values,
                });
            }
        }
        metrics
    }

    /// The rows of one KPI: none if it has no value in this run (ADR-0025 §2).
    fn kpi_values(
        &self,
        spec: &KpiSpec,
        accuracy: &Accuracy,
        targets: Option<&Targets>,
    ) -> Vec<MetricValue> {
        let inside = self.wip_end();
        let target = targets.and_then(|targets| targets.get(&spec.metric));
        kpi_rows(spec, &self.kpis)
            .into_iter()
            .map(|(labels, value)| {
                let mut first = accuracy.clone();
                if spec.is_target_row(labels.as_ref()) {
                    first.target = target.map(|ratio| AccuracyTarget {
                        relative: Some(ratio.as_fraction()),
                    });
                }
                let mut statements = vec![first];
                if spec.kpi == Kpi::Transit && inside > 0 {
                    #[allow(clippy::cast_precision_loss)] // A count of units in a line.
                    statements.push(Accuracy::censored(inside as f64));
                }
                MetricValue {
                    value: Scalar::measured(value),
                    labels,
                    accuracy: statements,
                }
            })
            .collect()
    }
}

/// The labels of a row.
pub(crate) type Labels = std::collections::BTreeMap<String, String>;

/// One row: its labels and its value.
type KpiRow = (Option<Labels>, f64);

fn single_label(key: &str, value: &str) -> Labels {
    BTreeMap::from([(key.to_owned(), value.to_owned())])
}

/// The rows of a KPI in this run.
fn kpi_rows(spec: &KpiSpec, facts: &KpiFacts) -> Vec<KpiRow> {
    let report = &facts.report;
    let place = spec.place.and_then(|index| report.place(index));
    match spec.kpi {
        Kpi::Deliveries => {
            let (Some(index), Some(place)) = (spec.place, place) else {
                return Vec::new();
            };
            let span = report.span();
            if span.as_nanos() == 0 {
                return Vec::new();
            }
            let per_second = |count: u64| {
                #[allow(clippy::cast_precision_loss)] // A count of delivered items.
                let count = count as f64;
                count / span.as_secs_f64()
            };
            let mut rows = vec![(None, per_second(place.delivered()))];
            rows.extend(facts.class_rows(index).into_iter().map(|row| {
                (
                    Some(single_label(row.dimension, &row.name)),
                    per_second(row.delivered),
                )
            }));
            rows
        }
        Kpi::Occupancy => {
            let mean = match spec.place {
                Some(_) => place.and_then(|place| place.mean_level(report)),
                None => report.mean_wip(),
            };
            mean.map(|value| (None, value)).into_iter().collect()
        }
        Kpi::AwaitingRelease => {
            let mean = match spec.place {
                Some(_) => place.and_then(|place| place.mean_held(report)),
                None => report.mean_held(),
            };
            mean.map(|value| (None, value)).into_iter().collect()
        }
        Kpi::StateShares => {
            let Some(place) = place else {
                return Vec::new();
            };
            spec.states
                .iter()
                .enumerate()
                .filter_map(|(state, name)| {
                    let state = u16::try_from(state).ok()?;
                    let share = place.state_share(report, state)?;
                    let labels = Labels::from([
                        ("resource".to_owned(), spec.subject.clone()),
                        ("state".to_owned(), (*name).to_owned()),
                    ]);
                    Some((Some(labels), share))
                })
                .collect()
        }
        Kpi::InProgress => {
            let Some(place) = place else {
                return Vec::new();
            };
            place
                .mean_in_progress(report)
                .map(|mean| (None, mean / f64::from(spec.places)))
                .into_iter()
                .collect()
        }
        Kpi::Transit => place.map_or_else(Vec::new, transit_rows),
    }
}

/// The statistics of a lead time, in the order of its rows.
const STATISTICS: [(&str, Option<u8>); 7] = [
    (MEAN, None),
    ("min", None),
    ("p50", Some(50)),
    ("p90", Some(90)),
    ("p95", Some(95)),
    ("p99", Some(99)),
    ("max", None),
];

fn transit_rows(place: &PlaceKpis) -> Vec<KpiRow> {
    let times: &LeadTimes = place.lead_times();
    let Some(mean) = times.mean_secs() else {
        return Vec::new();
    };
    STATISTICS
        .iter()
        .filter_map(|(word, percent)| {
            let value = match (*word, percent) {
                (MEAN, _) => mean,
                (_, Some(percent)) => times.percentile(*percent)?.as_secs_f64(),
                ("min", None) => times.min()?.as_secs_f64(),
                (_, None) => times.max()?.as_secs_f64(),
            };
            Some((Some(single_label(STATISTIC, word)), value))
        })
        .collect()
}

/// What the model declares: each text in `observe` or `experiment.precision`, once.
#[derive(Debug, Clone)]
pub(crate) struct Declared {
    pub(crate) text: String,
    function: String,
    args: Vec<String>,
    target: Option<Ratio>,
}

impl Declared {
    /// The metrics it asks for.
    pub(crate) fn metric_names(&self) -> Vec<String> {
        if self.args.is_empty() {
            vec![format!("{}()", self.function)]
        } else {
            self.args
                .iter()
                .map(|arg| format!("{}({arg})", self.function))
                .collect()
        }
    }
}

/// Everything `document` declares, sorted by text.
pub(crate) fn declared(document: &Document) -> Vec<Declared> {
    let mut all: BTreeMap<String, Declared> = BTreeMap::new();
    let read = |observable: &taktsim_model::Observable, target: Option<Ratio>| {
        let ExprKind::Call(call) = observable.expr().clone().into_kind() else {
            return None;
        };
        Some(Declared {
            text: observable.text().to_owned(),
            function: call.name.value.as_str().to_owned(),
            args: call
                .args
                .iter()
                .map(|arg| arg.value.as_str().to_owned())
                .collect(),
            target,
        })
    };
    for observable in document.observe() {
        if let Some(entry) = read(observable, None) {
            all.insert(entry.text.clone(), entry);
        }
    }
    for precision in document.experiment().precision() {
        if let Some(entry) = read(precision.observable(), Some(precision.target())) {
            all.insert(entry.text.clone(), entry);
        }
    }
    all.into_values().collect()
}

/// The metric a KPI gives for one component, or for the whole system.
#[derive(Debug, Clone)]
pub(crate) struct KpiSpec {
    /// Its name, the canonical text of an observable that answers it.
    pub(crate) metric: String,
    pub(crate) kpi: Kpi,
    /// The component's id; empty for the system.
    subject: String,
    /// The component's position; `None` for the system.
    place: Option<usize>,
    /// The states of its type.
    states: &'static [&'static str],
    /// What `utilization` divides by.
    places: u32,
    /// Every declared text it answers, sorted.
    pub(crate) observables: Vec<String>,
}

impl KpiSpec {
    /// The unit of its rows.
    pub(crate) const fn unit(&self) -> &'static str {
        match self.kpi {
            Kpi::Deliveries => ITEMS_PER_SECOND,
            Kpi::Occupancy | Kpi::AwaitingRelease => ITEMS,
            Kpi::StateShares | Kpi::InProgress => SHARE,
            Kpi::Transit => SECONDS,
        }
    }

    /// Whether a row with these labels is one a declared precision covers (ADR-0025 §5).
    pub(crate) fn is_target_row(&self, labels: Option<&Labels>) -> bool {
        match self.kpi {
            Kpi::Deliveries => labels.is_none(),
            Kpi::Occupancy | Kpi::AwaitingRelease | Kpi::StateShares | Kpi::InProgress => true,
            Kpi::Transit => labels.is_some_and(|l| l.get(STATISTIC).is_some_and(|s| s == MEAN)),
        }
    }

    /// The labels of its rows that the model determines, in order. A `Deliveries`
    /// metric also has class rows, which the runs determine.
    pub(crate) fn fixed_rows(&self) -> Vec<Option<Labels>> {
        match self.kpi {
            Kpi::Deliveries | Kpi::Occupancy | Kpi::AwaitingRelease | Kpi::InProgress => {
                vec![None]
            }
            Kpi::StateShares => self
                .states
                .iter()
                .map(|name| {
                    Some(Labels::from([
                        ("resource".to_owned(), self.subject.clone()),
                        ("state".to_owned(), (*name).to_owned()),
                    ]))
                })
                .collect(),
            Kpi::Transit => STATISTICS
                .iter()
                .map(|(word, _)| Some(single_label(STATISTIC, word)))
                .collect(),
        }
    }
}

/// The tightest target on each metric: a half-width within it is within the others
/// (ADR-0025 §5).
pub(crate) type Targets = BTreeMap<String, Ratio>;

/// Every KPI's metrics for `document`: sorted by KPI name, then the system's, then in
/// model order. Whether a component has a KPI is its signature's word, so this names no
/// component type (ADR-0025 §2).
pub(crate) fn kpi_specs<D: Domain>(registry: &Registry<D>, document: &Document) -> Vec<KpiSpec> {
    let declared = declared(document);
    let answering = |metric: &str| -> Vec<String> {
        declared
            .iter()
            .filter(|entry| entry.metric_names().iter().any(|name| name == metric))
            .map(|entry| entry.text.clone())
            .collect()
    };
    let mut specs = Vec::new();
    for (name, signature, kpi) in registry.kpis() {
        let has_system_row = match kpi {
            Kpi::Occupancy | Kpi::AwaitingRelease => true,
            Kpi::Deliveries | Kpi::StateShares | Kpi::InProgress | Kpi::Transit => false,
        };
        if has_system_row && signature.arity().allows(0) {
            let metric = format!("{name}()");
            specs.push(KpiSpec {
                observables: answering(&metric),
                metric,
                kpi,
                subject: String::new(),
                place: None,
                states: &[],
                places: 1,
            });
        }
        if !signature.arity().allows(1) {
            continue;
        }
        for (position, entry) in document.components().iter().enumerate() {
            let accepted = signature
                .accepts()
                .is_none_or(|types| types.contains(entry.type_name()));
            if accepted {
                let metric = format!("{name}({})", entry.id());
                specs.push(KpiSpec {
                    observables: answering(&metric),
                    metric,
                    kpi,
                    subject: entry.id().to_owned(),
                    place: Some(position),
                    states: registry.roles().states(entry.type_name()),
                    places: registry.places(entry),
                });
            }
        }
    }
    specs
}

/// The tightest declared target on each metric.
pub(crate) fn targets(document: &Document) -> Targets {
    let mut targets = Targets::new();
    for entry in declared(document) {
        let Some(target) = entry.target else {
            continue;
        };
        for metric in entry.metric_names() {
            let tighter = targets
                .get(&metric)
                .is_none_or(|known| target.as_fraction() < known.as_fraction());
            if tighter {
                targets.insert(metric, target);
            }
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use taktsim_components::{Flow, FlowMessage, RunOptions, standard};
    use taktsim_model::{Format, Loader};
    use taktsim_units::Count;

    use super::*;

    /// The guard behind `run.metrics_differ`: on a correct run the fold equals the
    /// engine's counters, so only a hand-made disagreement shows that each of the
    /// four sums is compared, and reported with its name, sum and counter.
    /// The fold of a run of a short line, for its KPI facts.
    fn folded_line() -> Fold {
        let registry = standard().unwrap();
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
        let places = Places::of(&registry, model.document());
        fold::<_, Flow>(
            TraceReader::<_, FlowMessage>::new(trace.as_slice()).unwrap(),
            &places,
        )
        .unwrap()
    }

    #[test]
    fn each_sum_that_differs_from_the_engine_is_reported() {
        // The rows are made by hand; the rest is a real fold's, which
        // `differs_from` does not read.
        let fold = Fold {
            rows: vec![
                Row {
                    created: 3,
                    delivered: 1,
                    scrapped: 0,
                    consumed: 0,
                },
                Row {
                    created: 1,
                    delivered: 1,
                    scrapped: 1,
                    consumed: 1,
                },
            ],
            last_event: SimTime::from_nanos(0),
            recent: Vec::new(),
            ..folded_line()
        };
        let same = ItemCounts {
            created: Count::new(4),
            held: Count::new(1),
            in_transit: Count::ZERO,
            delivered: Count::new(2),
            scrapped: Count::new(1),
            consumed: Count::new(1),
        };
        assert_eq!(fold.differs_from(&same), None);
        // Held and in transit are not in the trace, so not compared.
        let elsewhere = ItemCounts {
            held: Count::ZERO,
            in_transit: Count::new(9),
            ..same
        };
        assert_eq!(fold.differs_from(&elsewhere), None);
        let cases = [
            (
                ItemCounts {
                    created: Count::new(5),
                    ..same
                },
                ("created", 4, 5),
            ),
            (
                ItemCounts {
                    delivered: Count::new(3),
                    ..same
                },
                ("delivered", 2, 3),
            ),
            (
                ItemCounts {
                    scrapped: Count::ZERO,
                    ..same
                },
                ("scrapped", 1, 0),
            ),
            (
                ItemCounts {
                    consumed: Count::new(7),
                    ..same
                },
                ("consumed", 1, 7),
            ),
        ];
        for (counts, expected) in cases {
            assert_eq!(fold.differs_from(&counts), Some(expected), "{expected:?}");
        }
    }

    /// `recent` is the last `RECENT` records, oldest first, however long the trace:
    /// every run folds its whole trace, which must stream, not stay in memory.
    #[test]
    fn recent_is_the_last_records_oldest_first() {
        let registry = standard().unwrap();
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
        let places = Places::of(&registry, model.document());
        let folded = fold::<_, Flow>(
            TraceReader::<_, FlowMessage>::new(trace.as_slice()).unwrap(),
            &places,
        )
        .unwrap();

        let mut reader = TraceReader::<_, FlowMessage>::new(trace.as_slice()).unwrap();
        let names = reader.header().components.clone();
        let mut every = Vec::new();
        while let Some(record) = reader.next_record().unwrap() {
            every.push(record.display(&names).to_string());
        }
        assert!(every.len() > 2 * RECENT, "{} records", every.len());
        assert_eq!(folded.recent.len(), RECENT);
        assert_eq!(folded.recent, every[every.len() - RECENT..]);
    }
}
