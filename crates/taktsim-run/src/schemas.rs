//! The schemas a client prints by name (ADR-0024 §10): the CLI's `taktsim schema`
//! and the Python bindings pick them here, the same way.

use taktsim_model::Json;

use crate::result::{diagnostic_schema, result_schema, validation_schema};

/// The names [`schema_named`] answers besides the component types, in the order
/// they are listed.
///
/// ```
/// assert_eq!(taktsim_run::SCHEMA_NAMES[0], "all");
/// ```
pub const SCHEMA_NAMES: [&str; 5] = ["all", "model", "result", "validation", "error"];

/// The component types of the model schema `model`, in its order.
fn component_entries(model: &Json) -> impl Iterator<Item = (&str, &Json)> {
    model["$defs"]["Component"]["oneOf"]
        .as_array()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| Some((entry["properties"]["type"]["const"].as_str()?, entry)))
}

/// The schema called `name`, given the model schema `model` of a package (for the
/// standard catalog, `taktsim_components::standard_schema`): `all` and `model` give
/// `model` itself; `result`, `validation` and `error` the documents of this crate;
/// a component type its entry of `$defs.Component`, with the `$defs` its references
/// need. `None` for any other name; [`schema_names`] lists those there are.
///
/// ```
/// use taktsim_run::schema_named;
///
/// let model = taktsim_components::standard_schema()?;
/// let machine = schema_named(&model, "Machine").ok_or("a type")?;
/// assert_eq!(machine["title"], "Machine");
/// assert!(machine["$defs"]["Distribution"].as_object().is_some());
/// assert_eq!(schema_named(&model, "all"), Some(model.clone()));
/// assert_eq!(schema_named(&model, "Conveyor"), None);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use]
pub fn schema_named(model: &Json, name: &str) -> Option<Json> {
    match name {
        "all" | "model" => Some(model.clone()),
        "result" => Some(result_schema()),
        "validation" => Some(validation_schema()),
        "error" => Some(diagnostic_schema()),
        type_name => component_entries(model)
            .find(|(found, _)| *found == type_name)
            .map(|(_, entry)| {
                entry
                    .clone()
                    .with("$schema", &"https://json-schema.org/draft/2020-12/schema")
                    .with("title", &type_name)
                    .with("$defs", &model["$defs"])
            }),
    }
}

/// Every name [`schema_named`] answers for the model schema `model`:
/// [`SCHEMA_NAMES`], then its component types in its order.
///
/// ```
/// let model = taktsim_components::standard_schema()?;
/// let names = taktsim_run::schema_names(&model);
/// assert_eq!(names[..5], taktsim_run::SCHEMA_NAMES);
/// assert_eq!(names[5..], ["Buffer", "Machine", "Sink", "Source"]);
/// # Ok::<(), taktsim_model::CatalogError>(())
/// ```
#[must_use]
pub fn schema_names(model: &Json) -> Vec<&str> {
    SCHEMA_NAMES
        .into_iter()
        .chain(component_entries(model).map(|(name, _)| name))
        .collect()
}
