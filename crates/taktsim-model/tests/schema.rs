//! The JSON Schema against the loader (ADR-0020 §11): the schema accepts
//! every valid model of the corpus and rejects every malformed one whose
//! error a schema can state.
//!
//! A JSON Schema validator crate (`jsonschema`, `boon`) would bring a
//! dependency under MIT-0, which `deny.toml` does not allow, so the check
//! uses `Checker` below: the keywords the generator emits, and no others
//! (`only_supported_keywords` makes sure of that). Values are the model
//! crate's own `Json`.

mod support;

use std::collections::BTreeSet;

use regex::Regex;
use support::{catalog, data};
use taktsim_model::{
    Catalog, Decode, Format, Json, LoadErrorKind, Loader, Schemas, model_schema, parse,
};

/// The keywords `Checker` validates.
const VALIDATION: [&str; 23] = [
    "$ref",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "propertyNames",
    "items",
    "minItems",
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
const ANNOTATIONS: [&str; 7] = [
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
struct Checker<'a> {
    root: &'a Json,
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

    fn valid(&self, schema: &Json, value: &Json) -> bool {
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
fn keywords(schema: &Json, out: &mut BTreeSet<String>) {
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

#[test]
fn only_supported_keywords() {
    for schema in [model_schema(&Catalog::new()), model_schema(&catalog())] {
        let mut used = BTreeSet::new();
        keywords(&schema, &mut used);
        let unknown: Vec<_> = used
            .iter()
            .filter(|k| !VALIDATION.contains(&k.as_str()) && !ANNOTATIONS.contains(&k.as_str()))
            .collect();
        assert!(unknown.is_empty(), "{unknown:?}");
    }
}

#[allow(clippy::unwrap_used)] // Test helper: the corpus directories exist.
fn files(dir: &str) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(data(dir))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    paths
}

#[allow(clippy::unwrap_used)] // Test helper: the corpus files are model files.
fn tree(path: &std::path::Path) -> Option<Json> {
    let text = std::fs::read_to_string(path).unwrap();
    let format = Format::from_path(path).unwrap();
    parse(&text, format).ok().map(|n| n.to_json())
}

#[test]
fn the_schema_accepts_every_valid_model_and_included_file() {
    let schema = model_schema(&catalog());
    let checker = Checker { root: &schema };
    for path in files("valid") {
        let value = tree(&path).unwrap();
        assert!(checker.valid(&schema, &value), "{}", path.display());
    }
    let included = &schema["$defs"]["IncludedFile"];
    for path in std::fs::read_dir(data("valid").join("parts")).unwrap() {
        let path = path.unwrap().path();
        assert!(
            checker.valid(included, &tree(&path).unwrap()),
            "{}",
            path.display()
        );
    }
}

/// Whether a schema can state the rule an error is about. The rest needs
/// the whole document (references, unique ids, expressions), the
/// quantity's exact value, or other files.
fn schema_can_state(kind: &LoadErrorKind) -> bool {
    !matches!(
        kind,
        LoadErrorKind::UnknownComponent { .. }
            | LoadErrorKind::DuplicateId { .. }
            | LoadErrorKind::CaseClash { .. }
            | LoadErrorKind::SelfLoop { .. }
            | LoadErrorKind::DuplicateEdge { .. }
            | LoadErrorKind::Expression { .. }
            | LoadErrorKind::NotAnObservable { .. }
            | LoadErrorKind::DuplicateObservable { .. }
            | LoadErrorKind::Distribution(_)
            | LoadErrorKind::Unit(_)
            | LoadErrorKind::NotAllowed { .. }
            | LoadErrorKind::Io { .. }
            | LoadErrorKind::IncludeHash { .. }
            | LoadErrorKind::NestedInclude
            | LoadErrorKind::IncludeSelf { .. }
            | LoadErrorKind::DuplicateInclude { .. }
            | LoadErrorKind::IncludeUnavailable
            | LoadErrorKind::Normalize { .. }
    )
}

#[test]
fn the_schema_rejects_every_malformed_model_it_can() {
    let catalog = catalog();
    let schema = model_schema(&catalog);
    let checker = Checker { root: &schema };
    let loader = Loader::new(&catalog);
    let mut parsed = 0;
    let mut rejected = 0;
    let mut too_lax = Vec::new();
    for path in files("malformed") {
        // Syntax errors are found before any schema could look.
        let Some(value) = tree(&path) else { continue };
        let errors = loader.load_file(&path).unwrap_err();
        let in_this_file = errors
            .iter()
            .filter(|e| e.file() == Some(&*path.display().to_string()));
        let statable: Vec<_> = in_this_file
            .filter(|e| schema_can_state(e.kind()))
            .collect();
        if !statable.is_empty() {
            if checker.valid(&schema, &value) {
                too_lax.push(format!("{}: {}", path.display(), statable[0]));
            } else {
                rejected += 1;
            }
        }
        parsed += 1;
    }
    assert!(too_lax.is_empty(), "{}", too_lax.join("\n"));
    // The corpus files whose errors a schema can state, all rejected.
    assert_eq!((parsed, rejected), (89, 53));
}

#[test]
fn the_schema_has_an_entry_per_component_type_closed_to_other_keys() {
    let schema = model_schema(&catalog());
    let entries = schema["$defs"]["Component"]["oneOf"].as_array().unwrap();
    let types: Vec<&str> = entries
        .iter()
        .map(|e| e["properties"]["type"]["const"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["Buffer", "Inspection", "Machine", "Sink", "Source"]);
    for entry in entries {
        assert_eq!(entry["additionalProperties"], false);
        let required = entry["required"].as_array().unwrap();
        assert!(required.contains(&"id".into()) && required.contains(&"type".into()));
        // Optional parameters may be left out, but not written as null.
        let text = entry.to_string();
        assert!(!text.contains("\"null\""), "{text}");
    }
    let machine = &entries[2];
    assert_eq!(
        machine["required"],
        Json::Array(vec![
            Json::from("id"),
            Json::from("type"),
            Json::from("cycle_time")
        ])
    );
    assert_eq!(
        machine["properties"]["mtbf"]["$ref"],
        "#/$defs/Distribution"
    );
}

#[test]
fn without_component_types_or_observables_nothing_is_a_component_or_observable() {
    let schema = model_schema(&Catalog::new());
    let checker = Checker { root: &schema };
    let component = Json::object([
        ("id", Json::from("cut")),
        ("type", Json::from("Machine")),
        ("cycle_time", Json::from("42s")),
    ]);
    assert!(!checker.valid(&schema["$defs"]["Component"], &component));
    assert!(!checker.valid(&schema["$defs"]["Observable"], &"throughput(sink)".into()));
}

/// Whether the parser's error is about the text's form, which the
/// pattern states (a count's pattern also states that it is whole),
/// rather than its value (precision, size), which it leaves to the parser.
fn form_error(e: &taktsim_units::UnitError) -> bool {
    matches!(
        e,
        taktsim_units::UnitError::InvalidNumber { .. }
            | taktsim_units::UnitError::UnknownUnit { .. }
            | taktsim_units::UnitError::Negative { .. }
            | taktsim_units::UnitError::NotWhole { .. }
    )
}

/// Whether `T`'s schema pattern and its parser agree on `text`.
#[allow(clippy::unwrap_used)] // Test helper: quantities have patterns.
fn agrees<T: Decode + std::str::FromStr<Err = taktsim_units::UnitError>>(text: &str) -> bool {
    let schema = T::schema(&mut Schemas::default());
    let pattern = Regex::new(schema["pattern"].as_str().unwrap()).unwrap();
    let parses = match text.parse::<T>() {
        Ok(_) => true,
        Err(e) => !form_error(&e),
    };
    pattern.is_match(text) == parses
}

proptest::proptest! {
    /// Each quantity's pattern accepts exactly the text whose form its
    /// parser accepts.
    #[test]
    fn quantity_patterns_accept_what_the_parser_reads(
        text in "[0-9.]{0,6}[ ]{0,2}(d|h|min|s|ms|sec|part|parts|parts/s|part/min|parts/h|parts/d|%|x|)",
    ) {
        use taktsim_units::{Count, Rate, Ratio, SimDuration};
        proptest::prop_assert!(agrees::<SimDuration>(&text), "duration {text:?}");
        proptest::prop_assert!(agrees::<Count>(&text), "count {text:?}");
        proptest::prop_assert!(agrees::<Rate>(&text), "rate {text:?}");
        proptest::prop_assert!(agrees::<Ratio>(&text), "ratio {text:?}");
    }
}

#[test]
fn generating_twice_gives_the_same_text() {
    let a = model_schema(&catalog()).to_pretty();
    let b = model_schema(&catalog()).to_pretty();
    assert_eq!(a, b);
}
