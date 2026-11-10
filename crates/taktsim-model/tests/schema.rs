//! The JSON Schema against the loader (ADR-0020 §11): the schema accepts
//! every valid model of the corpus and rejects every malformed one whose
//! error a schema can state.
//!
//! A JSON Schema validator crate (`jsonschema`, `boon`) would bring a
//! dependency under MIT-0, which `deny.toml` does not allow, so the check
//! uses `Checker` (`support/schema_checker.rs`): the keywords the generator
//! emits, and no others (`only_supported_keywords` makes sure of that).

#[path = "support/schema_checker.rs"]
mod schema_checker;
mod support;

use std::collections::BTreeSet;

use regex::Regex;
use schema_checker::{ANNOTATIONS, Checker, VALIDATION, keywords};
use support::{catalog, data};
use taktsim_model::{
    Catalog, Decode, Format, Json, LoadErrorKind, Loader, Schemas, model_schema, parse,
};

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
            | LoadErrorKind::Condition { .. }
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
    assert_eq!((parsed, rejected), (92, 54));
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
    // A precision key is opaque to the schema: the experiment section is
    // part of the envelope and must not depend on the catalog. The loader
    // rejects an unknown function (see the malformed corpus).
    let reference = &schema["$defs"]["ObservableRef"];
    assert!(checker.valid(reference, &"throughput(sink)".into()));
    assert!(checker.valid(reference, &"thruput(sink)".into()));
    assert!(!checker.valid(reference, &"1 + 2".into()));
    assert!(!checker.valid(reference, &"throughput".into()));
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
