//! Independent tests of M7's command line through the real binary, written from
//! `docs/spec.md` §5 and ADR-0024, without reading the implementation. Expected values are
//! derived from those texts or counted by hand; each test says from where.

// Helpers outside `#[test]` functions: a failed expectation is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use regex::Regex;
use taktsim_model::{Format, Json, parse};
use taktsim_run::ResultDocument;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fresh directory for one test, below Cargo's temporary directory.
fn fresh(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("independent_m7_cli")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[derive(Debug)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn taktsim(dir: &Path, args: &[&str]) -> Out {
    let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    Out {
        code: output.status.code().unwrap(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn json(text: &str) -> Json {
    parse(text, Format::Json)
        .unwrap_or_else(|e| panic!("not JSON: {e}\n{text}"))
        .to_json()
}

/// Each line of stderr as JSON: with `--format json` nothing else may be there.
fn lines(stderr: &str) -> Vec<Json> {
    stderr.lines().map(json).collect()
}

fn errors_of(stderr: &str) -> Vec<Json> {
    lines(stderr)
        .into_iter()
        .filter(|l| l["kind"] == "error")
        .collect()
}

fn write(dir: &Path, name: &str, text: &str) {
    fs::write(dir.join(name), text).unwrap();
}

/// A store that cannot be reused between tests: each test has its own directory.
const EXACT: &str = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 125s}\n\
components:\n  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 10s}\n  - {id: buf, type: Buffer, capacity: 1 parts}\n  - {id: weld, type: Machine, cycle_time: 30s}\n  - {id: sink, type: Sink}\n\
flow:\n  - [source, cut, buf, weld, sink]\n";

const DRAWN: &str = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
components:\n  - {id: source, type: Source, interarrival: {dist: exponential, mean: 40s}}\n  - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 30s, sigma: 0.2}}\n  - {id: sink, type: Sink}\n\
flow:\n  - [source, cut, sink]\n";

/// Spec §4's model document with its header as ADR-0008 says to read it.
const SPEC_EXAMPLE: &str = r#"format: "1.0"
model:
  name: line-a
  time_unit: s
  seed: 42
experiment:
  type: terminating
  horizon: 8h
  precision: { throughput(sink): 2% }
components:
  - id: source
    type: Source
    interarrival: { dist: exponential, mean: 40s }
  - id: cut
    type: Machine
    cycle_time: { dist: lognormal, mean: 42s, sigma: 0.08 }
    mtbf: 6h
    mttr: { dist: exponential, mean: 12min }
  - id: buf1
    type: Buffer
    capacity: 20 parts
  - id: weld
    type: Machine
    cycle_time: 45s
    setup: { per_variant: 4min }
  - id: sink
    type: Sink
flow:
  - [source, cut, buf1, weld, sink]
observe:
  - throughput(sink)
  - state_shares(cut, weld)
  - wip(buf1)
"#;

fn value(doc: &Json, name: &str) -> i128 {
    let metrics = doc["metrics"].as_array().unwrap();
    let metric = metrics
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("no metric {name}"));
    let Json::Integer(n) = metric["values"][0]["value"] else {
        panic!("{name} is a count");
    };
    n
}

fn repo(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        |p, q| p.join(q),
    )
}

fn sorted_dir(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A JSON Schema check of what the published schemas use, written here and not taken from the
/// code under test. A keyword it does not know fails, so a schema cannot pass by being skipped.
#[allow(clippy::too_many_lines)] // One match, one arm per keyword: it reads best in one place.
fn check(schema: &Json, root: &Json, value: &Json, at: &str, errors: &mut Vec<String>) {
    let Some(keywords) = schema.as_object() else {
        errors.push(format!("{at}: a schema that is no object"));
        return;
    };
    for (keyword, rule) in keywords {
        match keyword.as_str() {
            "$schema" | "$defs" | "title" | "description" | "default" | "examples" | "then" => {}
            "$ref" => {
                let name = rule.as_str().unwrap().strip_prefix("#/$defs/").unwrap();
                check(&root["$defs"][name], root, value, at, errors);
            }
            "type" => {
                let allowed: Vec<&str> = rule.as_array().map_or_else(
                    || vec![rule.as_str().unwrap()],
                    |list| list.iter().map(|t| t.as_str().unwrap()).collect(),
                );
                let actual = match value {
                    Json::Null => "null",
                    Json::Bool(_) => "boolean",
                    Json::Integer(_) => "integer",
                    Json::Float(_) => "number",
                    Json::String(_) => "string",
                    Json::Array(_) => "array",
                    Json::Object(_) => "object",
                };
                let number = actual == "integer" && allowed.contains(&"number");
                if !allowed.contains(&actual) && !number {
                    errors.push(format!("{at}: {actual} is not {allowed:?}"));
                }
            }
            "required" => {
                if let Some(object) = value.as_object() {
                    for key in rule.as_array().unwrap() {
                        let key = key.as_str().unwrap();
                        if !object.contains_key(key) {
                            errors.push(format!("{at}: `{key}` is missing"));
                        }
                    }
                }
            }
            "properties" => {
                if let Some(object) = value.as_object() {
                    for (key, sub) in rule.as_object().unwrap() {
                        if let Some(field) = object.get(key) {
                            check(sub, root, field, &format!("{at}.{key}"), errors);
                        }
                    }
                }
            }
            "additionalProperties" => {
                if let Some(object) = value.as_object() {
                    let known = schema["properties"].as_object();
                    for (key, field) in object {
                        if known.is_some_and(|k| k.contains_key(key)) {
                            continue;
                        }
                        if rule == &Json::Bool(false) {
                            errors.push(format!("{at}: `{key}` is not allowed"));
                        } else if rule.as_object().is_some() {
                            check(rule, root, field, &format!("{at}.{key}"), errors);
                        }
                    }
                }
            }
            "propertyNames" => {
                if let Some(object) = value.as_object() {
                    for key in object.keys() {
                        check(rule, root, &Json::from(key.as_str()), at, errors);
                    }
                }
            }
            "items" => {
                if let Some(list) = value.as_array() {
                    for (i, item) in list.iter().enumerate() {
                        check(rule, root, item, &format!("{at}[{i}]"), errors);
                    }
                }
            }
            "minItems" => {
                if let (Some(list), Json::Integer(least)) = (value.as_array(), rule)
                    && (list.len() as i128) < *least
                {
                    errors.push(format!("{at}: fewer than {least} items"));
                }
            }
            "uniqueItems" => {
                if let Some(list) = value.as_array() {
                    let repeated = list
                        .iter()
                        .enumerate()
                        .any(|(i, item)| list[..i].contains(item));
                    if repeated && rule == &Json::Bool(true) {
                        errors.push(format!("{at}: items repeat"));
                    }
                }
            }
            "enum" => {
                if !rule.as_array().unwrap().contains(value) {
                    errors.push(format!("{at}: {value} is not in the enum"));
                }
            }
            "const" => {
                if rule != value {
                    errors.push(format!("{at}: {value} is not {rule}"));
                }
            }
            "pattern" => {
                if let Some(text) = value.as_str() {
                    let re = Regex::new(rule.as_str().unwrap()).unwrap();
                    if !re.is_match(text) {
                        errors.push(format!("{at}: {text} does not match {rule}"));
                    }
                }
            }
            "minimum" => {
                if let (Some(number), Some(least)) = (value.as_f64(), rule.as_f64())
                    && number < least
                {
                    errors.push(format!("{at}: {number} is below {least}"));
                }
            }
            "exclusiveMinimum" => {
                if let (Some(number), Some(least)) = (value.as_f64(), rule.as_f64())
                    && number <= least
                {
                    errors.push(format!("{at}: {number} is not above {least}"));
                }
            }
            "minProperties" => {
                if let (Some(object), Json::Integer(least)) = (value.as_object(), rule)
                    && (object.len() as i128) < *least
                {
                    errors.push(format!("{at}: fewer than {least} keys"));
                }
            }
            "not" => {
                let mut own = Vec::new();
                check(rule, root, value, at, &mut own);
                if own.is_empty() {
                    errors.push(format!("{at}: matches what `not` forbids"));
                }
            }
            "maximum" => {
                if let (Some(number), Some(most)) = (value.as_f64(), rule.as_f64())
                    && number > most
                {
                    errors.push(format!("{at}: {number} is above {most}"));
                }
            }
            "allOf" => {
                for sub in rule.as_array().unwrap() {
                    check(sub, root, value, at, errors);
                }
            }
            "anyOf" | "oneOf" => {
                let passing = rule
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|sub| {
                        let mut own = Vec::new();
                        check(sub, root, value, at, &mut own);
                        own.is_empty()
                    })
                    .count();
                let good = if keyword == "oneOf" {
                    passing == 1
                } else {
                    passing >= 1
                };
                if !good {
                    errors.push(format!("{at}: {passing} branches of {keyword} pass"));
                }
            }
            "if" => {
                let mut own = Vec::new();
                check(rule, root, value, at, &mut own);
                if own.is_empty() {
                    check(&schema["then"], root, value, at, errors);
                }
            }
            other => errors.push(format!("{at}: this checker does not know `{other}`")),
        }
    }
}

fn assert_matches_schema(schema: &Json, value: &Json, what: &str) {
    let mut errors = Vec::new();
    check(schema, schema, value, "$", &mut errors);
    assert!(errors.is_empty(), "{what}: {errors:#?}\n{value}");
}

fn schema_of(dir: &Path, name: &str) -> Json {
    let out = taktsim(dir, &["schema", name]);
    assert_eq!(out.code, 0, "schema {name}: {}", out.stderr);
    json(&out.stdout)
}

// ---------------------------------------------------------------------------
// `run`: a document on stdout, the same as in the store
// ---------------------------------------------------------------------------

/// Spec §5: stdout carries exactly one document. ADR-0024 §8: `result.json` is the "same bytes
/// as `run --format json`". §3: `\n` line ends on every target. A second run prints the same bytes.
/// The counts are the blocking timeline counted by hand: created at 0, 10, 20, 40, 70, 100;
/// delivered at 40, 70, 100; 3 alive at 125 s.
#[test]
fn stdout_is_one_document_with_the_stored_bytes() {
    let dir = fresh("one_document");
    write(&dir, "m.yaml", EXACT);
    let out = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let doc = json(&out.stdout);
    assert_eq!(value(&doc, "created(source)"), 6);
    assert_eq!(value(&doc, "delivered(sink)"), 3);
    assert_eq!(value(&doc, "items_alive"), 3);
    assert_eq!(doc["format"], "taktsim-result/1");
    assert!(!out.stdout.contains('\r'));

    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let stored = dir
        .join(".taktsim")
        .join("runs")
        .join(id)
        .join("result.json");
    assert_eq!(fs::read_to_string(&stored).unwrap(), out.stdout);

    let again = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(again.code, 0);
    assert_eq!(again.stdout, out.stdout);

    // Text is another rendering of the same value: it adds nothing to the store.
    let text = taktsim(&dir, &["run", "m.yaml"]);
    assert_eq!(text.code, 0);
    assert_eq!(sorted_dir(&dir.join(".taktsim").join("runs")).len(), 1);
    for word in ["delivered(sink)", id, "125s"] {
        assert!(text.stdout.contains(word), "{word} in\n{}", text.stdout);
    }
}

/// ADR-0024 §3: "One document for every run, stored and printed byte for byte the same", and the
/// reader rule ignores unknown keys. So a stored result with a key this build does not know is
/// reused, and what it prints is still what is stored.
#[test]
fn a_reused_result_prints_what_is_stored() {
    let dir = fresh("reused_later_key");
    write(&dir, "m.yaml", EXACT);
    let out = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    let doc = json(&out.stdout);
    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let stored = dir
        .join(".taktsim")
        .join("runs")
        .join(id)
        .join("result.json");
    let later = out.stdout.replacen(
        "\"complete\": true,",
        "\"a_later_key\": 1,\n  \"complete\": true,",
        1,
    );
    fs::write(&stored, &later).unwrap();
    let again = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert_eq!(again.stdout, later);
}

