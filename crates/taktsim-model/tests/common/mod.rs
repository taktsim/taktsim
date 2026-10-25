//! The typing environment shared by the expression tests: unit attributes
//! of every type, as an edge condition sees them, plus the observables a
//! KPI or constraint sees.

use taktsim_model::expr::{Arity, Dimension, Signature, Type, TypeEnv};

#[allow(clippy::unwrap_used)] // test helper; every name in it is valid
pub fn env() -> TypeEnv {
    let enumeration = |name: &str| Type::Enum(name.to_owned());
    let observable = |arity, dimension| Signature::new(arity, Type::Quantity(dimension));
    TypeEnv::new()
        .with_enum("variant", ["basic", "premium", "deluxe"])
        .unwrap()
        .with_enum("product", ["frame", "frame-assembly"])
        .unwrap()
        .with_enum("quality", ["ok", "rework", "scrap"])
        .unwrap()
        .with_path("variant", enumeration("variant"))
        .unwrap()
        .with_path("product", enumeration("product"))
        .unwrap()
        .with_path("quality", enumeration("quality"))
        .unwrap()
        .with_path("unit.priority", Type::Int)
        .unwrap()
        .with_path("unit.rank", Type::Int)
        .unwrap()
        .with_path("unit.age", Type::Quantity(Dimension::Time))
        .unwrap()
        .with_path("unit.batch", Type::Quantity(Dimension::Count))
        .unwrap()
        .with_path("unit.weight", Type::Number)
        .unwrap()
        .with_path("unit.reworked", Type::Bool)
        .unwrap()
        .with_path("unit.label", Type::String)
        .unwrap()
        .with_path("unit.share", Type::Quantity(Dimension::Ratio))
        .unwrap()
        .with_path("unit.rate", Type::Quantity(Dimension::Rate))
        .unwrap()
        .with_function("throughput", observable(Arity::Exactly(1), Dimension::Rate))
        .unwrap()
        .with_function(
            "wip",
            observable(Arity::Exactly(1), Dimension::Count).accepting(["Buffer"]),
        )
        .unwrap()
        .with_function("lead_time", observable(Arity::Exactly(1), Dimension::Time))
        .unwrap()
        .with_function(
            "state_shares",
            observable(Arity::AtLeast(1), Dimension::Ratio).accepting(["Machine"]),
        )
        .unwrap()
        .with_component("sink", "Sink")
        .unwrap()
        .with_component("buf1", "Buffer")
        .unwrap()
        .with_component("cut", "Machine")
        .unwrap()
        .with_component("weld-1", "Machine")
        .unwrap()
}
