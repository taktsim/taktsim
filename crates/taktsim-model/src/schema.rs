//! The JSON Schema of a model document (spec §4, ADR-0020 §11).
//!
//! It is generated from the decoders: the model file and an included file
//! are described by running the functions that read them, every value by
//! its own `Decode::schema`, and each component type by describing its
//! `ComponentParams::decode`. So it describes the input format, shorthands
//! included, exactly as the loader reads it. The loader stays the
//! authority for rules a schema cannot state (references to existing ids,
//! unique ids, expression types, the exactness of a quantity).

use taktsim_units::{Count, Ratio, SimDuration};

use crate::component::{Catalog, ComponentRef};
use crate::decode::Schemas;
use crate::distribution::Distribution;
use crate::document::{FORMAT_VERSION, read_main_fields};
use crate::json::Json;
use crate::load::read_fragment_fields;

/// The JSON Schema (draft 2020-12) of a model document whose component
/// types and observables are those of `catalog`.
///
/// Object keys are sorted, so the schema is the same text everywhere
/// ([`Json::to_pretty`]). It accepts what the loader can read as far as a
/// schema can say it; the loader also checks references, unique ids,
/// expression types and the exactness of quantities.
///
/// ```
/// use taktsim_model::{Catalog, model_schema};
///
/// let schema = model_schema(&Catalog::new());
/// assert_eq!(schema["properties"]["format"]["const"], "1.0");
/// assert_eq!(schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
/// ```
#[must_use]
pub fn model_schema(catalog: &Catalog) -> Json {
    let mut schemas = Schemas::new(catalog.observables().map(str::to_owned).collect());
    // Shared definitions for component types, also when none uses them.
    let _ = schemas.of::<SimDuration>();
    let _ = schemas.of::<Ratio>();
    let _ = schemas.of::<Count>();
    let _ = schemas.of::<Distribution>();
    let _ = schemas.of::<ComponentRef>();
    let entries = catalog.entry_schemas(&mut schemas);
    schemas.set_components(entries);

    let root = schemas.describe(|fields| {
        let _ = read_main_fields(fields, catalog);
    });
    let included = schemas
        .describe(|fields| {
            let _ = read_fragment_fields(fields, catalog);
        })
        .with("description", &"A file named in `include:`.");
    schemas.define("IncludedFile", included);

    // Without `include:`, the model file itself has the components and
    // the flow (the loader's rule after joining the files).
    let without_include = Json::object([(
        "not",
        Json::object([("required", Json::Array(vec![Json::from("include")]))]),
    )]);
    let at_least_one = Json::object([("minItems", Json::from(1))]);
    let then = Json::object([
        (
            "required",
            Json::Array(vec![Json::from("components"), Json::from("flow")]),
        ),
        (
            "properties",
            Json::object([("components", at_least_one.clone()), ("flow", at_least_one)]),
        ),
    ]);
    root.with("$schema", &"https://json-schema.org/draft/2020-12/schema")
        .with("title", &format!("taktsim model, format {FORMAT_VERSION}"))
        .with(
            "description",
            &"A taktsim model (spec §4). Generated from the Rust types; do not edit.",
        )
        .with("if", &without_include)
        .with("then", &then)
        .with("$defs", &Json::Object(schemas.into_defs()))
}