/// Stored and printed documents have no absolute path, no time of day, no host name
/// (ADR-0024 §3). So a model in another directory under another name, with another current
/// directory, an empty environment and a store given as a relative or absolute path gives the same
/// bytes for the result, the trace and the model. The file name is not in the result either.
#[test]
fn two_stores_two_places_one_set_of_bytes() {
    let top = fresh("two_places");
    let first = top.join("first");
    let second = top.join("second").join("deeper");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    write(&first, "model_one.yaml", DRAWN);
    write(&second, "other-name.yaml", DRAWN);

    let a = taktsim(
        &first,
        &[
            "run",
            "model_one.yaml",
            "--seed",
            "42",
            "--store",
            "store_a",
            "--format",
            "json",
        ],
    );
    assert_eq!(a.code, 0, "{}", a.stderr);
    let absolute = top.join("elsewhere");
    let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .current_dir(&second)
        .env_clear()
        .env("TZ", "Asia/Tokyo")
        .env("LANG", "de_DE.UTF-8")
        .args([
            "run",
            "other-name.yaml",
            "--seed",
            "42",
            "--store",
            absolute.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), a.stdout);

    let doc = json(&a.stdout);
    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let here = first.join("store_a").join("runs").join(id);
    let there = absolute.join("runs").join(id);
    for file in ["model.json", "result.json", "trace.takt"] {
        assert_eq!(
            fs::read(here.join(file)).unwrap(),
            fs::read(there.join(file)).unwrap(),
            "{file}"
        );
    }
    let result = fs::read_to_string(here.join("result.json")).unwrap();
    for word in [
        "model_one",
        "store_a",
        top.to_str().unwrap(),
        std::env::consts::OS.to_uppercase().as_str(),
    ] {
        assert!(!result.contains(word), "{word} in the result");
    }
    let platform = doc["provenance"]["platform"].as_str().unwrap();
    assert_eq!(
        platform,
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    );
}

