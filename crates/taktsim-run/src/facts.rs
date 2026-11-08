//! The bridge from a domain's records to the neutral facts of the KPI fold (ADR-0025
//! §1). It matches `Record` and `Disposal` exhaustively, and a domain's messages answer
//! through `read_message`, so a new record or message cannot miscount.

use std::collections::{BTreeMap, BTreeSet};

use taktsim_components::{Creation as ComponentCreation, Domain, MessageFact, Registry};
use taktsim_core::{ComponentId, Disposal};
use taktsim_model::Document;
use taktsim_stats::{Creation, Fact, KpiFold, KpiReport, PlaceSpec, StatsError};
use taktsim_trace::Record;
use taktsim_units::SimTime;

/// What the bridge knows about one component.
#[derive(Debug, Clone)]
pub(crate) struct Place {
    type_name: String,
    states: &'static [&'static str],
    creation: Creation,
}

/// The components of a model, in document order: a record's ids are positions here.
#[derive(Debug, Clone)]
pub(crate) struct Places(Vec<Place>);

impl Places {
    /// The components of `document`, with what their types declare.
    pub(crate) fn of<D: Domain>(registry: &Registry<D>, document: &Document) -> Self {
        let roles = registry.roles();
        Self(
            document
                .components()
                .iter()
                .map(|entry| Place {
                    type_name: entry.type_name().to_owned(),
                    states: roles.states(entry.type_name()),
                    creation: roles
                        .creation(entry.type_name())
                        .map_or(Creation::Never, creation),
                })
                .collect(),
        )
    }

    /// What the fold is told about them.
    pub(crate) fn specs(&self) -> Vec<PlaceSpec> {
        self.0
            .iter()
            .map(|place| PlaceSpec {
                states: u16::try_from(place.states.len()).unwrap_or(u16::MAX),
                creation: place.creation,
            })
            .collect()
    }
}

/// The stats crate's word for a component type's. Neither crate depends on the other.
const fn creation(creation: ComponentCreation) -> Creation {
    match creation {
        ComponentCreation::Never => Creation::Never,
        ComponentCreation::HeldUntilFirstMove => Creation::HeldUntilFirstMove,
        ComponentCreation::InSystem => Creation::InSystem,
    }
}

/// Why the records of a run cannot be folded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub(crate) enum BridgeError {
    /// The fold refused a fact.
    #[error("{0}")]
    Stats(#[from] StatsError),
    /// A note states a state its component's type does not declare.
    #[error(
        "component {place} of type `{type_name}` stated the state `{name}` at {time}, which its type does not declare; declared: {declared}"
    )]
    UndeclaredState {
        /// The position of the component.
        place: usize,
        /// Its type.
        type_name: String,
        /// The state.
        name: String,
        /// When.
        time: SimTime,
        /// What it declares.
        declared: String,
    },
}

/// The position of a component as the fold takes it. One beyond `u32` is refused by
/// the fold as a place it does not know.
fn place(id: ComponentId) -> u32 {
    u32::try_from(id.index()).unwrap_or(u32::MAX)
}

/// One pass over the records of a run.
#[derive(Debug)]
pub(crate) struct Bridge<'a> {
    places: &'a Places,
    fold: KpiFold,
    dimensions: Vec<&'static str>,
    names: BTreeMap<(u16, u16), String>,
}

impl<'a> Bridge<'a> {
    pub(crate) fn new(places: &'a Places) -> Self {
        Self {
            places,
            fold: KpiFold::new(&places.specs(), SimTime::START),
            dimensions: Vec::new(),
            names: BTreeMap::new(),
        }
    }

    /// The position of `dimension`, added at its first use.
    fn dimension(&mut self, dimension: &'static str) -> u16 {
        let position = self
            .dimensions
            .iter()
            .position(|known| *known == dimension)
            .unwrap_or_else(|| {
                self.dimensions.push(dimension);
                self.dimensions.len() - 1
            });
        u16::try_from(position).unwrap_or(u16::MAX)
    }

