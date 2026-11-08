//! The committed JSON Schema against the loader, for the standard catalog
//! (ADR-0020 §11): the schema accepts every documented example and rejects every
//! malformed model whose error a schema can state.

#[path = "../../taktsim-model/tests/support/schema_checker.rs"]
mod schema_checker;
mod support;

use std::collections::BTreeSet;
use std::path::Path;

use schema_checker::{ANNOTATIONS, Checker, VALIDATION, keywords};
use support::registry;
use taktsim_model::{Format, Json, LoadErrorKind, Loader, model_schema, parse};

fn schema() -> Json {
    model_schema(registry().catalog())
}

#[allow(clippy::unwrap_used)] // Test helper.
fn tree(text: &str) -> Json {
    parse(text, Format::Yaml).unwrap().to_json()
}

#[test]
fn only_supported_keywords() {
    let mut used = BTreeSet::new();
    keywords(&schema(), &mut used);
    let unknown: Vec<_> = used
        .iter()
        .filter(|k| !VALIDATION.contains(&k.as_str()) && !ANNOTATIONS.contains(&k.as_str()))
        .collect();
    assert!(unknown.is_empty(), "{unknown:?}");
}

#[test]
fn the_committed_schema_has_every_entry_closed_to_other_keys() {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("schemas")
            .join("model")
            .join("1.0.json"),
    )
    .unwrap();
    assert!(
        text.contains("\"const\": \"Machine\""),
        "regenerate schemas/"
    );
    let schema = schema();
    let entries = schema["$defs"]["Component"]["oneOf"].as_array().unwrap();
    assert_eq!(entries.len(), 4);
    for entry in entries {
        assert_eq!(entry["additionalProperties"], false);
    }
    let machine = entries
        .iter()
        .find(|e| e["properties"]["type"]["const"] == "Machine")
        .unwrap();
    assert_eq!(
        machine["required"],
        Json::Array(vec!["id".into(), "type".into(), "cycle_time".into()])
    );
    // The meaning of `mtbf` is in the schema, because a reliability engineer may
    // read it the other way.
    let mtbf = machine["properties"]["mtbf"]["description"]
        .as_str()
        .unwrap();
    assert!(mtbf.contains("end of a repair"), "{mtbf}");
    assert!(mtbf.contains("name a distribution"), "{mtbf}");
}

const VALID: &[&str] = &[
    "  - {id: source, type: Source}\n  - {id: m, type: Machine, cycle_time: 42s}\n  - {id: sink, type: Sink}",
    "  - {id: source, type: Source, interarrival: {dist: exponential, mean: 40s}, batch: 2 parts, variants: {basic: 80%, premium: 20%}, priority: 1}\n  - {id: m, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}, mtbf: 6h, mttr: {dist: exponential, mean: 12min}, setup: {per_variant: 4min, matrix: [{from: basic, to: premium, time: 6min}, {from: premium, to: basic, time: 0s}]}, capacity: 2 parts}\n  - {id: sink, type: Sink}",
    "  - {id: source, type: Source}\n  - {id: m, type: Machine, cycle_time: 42s, setup: {per_variant: 4min}}\n  - {id: sink, type: Sink}",
];

#[test]
fn documented_shapes_load_and_the_schema_accepts_them() {
    let schema = schema();
    let checker = Checker { root: &schema };
    for components in VALID {
        let yaml = support::document("1h", components, "  - [source, m, sink]");
        support::load(&yaml);
        assert!(checker.valid(&schema, &tree(&yaml)), "{yaml}");
    }
    let buffer = support::document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min}\n  \
         - {id: buf, type: Buffer, capacity: 20 parts, discipline: lifo}\n  - {id: sink, type: Sink}",
        "  - [source, buf, sink]",
    );
    assert!(checker.valid(&schema, &tree(&buffer)));
}

/// Whether a schema can state the rule an error is about. The rest needs the
/// whole document, the quantity's value, or a rule between keys.
fn schema_can_state(kind: &LoadErrorKind) -> bool {
    !matches!(
        kind,
        LoadErrorKind::NotAllowed { .. }
            | LoadErrorKind::Expression { .. }
            | LoadErrorKind::Unit(_)
            | LoadErrorKind::Distribution(_)
            | LoadErrorKind::BareShare { .. }
    )
}

#[test]
fn the_schema_rejects_every_malformed_model_it_can() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    let schema = schema();
    let checker = Checker { root: &schema };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("malformed");
    let (mut files, mut rejected) = (0, 0);
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let errors = loader.load_file(&path).unwrap_err();
        files += 1;
        if errors.iter().any(|e| schema_can_state(e.kind())) {
            assert!(
                !checker.valid(&schema, &tree(&text)),
                "{}: the schema accepts it: {errors}",
                path.display()
            );
            rejected += 1;
        }
    }
    // The corpus files whose errors a schema can state, all rejected.
    assert_eq!((files, rejected), (47, 9));
}

#[test]
fn the_schema_states_the_rules_a_pattern_or_keyword_can() {
    let schema = schema();
    let checker = Checker { root: &schema };
    let machine = |params: &str| {
        support::document(
            "1h",
            &format!(
                "  - {{id: source, type: Source}}\n  - {{id: m, type: Machine, cycle_time: 1min, {params}}}\n  - {{id: sink, type: Sink}}"
            ),
            "  - [source, m, sink]",
        )
    };
    // `setup` takes at least one of its keys, a known discipline, a closed shape.
    assert!(!checker.valid(&schema, &tree(&machine("setup: {}"))));
    assert!(!checker.valid(&schema, &tree(&machine("setup: {matrix: []}"))));
    assert!(checker.valid(&schema, &tree(&machine("setup: {per_variant: 4min}"))));
    assert!(!checker.valid(&schema, &tree(&machine("priority: 1"))));
    let source = |params: &str| {
        support::document(
            "1h",
            &format!("  - {{id: source, type: Source, {params}}}\n  - {{id: sink, type: Sink}}"),
            "  - [source, sink]",
        )
    };
    assert!(!checker.valid(&schema, &tree(&source("priority: 256"))));
    assert!(!checker.valid(&schema, &tree(&source("variants: {}"))));
    assert!(!checker.valid(&schema, &tree(&source("variants: {\"a b\": 100%}"))));
    assert!(checker.valid(&schema, &tree(&source("priority: 255"))));
    assert!(checker.valid(&schema, &tree(&source("variants: {a: 100%}"))));
}

