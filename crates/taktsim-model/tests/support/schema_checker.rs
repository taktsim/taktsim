//! A JSON Schema checker for the tests of the schema: the keywords the
//! generator emits, and no others (`keywords` lets a test make sure of that).
//! Values are the model crate's own `Json`.
//!
//! A validator crate (`jsonschema`, `boon`) would bring a dependency under
//! MIT-0, which `deny.toml` does not allow. Also used by the tests of
//! `taktsim-components`, through `#[path]`.

#![allow(dead_code)] // Shared by several test crates; each uses a part.

use std::collections::BTreeSet;

use regex::Regex;
use taktsim_model::Json;

/// The keywords `Checker` validates.
pub const VALIDATION: [&str; 24] = [
    "$ref",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "propertyNames",
    "items",
    "minItems",
    "minProperties",
    "uniqueItems",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "const",
    "enum",
    "pattern",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
];

/// Keywords that annotate but do not validate (draft 2020-12; `format` is
/// an annotation by default).
pub const ANNOTATIONS: [&str; 7] = [
    "$schema",
    "$defs",
    "title",
    "description",
    "examples",
    "default",
    "format",
];

/// A validator for the keywords of `VALIDATION`, over a root schema with
/// local `$ref`s only.
pub struct Checker<'a> {
    pub root: &'a Json,
}

// Test helper: a schema the checker does not understand fails the test.
#[allow(clippy::panic, clippy::unwrap_used)]
impl Checker<'_> {
    fn resolve(&self, reference: &str) -> &Json {
        let name = reference.strip_prefix("#/$defs/").unwrap();
        let target = &self.root["$defs"][name];
        assert!(*target != Json::Null, "{reference}");
        target
    }

    pub fn valid(&self, schema: &Json, value: &Json) -> bool {
        match schema {
            Json::Bool(b) => *b,
            Json::Object(map) => map
                .iter()
                .all(|(keyword, arg)| self.keyword(keyword, arg, value, schema)),
            Json::Null | Json::Integer(_) | Json::Float(_) | Json::String(_) | Json::Array(_) => {
                panic!("not a schema: {schema}")
            }
        }
    }

    #[allow(clippy::too_many_lines)] // One arm per keyword.
    fn keyword(&self, keyword: &str, arg: &Json, value: &Json, schema: &Json) -> bool {
        let number = value.as_f64();
        match keyword {
            "$ref" => self.valid(self.resolve(arg.as_str().unwrap()), value),
            "type" => match arg {
                Json::String(t) => has_type(value, t),
                Json::Array(types) => types.iter().any(|t| has_type(value, t.as_str().unwrap())),
                Json::Null
                | Json::Bool(_)
                | Json::Integer(_)
                | Json::Float(_)
                | Json::Object(_) => {
                    panic!("type {arg}")
                }
            },
            "properties" => value.as_object().is_none_or(|object| {
                arg.as_object()
                    .unwrap()
                    .iter()
                    .all(|(key, sub)| object.get(key).is_none_or(|v| self.valid(sub, v)))
            }),
            "required" => value.as_object().is_none_or(|object| {
                arg.as_array()
                    .unwrap()
                    .iter()
                    .all(|k| object.contains_key(k.as_str().unwrap()))
            }),
            "additionalProperties" => value.as_object().is_none_or(|object| {
                let declared = schema["properties"].as_object();
                object
                    .iter()
                    .filter(|(k, _)| declared.is_none_or(|d| !d.contains_key(*k)))
                    .all(|(_, v)| self.valid(arg, v))
            }),
            "propertyNames" => value.as_object().is_none_or(|object| {
                object
                    .keys()
                    .all(|k| self.valid(arg, &Json::String(k.clone())))
            }),
            "items" => value
                .as_array()
                .is_none_or(|items| items.iter().all(|v| self.valid(arg, v))),
            "minItems" => value
                .as_array()
                .is_none_or(|items| items.len() >= usize::try_from(arg_integer(arg)).unwrap()),
            "minProperties" => value
                .as_object()
                .is_none_or(|object| object.len() >= usize::try_from(arg_integer(arg)).unwrap()),
            "uniqueItems" => value.as_array().is_none_or(|items| {
                *arg != true
                    || items
                        .iter()
                        .enumerate()
                        .all(|(i, v)| !items[..i].contains(v))
            }),
            "oneOf" => {
                arg.as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| self.valid(s, value))
                    .count()
                    == 1
            }
            "anyOf" => arg.as_array().unwrap().iter().any(|s| self.valid(s, value)),
            "allOf" => arg.as_array().unwrap().iter().all(|s| self.valid(s, value)),
            "not" => !self.valid(arg, value),
            "if" => {
                let branch = if self.valid(arg, value) {
                    "then"
                } else {
                    "else"
                };
                schema
                    .as_object()
                    .unwrap()
                    .get(branch)
                    .is_none_or(|s| self.valid(s, value))
            }
            "then" | "else" => true,
            "const" => json_equal(arg, value),
            "enum" => arg.as_array().unwrap().iter().any(|e| json_equal(e, value)),
            "pattern" => value
                .as_str()
                .is_none_or(|s| Regex::new(arg.as_str().unwrap()).unwrap().is_match(s)),
            "minimum" => number.is_none_or(|x| x >= arg.as_f64().unwrap()),
            "maximum" => number.is_none_or(|x| x <= arg.as_f64().unwrap()),
            "exclusiveMinimum" => number.is_none_or(|x| x > arg.as_f64().unwrap()),
            "exclusiveMaximum" => number.is_none_or(|x| x < arg.as_f64().unwrap()),
            keyword if ANNOTATIONS.contains(&keyword) => true,
            keyword => panic!("unsupported keyword {keyword}"),
        }
    }
}

/// An integer keyword argument, such as `minItems`.
#[allow(clippy::panic)] // Test helper: the generator writes integers here.
fn arg_integer(arg: &Json) -> i128 {
    let Json::Integer(n) = arg else {
        panic!("not an integer: {arg}")
    };
    *n
}

#[allow(clippy::panic)] // Test helper: an unknown type fails the test.
fn has_type(value: &Json, t: &str) -> bool {
    match t {
        "null" => *value == Json::Null,
        "boolean" => matches!(value, Json::Bool(_)),
        "object" => value.as_object().is_some(),
        "array" => value.as_array().is_some(),
        "string" => value.as_str().is_some(),
        "number" => value.as_f64().is_some(),
        "integer" => {
            matches!(value, Json::Integer(_)) || value.as_f64().is_some_and(|x| x.fract() == 0.0)
        }
        other => panic!("type {other}"),
    }
}

/// JSON equality, with numbers compared by value.
fn json_equal(a: &Json, b: &Json) -> bool {
    if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
        // JSON numbers are equal when their values are: compare exactly.
        x.total_cmp(&y).is_eq()
    } else {
        a == b
    }
}

/// Every keyword used at a schema position of `schema`.
pub fn keywords(schema: &Json, out: &mut BTreeSet<String>) {
    let Json::Object(map) = schema else { return };
    for (keyword, arg) in map {
        out.insert(keyword.clone());
        match keyword.as_str() {
            "properties" | "$defs" => arg
                .as_object()
                .into_iter()
                .flatten()
                .for_each(|(_, s)| keywords(s, out)),
            "items" | "additionalProperties" | "propertyNames" | "not" | "if" | "then" | "else" => {
                keywords(arg, out);
            }
            "oneOf" | "anyOf" | "allOf" => arg
                .as_array()
                .into_iter()
                .flatten()
                .for_each(|s| keywords(s, out)),
            _ => {}
        }
    }
}