    /// What a message states, about `resource` and the item it carries.
    fn message<D: Domain>(
        &mut self,
        message: &D::Message,
        time: SimTime,
        resource: ComponentId,
        item: Option<u64>,
    ) -> Result<(), BridgeError> {
        let mut result = Ok(());
        D::read_message(message, &mut |fact: MessageFact<'_>| {
            if result.is_ok() {
                result = self.message_fact(fact, time, resource, item);
            }
        });
        result
    }

    fn message_fact(
        &mut self,
        fact: MessageFact<'_>,
        time: SimTime,
        resource: ComponentId,
        item: Option<u64>,
    ) -> Result<(), BridgeError> {
        match fact {
            MessageFact::State { name, in_progress } => {
                let at = self.places.0.get(resource.index());
                let state = at.and_then(|at| at.states.iter().position(|known| *known == name));
                let Some(state) = state else {
                    return Err(BridgeError::UndeclaredState {
                        place: resource.index(),
                        type_name: at.map_or_else(String::new, |at| at.type_name.clone()),
                        name: name.to_owned(),
                        time,
                        declared: at.map_or_else(String::new, |at| at.states.join(", ")),
                    });
                };
                self.fold.push(Fact::State {
                    time,
                    resource: place(resource),
                    state: u16::try_from(state).unwrap_or(u16::MAX),
                    in_progress,
                })?;
            }
            MessageFact::Class { dimension, value } => {
                let dimension = self.dimension(dimension);
                if let Some(item) = item {
                    self.fold.push(Fact::Class {
                        item,
                        dimension,
                        class: value,
                    })?;
                }
            }
            MessageFact::ClassName {
                dimension,
                value,
                name,
            } => {
                let dimension = self.dimension(dimension);
                self.names.insert((dimension, value), name.to_owned());
            }
        }
        Ok(())
    }

    /// Folds one record.
    pub(crate) fn push<D: Domain>(
        &mut self,
        record: &Record<D::Message>,
    ) -> Result<(), BridgeError> {
        match record {
            Record::Event {
                time,
                target,
                from,
                message,
                item,
                ..
            } => {
                let carried = item.map(taktsim_core::ItemId::as_u64);
                self.message::<D>(message, *time, *target, carried)?;
                if let Some(item) = carried
                    && from != target
                {
                    self.fold.push(Fact::Moved {
                        time: *time,
                        item,
                        from: place(*from),
                        to: place(*target),
                    })?;
                }
            }
            Record::ItemCreated { time, item, by } => self.fold.push(Fact::Created {
                time: *time,
                item: item.as_u64(),
                by: place(*by),
            })?,
            Record::ItemDisposed {
                time,
                item,
                by,
                how,
            } => {
                let (time, item, by) = (*time, item.as_u64(), place(*by));
                self.fold.push(match how {
                    Disposal::Delivered => Fact::Delivered { time, item, by },
                    Disposal::Scrapped | Disposal::Consumed => Fact::Removed { time, item, by },
                })?;
            }
            Record::Note { time, by, message } => {
                self.message::<D>(message, *time, *by, None)?;
            }
        }
        Ok(())
    }

    /// What the run's records say, over `[0, end]`.
    pub(crate) fn finish(self, end: SimTime) -> Result<KpiFacts, BridgeError> {
        Ok(KpiFacts {
            report: self.fold.finish(end)?,
            dimensions: self.dimensions,
            names: self.names,
        })
    }
}

/// The report of a run, with the names its classes go by.
#[derive(Debug, Clone)]
pub(crate) struct KpiFacts {
    pub(crate) report: KpiReport,
    dimensions: Vec<&'static str>,
    names: BTreeMap<(u16, u16), String>,
}

/// One class: its dimension's name, its own name, and what `place` delivered of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassRow {
    pub(crate) dimension: &'static str,
    pub(crate) name: String,
    pub(crate) delivered: u64,
}

impl KpiFacts {
    /// The class rows of the place at `index`: per dimension with two or more classes
    /// named or seen, sorted by dimension and name. A class named at the start and not
    /// delivered is a row with 0, so runs of one model have the same rows.
    pub(crate) fn class_rows(&self, index: usize) -> Vec<ClassRow> {
        let delivered = self
            .report
            .place(index)
            .map(taktsim_stats::PlaceKpis::delivered_by_class);
        let seen = delivered.iter().flat_map(|map| map.keys().copied());
        let classes: BTreeSet<(u16, u16)> = self.names.keys().copied().chain(seen).collect();
        let mut rows = Vec::new();
        for (position, dimension) in self.dimensions.iter().enumerate() {
            let position = u16::try_from(position).unwrap_or(u16::MAX);
            let of_this: Vec<(u16, u16)> = classes
                .iter()
                .copied()
                .filter(|(d, _)| *d == position)
                .collect();
            if of_this.len() >= 2 {
                rows.extend(of_this.into_iter().map(|key| {
                    ClassRow {
                        dimension,
                        name: self
                            .names
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| key.1.to_string()),
                        delivered: delivered
                            .and_then(|map| map.get(&key).copied())
                            .unwrap_or(0),
                    }
                }));
            }
        }
        rows.sort_by(|a, b| (a.dimension, &a.name).cmp(&(b.dimension, &b.name)));
        rows
    }
}
