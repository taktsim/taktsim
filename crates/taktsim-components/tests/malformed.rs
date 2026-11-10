//! The corpus of malformed models for the factory vocabulary and the flow: every
//! file in `data/malformed/` fails to load with exactly the errors its
//! `# expect:` lines list, in order, each `path @ line:column Kind`, and each
//! error has a field path, what was found, what is allowed and an example, as
//! far as its kind says them. The expectations are at the end of each file, so
//! they do not move the lines the errors point at.

mod support;

use std::path::Path;

use support::{document, load, model_files, registry, try_load};
use taktsim_model::{LoadErrors, Loader, Segment};

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

fn corpus() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("malformed");
    model_files(&dir)
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
            let at = path.display();
            let kind = e.kind();
            assert!(kind.found().is_some_and(|f| !f.is_empty()), "{at}: {e}");
            assert!(kind.allowed().is_some_and(|a| !a.is_empty()), "{at}: {e}");
            assert!(kind.example().is_some_and(|x| !x.is_empty()), "{at}: {e}");
        }
    }
    // The flow's own rules, which the corpus above does not all reach.
    let body = "  - {id: source, type: Source, interarrival: 1min}\n  \
                - {id: a, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}";
    for (flow, path, example) in [
        (
            "  - [source, a]\n  - {from: a, to: a}\n  - [a, sink]",
            "flow[1].to",
            "{from: weld, to: inspect}",
        ),
        (
            "  - [source, a, sink]\n  - [a, sink]",
            "flow[1][1]",
            "{from: a, to: sink}",
        ),
        (
            "  - [source, a]\n  - {from: [a, a], to: sink}",
            "flow[1].from[1]",
            "[a]",
        ),
    ] {
        let errors = try_load(&document("1h", body, flow)).unwrap_err();
        let e = errors.first();
        assert_eq!(e.path().to_string(), path, "{errors}");
        assert_eq!(e.kind().example().as_deref(), Some(example), "{errors}");
    }
    // The loader's message prescribes no route: the catalog decides what may
    // connect, and the standard one refuses a loop until M6 (ADR-0022 §3).
    let errors = try_load(&document(
        "1h",
        body,
        "  - [source, a]\n  - {from: a, to: a}\n  - [a, sink]",
    ))
    .unwrap_err();
    assert_eq!(
        errors.first().kind().to_string(),
        "an edge from `a` to itself"
    );
    let twice = format!("{body}\n  - {{id: a, type: Sink}}");
    let errors = try_load(&document("1h", &twice, "  - [source, a, sink]")).unwrap_err();
    assert_eq!(errors.first().path().to_string(), "components[3].id");
    assert_eq!(errors.first().kind().example().as_deref(), Some("a-2"));
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
        // At the second edge, with the one edge that replaces both.
        assert_eq!(errors.first().path().to_string(), "flow[1]", "{errors}");
        assert_eq!(
            errors.first().kind().example().as_deref(),
            Some("{from: source, to: [a, b]}"),
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

#[test]
fn a_corpus_is_its_model_files_only() {
    // macOS's Finder writes a binary `.DS_Store` into a folder it shows; git
    // ignores it, so the corpus tests must too.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("components-corpus");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(".DS_Store"),
        b"\x00\x00\x00\x01Bud1\x00\xff\xfe\x80",
    )
    .unwrap();
    std::fs::write(dir.join("b.yaml"), "b: 1\n").unwrap();
    std::fs::write(dir.join("a.json"), "{}").unwrap();
    std::fs::write(dir.join("notes.txt"), "not a model").unwrap();
    std::fs::create_dir_all(dir.join("parts.yaml")).unwrap();
    assert_eq!(model_files(&dir), [dir.join("a.json"), dir.join("b.yaml")]);
}

#[test]
fn a_loop_error_names_the_loop_itself() {
    // Only the cycle, without the path that led to it, and starting where it
    // starts, also when that is the first component of the search.
    let errors = registry_loader_errors("flow-loop.yaml");
    assert_eq!(
        errors.first().kind().found().as_deref(),
        Some("a loop: m -> back -> m")
    );
    let errors = try_load(&document(
        "1h",
        "  - {id: a, type: Machine, cycle_time: 1s}\n  \
         - {id: source, type: Source, interarrival: 1min}\n  \
         - {id: b, type: Machine, cycle_time: 1s}\n  - {id: sink, type: Sink}",
        "  - [source, a]\n  - [a, b]\n  - {from: b, to: [a, sink]}",
    ))
    .unwrap_err();
    assert_eq!(
        errors.first().kind().found().as_deref(),
        Some("a loop: a -> b -> a")
    );
}

/// The errors of one corpus file.
#[allow(clippy::unwrap_used)] // Test helper.
fn registry_loader_errors(name: &str) -> LoadErrors {
    let registry = registry();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("malformed")
        .join(name);
    Loader::new(registry.catalog())
        .load_file(&path)
        .unwrap_err()
}

#[test]
fn a_matrix_may_hold_two_changes_that_share_one_end() {
    // Only the same pair twice is refused: two changes from one variant, or to
    // one variant, are two different changes.
    for matrix in [
        "[{from: a, to: b, time: 1s}, {from: a, to: c, time: 2s}]",
        "[{from: a, to: c, time: 1s}, {from: b, to: c, time: 2s}]",
    ] {
        load(&document(
            "1h",
            &format!(
                "  - {{id: source, type: Source, interarrival: 1min, variants: {{a: 50%, b: 25%, c: 25%}}}}\n  \
                 - {{id: m, type: Machine, cycle_time: 1s, setup: {{matrix: {matrix}}}}}\n  \
                 - {{id: sink, type: Sink}}"
            ),
            "  - [source, m, sink]",
        ));
    }
}

