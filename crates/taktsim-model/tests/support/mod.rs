//! A catalog for the loader tests: stand-ins for the component types and
//! observables of spec §4, with the parameters its examples use.
//!
//! These are test fixtures, not the component library: the real Source,
//! Machine, Buffer and Sink, with their own parameters and rules, arrive
//! with M5, and the observables' result types are decided in M8
//! (ADR-0015, answer 5). The loader does not depend on any of them.

#![allow(dead_code, clippy::unwrap_used)] // Shared by several test crates; each uses a part.

use std::path::{Path, PathBuf};

use taktsim_model::expr::{Arity, Dimension, Signature, Type};
use taktsim_model::{
    Catalog, ComponentParams, ComponentRef, Distribution, Encode, Fields, Json, LoadErrors, Record,
};
use taktsim_units::{Count, Ratio, SimDuration};

#[derive(Debug)]
pub struct Source {
    pub interarrival: Distribution,
    pub batch: Option<Count>,
}

impl ComponentParams for Source {
    const TYPE: &'static str = "Source";
    const EXAMPLE: &'static str =
        "{id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let interarrival = fields.required("interarrival");
        let batch = fields.optional("batch");
        Some(Self {
            interarrival: interarrival?,
            batch: batch?,
        })
    }
}

#[derive(Debug)]
pub struct Setup {
    pub per_variant: SimDuration,
}

impl Record for Setup {
    const EXPECTED: &'static str = "a setup such as `{per_variant: 4min}`";
    const EXAMPLE: &'static str = "{per_variant: 4min}";

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            per_variant: fields.required("per_variant")?,
        })
    }
}

#[derive(Debug)]
pub struct Machine {
    pub cycle_time: Distribution,
    pub mtbf: Option<Distribution>,
    pub mttr: Option<Distribution>,
    pub setup: Option<Setup>,
    pub capacity: Option<Count>,
}

impl ComponentParams for Machine {
    const TYPE: &'static str = "Machine";
    const EXAMPLE: &'static str = "{id: cut, type: Machine, cycle_time: 42s}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let cycle_time = fields.required("cycle_time");
        let mtbf = fields.optional("mtbf");
        let mttr = fields.optional("mttr");
        let setup = fields.optional("setup");
        let capacity = fields.optional("capacity");
        Some(Self {
            cycle_time: cycle_time?,
            mtbf: mtbf?,
            mttr: mttr?,
            setup: setup?,
            capacity: capacity?,
        })
    }
}

#[derive(Debug)]
pub struct Buffer {
    pub capacity: Count,
}

impl ComponentParams for Buffer {
    const TYPE: &'static str = "Buffer";
    const EXAMPLE: &'static str = "{id: buf1, type: Buffer, capacity: 20 parts}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            capacity: fields.required("capacity")?,
        })
    }
}

#[derive(Debug)]
pub struct Sink {}

impl ComponentParams for Sink {
    const TYPE: &'static str = "Sink";
    const EXAMPLE: &'static str = "{id: sink, type: Sink}";

    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {})
    }
}

/// A component type that refers to another component (spec §4's
/// Inspection, reduced to its reference and shares).
#[derive(Debug)]
pub struct InspectionParams {
    pub inspection_time: Distribution,
    /// Shares are percentages with their unit (ADR-0020, answer 10).
    pub scrap_rate: Option<Ratio>,
    pub rework_rate: Option<Ratio>,
    pub rework_to: ComponentRef,
}

impl ComponentParams for InspectionParams {
    const TYPE: &'static str = "Inspection";
    const EXAMPLE: &'static str =
        "{id: qc, type: Inspection, inspection_time: 40s, rework_to: asm}";

    fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let inspection_time = fields.required("inspection_time");
        let scrap_rate = fields.optional("scrap_rate");
        let rework_rate = fields.optional("rework_rate");
        let rework_to = fields.required("rework_to");
        Some(Self {
            inspection_time: inspection_time?,
            scrap_rate: scrap_rate?,
            rework_rate: rework_rate?,
            rework_to: rework_to?,
        })
    }

    fn references(&self) -> Vec<&ComponentRef> {
        vec![&self.rework_to]
    }
}

impl Encode for Source {
    fn encode(&self) -> Json {
        Json::object([])
            .with("interarrival", &self.interarrival)
            .with_optional("batch", self.batch.as_ref())
    }
}

impl Encode for Setup {
    fn encode(&self) -> Json {
        Json::object([]).with("per_variant", &self.per_variant)
    }
}

impl Encode for Machine {
    fn encode(&self) -> Json {
        Json::object([])
            .with("cycle_time", &self.cycle_time)
            .with_optional("mtbf", self.mtbf.as_ref())
            .with_optional("mttr", self.mttr.as_ref())
            .with_optional("setup", self.setup.as_ref())
            .with_optional("capacity", self.capacity.as_ref())
    }
}

impl Encode for Buffer {
    fn encode(&self) -> Json {
        Json::object([]).with("capacity", &self.capacity)
    }
}

impl Encode for Sink {
    fn encode(&self) -> Json {
        Json::object([])
    }
}

impl Encode for InspectionParams {
    fn encode(&self) -> Json {
        Json::object([])
            .with("inspection_time", &self.inspection_time)
            .with_optional("scrap_rate", self.scrap_rate.as_ref())
            .with_optional("rework_rate", self.rework_rate.as_ref())
            .with("rework_to", &self.rework_to)
    }
}

/// The catalog of the tests: the stand-ins above and four observables.
pub fn catalog() -> Catalog {
    let rate =
        Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Rate)).accepting(["Sink"]);
    let wip = Signature::new(Arity::Exactly(1), Type::Number).accepting(["Buffer"]);
    let shares = Signature::new(Arity::AtLeast(1), Type::Number).accepting(["Machine"]);
    let lead_time =
        Signature::new(Arity::Exactly(1), Type::Quantity(Dimension::Time)).accepting(["Sink"]);
    Catalog::new()
        .with::<Source>()
        .unwrap()
        .with::<Machine>()
        .unwrap()
        .with::<Buffer>()
        .unwrap()
        .with::<Sink>()
        .unwrap()
        .with::<InspectionParams>()
        .unwrap()
        .with_observable("throughput", rate)
        .unwrap()
        .with_observable("wip", wip)
        .unwrap()
        .with_observable("state_shares", shares)
        .unwrap()
        .with_observable("lead_time", lead_time)
        .unwrap()
}

/// The directory of the test data.
pub fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

/// Each error as `path @ line:column Kind`, the kind being the variant's
/// name, for comparing with an expected list.
pub fn summary(errors: &LoadErrors) -> Vec<String> {
    errors
        .iter()
        .map(|e| {
            let kind = format!("{:?}", e.kind());
            let name = kind
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_owned();
            let at = e
                .location()
                .map_or_else(|| "-".to_owned(), |l| l.to_string());
            format!("{} @ {at} {name}", e.path())
        })
        .collect()
}
