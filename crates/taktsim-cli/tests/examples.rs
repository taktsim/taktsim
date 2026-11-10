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

/// The README says what `run` prints for its model today: counts from one seed, each
/// without an error bound, and throughput and WIP later (ADR-0024 §4). Once `run`
/// reports what the model observes, or gives a number an error bound, this fails
/// until the README says so.
#[test]
fn the_readme_says_what_run_reports() {
    let text = std::fs::read_to_string(repo(&["README.md"])).unwrap();
    let (_, block) = &yaml_blocks(&text)[0];
    let dir = Dir::new();
    dir.write("readme.yaml", block);
    let outcome = dir.taktsim(&["run", "readme.yaml", "--format", "json"]);
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    let result = outcome.json();
    // The prose as one line: a note's lines start with `>`.
    let prose = text
        .replace("\n>", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(!prose.contains("not built yet"), "`run` is built");
    let unreported: Vec<&str> = result["unreported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["observable"].as_str().unwrap())
        .collect();
    assert_eq!(
        prose.contains("Throughput and WIP with confidence intervals come with replications"),
        unreported == ["throughput(sink)", "wip(buf1)"],
        "not reported: {unreported:?}"
    );
    let kinds: Vec<&str> = result["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["values"].as_array().unwrap())
        .flat_map(|v| v["accuracy"].as_array().unwrap())
        .map(|a| a["kind"].as_str().unwrap())
        .collect();
    assert_ne!(kinds.len(), 0, "the model reports some number");
    assert_eq!(
        prose.contains("each marked as having no error bound"),
        kinds.iter().all(|k| *k == "none"),
        "accuracy: {kinds:?}"
    );
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
    assert!(dir.store_is_empty());
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
            "<model>",
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

/// ADR-0022's load rule (vocabulary, question 20 a): an on-demand Source fills at
/// most 65,535 buffer places, so 40,000 and 30,000 places behind one are refused
/// at the larger buffer's `capacity`. The CLI shows it as any model error: exit 1,
/// at its path, with the line and column, from `validate` and `run` alike.
#[test]
fn an_on_demand_source_filling_too_many_places_exits_1_at_the_capacity() {
    let model = repo(&[
        "crates",
        "taktsim-components",
        "tests",
        "data",
        "malformed",
        "flow-on-demand-buffers-too-large.yaml",
    ]);
    let name = model.display().to_string();
    let dir = Dir::new();
    for command in ["validate", "run"] {
        let outcome = dir.taktsim(&[command, &name, "--format", "json"]);
        assert_eq!(outcome.code, 1, "{command}: {}", outcome.stderr);
        let lines = outcome.lines();
        assert_eq!(lines.len(), 1, "{command}: {}", outcome.stderr);
        let line = &lines[0];
        assert_eq!(line["type"], "model.not_allowed", "{command}: {line}");
        assert_eq!(line["path"], "components[1].capacity", "{command}: {line}");
        assert_eq!(line["file"].as_str(), Some(name.as_str()), "{command}");
        assert_eq!(
            (line["line"].clone(), line["column"].clone()),
            (Json::from(6), Json::from(40)),
            "{command}: {line}"
        );
        assert!(line["found"].as_str().is_some(), "{command}: {line}");
        assert!(line["allowed"].as_str().is_some(), "{command}: {line}");
    }
    assert!(dir.store_is_empty());
}

/// A cycle time whose draws almost all round to 0 ns: a gamma of mean 1 s and cv
/// 10⁶, as in `taktsim-components`' `clock_stopped.rs`.
const CLOCK_STOPS: &str = "format: \"1.0\"\n\
model: {name: m, seed: 1}\n\
experiment: {type: terminating, horizon: 1h}\n\
components:\n\
  - {id: source, type: Source}\n\
  - {id: cut, type: Machine, cycle_time: {dist: gamma, mean: 1s, cv: 1000000}}\n\
  - {id: sink, type: Sink}\n\
flow:\n\
  - [source, cut, sink]\n";

/// ADR-0022 §6, ADR-0024 §9: a run whose clock stops moving, a component that
/// addressed more than 1,000,000 events to itself at one instant, is a failed run,
/// exit 2, `run.clock_stopped`, not `run.stalled` (no events left), and nothing is
/// kept. Its line names the component's entry and the instant, says what was found
/// and what is allowed, and gives one valid value, which does run.
#[test]
fn a_clock_that_stops_moving_exits_2_and_is_not_kept() {
    let dir = Dir::new();
    dir.write("m.yaml", CLOCK_STOPS);
    let outcome = dir.taktsim(&["run", "m.yaml", "--format", "json"]);
    assert_eq!(outcome.code, 2, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "");
    let lines = outcome.lines();
    assert_eq!(lines.len(), 1, "{}", outcome.stderr);
    let line = &lines[0];
    assert_eq!(line["kind"], "error", "{line}");
    assert_eq!(line["type"], "run.clock_stopped", "{line}");
    assert_eq!(line["path"], "components[1]", "{line}");
    assert_eq!(line["time"], "0s", "{line}");
    let found = line["found"].as_str().unwrap();
    assert!(
        found.starts_with("1000001 events `cut` addressed to itself at 0s"),
        "{found}"
    );
    assert!(
        line["allowed"]
            .as_str()
            .unwrap()
            .starts_with("at most 1000000 events"),
        "{line}"
    );
    let example = line["example"].as_str().unwrap();
    assert_eq!(example, "{dist: lognormal, mean: 42s, sigma: 0.08}");
    let runs = dir.path().join(".taktsim").join("runs");
    let kept = std::fs::read_dir(&runs).map_or(0, Iterator::count);
    assert_eq!(kept, 0, "a failed run is not kept");

    // The example, in place of the cycle time, runs.
    dir.write(
        "m.yaml",
        &CLOCK_STOPS.replace("{dist: gamma, mean: 1s, cv: 1000000}", example),
    );
    let fixed = dir.taktsim(&["run", "m.yaml", "--format", "json"]);
    assert_eq!(fixed.code, 0, "{}", fixed.stderr);
}
