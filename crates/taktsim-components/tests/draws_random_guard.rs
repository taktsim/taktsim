//! ADR-0024 §5's first guard: each component type's `draws_random` destructures its
//! parameters without `..`, so a parameter nobody answered for does not compile. A
//! type that skips it compiles all the same, so this reads the sources.

#![allow(clippy::unwrap_used, clippy::panic)] // A failed read is a failed test.

use std::path::Path;

/// The body of `draws_random` in each `impl ComponentType` of the crate's sources:
/// the file, the type and the body's lines up to its closing brace.
fn implementations() -> Vec<(String, String, Vec<String>)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&src)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "rs"))
        .collect();
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (i, line) in lines.iter().enumerate() {
            let Some(rest) = line.strip_prefix("impl ComponentType<") else {
                continue;
            };
            let type_name = rest.split(" for ").nth(1).unwrap_or(rest);
            let type_name = type_name.trim_end_matches(" {").to_owned();
            let impl_end = lines[i..]
                .iter()
                .position(|l| *l == "}")
                .map_or(lines.len(), |n| i + n);
            let Some(start) = lines[i..impl_end]
                .iter()
                .position(|l| l.trim() == "fn draws_random(&self) -> bool {")
                .map(|n| i + n + 1)
            else {
                found.push((name.clone(), type_name, Vec::new()));
                continue;
            };
            let end = lines[start..]
                .iter()
                .position(|l| *l == "    }")
                .map_or(lines.len(), |n| start + n);
            let body = lines[start..end]
                .iter()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty() && !l.starts_with("//"))
                .collect();
            found.push((name.clone(), type_name, body));
        }
    }
    found
}

#[test]
fn every_draws_random_destructures_its_parameters() {
    let found = implementations();
    let types: Vec<&str> = found.iter().map(|(_, t, _)| t.as_str()).collect();
    for expected in ["Buffer", "Machine", "Sink", "Source"] {
        assert!(
            types.contains(&expected),
            "{expected} not found in {types:?}"
        );
    }
    for (file, type_name, body) in &found {
        assert!(
            !body.is_empty(),
            "{file}: {type_name} has no `draws_random` of its own, so it says it draws"
        );
        let first = &body[0];
        assert!(
            first.starts_with("let Self"),
            "{file}: {type_name}'s `draws_random` starts with `{first}`, not by destructuring \
             `Self` (ADR-0024 §5)"
        );
        let pattern: Vec<&String> = body
            .iter()
            .take_while(|l| !l.ends_with("= self;"))
            .chain(body.iter().find(|l| l.ends_with("= self;")))
            .collect();
        assert!(
            pattern.last().is_some_and(|l| l.ends_with("= self;")),
            "{file}: {type_name}'s pattern does not end in `= self;`"
        );
        assert!(
            pattern.iter().all(|l| !l.contains("..")),
            "{file}: {type_name}'s pattern has `..`, so a new parameter compiles unanswered"
        );
    }
}
