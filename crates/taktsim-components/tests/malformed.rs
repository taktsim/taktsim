//! The corpus of malformed models for the factory vocabulary and the flow: every
//! file in `data/malformed/` fails to load with exactly the errors its
//! `# expect:` lines list, in order, each `path @ line:column Kind`, and each
//! error has a field path, what was found, what is allowed and an example, as
//! far as its kind says them. The expectations are at the end of each file, so
//! they do not move the lines the errors point at.

mod support;

use std::path::Path;

use support::{document, load, registry, try_load};
use taktsim_model::{LoadErrorKind, LoadErrors, Loader};

/// Each error as `path @ line:column Kind`.
fn summary(errors: &LoadErrors) -> Vec<String> {
    errors
        .iter()
        .map(|e| {
            let kind = format!("{:?}", e.kind());
            let name = kind
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_owned();
            let at = e
                .location()
                .map_or_else(|| "-".to_owned(), |l| l.to_string());
            format!("{} @ {at} {name}", e.path())
        })
        .collect()
}

#[allow(clippy::unwrap_used)] // Test helper.
fn corpus() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("malformed");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_malformed_model_fails_with_the_expected_errors() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    let paths = corpus();
    assert!(paths.len() >= 40);
    let mut failures = Vec::new();
    for path in &paths {
        let text = std::fs::read_to_string(path).unwrap();
        let expected: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("# expect: "))
            .collect();
        let actual = match loader.load_file(path) {
            Ok(_) => vec!["(loaded)".to_owned()],
            Err(errors) => summary(&errors),
        };
        if expected.is_empty() || actual != expected {
            failures.push(format!(
                "{}:\n  expected {expected:?}\n  actual   {actual:?}",
                path.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_rule_says_what_was_found_what_is_allowed_and_an_example() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    for path in corpus() {
        let errors = loader.load_file(&path).unwrap_err();
        for e in errors.iter() {
            if let LoadErrorKind::NotAllowed {
                found,
                allowed,
                example,
            } = e.kind()
            {
                let at = path.display();
                assert!(!found.is_empty() && !allowed.is_empty(), "{at}: {e}");
                assert!(!example.is_empty(), "{at}: {e}");
                assert!(e.kind().example().is_some(), "{at}: {e}");
            }
        }
    }
}

#[test]
fn second_edge_from_a_component_is_rejected() {
    let body = "  - {id: source, type: Source, interarrival: 1min}\n  \
                - {id: a, type: Sink}\n  - {id: b, type: Sink}";
    // In every spelling.
    for flow in [
        "  - [source, a]\n  - [source, b]",
        "  - {from: source, to: a}\n  - {from: source, to: b}",
        "  - {from: source, to: a}\n  - [source, b]",
    ] {
        let errors = try_load(&document("1h", body, flow)).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors}");
        assert_eq!(
            errors.first().kind().example().as_deref(),
            Some("{from: cut, to: [weld-1, weld-2]}"),
            "{errors}"
        );
    }
    // One edge with two targets is the way to write it.
    load(&document("1h", body, "  - {from: source, to: [a, b]}"));
    // Several sources on one edge is a merge, and fine.
    let two = "  - {id: p, type: Source, interarrival: 1min}\n  \
               - {id: q, type: Source, interarrival: 1min}\n  - {id: sink, type: Sink}";
    load(&document("1h", two, "  - {from: [p, q], to: sink}"));
}

#[test]
fn a_priority_outside_its_range_is_told_its_own_range() {
    for value in ["-1", "256", "4294967296", "1.5", "high", "[1]"] {
        let yaml = document(
            "1h",
            &format!(
                "  - {{id: source, type: Source, interarrival: 1min, priority: {value}}}\n  \
                 - {{id: sink, type: Sink}}"
            ),
            "  - [source, sink]",
        );
        let errors = try_load(&yaml).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors}");
        let kind = errors.first().kind();
        assert_eq!(
            errors.first().path().to_string(),
            "components[0].priority",
            "{errors}"
        );
        assert_eq!(
            kind.allowed().as_deref(),
            Some("a whole number from 0 to 255"),
            "{value}"
        );
        assert_eq!(kind.example().as_deref(), Some("1"), "{value}");
    }
}

#[test]
fn a_default_variant_in_the_matrix_is_declared_by_a_source_without_variants() {
    let yaml = document(
        "1h",
        "  - {id: p, type: Source, interarrival: 1min}\n  \
         - {id: q, type: Source, interarrival: 1min, variants: {red: 100%}}\n  \
         - {id: m, type: Machine, cycle_time: 1min, setup: {matrix: [{from: default, to: red, time: 1min}]}}\n  \
         - {id: sink, type: Sink}",
        "  - {from: [p, q], to: m}\n  - [m, sink]",
    );
    load(&yaml);
}

#[test]
fn shares_sum_to_100_within_rounding() {
    let yaml = |variants: &str| {
        document(
            "1h",
            &format!(
                "  - {{id: source, type: Source, interarrival: 1min, variants: {{{variants}}}}}\n  - {{id: sink, type: Sink}}"
            ),
            "  - [source, sink]",
        )
    };
    load(&yaml("a: 33.3%, b: 33.3%, c: 33.4%"));
    load(&yaml("a: 0.1%, b: 0.2%, c: 99.7%"));
    assert!(try_load(&yaml("a: 33.3%, b: 33.3%, c: 33.3%")).is_err());
    assert!(try_load(&yaml("a: 100.0001%")).is_err());
}

#[test]
fn a_matrix_entry_may_be_zero_and_a_batch_at_the_limit_loads() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min, batch: 65535 parts, variants: {a: 50%, b: 50%}}\n  \
         - {id: m, type: Machine, cycle_time: 1min, capacity: 65535 parts, setup: {matrix: [{from: a, to: b, time: 0s}]}}\n  \
         - {id: sink, type: Sink}",
        "  - [source, m, sink]",
    );
    load(&yaml);
}
