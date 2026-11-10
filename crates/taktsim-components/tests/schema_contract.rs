//! The committed JSON Schema against the loader, for the standard catalog
//! (ADR-0020 §11): the schema accepts every documented example and rejects every
//! malformed model whose error a schema can state, rules between keys included.

#[path = "../../taktsim-model/tests/support/schema_checker.rs"]
mod schema_checker;
mod support;

use std::collections::BTreeSet;
use std::path::Path;

use schema_checker::{ANNOTATIONS, Checker, VALIDATION, keywords};
use support::{model_files, registry};
use taktsim_model::{Format, Json, Loader, model_schema, parse};

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
    // The description names the catalog's types as the catalog lists them
    // (ADR-0022 §11: no list of the generator's own).
    let types: Vec<&str> = registry().catalog().types().collect();
    let committed = tree(&text);
    let description = committed["description"].as_str().unwrap();
    assert!(
        description.contains(&format!("catalog: {}.", types.join(", "))),
        "{description}"
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

/// The corpus files whose rule a schema cannot state, each with the reason.
/// Every other file must be rejected by the schema (ADR-0020 §11).
const BEYOND_A_SCHEMA: &[(&str, &str)] = &[
    // What may connect to what: the whole document.
    ("flow-edge-into-source.yaml", "the flow"),
    ("flow-edge-out-of-sink.yaml", "the flow"),
    ("flow-loop.yaml", "the flow"),
    ("flow-machine-not-entered.yaml", "the flow"),
    ("flow-machine-not-left.yaml", "the flow"),
    ("flow-on-demand-branch-without-station.yaml", "the flow"),
    ("flow-on-demand-through-buffer.yaml", "the flow"),
    ("flow-on-demand-to-sink.yaml", "the flow"),
    ("flow-second-edge-chains.yaml", "the flow"),
    ("flow-second-edge-mappings.yaml", "the flow"),
    ("flow-sink-not-entered.yaml", "the flow"),
    ("flow-source-not-left.yaml", "the flow"),
    // A variant a matrix names must be declared by a Source: the whole document.
    ("machine-matrix-default-only.yaml", "the Sources' variants"),
    (
        "machine-matrix-unknown-variant.yaml",
        "the Sources' variants",
    ),
    // An expression's type: the catalog's observables.
    ("observe-wrong-type.yaml", "the expression's type"),
    // Two values compared: entries of a list, or two keys of one entry.
    ("machine-matrix-twice.yaml", "two entries compared"),
    ("machine-matrix-same-variant.yaml", "two values compared"),
    // The value of a quantity written in a string, or a sum of shares.
    ("buffer-capacity-zero.yaml", "the quantity's value"),
    ("machine-capacity-too-large.yaml", "the quantity's value"),
    ("machine-capacity-zero.yaml", "the quantity's value"),
    ("machine-cycle-time-zero.yaml", "the quantity's value"),
    ("machine-mtbf-zero.yaml", "the quantity's value"),
    ("machine-mttr-zero.yaml", "the quantity's value"),
    (
        "machine-setup-per-variant-zero.yaml",
        "the quantity's value",
    ),
    ("source-batch-too-large.yaml", "the quantity's value"),
    ("source-batch-zero.yaml", "the quantity's value"),
    ("source-interarrival-zero.yaml", "the quantity's value"),
    ("source-variants-not-100.yaml", "a sum of shares"),
    ("source-variants-zero-share.yaml", "the quantity's value"),
];

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
    for path in model_files(&dir) {
        let text = std::fs::read_to_string(&path).unwrap();
        let errors = loader.load_file(&path).unwrap_err();
        files += 1;
        let name = path.file_name().unwrap().to_str().unwrap();
        if !BEYOND_A_SCHEMA.iter().any(|(beyond, _)| *beyond == name) {
            assert!(
                !checker.valid(&schema, &tree(&text)),
                "{}: the schema accepts it: {errors}",
                path.display()
            );
            rejected += 1;
        }
    }
    // Every listed file exists, so the list cannot hide a rule.
    for (name, _) in BEYOND_A_SCHEMA {
        assert!(dir.join(name).is_file(), "{name}");
    }
    // The corpus files whose rule a schema can state, all rejected; among them
    // rules between keys (`mtbf` with `mttr`, `batch` with `interarrival`) and
    // the length of a variant name.
    assert_eq!((files, rejected), (47, 18));
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
    // `mtbf` and `mttr` come together.
    assert!(!checker.valid(&schema, &tree(&machine("mtbf: 6h"))));
    assert!(!checker.valid(&schema, &tree(&machine("mttr: 12min"))));
    assert!(checker.valid(&schema, &tree(&machine("mtbf: 6h, mttr: 12min"))));
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
    // `batch` needs `interarrival`; a variant name has at most 255 bytes.
    assert!(!checker.valid(&schema, &tree(&source("batch: 2 parts"))));
    assert!(checker.valid(&schema, &tree(&source("interarrival: 40s, batch: 2 parts"))));
    let long = "v".repeat(256);
    assert!(!checker.valid(
        &schema,
        &tree(&source(&format!("variants: {{{long}: 100%}}")))
    ));
    let longest = "v".repeat(255);
    assert!(checker.valid(
        &schema,
        &tree(&source(&format!("variants: {{{longest}: 100%}}")))
    ));
    assert!(checker.valid(&schema, &tree(&source("variants: {a: 100%}"))));
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
