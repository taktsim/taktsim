//! `include:` through the API: in-memory resolvers, models given as text,
//! and the provenance recorded with a model. File-based cases are in the
//! corpora (`data/valid/with-include.yaml`, `data/malformed/include-*`).

mod support;

use std::collections::BTreeMap;

use support::{catalog, data, summary};
use taktsim_model::{Format, LoadErrorKind, Loader, ModelHash};

const MODEL: &str = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 8h}\n\
                     include: [lib/line.yaml]\nobserve: [throughput(sink)]\n";
const LINE: &str = "components:\n  - {id: source, type: Source, interarrival: 1min}\n  - {id: sink, type: Sink}\n\
                    flow: [[source, sink]]\n";

#[test]
fn a_map_resolves_includes_for_models_given_as_text() {
    let files = BTreeMap::from([("lib/line.yaml".to_owned(), LINE.to_owned())]);
    let catalog = catalog();
    let model = Loader::new(&catalog)
        .with_resolver(&files)
        .load_str(MODEL, Format::Yaml)
        .unwrap();
    assert_eq!(model.document().components().len(), 2);
    let included = &model.includes()[0];
    assert_eq!(included.path(), "lib/line.yaml");
    assert_eq!(
        included.sha256(),
        ModelHash::of(LINE.as_bytes()).to_string()
    );

    // The same model written in one file has the same hash.
    let inline = MODEL.replace("include: [lib/line.yaml]\n", LINE);
    let single = Loader::new(&catalog)
        .load_str(&inline, Format::Yaml)
        .unwrap();
    assert_eq!(single.hash(), model.hash());
    assert_eq!(single.includes(), []);
}

#[test]
fn text_without_a_resolver_cannot_include() {
    let catalog = catalog();
    let errors = Loader::new(&catalog)
        .load_str(MODEL, Format::Yaml)
        .unwrap_err();
    assert_eq!(errors.first().kind(), &LoadErrorKind::IncludeUnavailable);
    assert_eq!(summary(&errors)[0], "include[0] @ 4:11 IncludeUnavailable");
}

#[test]
fn errors_in_an_included_file_name_that_file() {
    let files = BTreeMap::from([(
        "lib/line.yaml".to_owned(),
        LINE.replace("type: Sink", "type: Snk"),
    )]);
    let catalog = catalog();
    let errors = Loader::new(&catalog)
        .with_resolver(&files)
        .load_str(MODEL, Format::Yaml)
        .unwrap_err();
    let e = errors.first();
    assert_eq!(e.file(), Some("lib/line.yaml"));
    assert_eq!(
        e.to_string(),
        "lib/line.yaml:3:22: components[1].type: unknown component type `Snk` (did you mean `Sink`?); allowed: Buffer, Inspection, Machine, Sink, Source"
    );
}

#[test]
fn included_json_and_syntax_errors() {
    let catalog = catalog();
    let model = MODEL.replace("lib/line.yaml", "lib/line.json");
    let files = BTreeMap::from([(
        "lib/line.json".to_owned(),
        "{\"flow\": [[\"a\", \"b\"],]}".to_owned(),
    )]);
    let errors = Loader::new(&catalog)
        .with_resolver(&files)
        .load_str(&model, Format::Yaml)
        .unwrap_err();
    assert_eq!(errors.first().file(), Some("lib/line.json"));
    assert!(matches!(errors.first().kind(), LoadErrorKind::Json { .. }));
}

#[test]
fn an_explicit_resolver_replaces_the_model_directory() {
    let files = BTreeMap::from([("parts/ok.yaml".to_owned(), LINE.to_owned())]);
    let catalog = catalog();
    // include-duplicate-id-across-files.yaml includes parts/ok.yaml, which
    // the map gives other content.
    let path = data("malformed").join("include-duplicate-id-across-files.yaml");
    let errors = Loader::new(&catalog)
        .with_resolver(&files)
        .load_file(&path)
        .unwrap_err();
    let lines = summary(&errors);
    assert!(
        lines.iter().all(|l| !l.contains("components[3].id")),
        "{lines:?}"
    );
}

#[test]
fn includes_after_a_failed_one_are_still_read() {
    let catalog = catalog();
    let model = MODEL.replace("[lib/line.yaml]", "[lib/missing.yaml, lib/line.yaml]");
    let files = BTreeMap::from([("lib/line.yaml".to_owned(), LINE.replace("1min", "1 minute"))]);
    let errors = Loader::new(&catalog)
        .with_resolver(&files)
        .load_str(&model, Format::Yaml)
        .unwrap_err();
    let files: Vec<Option<&str>> = errors.iter().map(|e| e.file()).collect();
    assert_eq!(files, [Some("lib/missing.yaml"), Some("lib/line.yaml")]);
    assert!(errors.first().kind().is_io());
}