/// A Source declaring `n` variant names whose shares sum to exactly 100 %.
#[allow(clippy::unwrap_used)] // Test helper.
fn many_variants(n: usize) -> String {
    use std::fmt::Write;

    // 100 % / 65536 is exact in decimal; the last name takes the rest.
    let share = 100.0 / 65536.0;
    let mut names = String::new();
    for i in 0..n - 1 {
        write!(names, "v{i:05}: {share}%, ").unwrap();
    }
    let rest = 100.0 - share * f64::from(u32::try_from(n - 1).unwrap());
    write!(names, "v{:05}: {rest}%", n - 1).unwrap();
    document(
        "1h",
        &format!(
            "  - {{id: source, type: Source, interarrival: 1min, variants: {{{names}}}}}\n  \
             - {{id: sink, type: Sink}}"
        ),
        "  - [source, sink]",
    )
}

#[test]
fn a_model_holds_at_most_65535_variant_names() {
    // The most a `VariantId` numbers loads and builds; one more is refused at
    // load time, so a model that loads always builds.
    let model = load(&many_variants(65_535));
    registry()
        .build(model.document(), taktsim_core::EngineConfig::new(1))
        .unwrap();
    let errors = try_load(&many_variants(65_536)).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    let e = errors.first();
    assert_eq!(e.path().to_string(), "components[0].variants");
    assert_eq!(
        e.kind().found().as_deref(),
        Some("65536 variant names in the model")
    );
}

/// The limit is reported where it can be fixed: at the Source whose names pass
/// it, whatever entry comes first, and at the entry of a Source that adds the
/// implicit `default`.
#[test]
fn too_many_variant_names_are_reported_at_the_source_that_passes_the_limit() {
    // A Sink listed before the Source.
    let doc = many_variants(65_536);
    let (head, rest) = doc.split_once("  - {id: source").unwrap();
    let (source, tail) = rest.split_once("\n  - {id: sink, type: Sink}").unwrap();
    let doc = format!("{head}  - {{id: sink, type: Sink}}\n  - {{id: source{source}{tail}");
    let errors = try_load(&doc).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    let e = errors.first();
    assert_eq!(e.path().to_string(), "components[1].variants");
    assert_eq!(
        e.kind().found().as_deref(),
        Some("65536 variant names in the model")
    );

    // 65,535 names, then a Source without variants, which makes `default`.
    let doc = many_variants(65_535).replace(
        "  - {id: sink, type: Sink}",
        "  - {id: sink, type: Sink}\n  - {id: plain, type: Source, interarrival: 1min}",
    );
    let doc = doc.replace(
        "  - [source, sink]",
        "  - [source, sink]\n  - [plain, sink]",
    );
    let errors = try_load(&doc).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    let e = errors.first();
    assert_eq!(e.path().to_string(), "components[2]");
    assert_eq!(
        e.kind().found().as_deref(),
        Some(
            "65536 variant names in the model, counting `default` for this Source, which declares none"
        )
    );
}

/// The shape of a flow-style YAML value from its first character: a list, a
/// mapping or a scalar.
fn shape(text: &str) -> &'static str {
    match text.trim_start().chars().next() {
        Some('[') => "a list",
        Some('{') => "a mapping",
        _ => "a scalar",
    }
}

/// Every example is a value of the shape the error's path holds, so writing it
/// at that path keeps the model's structure (spec §5: "one example of a valid
/// value"). A reference gets an id, a matrix entry one entry, and a rule that no
/// value of one key can satisfy is reported at the entry or edge it is about.
/// The corpus is written in flow style, so the character an error points at
/// tells the shape of the value there.
#[test]
fn every_example_has_the_shape_of_the_value_at_its_path() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    let mut failures = Vec::new();
    for path in corpus() {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let errors = loader.load_file(&path).unwrap_err();
        for e in errors.iter() {
            let (Some(at), Some(example)) = (e.location(), e.kind().example()) else {
                continue;
            };
            let kind = format!("{:?}", e.kind());
            let found = e.kind().found().unwrap_or_default();
            // A missing field points at its parent, and a wrong type is a
            // value of the wrong shape by definition.
            if kind.starts_with("MissingField") || kind.starts_with("WrongType") {
                continue;
            }
            // Known gap: a component that no edge enters or leaves is reported
            // at its entry with the edge that would connect it as the example,
            // because the missing edge has no path of its own.
            if found.starts_with("no edge entering") || found.starts_with("no edge leaving") {
                continue;
            }
            let value = &lines[at.line() - 1][at.column() - 1..];
            let segments = e.path().segments();
            let expected = match segments {
                // A whole edge, pointed at by its source: a chain or a mapping.
                [Segment::Key(flow), Segment::Index(_)] if flow == "flow" => {
                    if shape(&example) == "a scalar" {
                        "a list or a mapping"
                    } else {
                        shape(&example)
                    }
                }
                _ => shape(value),
            };
            // A duration where a distribution is allowed is a constant one.
            let constant = expected == "a scalar" && example.starts_with("{dist:");
            if shape(&example) != expected && !constant {
                failures.push(format!(
                    "{} {}: the path holds {expected} (`{}`), the example `{example}` is {}",
                    path.file_name().unwrap().to_string_lossy(),
                    e.path(),
                    value.chars().take(24).collect::<String>(),
                    shape(&example),
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
