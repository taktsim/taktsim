//! The JSON Schema of a model document (spec §4, ADR-0020 §11).
//!
//! The schema describes the input format, shorthands included: what a
//! person or an agent may write. It is assembled here from the Rust types:
//! quantities (`taktsim-units`), distributions and component references
//! (this crate), and each component type's parameter struct (its derived
//! `JsonSchema`), next to the decoder that reads the same document. The
//! loader stays the authority: rules a schema cannot state (references to
//! existing ids, unique ids, expression types, the precision of a
//! duration) are checked only there.

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::{Map, Value, json};
use taktsim_units::{Count, Ratio, SimDuration};

use crate::component::{Catalog, ComponentRef};
use crate::distribution::Distribution;
use crate::document::FORMAT_VERSION;
use crate::error::MAX_INTEGER;

/// The pattern of a component id (ADR-0015).
const ID_PATTERN: &str = "^[A-Za-z_][A-Za-z0-9_]*(-[A-Za-z0-9_]+)*$";

/// One segment of an include path: letters, digits, `.`, `_`, `-`, but
/// not `.` or `..` alone.
const SEGMENT: &str = r"(?:[A-Za-z0-9_-]|\.[A-Za-z0-9_-]|\.\.[A-Za-z0-9._-])[A-Za-z0-9._-]*";

impl JsonSchema for ComponentRef {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ComponentId".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "taktsim_model::ComponentId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": ID_PATTERN,
            "description": "A component id: letters, digits and `_`, starting with a letter or `_`, \
                            optionally joined by single `-` (ADR-0015).",
            "examples": ["cut", "weld-1"],
        })
    }
}

impl JsonSchema for Distribution {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Distribution".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "taktsim_model::Distribution".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let duration = generator.subschema_for::<SimDuration>().to_value();
        let positive = json!({"type": "number", "exclusiveMinimum": 0});
        let object = |name: &str, params: &[(&str, &Value)], description: &str| {
            let mut properties = Map::new();
            properties.insert("dist".to_owned(), json!({"const": name}));
            for (key, schema) in params {
                properties.insert((*key).to_owned(), (*schema).clone());
            }
            let mut required = vec!["dist".to_owned()];
            required.extend(params.iter().map(|(key, _)| (*key).to_owned()));
            json!({
                "type": "object",
                "description": description,
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            })
        };
        let k = json!({"type": "integer", "minimum": 1, "maximum": u32::MAX});
        let one_of = vec![
            json!({"$ref": duration["$ref"], "description": "A constant duration."}),
            object(
                "exponential",
                &[("mean", &duration)],
                "Exponential with the given mean.",
            ),
            object(
                "normal",
                &[("mean", &duration), ("sd", &duration)],
                "Normal, truncated at zero by rejection (ADR-0013).",
            ),
            object(
                "lognormal",
                &[("mean", &duration), ("sigma", &positive)],
                "Lognormal: `mean` is the distribution's mean, `sigma` the standard deviation of its logarithm.",
            ),
            object(
                "uniform",
                &[("min", &duration), ("max", &duration)],
                "Uniform between `min` and `max`.",
            ),
            object(
                "triangular",
                &[("min", &duration), ("mode", &duration), ("max", &duration)],
                "Triangular with `min <= mode <= max`.",
            ),
            object(
                "gamma",
                &[("mean", &duration), ("cv", &positive)],
                "Gamma with the given mean and coefficient of variation.",
            ),
            object(
                "erlang",
                &[("mean", &duration), ("k", &k)],
                "Erlang: the sum of `k` exponential phases.",
            ),
            object(
                "weibull",
                &[("scale", &duration), ("shape", &positive)],
                "Weibull.",
            ),
        ];
        let mut schema = Schema::default();
        schema.insert(
            "description".to_owned(),
            "A duration (a constant), or a distribution named by `dist` with named parameters (ADR-0003)."
                .into(),
        );
        schema.insert("oneOf".to_owned(), Value::Array(one_of));
        schema
    }
}

