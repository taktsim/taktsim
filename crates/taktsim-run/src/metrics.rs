//! What M7 reports (ADR-0024 §3): one fold over core's records of a stored trace.
//! The fold is keyed on each record's `by` and matches `Disposal` exhaustively, so
//! any component may create or dispose of items. `Ports` only chooses which zero
//! rows are shown.

use std::collections::VecDeque;
use std::io::Read;

use taktsim_components::{Ports, Roles};
use taktsim_core::{Disposal, ItemCounts};
use taktsim_model::Document;
use taktsim_trace::{ReadError, Record, TraceMessage, TraceReader};
use taktsim_units::{SimDuration, SimTime};

use crate::result::{Accuracy, Metric, MetricValue, Over, Scalar};

/// The events an error shows.
const RECENT: usize = 32;
/// The unit of every metric here: the engine's items, in any domain. A lot of a
/// continuous material is one item (spec §4); a quantity in a material's own unit
/// is a declared KPI.
const ITEMS: &str = "items";

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
}

/// Folds every record of `reader`.
pub(crate) fn fold<R: Read, M: TraceMessage>(
    mut reader: TraceReader<R, M>,
) -> Result<Fold, ReadError> {
    let names = reader.header().components.clone();
    let mut rows = vec![Row::default(); names.len()];
    let mut last_event = SimTime::from_nanos(0);
    let mut recent = VecDeque::with_capacity(RECENT);
    while let Some(record) = reader.next_record()? {
        match &record {
            Record::Event { time, .. } => last_event = *time,
            Record::ItemCreated { by, .. } => {
                if let Some(row) = rows.get_mut(by.index()) {
                    row.created += 1;
                }
            }
            Record::ItemDisposed { by, how, .. } => {
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
        if recent.len() == RECENT {
            recent.pop_front();
        }
        recent.push_back(record);
    }
    let recent = recent
        .iter()
        .map(|record: &Record<M>| record.display(&names).to_string())
        .collect();
    Ok(Fold {
        rows,
        last_event,
        recent,
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
    /// `items_alive`.
    pub(crate) fn metrics(
        &self,
        document: &Document,
        roles: &Roles,
        end: SimDuration,
        accuracy: &Accuracy,
    ) -> Vec<Metric> {
        let window = Over::Window {
            from: SimDuration::ZERO,
            to: end,
        };
        let metric = |name: String, over: Over, count: u64| Metric {
            name,
            unit: ITEMS.to_owned(),
            over,
            observable: None,
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
        metrics
    }
}

#[cfg(test)]
mod tests {
    use taktsim_units::Count;

    use super::*;

    /// The guard behind `run.metrics_differ`: on a correct run the fold equals the
    /// engine's counters, so only a hand-made disagreement shows that each of the
    /// four sums is compared, and reported with its name, sum and counter.
    #[test]
    fn each_sum_that_differs_from_the_engine_is_reported() {
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
}
