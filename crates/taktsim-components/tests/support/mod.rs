//! What the tests of the components share: loading a model, running it into a
//! trace, replaying it, and reading the trace back.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)] // Each test crate uses a part.

use taktsim_components::{
    DispatchDecision, Flow, FlowMessage, ItemAttrs, MachineStateNote, OfferedUnit, Registry,
    RouteDecision, RouteRule, RunOptions, RunReport, SourceStateNote, TargetStatus, VariantId,
    standard,
};
use taktsim_core::{ComponentId, Disposal, EngineConfig, ItemId};
use taktsim_model::{Format, LoadErrors, Loader, Model};
use taktsim_trace::{Record, TraceReader, replay};
use taktsim_units::{SimDuration, SimTime};

pub fn registry() -> Registry<Flow> {
    standard().unwrap()
}

pub fn t(text: &str) -> SimTime {
    text.parse().unwrap()
}

pub fn d(text: &str) -> SimDuration {
    text.parse().unwrap()
}

/// A model document from the text of its `components` and `flow`.
pub fn document(horizon: &str, components: &str, flow: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: test}}\nexperiment: {{type: terminating, horizon: {horizon}}}\n\
         components:\n{components}\nflow:\n{flow}\n"
    )
}

pub fn try_load(yaml: &str) -> Result<Model, LoadErrors> {
    Loader::new(registry().catalog()).load_str(yaml, Format::Yaml)
}

pub fn load(yaml: &str) -> Model {
    match try_load(yaml) {
        Ok(model) => model,
        Err(errors) => panic!("{yaml}\n{errors}"),
    }
}

/// A run, with what its trace says.
pub struct Run {
    pub names: Vec<String>,
    pub records: Vec<Record<FlowMessage>>,
    pub report: RunReport,
    pub bytes: Vec<u8>,
    pub end: SimTime,
}

/// An event as it was delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub time: SimTime,
    pub from: String,
    pub to: String,
    pub message: FlowMessage,
    pub item: Option<ItemId>,
}

/// Runs `yaml` with `seed` into a trace, checks that it replays, reads it back.
pub fn run(yaml: &str, seed: u64) -> Run {
    let model = load(yaml);
    run_model(&model, seed)
}

pub fn run_model(model: &Model, seed: u64) -> Run {
    let registry = registry();
    // A limit no test comes near, so an instant that never ends fails the test.
    let (bytes, report) = registry
        .prepare(model, RunOptions::new().seed(seed).max_events(20_000_000))
        .unwrap()
        .run_traced(Vec::new())
        .unwrap();
    let mut engine = registry
        .build(model.document(), EngineConfig::new(seed))
        .unwrap();
    replay(TraceReader::new(bytes.as_slice()).unwrap(), &mut engine).unwrap();
    read(bytes, report)
}

pub fn read(bytes: Vec<u8>, report: RunReport) -> Run {
    let mut reader = TraceReader::<_, FlowMessage>::new(bytes.as_slice()).unwrap();
    let names = reader.header().components.clone();
    let mut records = Vec::new();
    while let Some(record) = reader.next_record().unwrap() {
        records.push(record);
    }
    let end = reader.summary().unwrap().end.unwrap().time;
    Run {
        names,
        records,
        report,
        bytes,
        end,
    }
}

impl Run {
    pub fn id(&self, name: &str) -> ComponentId {
        let index = self.names.iter().position(|n| n == name).unwrap();
        ComponentId::from_index(index)
    }

    pub fn name(&self, id: ComponentId) -> &str {
        &self.names[id.index()]
    }

    /// Every delivered event, in order.
    pub fn events(&self) -> Vec<Delivered> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Event {
                    time,
                    target,
                    from,
                    message,
                    item,
                    ..
                } => Some(Delivered {
                    time: *time,
                    from: self.name(*from).to_owned(),
                    to: self.name(*target).to_owned(),
                    message: message.clone(),
                    item: *item,
                }),
                Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// The events delivered to `name` whose message `pick` accepts.
    pub fn delivered_to(&self, name: &str, pick: impl Fn(&FlowMessage) -> bool) -> Vec<Delivered> {
        self.events()
            .into_iter()
            .filter(|e| e.to == name && pick(&e.message))
            .collect()
    }

