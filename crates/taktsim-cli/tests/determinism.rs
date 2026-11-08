//! The same model and seed, in two stores: byte-identical result and trace (spec
//! §2 principle 3, ADR-0024 §3).

mod support;

use std::path::Path;

use support::{Dir, EXACT, LINE};

#[allow(clippy::unwrap_used)] // Test helper.
fn the_only_run(store: &Path) -> std::path::PathBuf {
    let runs: Vec<_> = std::fs::read_dir(store.join("runs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(runs.len(), 1);
    runs.into_iter().next().unwrap()
}

#[test]
fn two_stores_hold_the_same_bytes() {
    for model in [LINE, EXACT] {
        let dir = Dir::new();
        dir.write("m.yaml", model);
        let one = dir.taktsim(&["run", "m.yaml", "--store", "one", "--format", "json"]);
        let two = dir.taktsim(&["run", "m.yaml", "--store", "two", "--format", "json"]);
        assert_eq!(one.code, 0, "{}", one.stderr);
        assert_eq!(one.stdout, two.stdout);
        let (a, b) = (
            the_only_run(&dir.path().join("one")),
            the_only_run(&dir.path().join("two")),
        );
        assert_eq!(a.file_name(), b.file_name(), "the same run id");
        for name in ["result.json", "trace.takt", "model.json"] {
            assert_eq!(
                std::fs::read(a.join(name)).unwrap(),
                std::fs::read(b.join(name)).unwrap(),
                "{name}"
            );
        }
        // The stored result is what was printed, and uses `\n` everywhere.
        let stored = std::fs::read_to_string(a.join("result.json")).unwrap();
        assert_eq!(stored, one.stdout);
        assert!(!stored.contains('\r'));
    }
}

/// Another spelling of the model: the same hash, so the same run.
#[test]
fn another_spelling_is_the_same_run() {
    let dir = Dir::new();
    dir.write("a.yaml", LINE);
    dir.write(
        "b.yaml",
        &LINE.replace("cycle_time: 45s", "cycle_time: 0.75min"),
    );
    let a = dir.taktsim(&["run", "a.yaml", "--format", "json"]);
    let b = dir.taktsim(&["run", "b.yaml", "--format", "json"]);
    assert_eq!(a.code, 0, "{}", a.stderr);
    assert_eq!(a.stdout, b.stdout);
    assert_eq!(b.lines()[0]["reused"], true);
}
