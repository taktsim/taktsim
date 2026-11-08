//! Every documented example accepted, every malformed one rejected with a
//! path-level JSON error and the documented exit code (roadmap M7, spec §7).

mod support;

use std::path::PathBuf;

use support::{Dir, parse, repo};
use taktsim_model::Json;

/// Splits an example into its words, without the binary's name. Examples hold no
/// quoting, so an agent can run them as they stand.
#[allow(clippy::unwrap_used)] // Test helper.
fn words(example: &str) -> Vec<&str> {
    let mut words = example.split_whitespace();
    assert_eq!(words.next().unwrap(), "taktsim", "{example}");
    words.collect()
}

#[test]
fn every_example_in_describe_runs() {
    let dir = Dir::new().with_line();
    let description = dir.taktsim(&["describe"]).json();
    let commands = description["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 4);
    for command in commands {
        let example = command["example"].as_str().unwrap();
        let outcome = dir.taktsim(&words(example));
        assert_eq!(outcome.code, 0, "{example}: {}", outcome.stderr);
        assert!(!outcome.stdout.is_empty(), "{example} prints a result");
    }
}

/// Each YAML block of a Markdown file, with the label of the panel it stands in:
/// the reader of `crates/taktsim-components/tests/docs_examples.rs`.
fn yaml_blocks(markdown: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    let mut label = String::new();
    for line in markdown.lines() {
        match (&mut current, line.trim_start()) {
            (None, "```yaml") => current = Some(String::new()),
            (Some(block), "```") => {
                blocks.push((label.clone(), std::mem::take(block)));
                current = None;
            }
            (Some(block), _) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, text) => {
                if text.contains("ts-label") {
                    text.clone_into(&mut label);
                }
            }
        }
    }
    blocks
}

/// What a page's fragment needs around it to be a whole model, as in the reader
/// above; `None` for what is no list of components.
fn whole(block: &str) -> Option<String> {
    if block.starts_with("format:") {
        return Some(block.to_owned());
    }
    let list: Vec<&str> = block
        .lines()
        .skip_while(|line| *line == "components:")
        .take_while(|line| line.starts_with(' '))
        .collect();
    let ids: Vec<&str> = list
        .iter()
        .filter_map(|line| line.trim_start().strip_prefix("- id:"))
        .map(str::trim)
        .collect();
    if ids.is_empty() {
        return None;
    }
    Some(format!(
        "format: \"1.0\"\nmodel: {{name: fragment}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n  - {{id: source, type: Source, interarrival: 1min}}\n{}\n  - {{id: sink, type: Sink}}\n\
         flow:\n  - [source, {}, sink]\n",
        list.join("\n"),
        ids.join(", ")
    ))
}

#[test]
fn every_yaml_block_of_the_docs_validates() {
    let dir = Dir::new();
    let mut pages: Vec<PathBuf> = std::fs::read_dir(repo(&["docs", "concepts"]))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    pages.sort();
    pages.push(repo(&["README.md"]));
    pages.push(repo(&["docs", "introduction.md"]));
    let (mut checked, mut waiting) = (0, 0);
    for page in &pages {
        let text = std::fs::read_to_string(page).unwrap();
        for (label, block) in yaml_blocks(&text) {
            let Some(model) = whole(&block) else {
                // `flow:` alone, or a section of a later tier.
                waiting += 1;
                continue;
            };
            if label.contains("v0.2") {
                waiting += 1;
                continue;
            }
            dir.write("block.yaml", &model);
            let outcome = dir.taktsim(&["validate", "block.yaml", "--format", "json"]);
            assert_eq!(
                outcome.code,
                0,
                "{}:\n{block}\n{}",
                page.display(),
                outcome.stderr
            );
            assert_eq!(outcome.json()["valid"], true);
            checked += 1;
        }
    }
    // The concept pages as counted in the components' reader, the README and the
    // introduction.
    assert!(checked >= 9, "{checked}");
    assert_eq!(waiting, 4);
}

/// The first model of the README and the introduction runs, not only validates.
#[test]
fn the_readme_and_the_introduction_run() {
    for page in [repo(&["README.md"]), repo(&["docs", "introduction.md"])] {
        let dir = Dir::new();
        let text = std::fs::read_to_string(&page).unwrap();
        let (_, block) = &yaml_blocks(&text)[0];
        dir.write("readme.yaml", block);
        let outcome = dir.taktsim(&["run", "readme.yaml", "--seed", "42", "--format", "json"]);
        assert_eq!(outcome.code, 0, "{}: {}", page.display(), outcome.stderr);
        assert_eq!(outcome.json()["complete"], true);
    }
}