const ROUTED: &str = "  - {id: source, type: Source, interarrival: 1min, variants: {red: 50%, blue: 50%}, \
     due_in: {dist: uniform, min: 2h, max: 8h}, priority_mix: [{priority: 1, share: 20%}, {priority: 0, share: 80%}]}\n  \
     - {id: a, type: Machine, cycle_time: 1min, cycle_time_by_variant: {red: 2min}}\n  \
     - {id: b, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}";

#[test]
fn the_routing_words_load_and_the_schema_accepts_them() {
    let schema = schema();
    let checker = Checker { root: &schema };
    for (flow, control) in [
        (
            "  - {from: source, to: [a, b], choose: shortest_queue}\n  - {from: [a, b], to: sink}",
            "",
        ),
        (
            "  - {from: source, to: [a, b], choose: first_free}\n  - {from: [a, b], to: sink}",
            "",
        ),
        (
            "  - {from: source, split: {a: 50%, b: 50%}}\n  - {from: [a, b], to: sink}",
            "",
        ),
        (
            "  - {from: source, to: a, when: \"variant == 'red'\"}\n  - {from: source, to: b}\n  - {from: [a, b], to: sink}",
            "",
        ),
        (
            "  - {from: source, split: {a: 50%, b: 50%}, when: \"priority > 0\"}\n  - {from: source, to: a}\n  - {from: [a, b], to: sink}",
            "",
        ),
        (
            "  - {from: source, to: [a, b]}\n  - {from: [a, b], to: sink}",
            "control:\n  dispatch: {a: earliest_due_date, b: minimum_setup}\n",
        ),
    ] {
        let yaml = format!("{}{control}", support::document("1h", ROUTED, flow));
        support::load(&yaml);
        assert!(checker.valid(&schema, &tree(&yaml)), "{yaml}");
    }
    let rejected = |flow: &str, control: &str| {
        let yaml = format!("{}{control}", support::document("1h", ROUTED, flow));
        assert!(!checker.valid(&schema, &tree(&yaml)), "{yaml}");
    };
    let rest = "\n  - {from: [a, b], to: sink}";
    // An unknown rule, both shapes at once, and `choose` on a split.
    rejected(
        &format!("  - {{from: source, to: [a, b], choose: shortest}}{rest}"),
        "",
    );
    rejected(
        &format!("  - {{from: source, to: a, split: {{a: 50%, b: 50%}}}}{rest}"),
        "",
    );
    rejected(
        &format!("  - {{from: source, split: {{a: 50%, b: 50%}}, choose: preferred}}{rest}"),
        "",
    );
    // A split of one target, and a condition that is no text.
    rejected(
        &format!("  - {{from: source, split: {{a: 100%}}}}{rest}"),
        "",
    );
    rejected(&format!("  - {{from: source, to: a, when: 1}}{rest}"), "");
    // `release` waits for v0.2, and a rule is one of the five.
    let flow = format!("  - {{from: source, to: [a, b]}}{rest}");
    rejected(&flow, "control:\n  release: {policy: conwip}\n");
    rejected(&flow, "control:\n  dispatch: {a: oldest}\n");
    // The included file has no `control`.
    let included = &schema["$defs"]["IncludedFile"]["properties"];
    assert_eq!(included["control"], Json::Null);
    assert_eq!(schema["properties"]["control"]["$ref"], "#/$defs/Control");
}

#[test]
fn the_schema_rejects_every_malformed_routed_model_it_can() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    let schema = schema();
    let checker = Checker { root: &schema };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("routing_malformed");
    let (mut files, mut rejected) = (0, 0);
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let errors = loader.load_file(&path).unwrap_err();
        files += 1;
        if errors
            .iter()
            .any(|e| schema_can_state(e.kind()) && !needs_the_document(e.kind()))
        {
            assert!(
                !checker.valid(&schema, &tree(&text)),
                "{}: the schema accepts it: {errors}",
                path.display()
            );
            rejected += 1;
        }
    }
    // The corpus files whose errors a schema of one value can state, all rejected.
    assert_eq!((files, rejected), (41, 5));
}

/// A rule between components, which a schema of one value cannot state.
fn needs_the_document(kind: &LoadErrorKind) -> bool {
    matches!(
        kind,
        LoadErrorKind::UnknownComponent { .. } | LoadErrorKind::DuplicateEdge { .. }
    )
}

#[test]
fn a_catalog_without_counts_or_distributions_does_not_offer_them() {
    // What a schema offers follows the catalog (ADR-0022, question 17).
    let empty = model_schema(&taktsim_model::Catalog::new());
    let defs = empty["$defs"].as_object().unwrap();
    assert!(!defs.contains_key("Count"));
    assert!(!defs.contains_key("Distribution"));
    assert!(defs.contains_key("Duration"));
    let standard = schema();
    let defs = standard["$defs"].as_object().unwrap();
    assert!(defs.contains_key("Count") && defs.contains_key("Distribution"));
}
