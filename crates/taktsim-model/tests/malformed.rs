//! The corpus of malformed models (roadmap M4): every file in
//! `data/malformed/` fails to load with exactly the errors its
//! `# expect:` lines list, in order. Each line is
//! `[file: ]path @ line:column Kind`: the field path, where the error is,
//! and the name of the `LoadErrorKind` variant. A file prefix names an
//! included file; errors without one are in the model file.
//!
//! The expectations are at the end of each file, so they do not move the
//! lines the errors point at. The files are also meant for the CLI (M7)
//! and Python (M12) contract tests.

mod support;

use support::{catalog, data, summary};
use taktsim_model::{Format, LoadErrorKind, Loader};

#[test]
fn every_malformed_model_fails_with_the_expected_errors() {
    let dir = data("malformed");
    let catalog = catalog();
    let loader = Loader::new(&catalog);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    let mut failures = Vec::new();
    for path in &paths {
        let text = std::fs::read_to_string(path).unwrap();
        let expected: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("# expect: "))
            .collect();
        let main = path.display().to_string();
        let actual: Vec<String> = match loader.load_file(path) {
            Ok(_) => vec!["(loaded)".to_owned()],
            Err(errors) => {
                let lines = summary(&errors);
                errors
                    .iter()
                    .zip(lines)
                    .map(|(e, line)| match e.file() {
                        Some(file) if file != main => format!("{file}: {line}"),
                        Some(_) | None => line,
                    })
                    .collect()
            }
        };
        if expected.is_empty() || actual != expected {
            let name = path.file_name().unwrap().to_string_lossy();
            let lines: Vec<String> = actual.iter().map(|l| format!("# expect: {l}")).collect();
            failures.push(format!("{name}:\n{}", lines.join("\n")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} files differ:\n\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n\n")
    );
    assert_eq!(paths.len(), 100);
}

/// Every error carries what was found, what is allowed and, for a value
/// with a valid alternative, an example, so the CLI can print them as
/// fields (spec §5).
#[test]
fn every_error_says_what_was_found_and_what_is_allowed() {
    let dir = data("malformed");
    let catalog = catalog();
    let loader = Loader::new(&catalog);
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_file() {
            continue;
        }
        let errors = loader.load_file(&path).unwrap_err();
        for e in errors.iter() {
            let kind = e.kind();
            assert!(
                kind.found().is_some_and(|f| !f.is_empty()),
                "{}: {e}",
                path.display()
            );
            assert!(
                kind.allowed().is_some_and(|a| !a.is_empty()),
                "{}: {e}",
                path.display()
            );
            let needs_example = !matches!(
                kind,
                LoadErrorKind::Io { .. }
                    | LoadErrorKind::Yaml { .. }
                    | LoadErrorKind::TooDeep { .. }
                    | LoadErrorKind::MultipleDocuments
                    | LoadErrorKind::Anchor
                    | LoadErrorKind::Alias
                    | LoadErrorKind::MergeKey
                    | LoadErrorKind::DuplicateKey { .. }
                    | LoadErrorKind::Repeated { .. }
                    | LoadErrorKind::SelfLoop { .. }
                    | LoadErrorKind::DuplicateEdge { .. }
                    | LoadErrorKind::DuplicateObservable { .. }
                    | LoadErrorKind::DuplicateId { .. }
                    | LoadErrorKind::CaseClash { .. }
                    | LoadErrorKind::UnknownComponent {
                        suggestion: None,
                        ..
                    }
                    | LoadErrorKind::NestedInclude
                    | LoadErrorKind::IncludeSelf { .. }
                    | LoadErrorKind::DuplicateInclude { .. }
            );
            if needs_example {
                assert!(kind.example().is_some(), "{}: {e}", path.display());
            }
            // The message names the path and the location.
            let text = e.to_string();
            assert!(text.contains(&e.path().to_string()), "{text}");
        }
    }
}

/// JSON has its own parser; its errors have lines and columns too.
#[test]
fn malformed_json() {
    let catalog = catalog();
    let loader = Loader::new(&catalog);
    let cases = [
        ("{\"format\": \"1.0\",}", vec!["(document) @ 1:18 Json"]),
        (
            "{\"format\": \"1.0\", \"format\": \"1.0\"}",
            vec!["format @ 1:19 DuplicateKey"],
        ),
        ("// a comment\n{}", vec!["(document) @ 1:1 Json"]),
        (
            "{\"model\": {\"name\": \"m\", \"seed\": 1e400}}",
            vec!["model.seed @ 1:33 NotFinite"],
        ),
        (
            "{\"model\": {\"name\": \"m\", \"seed\": 01}}",
            vec!["model.seed @ 1:33 Json"],
        ),
        ("{\"a\": \"\\x\"}", vec!["a @ 1:8 Json"]),
        ("[1, 2]", vec!["(document) @ 1:1 WrongType"]),
        ("", vec!["(document) @ - Empty"]),
        (
            "{\n  \"format\": \"1.0\",\n  \"model\": {\"name\": \"m\"},\n  \"experiment\": {\"type\": \"terminating\", \"horizon\": 8}\n}",
            vec![
                "components @ 1:1 MissingField",
                "flow @ 1:1 MissingField",
                "experiment.horizon @ 4:52 WrongType",
            ],
        ),
    ];
    for (text, expected) in cases {
        let errors = loader.load_str(text, Format::Json).unwrap_err();
        assert_eq!(summary(&errors), expected, "{text}");
    }
}

#[test]
fn files_that_are_not_utf8_or_not_model_files() {
    let catalog = catalog();
    let loader = Loader::new(&catalog);
    let dir = std::env::temp_dir().join(format!("taktsim-malformed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let latin1 = dir.join("latin1.yaml");
    std::fs::write(&latin1, b"format: \"1.0\"\nmodel: {name: m\xe4}\n").unwrap();
    let errors = loader.load_file(&latin1).unwrap_err();
    assert_eq!(
        errors.first().kind(),
        &LoadErrorKind::NotUtf8 { offset: 29 }
    );
    let text = dir.join("model.txt");
    std::fs::write(&text, "format: \"1.0\"\n").unwrap();
    let errors = loader.load_file(&text).unwrap_err();
    assert!(matches!(
        errors.first().kind(),
        LoadErrorKind::NotAllowed { .. }
    ));
    std::fs::remove_dir_all(&dir).unwrap();
}