#[test]
fn malformed_models_exit_1_with_a_path() {
    let corpus = repo(&["crates", "taktsim-components", "tests", "data", "malformed"]);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    assert!(paths.len() >= 40);
    let dir = Dir::new();
    for path in paths {
        let name = path.display().to_string();
        let outcome = dir.taktsim(&["validate", &name, "--format", "json"]);
        assert_eq!(outcome.code, 1, "{name}: {}", outcome.stderr);
        // The report on stdout, and each error on stderr.
        let report = outcome.json();
        assert_eq!(report["valid"], false, "{name}");
        assert_eq!(report["model_hash"], Json::Null, "{name}");
        let errors = report["errors"].as_array().unwrap();
        let lines = outcome.lines();
        assert_eq!(errors, lines.as_slice(), "{name}");
        assert_ne!(lines.len(), 0, "{name}");
        for line in &lines {
            assert_eq!(line["kind"], "error", "{name}");
            assert!(
                line["type"].as_str().unwrap().starts_with("model."),
                "{name}: {line}"
            );
            let at = line["path"].as_str().unwrap();
            assert!(!at.is_empty() && at != "(document)", "{name}: {line}");
            assert!(line["found"].as_str().is_some(), "{name}: {line}");
            assert!(line["allowed"].as_str().is_some(), "{name}: {line}");
            assert_eq!(line["file"].as_str(), Some(name.as_str()), "{name}");
            assert!(line["line"] != Json::Null, "{name}: {line}");
        }
        // `run` refuses it the same way, and nothing starts.
        let run = dir.taktsim(&["run", &name, "--format", "json"]);
        assert_eq!(run.code, 1, "{name}");
        assert_eq!(run.stdout, "", "{name}");
        assert_eq!(run.lines(), lines, "{name}");
    }
    assert!(!dir.path().join(".taktsim").exists());
}

/// Each of these exits 1 with an empty stdout and one typed line at its path.
#[test]
fn malformed_invocations_exit_1_at_their_path() {
    let dir = Dir::new().with_line();
    let table: [(&[&str], &str, &str); 14] = [
        (&[], "args.missing_command", "<command>"),
        (&["simulate"], "args.unknown_command", "<command>"),
        (&["run"], "args.missing_argument", "<model>"),
        (&["validate"], "args.missing_argument", "<model>"),
        (&["schema"], "args.missing_argument", "<type>"),
        (
            &["run", "line.yaml", "extra"],
            "args.unknown_argument",
            "extra",
        ),
        (
            &["run", "line.yaml", "--sed", "1"],
            "args.unknown_argument",
            "--sed",
        ),
        (
            &["run", "line.yaml", "--seed"],
            "args.invalid_value",
            "--seed",
        ),
        (
            &["run", "line.yaml", "--seed", "x"],
            "args.invalid_value",
            "--seed",
        ),
        (
            &["run", "line.yaml", "--seed", "-1"],
            "args.invalid_value",
            "--seed",
        ),
        (
            &["run", "line.yaml", "--seed", "1", "--seed", "2"],
            "args.repeated",
            "--seed",
        ),
        (
            &["run", "line.yaml", "--horizon", "5"],
            "args.invalid_value",
            "--horizon",
        ),
        (
            &["run", "line.yaml", "--max-events", "x"],
            "args.invalid_value",
            "--max-events",
        ),
        (
            &["run", "line.yaml", "--format", "yaml"],
            "args.invalid_value",
            "--format",
        ),
    ];
    for (args, error_type, path) in table {
        let mut args = args.to_vec();
        if !args.contains(&"--format") {
            args.extend(["--format", "json"]);
        }
        let outcome = dir.taktsim(&args);
        assert_eq!(outcome.code, 1, "{args:?}: {}", outcome.stderr);
        assert_eq!(outcome.stdout, "", "{args:?}");
        let raw = outcome.stderr.lines().next().unwrap_or_default();
        // A bad `--format` has no format to speak in: the line is for a person.
        let line = if raw.starts_with('{') {
            parse(raw)
        } else {
            assert!(args.contains(&"yaml"), "{args:?}: {}", outcome.stderr);
            assert!(raw.contains(error_type) && raw.contains(path), "{raw}");
            continue;
        };
        assert_eq!(line["type"], error_type, "{args:?}: {line}");
        assert_eq!(line["path"], path, "{args:?}: {line}");
    }
    // No model file, a model of another kind: the file's problem, at the model.
    let missing = dir.taktsim(&["run", "nothing.yaml", "--format", "json"]);
    assert_eq!(missing.code, 7);
    assert_eq!(missing.lines()[0]["path"], "<model>");
    dir.write("model.txt", "x");
    let kind = dir.taktsim(&["run", "model.txt", "--format", "json"]);
    assert_eq!(kind.code, 1);
    assert_eq!(kind.lines()[0]["example"], "line-a.yaml");
    // A zero horizon and a seed beyond the exact range: refused, with an example.
    let zero = dir.taktsim(&["run", "line.yaml", "--horizon", "0s", "--format", "json"]);
    assert_eq!(zero.code, 1);
    assert_eq!(zero.lines()[0]["path"], "--horizon");
    let big = dir.taktsim(&[
        "run",
        "line.yaml",
        "--seed",
        "9007199254740992",
        "--format",
        "json",
    ]);
    assert_eq!(big.code, 1);
    assert_eq!(big.lines()[0]["type"], "run.seed_out_of_range");
}