    /// The parts that arrived at `name`.
    pub fn arrivals(&self, name: &str) -> Vec<Delivered> {
        self.delivered_to(name, |m| matches!(m, FlowMessage::Arrive(_)))
    }

    /// The times of the events delivered to `name` whose message `pick`
    /// accepts.
    pub fn times(&self, name: &str, pick: impl Fn(&FlowMessage) -> bool) -> Vec<SimTime> {
        self.delivered_to(name, pick)
            .into_iter()
            .map(|e| e.time)
            .collect()
    }

    /// Every machine note of `name`, in order.
    pub fn machine_notes(&self, name: &str) -> Vec<(SimTime, MachineStateNote)> {
        let id = self.id(name);
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Note {
                    time,
                    by,
                    message: FlowMessage::MachineState(note),
                } if *by == id => Some((*time, *note)),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// Every Source note of `name`, in order.
    pub fn source_notes(&self, name: &str) -> Vec<(SimTime, SourceStateNote)> {
        let id = self.id(name);
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Note {
                    time,
                    by,
                    message: FlowMessage::SourceState(note),
                } if *by == id => Some((*time, *note)),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// The time and item of each part delivered to a sink, in order.
    pub fn delivered(&self) -> Vec<(SimTime, ItemId)> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::ItemDisposed {
                    time,
                    item,
                    how: Disposal::Delivered,
                    ..
                } => Some((*time, *item)),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// The time-weighted share of `name`'s states, by code, to the stated end.
    pub fn state_shares(&self, name: &str) -> [f64; 6] {
        let notes = self.machine_notes(name);
        let mut time = [0.0; 6];
        for pair in notes.windows(2) {
            time[usize::from(pair[0].1.state.code())] +=
                (pair[1].0.as_secs_f64()) - pair[0].0.as_secs_f64();
        }
        let (last_time, last) = notes.last().unwrap();
        time[usize::from(last.state.code())] += self.end.as_secs_f64() - last_time.as_secs_f64();
        let total = self.end.as_secs_f64();
        time.map(|x| x / total)
    }

    /// The variant names the trace states.
    pub fn variant_name(&self, id: VariantId) -> Option<String> {
        self.records.iter().find_map(|r| match r {
            Record::Note {
                message: FlowMessage::VariantName { id: named, name },
                ..
            } if *named == id => Some(name.clone()),
            Record::Event { .. }
            | Record::ItemCreated { .. }
            | Record::ItemDisposed { .. }
            | Record::Note { .. }
            | Record::Decision { .. } => None,
        })
    }
}

impl Run {
    /// Every routing record, with its time and who wrote it, in order.
    pub fn routes(&self) -> Vec<(SimTime, ComponentId, RouteDecision)> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Decision {
                    time,
                    by,
                    message: FlowMessage::RouteDecision(record),
                } => Some((*time, *by, (**record).clone())),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// Every dispatching record, with its time and who wrote it, in order.
    pub fn dispatches(&self) -> Vec<(SimTime, ComponentId, DispatchDecision)> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Decision {
                    time,
                    by,
                    message: FlowMessage::DispatchDecision(record),
                } => Some((*time, *by, (**record).clone())),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// Every `Status` a holder heard from a target: time, from, to, status.
    pub fn statuses(&self) -> Vec<(SimTime, String, String, TargetStatus)> {
        self.records
            .iter()
            .filter_map(|r| match r {
                Record::Event {
                    time,
                    target,
                    from,
                    message: FlowMessage::Status(status),
                    ..
                } => Some((
                    *time,
                    self.name(*from).to_owned(),
                    self.name(*target).to_owned(),
                    **status,
                )),
                Record::Event { .. }
                | Record::ItemCreated { .. }
                | Record::ItemDisposed { .. }
                | Record::Note { .. }
                | Record::Decision { .. } => None,
            })
            .collect()
    }

    /// The time and item of each part sent from `from` to `to`.
    pub fn sent(&self, from: &str, to: &str) -> Vec<(SimTime, ItemId)> {
        self.events()
            .into_iter()
            .filter(|e| {
                e.from == from
                    && e.to == to
                    && matches!(e.message, FlowMessage::Arrive(_) | FlowMessage::Return(_))
            })
            .filter_map(|e| e.item.map(|item| (e.time, item)))
            .collect()
    }

    /// The name of the variant of each part, by its order of arrival at `name`.
    pub fn arrival_variants(&self, name: &str) -> Vec<String> {
        self.arrivals(name)
            .iter()
            .filter_map(|e| arrive_attrs(e).and_then(|attrs| self.variant_name(attrs.variant)))
            .collect()
    }
}

/// Recomputes every record of a run from its own fields (ADR-0023 §9): each gives its
/// own choice, except that `first_free` states and does not explain, and a status rule
/// is exempt where an `Accept` crossed a move, which happens only at that instant.
pub fn check_records(r: &Run) {
    for (time, by, record) in r.routes() {
        let chosen = usize::from(record.chosen);
        assert!(chosen < record.candidates.len(), "{record:?}");
        match record.rule {
            RouteRule::When => {
                assert_eq!(chosen, record.candidates.len() - 1, "{record:?}");
                assert_eq!(
                    u64::from(record.edge),
                    u64::from(record.candidates[chosen].id)
                );
                for (i, c) in record.candidates.iter().enumerate() {
                    assert_eq!(c.id as usize, i, "{record:?}");
                    assert_eq!(c.key, u64::from(i == chosen), "{record:?}");
                    assert_eq!(c.room, None, "{record:?}");
                }
                assert_eq!((record.detail, record.crossed), (0, false), "{record:?}");
            }
            RouteRule::Split => {
                let draw = f64::from_bits(record.detail);
                let mut total = 0.0;
                let mut picked = None;
                for (i, c) in record.candidates.iter().enumerate() {
                    total += f64::from_bits(c.key);
                    if picked.is_none() && draw < total {
                        picked = Some(i);
                    }
                    assert_eq!(c.room, None, "{record:?}");
                }
                assert!(total > 0.999_999 && total < 1.000_001, "{record:?}");
                // The last takes what rounding leaves.
                let picked = picked.unwrap_or(record.candidates.len() - 1);
                assert_eq!(picked, chosen, "{record:?}");
                assert!(!record.crossed, "{record:?}");
            }
            RouteRule::FirstFree => {
                for c in &record.candidates {
                    assert_eq!((c.key, c.room), (0, None), "{record:?}");
                }
                assert_eq!((record.detail, record.crossed), (0, false), "{record:?}");
            }
            RouteRule::Preferred | RouteRule::ShortestQueue | RouteRule::LowestUtilization => {
                if record.rule == RouteRule::Preferred {
                    for (i, c) in record.candidates.iter().enumerate() {
                        assert_eq!(c.key, i as u64, "{record:?}");
                    }
                }
                assert!(record.detail <= time.as_nanos(), "{record:?}");
                if record.crossed {
                    // Only at the instant of a move.
                    assert_eq!(record.detail, time.as_nanos(), "{record:?}");
                } else {
                    let best = record
                        .candidates
                        .iter()
                        .enumerate()
                        .min_by_key(|(i, c)| (c.room != Some(true), c.key, *i))
                        .map(|(i, _)| i);
                    assert_eq!(best, Some(chosen), "by {}: {record:?}", r.name(by));
                }
            }
        }
    }
    for (_, by, record) in r.dispatches() {
        let list = &record.candidates;
        assert!(!list.is_empty() && list.len() <= 1024, "{record:?}");
        assert!(record.total >= list.len() as u64, "{record:?}");
        if record.total <= 1024 {
            assert_eq!(record.total, list.len() as u64, "{record:?}");
        }
        for pair in list.windows(2) {
            assert!(
                (pair[0].key, pair[0].since) <= (pair[1].key, pair[1].since),
                "by {}: {record:?}",
                r.name(by)
            );
        }
        // The chosen unit came, and has an item.
        assert!(list[0].item.is_some(), "{record:?}");
    }
}

/// What a part carried, if the event is its arrival.
pub fn arrive_attrs(e: &Delivered) -> Option<ItemAttrs> {
    if let FlowMessage::Arrive(attrs) = e.message {
        Some(attrs)
    } else {
        None
    }
}

/// What an offer says about its unit, if the event is such an offer.
pub fn offered_unit(e: &Delivered) -> Option<OfferedUnit> {
    if let FlowMessage::OfferOf(unit) = &e.message {
        Some(**unit)
    } else {
        None
    }
}

pub fn is_arrive(message: &FlowMessage) -> bool {
    matches!(message, FlowMessage::Arrive(_))
}