/// The JSON Schema (draft 2020-12) of a model document whose component
/// types and observables are those of `catalog`.
///
/// Keys are sorted and every value is final, so the schema is the same
/// text on every platform. It accepts what the loader can read as far as a
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
pub fn model_schema(catalog: &Catalog) -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let duration = generator.subschema_for::<SimDuration>().to_value();
    let ratio = generator.subschema_for::<Ratio>().to_value();
    generator.subschema_for::<Count>();
    generator.subschema_for::<Distribution>();
    let id = generator.subschema_for::<ComponentRef>().to_value();
    let components = catalog.entry_schemas(&mut generator);
    let mut defs = generator.take_definitions(true);

    let lists = json!({
        "components": {"type": "array", "items": {"$ref": "#/$defs/Component"}},
        "flow": {"type": "array", "items": {"$ref": "#/$defs/FlowEntry"}},
        "observe": {"type": "array", "items": {"$ref": "#/$defs/Observable"}},
    });
    let (edge_end, flow_entry) = flow_schemas(&id);
    let included_file = json!({
        "type": "object",
        "description": "A file named in `include:`.",
        "properties": lists,
        "additionalProperties": false,
    });
    for (name, schema) in [
        ("Observable", observable_schema(catalog)),
        ("Component", component_schema(&components)),
        ("EdgeEnd", edge_end),
        ("FlowEntry", flow_entry),
        ("Include", include_schema()),
        ("Model", model_section()),
        ("Experiment", experiment_section(&duration, &ratio)),
        ("IncludedFile", included_file),
    ] {
        defs.insert(name.to_owned(), schema);
    }

    let mut properties = Map::new();
    properties.insert(
        "format".to_owned(),
        json!({"const": FORMAT_VERSION, "description": "The format version (ADR-0008); quote it."}),
    );
    properties.insert("model".to_owned(), json!({"$ref": "#/$defs/Model"}));
    properties.insert(
        "experiment".to_owned(),
        json!({"$ref": "#/$defs/Experiment"}),
    );
    properties.insert(
        "include".to_owned(),
        json!({"type": "array", "items": {"$ref": "#/$defs/Include"}}),
    );
    if let Value::Object(lists) = lists {
        properties.extend(lists);
    }
    let mut schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": format!("taktsim model, format {FORMAT_VERSION}"),
        "description": "A taktsim model (spec §4). Generated from the Rust types; do not edit.",
        "type": "object",
        "properties": properties,
        "required": ["format", "model", "experiment"],
        "additionalProperties": false,
        // Without `include:`, the model file itself has the components
        // and the flow.
        "if": {"not": {"required": ["include"]}},
        "then": {
            "required": ["components", "flow"],
            "properties": {"components": {"minItems": 1}, "flow": {"minItems": 1}},
        },
        "$defs": defs,
    });
    without_null(&mut schema);
    sorted(schema)
}

/// An observable: a call of one of the catalog's functions; nothing if
/// the catalog has none.
fn observable_schema(catalog: &Catalog) -> Value {
    let functions = catalog.observables().collect::<Vec<_>>();
    let Some(first) = functions.first() else {
        return json!({
            "description": "An observable; this catalog declares none, so none is valid.",
            "not": {},
        });
    };
    json!({
        "type": "string",
        "description": format!(
            "An observable: a call of {} on component ids, e.g. `{first}(sink)` (ADR-0009, ADR-0015). \
             The loader checks its arguments and type.",
            functions.join(", "),
        ),
        "pattern": format!(r"^[ \t]*({})[ \t]*\(", functions.join("|")),
    })
}

/// One of the component entries; nothing if no type is registered.
fn component_schema(entries: &[Value]) -> Value {
    if entries.is_empty() {
        return json!({
            "description": "A component: `id`, `type` and the parameters of that type. \
                            No component type is registered in this catalog, so none is valid.",
            "not": {},
        });
    }
    json!({
        "description": "A component: `id`, `type` and the parameters of that type.",
        "oneOf": entries,
    })
}

/// The end of an edge, and a flow entry: a chain or an edge.
fn flow_schemas(id: &Value) -> (Value, Value) {
    let edge_end = json!({
        "description": "A component id, or a list of different ones.",
        "oneOf": [
            id,
            {"type": "array", "items": id, "minItems": 1, "uniqueItems": true},
        ],
    });
    let flow_entry = json!({
        "description": "A chain of at least two components, or an edge from one or several components \
                        to one or several components (branch, merge, loop-back).",
        "oneOf": [
            {"type": "array", "items": id, "minItems": 2},
            {
                "type": "object",
                "properties": {"from": {"$ref": "#/$defs/EdgeEnd"}, "to": {"$ref": "#/$defs/EdgeEnd"}},
                "required": ["from", "to"],
                "additionalProperties": false,
            },
        ],
    });
    (edge_end, flow_entry)
}

