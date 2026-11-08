//! What the tests of the components share: loading a model, running it into a
//! trace, replaying it, and reading the trace back.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)] // Each test crate uses a part.

use taktsim_components::{
    Flow, FlowMessage, MachineStateNote, Registry, RunOptions, RunReport, SourceStateNote,
    VariantId, standard,
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
    let (bytes, report) = registry
        .prepare(model, RunOptions::new().seed(seed))
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
                Record::ItemCreated { .. } | Record::ItemDisposed { .. } | Record::Note { .. } => {
                    None
                }
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
                | Record::Note { .. } => None,
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
                | Record::Note { .. } => None,
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
                | Record::Note { .. } => None,
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
            | Record::Note { .. } => None,
        })
    }
}

pub fn is_arrive(message: &FlowMessage) -> bool {
    matches!(message, FlowMessage::Arrive(_))
}