/// ADR-0024 §8: the nearest `.taktsim/` upward, else `.taktsim/` here; `--store` overrides. A
/// `progress` line states where the store is and whether the run was reused (the stored document
/// holds no absolute path, so stderr must say it).
#[test]
fn the_store_is_found_upward_and_stated() {
    let top = std::env::temp_dir().join(format!("taktsim-independent-m7-{}", std::process::id()));
    if top.exists() {
        fs::remove_dir_all(&top).unwrap();
    }
    let below = top.join("a").join("b");
    fs::create_dir_all(&below).unwrap();
    write(&below, "m.yaml", EXACT);

    let out = taktsim(&below, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(below.join(".taktsim").join("store.json").is_file());
    let progress: Vec<Json> = lines(&out.stderr)
        .into_iter()
        .filter(|l| l["kind"] == "progress")
        .collect();
    assert_eq!(progress.len(), 1, "{}", out.stderr);
    assert_eq!(progress[0]["reused"], false);
    assert!(progress[0]["store"].as_str().unwrap().ends_with(".taktsim"));

    fs::remove_dir_all(below.join(".taktsim")).unwrap();
    fs::create_dir_all(top.join(".taktsim")).unwrap();
    let out = taktsim(&below, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!below.join(".taktsim").exists());
    assert!(top.join(".taktsim").join("runs").is_dir());
    let out = taktsim(&below, &["run", "m.yaml", "--format", "json"]);
    let line = lines(&out.stderr)
        .into_iter()
        .find(|l| l["kind"] == "progress")
        .unwrap();
    assert_eq!(line["reused"], true);

    let chosen = taktsim(
        &below,
        &["run", "m.yaml", "--store", "mine", "--format", "json"],
    );
    assert_eq!(chosen.code, 0);
    assert!(below.join("mine").join("runs").is_dir());
    fs::remove_dir_all(&top).unwrap();
}

/// "Opening one does not create it; the first write does." A store several directories deep is
/// made as a whole.
#[test]
fn a_new_store_may_be_several_directories_deep() {
    let dir = fresh("deep_store");
    write(&dir, "m.yaml", EXACT);
    let out = taktsim(
        &dir,
        &["run", "m.yaml", "--store", "x/y/z", "--format", "json"],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        dir.join("x")
            .join("y")
            .join("z")
            .join("store.json")
            .is_file()
    );
}

/// ADR-0024 §8: "another process did the same run, and the temporary one is dropped". Eight processes at
/// once on one new store: every one succeeds with the same document, one directory is kept, and no
/// temporary directory is left. A race needs its moment, so this is 30 new stores in a row; the timing is
/// the only thing that varies, and the expected outcome holds for every timing.
#[test]
fn processes_racing_on_one_store_all_succeed() {
    let top = fresh("race");
    let mut refused = Vec::new();
    for round in 0..30 {
        let dir = top.join(round.to_string());
        fs::create_dir_all(&dir).unwrap();
        write(&dir, "m.yaml", DRAWN);
        let children: Vec<_> = (0..8)
            .map(|_| {
                Command::new(env!("CARGO_BIN_EXE_taktsim"))
                    .current_dir(&dir)
                    .args(["run", "m.yaml", "--seed", "42", "--format", "json"])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let mut documents = Vec::new();
        for child in children {
            let output = child.wait_with_output().unwrap();
            if output.status.code() == Some(0) {
                documents.push(String::from_utf8(output.stdout).unwrap());
            } else {
                refused.push(format!(
                    "round {round}: {:?}: {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        assert!(documents.windows(2).all(|w| w[0] == w[1]), "round {round}");
        assert_eq!(sorted_dir(&dir.join(".taktsim").join("runs")).len(), 1);
        assert_eq!(sorted_dir(&dir.join(".taktsim").join("tmp")).len(), 0);
    }
    assert!(
        refused.is_empty(),
        "{} refused: {refused:#?}",
        refused.len()
    );
}

// ---------------------------------------------------------------------------
// Exit codes: ADR-0024 §9's table
// ---------------------------------------------------------------------------

/// Exit 3: the partial result on stdout, with no `run_id`, `trace` or `experiment_id` keys at all,
/// and nothing kept. It matches the published schema, and its text form says it is partial.
#[test]
fn a_limit_is_exit_3_with_a_partial_document() {
    let dir = fresh("exit_3");
    write(&dir, "m.yaml", DRAWN);
    let out = taktsim(
        &dir,
        &[
            "run",
            "m.yaml",
            "--seed",
            "1",
            "--max-events",
            "5",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.code, 3, "{}", out.stderr);
    let doc = json(&out.stdout);
    assert_eq!(doc["complete"], false);
    assert_eq!(doc["stop_reason"], "event_limit");
    let top = doc.as_object().unwrap();
    assert!(!top.contains_key("experiment_id"));
    let entry = doc["runs"][0].as_object().unwrap();
    assert!(!entry.contains_key("run_id"));
    assert!(!entry.contains_key("trace"));
    assert_eq!(entry["seed"], Json::from(1));
    assert_eq!(entry["events"], Json::from(5));
    let runs = dir.join(".taktsim").join("runs");
    assert!(!runs.exists() || sorted_dir(&runs).is_empty());
    assert_matches_schema(&schema_of(&dir, "result"), &doc, "partial");

    let text = taktsim(&dir, &["run", "m.yaml", "--seed", "1", "--max-events", "5"]);
    assert_eq!(text.code, 3);
    assert!(text.stdout.contains("event_limit"), "{}", text.stdout);
}

/// Exit 2, `--no-reuse` mismatch: stdout empty (§9: "other commands that fail print nothing"), one
/// typed error, without an `example`, naming the store's difference. The stored files stay.
#[test]
fn a_store_mismatch_is_exit_2() {
    let dir = fresh("exit_2");
    write(&dir, "m.yaml", DRAWN);
    let out = taktsim(&dir, &["run", "m.yaml", "--seed", "1", "--format", "json"]);
    let doc = json(&out.stdout);
    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let hash = doc["runs"][0]["trace"]["stream_hash"].as_str().unwrap();
    let file = dir
        .join(".taktsim")
        .join("runs")
        .join(id)
        .join("result.json");
    let forged = out.stdout.replace(hash, &"0".repeat(64));
    fs::write(&file, &forged).unwrap();

    let again = taktsim(
        &dir,
        &[
            "run",
            "m.yaml",
            "--seed",
            "1",
            "--no-reuse",
            "--format",
            "json",
        ],
    );
    assert_eq!(again.code, 2, "{}", again.stderr);
    assert_eq!(again.stdout, "");
    let errors = errors_of(&again.stderr);
    assert_eq!(errors.len(), 1);
    assert!(errors[0]["type"].as_str().unwrap().starts_with("run."));
    assert!(!errors[0].as_object().unwrap().contains_key("example"));
    assert_eq!(fs::read_to_string(&file).unwrap(), forged);

    let limited = taktsim(
        &dir,
        &[
            "run",
            "m.yaml",
            "--seed",
            "1",
            "--no-reuse",
            "--max-events",
            "2",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        limited.code, 3,
        "a shorter stream says nothing about the engine"
    );
}

/// Exit 7 for what cannot be read or written (§9), with no `example` and nothing on stdout.
#[test]
fn what_cannot_be_read_or_written_is_exit_7() {
    let dir = fresh("exit_7");
    write(&dir, "m.yaml", EXACT);
    write(
        &dir,
        "with_include.yaml",
        &EXACT.replace("flow:", "include:\n  - nothing.yaml\nflow:"),
    );
    write(&dir, "a_file", "x");
    fs::create_dir_all(dir.join("a_directory.yaml")).unwrap();
    fs::create_dir_all(dir.join("future")).unwrap();
    write(&dir.join("future"), "store.json", r#"{"store_format": 99}"#);

    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("no such model", vec!["run", "nothing.yaml"]),
        ("included file", vec!["run", "with_include.yaml"]),
        ("a directory as the model", vec!["run", "a_directory.yaml"]),
        (
            "store is a file",
            vec!["run", "m.yaml", "--store", "a_file"],
        ),
        (
            "unknown store format",
            vec!["run", "m.yaml", "--store", "future"],
        ),
        (
            "store below a file",
            vec!["run", "m.yaml", "--store", "a_file/below"],
        ),
    ];
    for (name, mut args) in cases {
        args.extend(["--format", "json"]);
        let out = taktsim(&dir, &args);
        assert_eq!(out.code, 7, "{name}: {}", out.stderr);
        assert_eq!(out.stdout, "", "{name}");
        let errors = errors_of(&out.stderr);
        assert_eq!(errors.len(), 1, "{name}: {}", out.stderr);
        assert!(
            !errors[0].as_object().unwrap().contains_key("example"),
            "{name}: {}",
            errors[0]
        );
        assert_matches_schema(&schema_of(&dir, "error"), &errors[0], name);
    }

    let out = taktsim(&dir, &["validate", "nothing.yaml", "--format", "json"]);
    assert_eq!(out.code, 7);
    assert_eq!(errors_of(&out.stderr)[0]["type"], "model.io");
}

/// A stored result that cannot be read is exit 7, `store.result` in `describe`'s list; it is not
/// returned, and not silently replaced.
#[test]
fn a_damaged_stored_result_is_exit_7() {
    let dir = fresh("damaged_result");
    write(&dir, "m.yaml", EXACT);
    let out = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    let doc = json(&out.stdout);
    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let file = dir
        .join(".taktsim")
        .join("runs")
        .join(id)
        .join("result.json");
    fs::write(&file, "{ not json").unwrap();
    let again = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(again.code, 7, "{}", again.stderr);
    assert_eq!(again.stdout, "");
    assert_eq!(errors_of(&again.stderr)[0]["type"], "store.result");
    assert_eq!(fs::read_to_string(&file).unwrap(), "{ not json");
}

/// A store that cannot be written: exit 7. Only where a directory's mode says so.
#[cfg(unix)]
#[test]
fn a_read_only_store_is_exit_7() {
    use std::os::unix::fs::PermissionsExt;
    let dir = fresh("read_only");
    write(&dir, "m.yaml", EXACT);
    let store = dir.join("store");
    fs::create_dir_all(&store).unwrap();
    fs::set_permissions(&store, fs::Permissions::from_mode(0o555)).unwrap();
    let out = taktsim(
        &dir,
        &["run", "m.yaml", "--store", "store", "--format", "json"],
    );
    fs::set_permissions(&store, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.code, 7, "{}", out.stderr);
    assert_eq!(out.stdout, "");
}

/// A failed run prints nothing on stdout. Run on a model that fails to load is exit 1 with the same
/// typed error as `validate`, and nothing goes into a store.
#[test]
fn run_on_an_invalid_model_is_exit_1_and_stores_nothing() {
    let dir = fresh("run_invalid");
    write(
        &dir,
        "m.yaml",
        &EXACT.replace("cycle_time: 10s", "cycle_time: 10"),
    );
    let out = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    let errors = errors_of(&out.stderr);
    assert_eq!(errors[0]["path"], "components[1].cycle_time");
    assert!(!dir.join(".taktsim").join("runs").exists());

    let validate = taktsim(&dir, &["validate", "m.yaml", "--format", "json"]);
    assert_eq!(errors_of(&validate.stderr), errors);
}

/// ADR-0024 §5 and §9: no seed for a model that draws is exit 1, `run.no_seed`, path `model.seed`,
/// example `--seed 42`. Following the example's advice then works.
#[test]
fn no_seed_is_exit_1_with_a_fix_that_works() {
    let dir = fresh("no_seed");
    write(&dir, "m.yaml", DRAWN);
    let out = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    let error = &errors_of(&out.stderr)[0];
    assert_eq!(error["type"], "run.no_seed");
    assert_eq!(error["path"], "model.seed");
    let example = error["example"].as_str().unwrap();
    assert_eq!(example, "--seed 42");
    let mut args = vec!["run", "m.yaml"];
    args.extend(example.split(' '));
    assert_eq!(taktsim(&dir, &args).code, 0);
}

/// A seed that is unused is a warning that names it, and `seeds` stays empty. With `model.seed`, it
/// names `model.seed`. Warnings do not change the exit code or stdout's one document.
#[test]
fn an_unused_seed_is_a_warning_on_stderr() {
    let dir = fresh("unused_seed");
    write(&dir, "m.yaml", EXACT);
    let out = taktsim(&dir, &["run", "m.yaml", "--seed", "3", "--format", "json"]);
    assert_eq!(out.code, 0);
    let warning = lines(&out.stderr)
        .into_iter()
        .find(|l| l["kind"] == "warning")
        .unwrap();
    assert_eq!(warning["path"], "--seed");
    assert!(warning.to_string().contains('3'));
    assert_eq!(
        json(&out.stdout)["provenance"]["seeds"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    write(
        &dir,
        "seeded.yaml",
        &EXACT.replace("model: {name: m}", "model: {name: m, seed: 5}"),
    );
    let out = taktsim(&dir, &["run", "seeded.yaml", "--format", "json"]);
    let warning = lines(&out.stderr)
        .into_iter()
        .find(|l| l["kind"] == "warning")
        .unwrap();
    assert_eq!(warning["path"], "model.seed");
}

/// `--seed` takes one value, from 0 to 2^53 - 1 (ADR-0024 §5). Spec §5: `run`'s `--horizon` makes a
/// new version of the model. The end of the run and the windows follow it.
#[test]
fn flags_reach_the_result() {
    let dir = fresh("flags");
    write(&dir, "m.yaml", DRAWN);
    let out = taktsim(
        &dir,
        &[
            "run",
            "m.yaml",
            "--seed",
            "9007199254740991",
            "--horizon",
            "10min",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let doc = json(&out.stdout);
    assert_eq!(
        doc["provenance"]["seeds"][0],
        Json::from(9_007_199_254_740_991_u64)
    );
    assert_eq!(doc["runs"][0]["end_time"], "10min");
    assert_eq!(doc["metrics"][0]["over"]["to"], "10min");
    assert_eq!(doc["metrics"][0]["over"]["from"], "0s");
}

// ---------------------------------------------------------------------------
// Bad invocations
// ---------------------------------------------------------------------------

/// ADR-0024 §9: a bad flag or value is exit 1, not clap's 2; stdout is empty; one error line, `type`
/// `args.*` for what the parser refuses, with `path` the flag or argument, and `found`, `allowed` and
/// `example`. Each case's path comes from the ADR's words: "the flag or argument (`--horizon`,
/// `<model>`)".
#[test]
fn bad_invocations_are_exit_1_with_the_flag_as_the_path() {
    let dir = fresh("bad_args");
    write(&dir, "m.yaml", DRAWN);
    let cases: &[(&[&str], &str)] = &[
        (&[], "<command>"),
        (&["frobnicate"], "<command>"),
        (&["run"], "<model>"),
        (&["validate"], "<model>"),
        (&["schema"], "<type>"),
        (&["run", "m.yaml", "--seed", "abc"], "--seed"),
        (&["run", "m.yaml", "--seed", "-1"], "--seed"),
        (&["run", "m.yaml", "--seed", "1.5"], "--seed"),
        (&["run", "m.yaml", "--seed", "1", "--seed", "2"], "--seed"),
        (
            &["run", "m.yaml", "--seed", "18446744073709551616"],
            "--seed",
        ),
        (&["run", "m.yaml", "--seed"], "--seed"),
        (&["run", "m.yaml", "--horizon", "soon"], "--horizon"),
        (&["run", "m.yaml", "--horizon", "10"], "--horizon"),
        (&["run", "m.yaml", "--horizon", "5 parts"], "--horizon"),
        (&["run", "m.yaml", "--horizon", "-5s"], "--horizon"),
        (&["run", "m.yaml", "--horizon"], "--horizon"),
        (&["run", "m.yaml", "--max-events", "many"], "--max-events"),
        (&["run", "m.yaml", "--max-events", "-5"], "--max-events"),
        (&["run", "m.yaml", "--max-events", "1.5"], "--max-events"),
        (&["run", "m.yaml", "--bogus"], "--bogus"),
    ];
    let error_schema = schema_of(&dir, "error");
    let mut wrong = Vec::new();
    for (args, path) in cases {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        let out = taktsim(&dir, &args);
        assert_eq!(out.code, 1, "{args:?}: {}", out.stderr);
        assert_eq!(out.stdout, "", "{args:?}");
        let all = lines(&out.stderr);
        assert_eq!(all.len(), 1, "{args:?}: {}", out.stderr);
        let line = &all[0];
        assert_eq!(line["kind"], "error", "{args:?}");
        if line["path"] != *path {
            wrong.push(format!("{args:?}: wanted {path}, got {line}"));
        }
        assert!(
            line["type"].as_str().unwrap().starts_with("args."),
            "{args:?}: {line}"
        );
        for key in ["message", "found", "allowed", "example"] {
            assert!(
                line[key].as_str().is_some_and(|s| !s.is_empty()),
                "{args:?}: {key} in {line}"
            );
        }
        assert_matches_schema(&error_schema, line, "bad invocation");

        // The text form of the same value, with the same words in it.
        let plain: Vec<&str> = args[..args.len() - 2].to_vec();
        let text = taktsim(&dir, &plain);
        assert_eq!(text.code, 1);
        assert_eq!(text.stdout, "");
        for key in ["found", "allowed", "example"] {
            assert!(
                text.stderr.contains(line[key].as_str().unwrap()),
                "{args:?}: {key} of {line} in\n{}",
                text.stderr
            );
        }
        assert!(
            text.stderr.contains(line["path"].as_str().unwrap()),
            "{args:?}: {}",
            text.stderr
        );
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Exit 1 for a value of the right form that the rules refuse: above 2^53 - 1 and a zero horizon
/// (ADR-0024 §5, §9). The path is the flag and there is `found`, `allowed` and an `example`.
#[test]
fn values_the_rules_refuse_are_exit_1_at_the_flag() {
    let dir = fresh("rule_values");
    write(&dir, "m.yaml", DRAWN);
    write(&dir, "exact.yaml", EXACT);
    let mut wrong = Vec::new();
    for (model, args, path) in [
        ("m.yaml", vec!["--seed", "9007199254740992"], "--seed"),
        // It takes at most 2^53 - 1 whether or not the model uses it.
        ("exact.yaml", vec!["--seed", "9007199254740992"], "--seed"),
        ("m.yaml", vec!["--horizon", "0s"], "--horizon"),
        ("m.yaml", vec!["--horizon", "0min"], "--horizon"),
        ("m.yaml", vec!["--horizon", "99999999999d"], "--horizon"),
    ] {
        let mut all = vec!["run", model, "--format", "json"];
        if path != "--seed" {
            all.extend(["--seed", "1"]);
        }
        all.extend(args);
        let out = taktsim(&dir, &all);
        assert_eq!(out.code, 1, "{all:?}: {}", out.stderr);
        assert_eq!(out.stdout, "", "{all:?}");
        let errors = errors_of(&out.stderr);
        if errors[0]["path"] != path {
            wrong.push(format!("{all:?}: wanted {path}, got {}", errors[0]));
        }
        for key in ["found", "allowed", "example"] {
            assert!(errors[0][key].as_str().is_some(), "{all:?}: {key}");
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// "Every command has `--format text|json` as a global flag" and "the format is found by scanning the
/// raw arguments, since a failed parse has no value". So a failed parse answers in JSON wherever the
/// flag stands, in either spelling, and in text without it.
#[test]
fn a_failed_parse_answers_in_the_format_asked_for() {
    let dir = fresh("format_scan");
    let wants_json: &[&[&str]] = &[
        &["--format", "json", "run", "--bogus"],
        &["run", "--format", "json", "--bogus"],
        &["run", "--bogus", "--format", "json"],
        &["--format=json", "run", "--bogus"],
        &["run", "--bogus", "--format=json"],
        &["--format", "json", "frobnicate"],
        &["--format", "json"],
    ];
    for args in wants_json {
        let out = taktsim(&dir, args);
        assert_eq!(out.code, 1, "{args:?}");
        assert_eq!(out.stdout, "");
        let line = &lines(&out.stderr)[0];
        assert_eq!(line["kind"], "error", "{args:?}");
    }

    let wants_text: &[&[&str]] = &[&["run", "--bogus"], &["run", "--bogus", "--format", "text"]];
    for args in wants_text {
        let out = taktsim(&dir, args);
        assert_eq!(out.code, 1, "{args:?}");
        assert!(
            parse(&out.stderr, Format::Json).is_err(),
            "{args:?}: {}",
            out.stderr
        );
    }

    let bad = taktsim(&dir, &["run", "m.yaml", "--format", "xml"]);
    assert_eq!(bad.code, 1);
    assert!(bad.stderr.contains("--format"), "{}", bad.stderr);
    assert!(bad.stderr.contains("json") && bad.stderr.contains("text"));
}

/// `--help` and `--version` keep clap's rendering, exit 0 (ADR-0024 §10), for people. `run --help`
/// names every flag of §10's table.
#[test]
fn help_and_version_are_exit_0() {
    let dir = fresh("help");
    let version = taktsim(&dir, &["--version"]);
    assert_eq!(version.code, 0);
    assert!(version.stdout.contains(env!("CARGO_PKG_VERSION")));
    assert_eq!(taktsim(&dir, &["--help"]).code, 0);
    let help = taktsim(&dir, &["run", "--help"]);
    assert_eq!(help.code, 0);
    for flag in [
        "--format",
        "--seed",
        "--horizon",
        "--store",
        "--max-events",
        "--no-reuse",
    ] {
        assert!(help.stdout.contains(flag), "{flag}");
    }
}

/// What no sensible run asks for still may not panic: exit 101 is for a bug in the engine
/// (ADR-0007), and an agent would read it as one.
#[test]
fn odd_arguments_do_not_panic() {
    let dir = fresh("odd");
    write(&dir, "m.yaml", EXACT);
    let cases: &[&[&str]] = &[
        &["run", ""],
        &["validate", ""],
        &["schema", ""],
        &["run", "m.yaml", "--store", ""],
        &["run", "m.yaml", "--horizon", "99999999999d"],
        &["run", "m.yaml", "--horizon", "0.0000000001s"],
        &["run", "m.yaml", "--horizon", "1e9s"],
        &["run", "m.yaml", "--max-events", "18446744073709551616"],
        &["run", "."],
        &["run", "--", "--format"],
    ];
    for args in cases {
        for format in ["json", "text"] {
            let mut all = args.to_vec();
            if !all.contains(&"--") {
                all.extend(["--format", format]);
            }
            let out = taktsim(&dir, &all);
            assert_ne!(out.code, 101, "{all:?}: {}", out.stderr);
            assert!(out.code == 1 || out.code == 7, "{all:?}: {out:?}");
            assert_eq!(out.stdout, "", "{all:?}");
        }
    }
}

/// A pipe whose reader has already gone: every write to it fails, whatever the timing.
fn broken_pipe() -> Stdio {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    Stdio::from(writer)
}

/// `describe` lists exit 7 for all four commands, with `io.stdout`: printing can fail. A consumer that
/// went away (`| head -1`) gets that, not a panic: exit 101 is reserved for a bug in the engine (§9).
#[test]
fn a_broken_stdout_is_exit_7() {
    let dir = fresh("broken_stdout");
    write(&dir, "m.yaml", EXACT);
    for args in [
        vec!["describe"],
        vec!["schema", "all"],
        vec!["validate", "m.yaml", "--format", "json"],
        vec!["validate", "m.yaml"],
        vec!["run", "m.yaml", "--format", "json"],
        vec!["run", "m.yaml"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
            .current_dir(&dir)
            .args(&args)
            .stdout(broken_pipe())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(7),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Stderr carries warnings, progress and errors. If nobody reads it, that is no reason to panic. The
/// run did what it did and its exit code says so: 0 or 7 for a run that succeeded, 1 for a refusal.
#[test]
fn a_broken_stderr_is_not_a_panic() {
    let dir = fresh("broken_stderr");
    write(&dir, "m.yaml", EXACT);
    write(
        &dir,
        "bad.yaml",
        &EXACT.replace("cycle_time: 10s", "cycle_time: 10"),
    );
    let cases: &[(&[&str], &[i32])] = &[
        (&["run", "m.yaml", "--format", "json"], &[0, 7]),
        (&["run", "m.yaml"], &[0, 7]),
        (
            &["run", "m.yaml", "--seed", "3", "--format", "json"],
            &[0, 7],
        ),
        (&["validate", "bad.yaml", "--format", "json"], &[1, 7]),
        (&["validate", "bad.yaml"], &[1, 7]),
        (&["run", "bad.yaml", "--format", "json"], &[1, 7]),
        (&["run", "--bogus"], &[1, 7]),
    ];
    for (args, codes) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
            .current_dir(&dir)
            .args(*args)
            .stdout(Stdio::piped())
            .stderr(broken_pipe())
            .output()
            .unwrap();
        let code = output.status.code().unwrap();
        assert!(codes.contains(&code), "{args:?}: exit {code}");
    }
}

// ---------------------------------------------------------------------------
// `validate`
// ---------------------------------------------------------------------------

/// A valid model: exit 0, `{format, valid: true, errors: [], model_hash, system_hash}`, and the hashes
/// are those of the run. It reads only: no store appears.
#[test]
fn validate_of_a_valid_model() {
    let dir = fresh("validate_ok");
    write(&dir, "m.yaml", EXACT);
    let out = taktsim(&dir, &["validate", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let report = json(&out.stdout);
    assert_eq!(report["format"], "taktsim-validation/1");
    assert_eq!(report["valid"], true);
    assert_eq!(report["errors"].as_array().unwrap().len(), 0);
    assert_matches_schema(&schema_of(&dir, "validation"), &report, "report");
    assert!(!dir.join(".taktsim").exists(), "validate wrote a store");

    let run = json(&taktsim(&dir, &["run", "m.yaml", "--format", "json"]).stdout);
    assert_eq!(report["model_hash"], run["provenance"]["model_hash"]);
    assert_eq!(report["system_hash"], run["provenance"]["system_hash"]);
    for key in ["model_hash", "system_hash"] {
        let hash = report[key].as_str().unwrap();
        assert_eq!(hash.len(), 64, "{key}");
        assert!(
            hash.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}

/// An invalid model: the report goes to stdout and each error also to stderr, exit 1 (§9). Two errors
/// in one model are both reported, with no hashes. Line and column are counted in the file:
/// `  - {id: cut, type: Machine, cycle_time: 10}` has its `1` in column 42, on line 6.
#[test]
fn validate_of_an_invalid_model_reports_every_error() {
    let dir = fresh("validate_bad");
    let text = EXACT
        .replace("cycle_time: 10s", "cycle_time: 10")
        .replace("capacity: 1 parts", "capacity: 0 parts");
    write(&dir, "m.yaml", &text);
    let out = taktsim(&dir, &["validate", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 1);
    let report = json(&out.stdout);
    assert_eq!(report["valid"], false);
    let top = report.as_object().unwrap();
    assert!(!top.contains_key("model_hash"));
    assert!(!top.contains_key("system_hash"));
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors, &lines(&out.stderr)[..]);
    let paths: Vec<&str> = errors.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        ["components[1].cycle_time", "components[2].capacity"]
    );

    let first = &errors[0];
    assert_eq!(first["file"], "m.yaml");
    assert_eq!(first["line"], Json::from(6));
    assert_eq!(first["column"], Json::from(42));
    assert_eq!(first["type"], "model.wrong_type");
    assert!(first["found"].as_str().unwrap().contains("10"));
    assert_matches_schema(&schema_of(&dir, "validation"), &report, "report");

    let text_out = taktsim(&dir, &["validate", "m.yaml"]);
    assert_eq!(text_out.code, 1);
    for line in errors {
        for key in ["path", "found", "allowed"] {
            assert!(
                text_out.stderr.contains(line[key].as_str().unwrap())
                    || text_out.stdout.contains(line[key].as_str().unwrap()),
                "{key} in the text form"
            );
        }
    }
}

/// The acceptance of M7: "every malformed one rejected with a path-level JSON error and the documented
/// exit code". These are written here, with the path each must name, from spec §4's rules: unknown keys
/// are errors, every reference is checked, every quantity has its unit, ids are unique. Run must refuse
/// the same way.
#[test]
#[allow(clippy::too_many_lines)] // A table of cases, each with its path.
fn malformed_models_are_rejected_at_their_path() {
    let dir = fresh("malformed");
    let error_schema = schema_of(&dir, "error");
    let cut = "{id: cut, type: Machine, cycle_time: 10s}";
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "unknown key",
            EXACT.replace(cut, "{id: cut, type: Machine, cycle_time: 10s, speed: 3}"),
            "components[1].speed",
        ),
        (
            "missing cycle time",
            EXACT.replace(cut, "{id: cut, type: Machine}"),
            "components[1].cycle_time",
        ),
        (
            "unknown type",
            EXACT.replace(cut, "{id: cut, type: Teleporter}"),
            "components[1].type",
        ),
        (
            "duplicate id",
            EXACT.replace("{id: weld, type: Machine", "{id: cut, type: Machine"),
            "components[3].id",
        ),
        (
            "unknown id in flow",
            EXACT.replace("[source, cut, buf, weld, sink]", "[source, cut, buf, nothing, sink]"),
            "flow[0][3]",
        ),
        (
            "wrong unit: a count where a time goes",
            EXACT.replace("cycle_time: 10s", "cycle_time: 10 parts"),
            "components[1].cycle_time",
        ),
        (
            "wrong unit: a time where a count goes",
            EXACT.replace("capacity: 1 parts", "capacity: 1s"),
            "components[2].capacity",
        ),
        (
            "zero capacity",
            EXACT.replace("capacity: 1 parts", "capacity: 0 parts"),
            "components[2].capacity",
        ),
        (
            "zero cycle",
            EXACT.replace("cycle_time: 10s", "cycle_time: 0s"),
            "components[1].cycle_time",
        ),
        (
            "unknown format",
            EXACT.replace("format: \"1.0\"", "format: \"2.0\""),
            "format",
        ),
        (
            "seed below zero",
            EXACT.replace("model: {name: m}", "model: {name: m, seed: -1}"),
            "model.seed",
        ),
        (
            "seed above 2^53 - 1",
            EXACT.replace("model: {name: m}", "model: {name: m, seed: 9007199254740992}"),
            "model.seed",
        ),
        (
            "seed as text",
            EXACT.replace("model: {name: m}", "model: {name: m, seed: abc}"),
            "model.seed",
        ),
        (
            "zero horizon",
            EXACT.replace("horizon: 125s", "horizon: 0s"),
            "experiment.horizon",
        ),
        (
            "no horizon",
            EXACT.replace(", horizon: 125s", ""),
            "experiment.horizon",
        ),
        (
            "steady state is not in v0.1's schema",
            EXACT.replace("type: terminating", "type: steady_state"),
            "experiment.type",
        ),
        (
            "precision without a unit",
            EXACT.replace(
                "horizon: 125s}",
                "horizon: 125s, precision: {throughput(sink): 2}}",
            ),
            "experiment.precision[\"throughput(sink)\"]",
        ),
        (
            "precision on an unknown observable",
            EXACT.replace(
                "horizon: 125s}",
                "horizon: 125s, precision: {nonsense(sink): 2%}}",
            ),
            "experiment.precision[\"nonsense(sink)\"]",
        ),
        (
            "precision on a component that cannot answer it",
            EXACT.replace(
                "horizon: 125s}",
                "horizon: 125s, precision: {wip(cut): 2%}}",
            ),
            "experiment.precision[\"wip(cut)\"]",
        ),
        ("unknown top-level key", format!("{EXACT}colour: red\n"), "colour"),
        (
            "unknown component in observe",
            format!("{EXACT}observe:\n  - wip(nothing)\n"),
            "observe[0]",
        ),
        (
            "unknown observable",
            format!("{EXACT}observe:\n  - nonsense(sink)\n"),
            "observe[0]",
        ),
        (
            "unknown distribution",
            EXACT.replace("cycle_time: 10s", "cycle_time: {dist: gamma_ish, mean: 1s}"),
            "components[1].cycle_time.dist",
        ),
        (
            "missing distribution parameter",
            EXACT.replace("cycle_time: 10s", "cycle_time: {dist: exponential}"),
            "components[1].cycle_time.mean",
        ),
        ("no components", "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\nflow: [[a, b]]\n".to_owned(), "components"),
        ("no model", "format: \"1.0\"\nexperiment: {type: terminating, horizon: 1h}\ncomponents: []\nflow: [[a, b]]\n".to_owned(), "model"),
    ];
    for (name, text, path) in cases {
        let file = format!("{}.yaml", name.replace([' ', ':', '^'], "_"));
        write(&dir, &file, &text);
        let out = taktsim(&dir, &["validate", &file, "--format", "json"]);
        assert_eq!(out.code, 1, "{name}: {}", out.stdout);
        let report = json(&out.stdout);
        assert_eq!(report["valid"], false, "{name}");
        let errors = errors_of(&out.stderr);
        assert!(
            errors.iter().any(|e| e["path"] == path),
            "{name}: wanted {path} in {errors:#?}"
        );
        for error in &errors {
            assert!(
                error["type"].as_str().unwrap().starts_with("model."),
                "{name}"
            );
            assert_eq!(error["file"], file.as_str(), "{name}");
            assert!(
                matches!(error["line"], Json::Integer(n) if n >= 1),
                "{name}"
            );
            assert_matches_schema(&error_schema, error, name);
        }

        let run = taktsim(&dir, &["run", &file, "--seed", "1", "--format", "json"]);
        assert_eq!(run.code, 1, "{name}");
        assert_eq!(run.stdout, "", "{name}");
        assert_eq!(errors_of(&run.stderr), errors, "{name}");
    }
    assert!(!dir.join(".taktsim").join("runs").exists());
}

/// Files that are not a model at all: a load error at exit 1, never a panic. Empty, syntax errors,
/// a list on top, bytes that are no UTF-8, a byte order mark from a Windows editor (which is no
/// error in itself, ADR-0010: a file must not fail for being from Windows), deep nesting.
#[test]
fn files_that_are_no_model_are_rejected() {
    let dir = fresh("not_a_model");
    let deep = format!("a: {}1{}", "[".repeat(5000), "]".repeat(5000));
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("syntax", b"format: [\n".to_vec()),
        ("tabs", b"format:\t\"1.0\"\n\tmodel: 3\n".to_vec()),
        ("a list", b"- a\n- b\n".to_vec()),
        ("a number", b"7\n".to_vec()),
        (
            "repeated key",
            b"format: \"1.0\"\nformat: \"1.0\"\n".to_vec(),
        ),
        ("not utf8", vec![0xff, 0xfe, 0x00, 0x41]),
        ("deep", deep.into_bytes()),
        ("alias", b"a: &x [1]\nb: *x\n".to_vec()),
    ];
    for (name, bytes) in cases {
        let file = format!("{}.yaml", name.replace(' ', "_"));
        fs::write(dir.join(&file), bytes).unwrap();
        let out = taktsim(&dir, &["validate", &file, "--format", "json"]);
        assert_eq!(out.code, 1, "{name}: {out:?}");
        let errors = errors_of(&out.stderr);
        assert!(!errors.is_empty(), "{name}");
        assert_eq!(json(&out.stdout)["valid"], false, "{name}");
        let run = taktsim(&dir, &["run", &file, "--format", "json"]);
        assert_eq!(run.code, 1, "{name}: {run:?}");
    }

    let mut with_mark = vec![0xef, 0xbb, 0xbf];
    with_mark.extend_from_slice(EXACT.as_bytes());
    fs::write(dir.join("mark.yaml"), with_mark).unwrap();
    let plain = dir.join("plain.yaml");
    fs::write(&plain, EXACT).unwrap();
    let mark = taktsim(&dir, &["validate", "mark.yaml", "--format", "json"]);
    let want = taktsim(&dir, &["validate", "plain.yaml", "--format", "json"]);
    assert_eq!(mark.code, 0, "{}", mark.stderr);
    assert_eq!(mark.stdout, want.stdout);
}

/// JSON models run as YAML models do (spec §4: "a YAML or JSON document"), with one model hash. The
/// format follows the file's extension.
#[test]
fn a_json_model_is_the_same_model() {
    let dir = fresh("json_model");
    write(&dir, "m.yaml", EXACT);
    write(
        &dir,
        "m.json",
        r#"{"format":"1.0","model":{"name":"m"},"experiment":{"type":"terminating","horizon":"125s"},
            "components":[{"id":"source","type":"Source"},{"id":"cut","type":"Machine","cycle_time":"10s"},
            {"id":"buf","type":"Buffer","capacity":"1 parts"},{"id":"weld","type":"Machine","cycle_time":"30s"},{"id":"sink","type":"Sink"}],
            "flow":[["source","cut","buf","weld","sink"]]}"#,
    );
    let a = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    let b = taktsim(&dir, &["run", "m.json", "--format", "json"]);
    assert_eq!(b.code, 0, "{}", b.stderr);
    assert_eq!(a.stdout, b.stdout);
    let progress = lines(&b.stderr)
        .into_iter()
        .find(|l| l["kind"] == "progress")
        .unwrap();
    assert_eq!(progress["reused"], true);
}

/// A changed included file is an error when pinned by `sha256` (`include.rs`'s doc), at exit 1: the file
/// was read, and it is wrong. The model hash covers the resolved content: the same components included or
/// inline have one hash.
#[test]
fn includes_are_pinned_and_covered_by_the_hash() {
    let dir = fresh("includes");
    let cells = "components:\n  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 10s}\n";
    write(&dir, "cells.yaml", cells);
    let head = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n";
    let tail = "  - {id: sink, type: Sink}\nflow:\n  - [source, cut, sink]\n";
    write(
        &dir,
        "inline.yaml",
        &format!(
            "{head}components:\n  - {{id: source, type: Source}}\n  - {{id: cut, type: Machine, cycle_time: 10s}}\n{tail}"
        ),
    );
    write(
        &dir,
        "included.yaml",
        &format!("{head}include:\n  - cells.yaml\ncomponents:\n{tail}"),
    );
    write(
        &dir,
        "pinned.yaml",
        &format!(
            "{head}include:\n  - {{path: cells.yaml, sha256: {}}}\ncomponents:\n{tail}",
            "0".repeat(64)
        ),
    );
    let inline = json(&taktsim(&dir, &["validate", "inline.yaml", "--format", "json"]).stdout);
    let included = taktsim(&dir, &["validate", "included.yaml", "--format", "json"]);
    assert_eq!(included.code, 0, "{}", included.stderr);
    assert_eq!(json(&included.stdout)["model_hash"], inline["model_hash"]);

    let pinned = taktsim(&dir, &["validate", "pinned.yaml", "--format", "json"]);
    assert_eq!(pinned.code, 1, "{}", pinned.stderr);
    let errors = errors_of(&pinned.stderr);
    assert!(
        errors[0]["path"]
            .as_str()
            .unwrap()
            .starts_with("include[0]")
    );
}

// ---------------------------------------------------------------------------
// Spec §4's own example
// ---------------------------------------------------------------------------

/// Spec §4's document is valid (`docs` examples must be) and runs. It states `seed: 42`, so `seeds` is
/// `[42]`; the seed is drawn on because `interarrival` is exponential. It declares `throughput(sink)`
/// twice, in `observe` and in `precision`, `state_shares(cut, weld)` and `wip(buf1)`: M7 computes none,
/// so the three texts stand in `unreported`, once each (ADR-0024 §3 and its consequence "A model that
/// declares `experiment.precision` runs once in M7, with the KPI under `unreported`"). A source every 40 s
/// (exponential) with weld at 45 s plus setups: over 28 800 s, created is near the source's rate or below
/// it, held up by the bottleneck: at most 28 800 / 40 + 4 sqrt(720) = 827.
#[test]
fn the_spec_example_validates_and_runs() {
    let dir = fresh("spec_example");
    write(&dir, "line-a.yaml", SPEC_EXAMPLE);
    let valid = taktsim(&dir, &["validate", "line-a.yaml", "--format", "json"]);
    assert_eq!(valid.code, 0, "{}", valid.stdout);

    let out = taktsim(&dir, &["run", "line-a.yaml", "--format", "json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let doc = json(&out.stdout);
    assert_eq!(doc["complete"], true);
    assert_eq!(doc["stop_reason"], "horizon");
    assert_eq!(doc["provenance"]["seeds"][0], Json::from(42));
    assert_eq!(doc["runs"][0]["end_time"], "8h");
    assert_eq!(doc["runs"][0]["seed"], Json::from(42));
    let mut told: Vec<&str> = doc["unreported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["observable"].as_str().unwrap())
        .collect();
    told.sort_unstable();
    assert_eq!(
        told,
        ["state_shares(cut, weld)", "throughput(sink)", "wip(buf1)"]
    );

    let created = value(&doc, "created(source)");
    let delivered = value(&doc, "delivered(sink)");
    assert!(created <= 827, "created {created}");
    assert!(
        delivered <= 640,
        "weld at 45 s delivers at most 28 800 / 45"
    );
    assert_eq!(created - delivered, value(&doc, "items_alive"));
    assert_matches_schema(&schema_of(&dir, "result"), &doc, "the spec's result");
}

/// The reference models of ADR-0024 §11 go through the binary: each states its seed and horizon, validates,
/// runs complete and gives the same bytes in a second store.
#[test]
fn reference_models_run_through_the_binary() {
    let dir = fresh("reference");
    let models = repo(&["benchmarks", "models"]);
    let mut seen = 0;
    for name in sorted_dir(&models) {
        let Some(stem) = name.strip_suffix(".yaml") else {
            continue;
        };
        seen += 1;
        assert!(
            stem.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name}"
        );
        let path = models.join(&name);
        let path = path.to_str().unwrap();
        assert_eq!(taktsim(&dir, &["validate", path]).code, 0, "{name}");
        let a = taktsim(&dir, &["run", path, "--store", "a", "--format", "json"]);
        assert_eq!(a.code, 0, "{name}: {}", a.stderr);
        let doc = json(&a.stdout);
        assert_eq!(doc["complete"], true, "{name}");
        assert_eq!(
            doc["provenance"]["seeds"].as_array().unwrap().len(),
            1,
            "{name}"
        );
        let b = taktsim(&dir, &["run", path, "--store", "b", "--format", "json"]);
        assert_eq!(a.stdout, b.stdout, "{name}");
        let reread: ResultDocument = taktsim_model::decode_str(&a.stdout, Format::Json).unwrap();
        assert_eq!(reread.to_text(), a.stdout, "{name}");
    }
    assert!(seen >= 2, "chain and chain_failures");
}

// ---------------------------------------------------------------------------
// `describe` and `schema`
// ---------------------------------------------------------------------------

fn describe(dir: &Path) -> Json {
    let out = taktsim(dir, &["describe"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    json(&out.stdout)
}

fn command<'a>(doc: &'a Json, name: &str) -> &'a Json {
    doc["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no command {name}"))
}

/// `describe` is committed and printed byte for byte, with JSON either way (§10), and the format word and
/// package version of §10. The exit code table holds the eight codes of §9 with 101.
#[test]
fn describe_is_the_committed_file() {
    let dir = fresh("describe");
    let out = taktsim(&dir, &["describe"]);
    assert_eq!(out.code, 0);
    let committed = fs::read_to_string(repo(&["schemas", "cli", "describe.json"])).unwrap();
    assert_eq!(out.stdout, committed);
    let as_text = taktsim(&dir, &["describe", "--format", "text"]);
    assert_eq!(as_text.stdout, committed, "JSON either way");
    let as_json = taktsim(&dir, &["describe", "--format", "json"]);
    assert_eq!(as_json.stdout, committed);

    let doc = json(&committed);
    assert_eq!(doc["format"], "taktsim-describe/1");
    assert_eq!(doc["package_version"], env!("CARGO_PKG_VERSION"));
    let codes: Vec<i128> = doc["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let Json::Integer(n) = c["code"] else {
                panic!("a code is an integer")
            };
            n
        })
        .collect();
    assert_eq!(codes, [0, 1, 2, 3, 4, 5, 6, 7, 101]);
    assert_matches_schema(&doc["schema"], &doc, "describe");
}

/// §10's table: the four commands and their flags. §9: per command only the codes it can give: `run`
/// has 0, 1, 2, 3, 7; `validate` 0, 1, 7. Codes 4, 5, 6 are on no M7 command. Spec §5: what each writes,
/// whether it needs a budget or a person, and one example, which must itself run.
#[test]
fn describe_states_the_table_of_commands() {
    let dir = fresh("describe_table");
    write(&dir, "line.yaml", DRAWN);
    let doc = describe(&dir);
    let want: &[(&str, &[&str], &[i128], bool)] = &[
        ("describe", &["--format"], &[0, 1, 7], false),
        ("schema", &["<type>", "--format"], &[0, 1, 7], false),
        ("validate", &["<model>", "--format"], &[0, 1, 7], false),
        (
            "run",
            &[
                "<model>",
                "--format",
                "--seed",
                "--horizon",
                "--store",
                "--max-events",
                "--no-reuse",
            ],
            &[0, 1, 2, 3, 7],
            true,
        ),
    ];
    let listed: Vec<&str> = doc["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed.len(), want.len(), "{listed:?}");
    for (name, flags, codes, writes) in want {
        let entry = command(&doc, name);
        let mut have: Vec<&str> = entry["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        have.sort_unstable();
        let mut flags = flags.to_vec();
        flags.sort_unstable();
        assert_eq!(have, flags, "{name}");
        let have_codes: Vec<i128> = entry["exit_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let Json::Integer(n) = c["code"] else {
                    panic!("a code is an integer")
                };
                n
            })
            .collect();
        assert_eq!(have_codes, *codes, "{name}");
        assert_eq!(entry["long_running"], false, "{name}");
        assert_eq!(entry["needs_person"], false, "{name}");
        assert_eq!(entry["writes"] != "nothing", *writes, "{name}");

        let example = entry["example"].as_str().unwrap();
        let words: Vec<&str> = example.split(' ').collect();
        assert_eq!(words[0], "taktsim", "{name}");
        let out = taktsim(&dir, &words[1..]);
        assert_eq!(out.code, 0, "{example}: {}", out.stderr);
    }
    let format_flag = command(&doc, "run")["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "--format")
        .unwrap()
        .clone();
    assert_eq!(format_flag["default"], "text");
    let mut allowed: Vec<&str> = format_flag["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    allowed.sort_unstable();
    assert_eq!(allowed, ["json", "text"]);
}

/// A flag listed in `describe` is one the binary takes, with a value when `takes_value` says so: an
/// agent reads this and nothing else.
#[test]
fn describe_tells_how_to_use_each_flag() {
    let dir = fresh("describe_flags");
    write(&dir, "line.yaml", DRAWN);
    let doc = describe(&dir);
    for argument in command(&doc, "run")["arguments"].as_array().unwrap() {
        let name = argument["name"].as_str().unwrap();
        if argument["positional"] == true {
            continue;
        }
        let takes = argument["takes_value"] == true;
        let good = match name {
            "--format" => "json",
            "--seed" => "42",
            "--horizon" => "10min",
            "--store" => "here",
            "--max-events" => "1000000000",
            "--no-reuse" => "",
            other => panic!("{other} is not in ADR-0024 §10"),
        };
        assert_eq!(takes, !good.is_empty(), "{name}");
        let mut args = vec!["run", "line.yaml", name];
        if takes {
            args.push(good);
        }
        if name != "--seed" {
            args.extend(["--seed", "42"]);
        }
        let out = taktsim(&dir, &args);
        assert_eq!(out.code, 0, "{args:?}: {}", out.stderr);
    }
}

/// What each error type says it is: every error the commands give in these cases is listed in `describe`
/// for that command with that code, so an agent can read what comes.
#[test]
fn errors_met_are_listed_in_describe() {
    let dir = fresh("describe_errors");
    write(&dir, "m.yaml", DRAWN);
    write(&dir, "exact.yaml", EXACT);
    write(&dir, "bad.yaml", &DRAWN.replace("mean: 40s", "mean: 40"));
    let doc = describe(&dir);
    let cases: &[(&str, &[&str])] = &[
        ("run", &["run", "bad.yaml", "--seed", "1"]),
        ("run", &["run", "m.yaml"]),
        ("run", &["run", "m.yaml", "--seed", "9007199254740992"]),
        ("run", &["run", "m.yaml", "--seed", "1", "--horizon", "0s"]),
        ("run", &["run", "nothing.yaml"]),
        ("run", &["run", "--bogus"]),
        ("validate", &["validate", "bad.yaml"]),
        ("validate", &["validate", "nothing.yaml"]),
        ("validate", &["validate", "--bogus"]),
        ("schema", &["schema", "Teleporter"]),
        ("describe", &["describe", "--bogus"]),
    ];
    for (name, args) in cases {
        let mut all = args.to_vec();
        all.extend(["--format", "json"]);
        let out = taktsim(&dir, &all);
        let errors = errors_of(&out.stderr);
        assert!(!errors.is_empty(), "{all:?}");
        let exits = command(&doc, name)["exit_codes"].as_array().unwrap();
        let entry = exits
            .iter()
            .find(|e| e["code"] == Json::from(out.code))
            .unwrap_or_else(|| panic!("{all:?}: exit {} is not listed", out.code));
        for error in errors {
            let kind = error["type"].as_str().unwrap();
            let listed = entry["error_types"].as_array().unwrap().iter().any(|t| {
                let t = t.as_str().unwrap();
                t.strip_suffix('*')
                    .map_or(t == kind, |prefix| kind.starts_with(prefix))
            });
            assert!(listed, "{all:?}: {kind} at exit {} is not listed", out.code);
        }
    }
}

/// §10: `all` and `model` print the bytes of `schemas/model/1.0.json`. Its result schema, error schema and
/// `describe`'s own are those in `describe` (§10: "Result schemas sit inside it"). JSON either way.
#[test]
fn schema_prints_the_committed_schemas() {
    let dir = fresh("schema");
    let committed = fs::read_to_string(repo(&["schemas", "model", "1.0.json"])).unwrap();
    for name in ["all", "model"] {
        let out = taktsim(&dir, &["schema", name]);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, committed, "{name}");
        let text = taktsim(&dir, &["schema", name, "--format", "text"]);
        assert_eq!(text.stdout, committed, "{name}");
    }

    let doc = describe(&dir);
    assert_eq!(schema_of(&dir, "error"), doc["error_schema"]);
    assert_eq!(schema_of(&dir, "describe"), doc["schema"]);
    assert_eq!(
        schema_of(&dir, "result"),
        command(&doc, "run")["result_schema"]
    );
    assert_eq!(
        schema_of(&dir, "validation"),
        command(&doc, "validate")["result_schema"]
    );
    // Result documents are open, §2: new fields must not fail a v0.1 validator.
    let result = schema_of(&dir, "result");
    assert_ne!(result["additionalProperties"], false);
    for (name, def) in result["$defs"].as_object().unwrap() {
        assert_ne!(def["additionalProperties"], false, "{name} is closed");
    }
}

/// §10: a component type prints its entry. The entry says what a model may write: for each type its
/// parameters, with `id` and `type` too (spec §5: "lets an agent discover parameters without reading docs").
/// Every `$ref` in it resolves within what is printed, so it stands alone. A valid entry passes it, an
/// unknown key does not.
#[test]
fn schema_of_a_type_stands_alone() {
    let dir = fresh("schema_type");
    let want: &[(&str, &[&str], &str, &str)] = &[
        (
            "Source",
            &[
                "id",
                "type",
                "interarrival",
                "batch",
                "variants",
                "priority",
            ],
            r#"{"id":"s","type":"Source","interarrival":"40s"}"#,
            r#"{"id":"s","type":"Source","speed":"40s"}"#,
        ),
        (
            "Machine",
            &[
                "id",
                "type",
                "cycle_time",
                "mtbf",
                "mttr",
                "setup",
                "capacity",
            ],
            r#"{"id":"m","type":"Machine","cycle_time":{"dist":"exponential","mean":"1s"}}"#,
            r#"{"id":"m","type":"Machine"}"#,
        ),
        (
            "Buffer",
            &["id", "type", "capacity", "discipline"],
            r#"{"id":"b","type":"Buffer","capacity":"5 parts"}"#,
            r#"{"id":"b","type":"Buffer"}"#,
        ),
        (
            "Sink",
            &["id", "type"],
            r#"{"id":"k","type":"Sink"}"#,
            r#"{"id":"k","type":"Sink","capacity":"1 parts"}"#,
        ),
    ];
    for (name, keys, good, bad) in want {
        let schema = schema_of(&dir, name);
        let properties = schema["properties"].as_object().unwrap();
        let mut have: Vec<&str> = properties.keys().map(String::as_str).collect();
        have.sort_unstable();
        let mut keys = keys.to_vec();
        keys.sort_unstable();
        assert_eq!(have, keys, "{name}");
        assert_eq!(schema["properties"]["type"]["const"], *name, "{name}");
        assert_matches_schema(&schema, &json(good), name);
        let mut errors = Vec::new();
        check(&schema, &schema, &json(bad), "$", &mut errors);
        assert!(!errors.is_empty(), "{name}: {bad} passed");
    }
}

/// §10: an unknown name is exit 1 with the list in `allowed`, which is enough to try again: each word it
/// offers works.
#[test]
fn schema_of_an_unknown_name_offers_what_works() {
    let dir = fresh("schema_unknown");
    let out = taktsim(&dir, &["schema", "Teleporter", "--format", "json"]);
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    let error = &errors_of(&out.stderr)[0];
    assert_eq!(error["found"], "Teleporter");
    let allowed = error["allowed"].as_str().unwrap();
    for word in [
        "all",
        "model",
        "result",
        "validation",
        "error",
        "describe",
        "Buffer",
        "Machine",
        "Sink",
        "Source",
    ] {
        assert!(allowed.contains(word), "{word} in {allowed}");
        let named = taktsim(&dir, &["schema", word]);
        assert_eq!(named.code, 0, "{word}");
        json(&named.stdout);
    }
}

/// Everything a command prints in JSON mode matches what `schema` publishes: each result document, the
/// report and every line of stderr, in a full round of success and failure.
#[test]
fn what_comes_out_matches_the_published_schemas() {
    let dir = fresh("schemas_match");
    write(&dir, "m.yaml", DRAWN);
    write(&dir, "exact.yaml", EXACT);
    write(
        &dir,
        "bad.yaml",
        &EXACT.replace("cycle_time: 10s", "cycle_time: 10"),
    );
    write(&dir, "spec.yaml", SPEC_EXAMPLE);
    let result = schema_of(&dir, "result");
    let report = schema_of(&dir, "validation");
    let error = schema_of(&dir, "error");

    let cases: &[(&[&str], &Json)] = &[
        (&["run", "m.yaml", "--seed", "1"], &result),
        (&["run", "m.yaml", "--seed", "1"], &result),
        (&["run", "exact.yaml", "--seed", "1"], &result),
        (&["run", "spec.yaml"], &result),
        (
            &["run", "m.yaml", "--seed", "2", "--max-events", "3"],
            &result,
        ),
        (&["validate", "exact.yaml"], &report),
        (&["validate", "bad.yaml"], &report),
    ];
    for (args, schema) in cases {
        let mut all = args.to_vec();
        all.extend(["--format", "json"]);
        let out = taktsim(&dir, &all);
        assert_matches_schema(schema, &json(&out.stdout), &format!("{all:?}"));
        for line in lines(&out.stderr) {
            assert_matches_schema(&error, &line, &format!("{all:?}"));
        }
    }
}

/// Writes each key of `value` that `schema` does not declare to `missing`.
fn walk(schema: &Json, root: &Json, value: &Json, at: &str, missing: &mut String) {
    if let Some(name) = schema["$ref"].as_str() {
        let name = name.strip_prefix("#/$defs/").unwrap();
        walk(&root["$defs"][name], root, value, at, missing);
        return;
    }
    if let Some(object) = value.as_object() {
        let known = schema["properties"].as_object();
        for (key, field) in object {
            match known.and_then(|k| k.get(key)) {
                Some(sub) => walk(sub, root, field, &format!("{at}.{key}"), missing),
                None => {
                    if schema["oneOf"].as_array().is_none() && schema["anyOf"].as_array().is_none()
                    {
                        let _ = writeln!(missing, "{at}.{key}");
                    }
                }
            }
        }
    }
    if let (Some(list), Some(_)) = (value.as_array(), schema["items"].as_object()) {
        for (i, item) in list.iter().enumerate() {
            walk(&schema["items"], root, item, &format!("{at}[{i}]"), missing);
        }
    }
}

/// The result of a run read through the library's reader: an agent's Rust or Python client gets the same
/// value. Every key this test finds at a level of the run's output was either required or declared in the
/// schema, so nothing is printed that the schema does not state.
#[test]
fn every_printed_key_is_declared() {
    let dir = fresh("declared");
    write(&dir, "m.yaml", DRAWN);
    let out = taktsim(&dir, &["run", "m.yaml", "--seed", "1", "--format", "json"]);
    let doc = json(&out.stdout);
    let schema = schema_of(&dir, "result");

    let mut missing = String::new();
    walk(&schema, &schema, &doc, "$", &mut missing);
    assert!(missing.is_empty(), "printed, not declared:\n{missing}");
}

// ---------------------------------------------------------------------------
// Further rules
// ---------------------------------------------------------------------------

/// Only `--seed` is named in §5 as refusing repetition, but a value given twice has no meaning in any
/// flag: which one ran? Exit 1 at the flag, never a silent choice.
#[test]
fn a_flag_given_twice_is_refused() {
    let dir = fresh("twice");
    write(&dir, "m.yaml", EXACT);
    let cases: &[(&[&str], &str)] = &[
        (&["--horizon", "1h", "--horizon", "2h"], "--horizon"),
        (&["--store", "a", "--store", "b"], "--store"),
        (&["--max-events", "1", "--max-events", "2"], "--max-events"),
        (&["--format", "json", "--format", "text"], "--format"),
        (&["--no-reuse", "--no-reuse"], "--no-reuse"),
    ];
    for (flags, path) in cases {
        let mut all = vec!["run", "m.yaml"];
        all.extend(*flags);
        let out = taktsim(&dir, &all);
        assert_eq!(out.code, 1, "{all:?}: {}", out.stderr);
        assert_eq!(out.stdout, "", "{all:?}");
        assert!(out.stderr.contains(path), "{all:?}: {}", out.stderr);
    }
    assert!(!dir.join(".taktsim").exists());
}

/// §8: `--no-reuse` "runs anyway" and prints the same bytes for an unchanged engine, in exit 0.
#[test]
fn no_reuse_prints_the_same_bytes() {
    let dir = fresh("no_reuse_bytes");
    write(&dir, "m.yaml", DRAWN);
    let first = taktsim(&dir, &["run", "m.yaml", "--seed", "1", "--format", "json"]);
    let again = taktsim(
        &dir,
        &[
            "run",
            "m.yaml",
            "--seed",
            "1",
            "--no-reuse",
            "--format",
            "json",
        ],
    );
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert_eq!(again.stdout, first.stdout);
    let progress = lines(&again.stderr)
        .into_iter()
        .find(|l| l["kind"] == "progress")
        .unwrap();
    assert_eq!(progress["reused"], false);
}

/// §8: reuse with `--max-events`: "nothing runs, so no limit is reached, and it is exit 0".
#[test]
fn a_limit_on_a_stored_run_is_exit_0() {
    let dir = fresh("limit_on_stored");
    write(&dir, "m.yaml", DRAWN);
    let first = taktsim(&dir, &["run", "m.yaml", "--seed", "1", "--format", "json"]);
    let limited = taktsim(
        &dir,
        &[
            "run",
            "m.yaml",
            "--seed",
            "1",
            "--max-events",
            "1",
            "--format",
            "json",
        ],
    );
    assert_eq!(limited.code, 0, "{}", limited.stderr);
    assert_eq!(limited.stdout, first.stdout);
}

/// Include paths "stay in [the model's] directory or below" (`include.rs`'s doc). A path climbing out,
/// an absolute one and one with `\` are refused at `include[0]`, even when the file is there, and at
/// exit 1 since the fix is in the model. Nothing of the outside file is read as a model.
#[test]
fn include_paths_cannot_leave_the_model_directory() {
    let top = fresh("include_escape");
    let inner = top.join("inner");
    let sub = inner.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let cells = "components:\n  - {id: source, type: Source, interarrival: 1min}\n";
    write(&top, "outside.yaml", cells);
    write(&sub, "cells.yaml", cells);
    let absolute = top.join("outside.yaml");
    let cases = [
        "../outside.yaml".to_owned(),
        "sub/../../outside.yaml".to_owned(),
        absolute.to_str().unwrap().to_owned(),
        "sub\\cells.yaml".to_owned(),
    ];
    for path in &cases {
        let text = format!(
            "format: \"1.0\"\nmodel: {{name: m}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
             include:\n  - '{path}'\ncomponents:\n  - {{id: sink, type: Sink}}\nflow:\n  - [source, sink]\n"
        );
        write(&inner, "m.yaml", &text);
        let out = taktsim(&inner, &["validate", "m.yaml", "--format", "json"]);
        assert_eq!(out.code, 1, "{path}: {}", out.stderr);
        let errors = errors_of(&out.stderr);
        assert!(
            errors[0]["path"]
                .as_str()
                .unwrap()
                .starts_with("include[0]"),
            "{path}: {}",
            errors[0]
        );
    }
    let below = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n\
                 include:\n  - sub/cells.yaml\ncomponents:\n  - {id: sink, type: Sink}\nflow:\n  - [source, sink]\n";
    write(&inner, "m.yaml", below);
    let out = taktsim(&inner, &["validate", "m.yaml"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
}

/// An error's text comes from the model, and it stays one JSON object on one line (§9: "one JSON object
/// per line"). A key with a quote, a newline and non-ASCII letters, a value likewise.
#[test]
fn an_error_line_stays_one_line_whatever_the_model_says() {
    let dir = fresh("error_escape");
    let text = r#"{"format":"1.0","model":{"name":"m"},"experiment":{"type":"terminating","horizon":"1h"},
        "we\"i\nrd é☃":1,
        "components":[{"id":"source","type":"Source"},{"id":"cut","type":"Machine","cycle_time":"a\"b\nc"},{"id":"sink","type":"Sink"}],
        "flow":[["source","cut","sink"]]}"#;
    write(&dir, "m.json", text);
    let out = taktsim(&dir, &["validate", "m.json", "--format", "json"]);
    assert_eq!(out.code, 1);
    let all = lines(&out.stderr);
    assert!(all.len() >= 2, "{}", out.stderr);
    let paths: Vec<String> = all
        .iter()
        .map(|l| l["path"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        paths.iter().any(|p| p == "components[1].cycle_time"),
        "{paths:?}"
    );
    assert!(paths.iter().any(|p| p.contains("rd é")), "{paths:?}");
    let text_form = taktsim(&dir, &["validate", "m.json"]);
    assert_eq!(text_form.code, 1);
}

/// A model path with spaces, quotes and non-ASCII letters is only a path: it runs, and an error names it.
/// Windows allows no `"` in a file name (ADR-0010: tier 1), so there the quotes are single.
#[test]
fn file_names_are_only_names() {
    let dir = fresh("names");
    let good = if cfg!(windows) {
        "my 'line' é.yaml"
    } else {
        "my \"line\" é.yaml"
    };
    write(&dir, good, EXACT);
    let out = taktsim(&dir, &["run", good, "--store", "s t", "--format", "json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let bad = "my 'bad' é.yaml";
    write(
        &dir,
        bad,
        &EXACT.replace("cycle_time: 10s", "cycle_time: 10"),
    );
    let out = taktsim(&dir, &["validate", bad, "--format", "json"]);
    assert_eq!(out.code, 1);
    assert_eq!(errors_of(&out.stderr)[0]["file"], bad);
}

/// A path need not be UTF-8 on Linux or macOS. A model there is still a model, or a typed refusal at
/// exit 1 or 7: not a panic.
#[cfg(target_os = "linux")]
#[test]
fn a_path_that_is_not_utf8_is_not_a_panic() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let dir = fresh("not_utf8_path");
    let name = OsStr::from_bytes(b"m\xff.yaml");
    fs::write(dir.join(name), EXACT).unwrap();
    for command in ["validate", "run"] {
        let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
            .current_dir(&dir)
            .arg(command)
            .arg(name)
            .args(["--format", "json"])
            .output()
            .unwrap();
        let code = output.status.code().unwrap();
        assert!(
            code == 0 || code == 1 || code == 7,
            "{command}: {code}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for line in String::from_utf8_lossy(&output.stderr).lines() {
            json(line);
        }
    }
    let store = OsStr::from_bytes(b"s\xff");
    fs::write(dir.join("m.yaml"), EXACT).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .current_dir(&dir)
        .args(["run", "m.yaml", "--store"])
        .arg(store)
        .args(["--format", "json"])
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    assert!(code == 0 || code == 1 || code == 7, "{code}");
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        json(line);
    }
}

/// "Its format is chosen by its extension" and a model is "a YAML or JSON document". `.yml` is YAML's
/// other extension, and on Windows, a tier-1 target, `M.YAML` and `m.yaml` are one file: the extension's
/// case cannot decide whether a file loads. All give one hash.
#[test]
fn the_extension_follows_the_common_spellings() {
    let dir = fresh("extensions");
    write(&dir, "m.yaml", EXACT);
    let want = json(&taktsim(&dir, &["validate", "m.yaml", "--format", "json"]).stdout);
    let mut wrong = Vec::new();
    for name in ["a.yml", "b.YAML", "c.YML", "d.Yaml"] {
        write(&dir, name, EXACT);
        let out = taktsim(&dir, &["validate", name, "--format", "json"]);
        if out.code != 0 || json(&out.stdout)["model_hash"] != want["model_hash"] {
            wrong.push(format!("{name}: exit {}: {}", out.code, out.stderr));
        }
    }
    let as_json = r#"{"format":"1.0","model":{"name":"m"},"experiment":{"type":"terminating","horizon":"125s"},
            "components":[{"id":"source","type":"Source"},{"id":"cut","type":"Machine","cycle_time":"10s"},
            {"id":"buf","type":"Buffer","capacity":"1 parts"},{"id":"weld","type":"Machine","cycle_time":"30s"},{"id":"sink","type":"Sink"}],
            "flow":[["source","cut","buf","weld","sink"]]}"#;
    write(&dir, "e.JSON", as_json);
    let out = taktsim(&dir, &["validate", "e.JSON", "--format", "json"]);
    if out.code != 0 || json(&out.stdout)["model_hash"] != want["model_hash"] {
        wrong.push(format!("e.JSON: exit {}: {}", out.code, out.stderr));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A file with no known extension is a typed answer, never a panic; if it is refused, it says what is
/// allowed.
#[test]
fn an_unknown_extension_gets_a_typed_answer() {
    let dir = fresh("unknown_extension");
    write(&dir, "m.txt", EXACT);
    write(&dir, "noextension", EXACT);
    for name in ["m.txt", "noextension"] {
        let out = taktsim(&dir, &["validate", name, "--format", "json"]);
        assert!(out.code == 0 || out.code == 1, "{name}: {out:?}");
        if out.code == 1 {
            let error = &errors_of(&out.stderr)[0];
            assert!(error["allowed"].as_str().is_some(), "{name}: {error}");
        }
    }
}

/// A `.taktsim` that is a file in the current directory, and a store found upward of an unknown format:
/// exit 7 on both, without a panic (§8, §9).
#[test]
fn a_bad_store_found_by_discovery_is_exit_7() {
    let top = fresh("discover_bad");
    let file_case = top.join("file");
    fs::create_dir_all(&file_case).unwrap();
    write(&file_case, ".taktsim", "x");
    write(&file_case, "m.yaml", EXACT);
    let out = taktsim(&file_case, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 7, "{}", out.stderr);
    assert_eq!(out.stdout, "");

    let future = top.join("future");
    let below = future.join("below");
    fs::create_dir_all(future.join(".taktsim")).unwrap();
    fs::create_dir_all(&below).unwrap();
    write(
        &future.join(".taktsim"),
        "store.json",
        r#"{"store_format": 99}"#,
    );
    write(&below, "m.yaml", EXACT);
    let out = taktsim(&below, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 7, "{}", out.stderr);
    assert_eq!(errors_of(&out.stderr)[0]["type"], "store.format");
    // `validate` touches no store, so it does not care.
    assert_eq!(taktsim(&below, &["validate", "m.yaml"]).code, 0);
}

/// A run directory whose `result.json` is gone was not written by any run ("no half-written run is ever
/// visible"), but a person can remove a file. Then the run is not in the store (§8: "exists and reads"):
/// it runs, or says it cannot. It must not panic, and must not claim a reuse.
#[test]
fn a_stored_run_without_a_result_is_not_reused() {
    let dir = fresh("no_result_file");
    write(&dir, "m.yaml", EXACT);
    let first = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    let doc = json(&first.stdout);
    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let run_dir = dir.join(".taktsim").join("runs").join(id);
    fs::remove_file(run_dir.join("result.json")).unwrap();
    let again = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert!(again.code == 0 || again.code == 7, "{again:?}");
    if again.code == 0 {
        assert_eq!(again.stdout, first.stdout);
        let progress = lines(&again.stderr)
            .into_iter()
            .find(|l| l["kind"] == "progress")
            .unwrap();
        assert_eq!(progress["reused"], false);
    }
}

/// Reading `describe`'s input schema reference and running the command there, as an agent does.
#[test]
fn input_schema_commands_run() {
    let dir = fresh("schema_commands");
    let doc = describe(&dir);
    let committed = fs::read_to_string(repo(&["schemas", "model", "1.0.json"])).unwrap();
    for name in ["validate", "run"] {
        let inputs = command(&doc, name)["input_schemas"].as_array().unwrap();
        assert_eq!(inputs.len(), 1, "{name}");
        assert_eq!(inputs[0]["argument"], "<model>", "{name}");
        let line = inputs[0]["schema_command"].as_str().unwrap();
        let words: Vec<&str> = line.split(' ').collect();
        assert_eq!(words[0], "taktsim");
        let out = taktsim(&dir, &words[1..]);
        assert_eq!(out.code, 0, "{line}");
        assert_eq!(out.stdout, committed, "{line}");
    }
}

/// Spec §4: "Unknown keys are errors" is for model files; ADR-0024 §2 keeps them closed while results are
/// open. The model's schema says so at its root.
#[test]
fn model_files_stay_closed() {
    let dir = fresh("closed_models");
    let schema = schema_of(&dir, "model");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["$defs"]["Experiment"]["additionalProperties"], false);
    assert_eq!(schema["$defs"]["Model"]["additionalProperties"], false);
}

/// The file's extension says how it is read: JSON in `.json` and YAML in `.yaml`. YAML syntax in a `.json`
/// is an error with a line, as are JSON's own syntax errors. JSON in a `.yaml` is valid YAML and the same
/// model.
#[test]
fn json_files_are_read_as_json() {
    let dir = fresh("strict_json");
    write(&dir, "yaml_in_json.json", EXACT);
    write(&dir, "comma.json", "{\"format\": \"1.0\",}");
    write(&dir, "comment.json", "// no\n{\"format\": \"1.0\"}");
    write(&dir, "cut.json", "{\"format\": ");
    write(&dir, "two.json", "{} {}");
    for name in [
        "yaml_in_json.json",
        "comma.json",
        "comment.json",
        "cut.json",
        "two.json",
    ] {
        let out = taktsim(&dir, &["validate", name, "--format", "json"]);
        assert_eq!(out.code, 1, "{name}: {out:?}");
        let error = &errors_of(&out.stderr)[0];
        assert_eq!(error["file"], name, "{name}");
        assert!(
            matches!(error["line"], Json::Integer(n) if n >= 1),
            "{name}: {error}"
        );
    }

    let as_json = r#"{"format":"1.0","model":{"name":"m"},"experiment":{"type":"terminating","horizon":"125s"},
            "components":[{"id":"source","type":"Source"},{"id":"cut","type":"Machine","cycle_time":"10s"},
            {"id":"buf","type":"Buffer","capacity":"1 parts"},{"id":"weld","type":"Machine","cycle_time":"30s"},{"id":"sink","type":"Sink"}],
            "flow":[["source","cut","buf","weld","sink"]]}"#;
    write(&dir, "json_in_yaml.yaml", as_json);
    write(&dir, "m.yaml", EXACT);
    let a = json(&taktsim(&dir, &["validate", "json_in_yaml.yaml", "--format", "json"]).stdout);
    let b = json(&taktsim(&dir, &["validate", "m.yaml", "--format", "json"]).stdout);
    assert_eq!(a["valid"], true);
    assert_eq!(a["model_hash"], b["model_hash"]);
}

/// `seed: 1.0` is the number one written another way: refused at its path as no integer, or the same model
/// as `seed: 1`. Two hashes for one value would make one run look like two.
#[test]
fn a_seed_written_as_a_float_is_refused_or_the_same() {
    let dir = fresh("float_seed");
    write(
        &dir,
        "int.yaml",
        &DRAWN.replace("model: {name: m}", "model: {name: m, seed: 1}"),
    );
    let want = json(&taktsim(&dir, &["validate", "int.yaml", "--format", "json"]).stdout);
    for spelled in ["1.0", "1e0"] {
        write(
            &dir,
            "float.yaml",
            &DRAWN.replace(
                "model: {name: m}",
                &format!("model: {{name: m, seed: {spelled}}}"),
            ),
        );
        let out = taktsim(&dir, &["validate", "float.yaml", "--format", "json"]);
        if out.code == 0 {
            assert_eq!(
                json(&out.stdout)["model_hash"],
                want["model_hash"],
                "{spelled}"
            );
        } else {
            assert_eq!(out.code, 1, "{spelled}");
            assert_eq!(errors_of(&out.stderr)[0]["path"], "model.seed", "{spelled}");
        }
    }
}

/// ADR-0024 §3: `--horizon` goes through `Model::with_horizon`, so "the stored model, its hash and the
/// header all describe the run". A flag repeating what the file says describes the same run, in any
/// spelling the file uses. So it is one id with or without the flag.
#[test]
fn a_horizon_flag_repeating_the_file_is_the_same_run() {
    let dir = fresh("same_horizon");
    let mut wrong = Vec::new();
    for spelled in [
        "3600s", "60min", "1h", "1.0h", "\"1 h\"", "0.5h", "90min", "1500ms", "1d",
    ] {
        let plain = spelled.trim_matches('"');
        write(
            &dir,
            "m.yaml",
            &EXACT.replace("horizon: 125s", &format!("horizon: {spelled}")),
        );
        let file = taktsim(&dir, &["validate", "m.yaml", "--format", "json"]);
        assert_eq!(file.code, 0, "{spelled}: {}", file.stderr);
        // A short run: the Source on demand with a cycle of 1 h does a few events only.
        let text = EXACT
            .replace("horizon: 125s", &format!("horizon: {spelled}"))
            .replace("cycle_time: 10s", "cycle_time: 1h")
            .replace("cycle_time: 30s", "cycle_time: 1h");
        write(&dir, "m.yaml", &text);
        let without = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
        assert_eq!(without.code, 0, "{spelled}: {}", without.stderr);
        let with = taktsim(
            &dir,
            &["run", "m.yaml", "--horizon", plain, "--format", "json"],
        );
        assert_eq!(with.code, 0, "{spelled}: {}", with.stderr);
        if with.stdout != without.stdout {
            wrong.push(spelled);
        }
    }
    assert!(
        wrong.is_empty(),
        "another run for the same horizon: {wrong:?}"
    );
}

/// Model hash "covers the resolved content, not the file names, so moving components between files does
/// not change it" (`include.rs`). So `--horizon` over a model with an include gives what it gives over the
/// same model inline, and repeating the file's horizon changes nothing there either.
#[test]
fn a_horizon_flag_over_an_include_is_the_same_model() {
    let dir = fresh("horizon_include");
    write(
        &dir,
        "cells.yaml",
        "components:\n  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 10s}\n",
    );
    let head = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 1h}\n";
    let tail = "  - {id: sink, type: Sink}\nflow:\n  - [source, cut, sink]\n";
    write(
        &dir,
        "included.yaml",
        &format!("{head}include:\n  - cells.yaml\ncomponents:\n{tail}"),
    );
    write(
        &dir,
        "inline.yaml",
        &format!(
            "{head}components:\n  - {{id: source, type: Source}}\n  - {{id: cut, type: Machine, cycle_time: 10s}}\n{tail}"
        ),
    );
    let run = |name: &str, extra: &[&str]| {
        let mut args = vec!["run", name, "--format", "json"];
        args.extend(extra);
        let out = taktsim(&dir, &args);
        assert_eq!(out.code, 0, "{name} {extra:?}: {}", out.stderr);
        out.stdout
    };
    assert_eq!(run("included.yaml", &[]), run("inline.yaml", &[]));
    assert_eq!(
        run("included.yaml", &["--horizon", "10min"]),
        run("inline.yaml", &["--horizon", "10min"])
    );
    assert_eq!(
        run("included.yaml", &["--horizon", "1h"]),
        run("included.yaml", &[])
    );
}

/// `--horizon` makes another version of the model, and changes nothing else in it: what `observe` and
/// `precision` declare stays declared, in the stored model and in `unreported`.
#[test]
fn a_horizon_flag_keeps_what_the_model_declares() {
    let dir = fresh("horizon_keeps");
    write(&dir, "m.yaml", SPEC_EXAMPLE);
    let out = taktsim(
        &dir,
        &["run", "m.yaml", "--horizon", "10min", "--format", "json"],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let doc = json(&out.stdout);
    assert_eq!(doc["unreported"].as_array().unwrap().len(), 3);
    let id = doc["runs"][0]["run_id"].as_str().unwrap();
    let stored = fs::read_to_string(
        dir.join(".taktsim")
            .join("runs")
            .join(id)
            .join("model.json"),
    )
    .unwrap();
    let model = json(&stored);
    assert_eq!(model["experiment"]["horizon"], "10min");
    assert_eq!(model["experiment"]["precision"]["throughput(sink)"], "2%");
    assert_eq!(model["observe"].as_array().unwrap().len(), 3);
    assert_eq!(model["model"]["seed"], Json::from(42));
}

/// Principle 2: models have one schema. The stored canonical model of a run, and each model these tests
/// call valid, written out as JSON, matches `schema model`, so the loader and the schema agree on them.
#[test]
fn valid_models_match_the_published_schema() {
    let dir = fresh("schema_agrees");
    let schema = schema_of(&dir, "model");
    let rich = "format: \"1.0\"\nmodel: {name: line-a, time_unit: min, seed: 1}\nexperiment: {type: terminating, horizon: 0.5h}\n\
        components:\n  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}, batch: 2 parts, variants: {a: 25%, b: 75%}, priority: 3}\n  - {id: buf, type: Buffer, capacity: 3 parts, discipline: lifo}\n  - id: cut\n    type: Machine\n    cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}\n    mtbf: {dist: exponential, mean: 1h}\n    mttr: 5min\n    capacity: 2 parts\n    setup:\n      per_variant: 4min\n      matrix:\n        - {from: a, to: b, time: 1min}\n        - {from: b, to: a, time: 0s}\n  - {id: sink, type: Sink}\n\
        flow:\n  - [source, buf, cut, sink]\nobserve:\n  - throughput(sink)\n";
    for (name, text) in [
        ("exact", EXACT),
        ("drawn", DRAWN),
        ("spec", SPEC_EXAMPLE),
        ("rich", rich),
    ] {
        let as_json = parse(text, Format::Yaml).unwrap().to_json();
        assert_matches_schema(&schema, &as_json, name);

        let file = format!("{name}.yaml");
        write(&dir, &file, text);
        let out = taktsim(&dir, &["run", &file, "--seed", "1", "--format", "json"]);
        assert_eq!(out.code, 0, "{name}: {}", out.stderr);
        let doc = json(&out.stdout);
        let id = doc["runs"][0]["run_id"].as_str().unwrap();
        let stored = fs::read_to_string(
            dir.join(".taktsim")
                .join("runs")
                .join(id)
                .join("model.json"),
        )
        .unwrap();
        assert_matches_schema(&schema, &json(&stored), &format!("stored {name}"));
    }
}

/// Spec §5: "Every error message is actionable: field path, what was found, what is allowed, one example of
/// a valid value." So taking the example at its word fixes the model: where the error gives an example,
/// put in its place, the model is valid. Each model here has one mistake; `@@` is where the example goes,
/// which for `format` is the whole line, as its example is.
#[test]
#[allow(clippy::too_many_lines)] // A table of cases.
fn the_example_of_an_error_fixes_the_model() {
    let dir = fresh("examples_fix");
    let template = |line: &str| {
        format!(
            "format: \"1.0\"\nmodel: {{name: m, seed: 1}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
             components:\n  - {{id: source, type: Source, interarrival: 1min}}\n  - {{id: buf, type: Buffer, capacity: 3 parts}}\n  - {line}\n  - {{id: sink, type: Sink}}\n\
             flow:\n  - [source, buf, cut, sink]\n"
        )
    };
    let machine = |rest: &str| format!("{{id: cut, type: Machine, cycle_time: 1min{rest}}}");
    let source_line = |bad: &str| {
        template(&machine("")).replace(
            "{id: source, type: Source, interarrival: 1min}",
            &format!("{{id: source, type: Source, {bad}}}"),
        )
    };
    let buffer_line = |bad: &str| {
        template(&machine("")).replace(
            "{id: buf, type: Buffer, capacity: 3 parts}",
            &format!("{{id: buf, type: Buffer, capacity: 3 parts, {bad}}}"),
        )
    };
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "cycle time",
            template("{id: cut, type: Machine, cycle_time: 10}"),
            "cycle_time: 10",
            "cycle_time: @@",
        ),
        (
            "capacity",
            template(&machine(", capacity: 0 parts")),
            "capacity: 0 parts",
            "capacity: @@",
        ),
        (
            "mtbf",
            template(&machine(", mtbf: 0s, mttr: 1min")),
            "mtbf: 0s",
            "mtbf: @@",
        ),
        (
            "mttr",
            template(&machine(", mtbf: 1h, mttr: 3")),
            "mttr: 3",
            "mttr: @@",
        ),
        (
            "interarrival",
            source_line("interarrival: 0s"),
            "interarrival: 0s",
            "interarrival: @@",
        ),
        (
            "batch",
            source_line("interarrival: 1min, batch: 0 parts"),
            "batch: 0 parts",
            "batch: @@",
        ),
        (
            "priority",
            source_line("interarrival: 1min, priority: 300"),
            "priority: 300",
            "priority: @@",
        ),
        (
            "discipline",
            buffer_line("discipline: random"),
            "discipline: random",
            "discipline: @@",
        ),
        (
            "horizon",
            template(&machine("")).replace("horizon: 1h", "horizon: 0s"),
            "horizon: 0s",
            "horizon: @@",
        ),
        (
            "seed",
            template(&machine("")).replace("seed: 1", "seed: -1"),
            "seed: -1",
            "seed: @@",
        ),
        (
            "format",
            template(&machine("")).replace("format: \"1.0\"", "format: \"9.0\""),
            "format: \"9.0\"",
            "@@",
        ),
    ];
    let mut wrong = String::new();
    for (name, text, bad, slot) in &cases {
        write(&dir, "m.yaml", text);
        let out = taktsim(&dir, &["validate", "m.yaml", "--format", "json"]);
        assert_eq!(out.code, 1, "{name}: {}", out.stderr);
        let errors = errors_of(&out.stderr);
        let Some(example) = errors[0]["example"].as_str() else {
            continue;
        };
        write(
            &dir,
            "m.yaml",
            &text.replace(bad, &slot.replace("@@", example)),
        );
        let fixed = taktsim(&dir, &["validate", "m.yaml", "--format", "json"]);
        if fixed.code != 0 {
            let _ = writeln!(wrong, "{name}: example `{example}` gives {}", fixed.stderr);
        }
    }
    assert!(wrong.is_empty(), "{wrong}");
}

/// A clock that would pass what a time can hold (about 584 years) during a run is an engine error at exit 2,
/// at the component and the time: 200 000 d + 200 000 d is more than 213 503 d. It is not a panic, and
/// nothing is kept (§9). It could not be refused at load time, since only the run reaches that moment.
#[test]
fn a_clock_that_overflows_is_a_failed_run() {
    let dir = fresh("clock_overflow");
    write(
        &dir,
        "m.yaml",
        &EXACT
            .replace("horizon: 125s", "horizon: 200000d")
            .replace("cycle_time: 10s", "cycle_time: 200000d"),
    );
    let out = taktsim(&dir, &["run", "m.yaml", "--format", "json"]);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert_eq!(out.stdout, "");
    let error = &errors_of(&out.stderr)[0];
    assert_eq!(error["path"], "components[1]");
    assert_eq!(error["time"], "200000d");
    assert!(
        error["recent"]
            .as_array()
            .is_some_and(|recent| !recent.is_empty())
    );
    assert!(!error.as_object().unwrap().contains_key("example"));
    let runs = dir.join(".taktsim").join("runs");
    assert!(!runs.exists() || sorted_dir(&runs).is_empty());
}

/// ADR-0024 §9: `type` is a stable word in a namespace, and an agent acts on it. `describe` and `schema`
/// write "nothing" and touch no store, and a `validate` "reads only", so a consumer that went away cannot be
/// a `store.*` error: that sends the reader to look at a store that is fine, or does not exist.
#[test]
fn a_broken_stdout_does_not_blame_the_store() {
    let dir = fresh("broken_stdout_type");
    write(&dir, "m.yaml", EXACT);
    let mut wrong = Vec::new();
    for args in [
        vec!["describe", "--format", "json"],
        vec!["schema", "all", "--format", "json"],
        vec!["validate", "m.yaml", "--format", "json"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
            .current_dir(&dir)
            .args(&args)
            .stdout(broken_pipe())
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        let error = &errors_of(&stderr)[0];
        if error["type"].as_str().unwrap().starts_with("store.") {
            wrong.push(format!("{args:?}: {error}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
