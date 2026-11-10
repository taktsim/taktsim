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
    assert_eq!(single.system_hash(), model.system_hash());
    assert_eq!(single.includes(), []);
}

/// A fragment's `include` is an error where it stands and is not offered
/// as an allowed key beside it.
#[test]
fn a_fragment_does_not_offer_include_as_allowed() {
    let files = BTreeMap::from([(
        "lib/line.yaml".to_owned(),
        format!("include: [other.yaml]\nflo: [[source, sink]]\n{LINE}"),
    )]);
    let catalog = catalog();
    let errors = Loader::new(&catalog)
        .with_resolver(&files)
        .load_str(MODEL, Format::Yaml)
        .unwrap_err();
    assert_eq!(
        summary(&errors),
        ["include @ 1:1 NestedInclude", "flo @ 2:1 UnknownField",]
    );
    let unknown = &errors.iter().nth(1).unwrap();
    let LoadErrorKind::UnknownField {
        allowed,
        suggestion,
        ..
    } = unknown.kind()
    else {
        panic!("{unknown}");
    };
    assert_eq!(allowed, &["components", "flow", "observe"]);
    assert_eq!(suggestion.as_deref(), Some("flow"));
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

/// A file that cannot be read is named by its own name and placed at the entry of
/// `include:` that names it, where the fix goes, in either form of the entry.
#[test]
fn an_unreadable_include_is_at_its_entry() {
    let catalog = catalog();
    let files = BTreeMap::from([("lib/line.yaml".to_owned(), LINE.to_owned())]);
    for (entries, path) in [
        ("[lib/line.yaml, lib/missing.yaml]", "include[1]"),
        ("[{path: lib/missing.yaml}, lib/line.yaml]", "include[0]"),
    ] {
        let model = MODEL.replace("[lib/line.yaml]", entries);
        let errors = Loader::new(&catalog)
            .with_resolver(&files)
            .load_str(&model, Format::Yaml)
            .unwrap_err();
        let error = errors.first();
        assert!(error.kind().is_io(), "{entries}");
        assert_eq!(error.file(), Some("lib/missing.yaml"), "{entries}");
        assert_eq!(error.path().to_string(), path, "{entries}");
    }
}