/// An entry of `include:`: a path, or a path with a pinned hash.
fn include_schema() -> Value {
    let path = json!({
        "type": "string",
        "description": "A path relative to the model file, with `/` separators, in its directory or \
                        below, to a .yaml, .yml or .json file.",
        "pattern": format!(r"^(?:{SEGMENT}/)*[A-Za-z0-9._-]*\.(?:[Yy][Aa]?[Mm][Ll]|[Jj][Ss][Oo][Nn])$"),
        "examples": ["cells/welding.yaml"],
    });
    json!({
        "description": "A file whose components, flow and observables come before the model's own; \
                        optionally pinned to the SHA-256 of its content.",
        "oneOf": [
            path,
            {
                "type": "object",
                "properties": {
                    "path": path,
                    "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        ],
    })
}

/// The `model` section.
fn model_section() -> Value {
    json!({
        "type": "object",
        "description": "The model's name and defaults.",
        "properties": {
            "name": {"type": "string", "pattern": ID_PATTERN, "examples": ["line-a"]},
            "time_unit": {"enum": ["d", "h", "min", "s", "ms"]},
            "seed": {"type": "integer", "minimum": 0, "maximum": MAX_INTEGER},
        },
        "required": ["name"],
        "additionalProperties": false,
    })
}

/// The `experiment` section.
fn experiment_section(duration: &Value, ratio: &Value) -> Value {
    json!({
        "type": "object",
        "description": "How the model is run: a terminating experiment over `horizon`, with optional \
                        relative precision targets per observable (ADR-0014).",
        "properties": {
            "type": {"const": "terminating"},
            "horizon": duration,
            "precision": {
                "type": "object",
                "propertyNames": {"$ref": "#/$defs/Observable"},
                "additionalProperties": ratio,
            },
        },
        "required": ["type", "horizon"],
        "additionalProperties": false,
    })
}

/// Removes `null` from every type: a model never accepts null, while
/// derived schemas let optional fields be null.
fn without_null(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(types)) = map.get_mut("type") {
                types.retain(|t| t != "null");
                if types.len() == 1 {
                    let only = types.remove(0);
                    map.insert("type".to_owned(), only);
                }
            }
            for key in ["anyOf", "oneOf"] {
                if let Some(Value::Array(options)) = map.get_mut(key) {
                    options.retain(|o| o.get("type").is_none_or(|t| t != "null"));
                    if options.len() == 1 {
                        let only = options.remove(0);
                        map.remove(key);
                        if let Value::Object(only) = only {
                            for (k, v) in only {
                                map.entry(k).or_insert(v);
                            }
                        }
                    }
                }
            }
            map.values_mut().for_each(without_null);
        }
        Value::Array(items) => items.iter_mut().for_each(without_null),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// `value` with every object's keys in sorted order, whatever map type
/// `serde_json` was built with.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => value,
    }
}

/// The schema of one component entry of type `name`: the parameter
/// struct's object schema with `id` and `type` added, closed to other keys.
pub(crate) fn entry_schema(name: &str, params: Schema, generator: &mut SchemaGenerator) -> Value {
    let id = generator.subschema_for::<ComponentRef>().to_value();
    let mut entry = match params.to_value() {
        Value::Object(map) => map,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            Map::new()
        }
    };
    let mut properties = match entry.remove("properties") {
        Some(Value::Object(p)) => p,
        Some(_) | None => Map::new(),
    };
    properties.insert("id".to_owned(), id);
    properties.insert("type".to_owned(), json!({"const": name}));
    let mut required = vec![json!("id"), json!("type")];
    if let Some(Value::Array(own)) = entry.remove("required") {
        required.extend(own);
    }
    entry.insert("type".to_owned(), json!("object"));
    entry.insert("properties".to_owned(), Value::Object(properties));
    entry.insert("required".to_owned(), Value::Array(required));
    entry.insert("additionalProperties".to_owned(), json!(false));
    entry.remove("title");
    entry.remove("$schema");
    Value::Object(entry)
}
