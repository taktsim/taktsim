//! The CLI's contract with an agent (ADR-0019, ADR-0024 §9, §10): `describe` is
//! fresh and true, every error line matches its schema, and each exit code means
//! what the table says.

mod support;

#[path = "../../taktsim-model/tests/support/schema_checker.rs"]
mod schema_checker;

use std::collections::BTreeSet;

use schema_checker::{ANNOTATIONS, Checker, VALIDATION, keywords};
use support::{Dir, EXACT, UNSEEDED, repo};
use taktsim_model::Json;

#[allow(clippy::unwrap_used)] // Test helper.
fn describe() -> Json {
    let outcome = Dir::new().taktsim(&["describe"]);
    assert_eq!(outcome.code, 0);
    outcome.json()
}

#[allow(clippy::unwrap_used, clippy::panic)] // Test helper.
fn command<'a>(description: &'a Json, name: &str) -> &'a Json {
    description["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no command {name}"))
}

#[allow(clippy::panic)] // Test helper.
fn code_of(entry: &Json) -> i128 {
    let Json::Integer(code) = entry["code"] else {
        panic!("a code is an integer: {entry}");
    };
    code
}

/// The committed file is what the binary prints (spec §5, ADR-0019 item 1).
#[test]
fn describe_is_fresh() {
    let committed = std::fs::read_to_string(repo(&["schemas", "cli", "describe.json"])).unwrap();
    let printed = Dir::new().taktsim(&["describe"]).stdout;
    assert!(
        committed == printed,
        "schemas/cli/describe.json is stale; regenerate it with\n  \
         cargo run -p taktsim-cli -- describe > schemas/cli/describe.json"
    );
}

#[test]
fn describe_carries_its_format_version_and_schema() {
    let description = describe();
    assert_eq!(description["format"], "taktsim-describe/1");
    assert_eq!(description["package_version"], env!("CARGO_PKG_VERSION"));
    let schema = &description["schema"];
    let checker = Checker { root: schema };
    assert!(
        checker.valid(schema, &description),
        "it matches its own schema"
    );
    let names: BTreeSet<&str> = description["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["describe", "run", "schema", "validate"])
    );
    let run = command(&description, "run");
    assert_eq!(run["writes"], "a run in the run store");
    assert_eq!(command(&description, "validate")["writes"], "nothing");
    let flags: Vec<&str> = run["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    for flag in [
        "<model>",
        "--seed",
        "--horizon",
        "--store",
        "--max-events",
        "--max-runs",
        "--max-seconds",
        "--threads",
        "--max-memory-mib",
        "--no-reuse",
        "--format",
    ] {
        assert!(flags.contains(&flag), "{flag}: {flags:?}");
    }
    let format = run["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "--format")
        .unwrap();
    assert_eq!(format["default"], "text");
    assert_eq!(
        format["allowed"],
        Json::Array(vec![Json::from("text"), Json::from("json")])
    );
    assert_eq!(format["type"], "enum");
    let no_reuse = run["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "--no-reuse")
        .unwrap();
    assert_eq!(no_reuse["type"], "boolean");
    assert_eq!(no_reuse["takes_value"], false);
    let model = &run["arguments"][0];
    assert_eq!(model["required"], true);
    assert_eq!(model["positional"], true);
}

/// The result schemas sit inside `describe`; what `run` and `validate` print
/// matches them.
#[test]
fn emitted_documents_match_the_schemas_in_describe() {
    let description = describe();
    let dir = Dir::new().with_line();
    let run_schema = &command(&description, "run")["result_schema"];
    let checker = Checker { root: run_schema };
    for args in [
        &["run", "line.yaml", "--format", "json"][..],
        &[
            "run",
            "line.yaml",
            "--format",
            "json",
            "--max-events",
            "5",
            "--no-reuse",
        ][..],
    ] {
        let outcome = dir.taktsim(args);
        assert!(checker.valid(run_schema, &outcome.json()), "{args:?}");
    }
    let validate_schema = &command(&description, "validate")["result_schema"];
    let checker = Checker {
        root: validate_schema,
    };
    dir.write("bad.yaml", "format: \"1.0\"\n");
    for name in ["line.yaml", "bad.yaml", "nothing.yaml"] {
        let outcome = dir.taktsim(&["validate", name, "--format", "json"]);
        assert!(checker.valid(validate_schema, &outcome.json()), "{name}");
    }
}

/// Every line on stderr in JSON matches the error schema, whatever it is.
#[test]
fn error_lines_match_their_schema() {
    let schema = describe()["error_schema"].clone();
    let mut used = BTreeSet::new();
    keywords(&schema, &mut used);
    for word in used {
        assert!(
            VALIDATION.contains(&word.as_str()) || ANNOTATIONS.contains(&word.as_str()),
            "{word}"
        );
    }
    let checker = Checker { root: &schema };
    let dir = Dir::new().with_line();
    dir.write("unseeded.yaml", UNSEEDED);
    dir.write("exact.yaml", EXACT);
    dir.write("bad.yaml", "{a: 1}\n");
    std::fs::create_dir_all(dir.path().join("taken")).unwrap();
    dir.write("taken/store.json", "{\"store_format\": 99}");
    let cases: [&[&str]; 19] = [
        &["--format", "json"],
        &["nope", "--format", "json"],
        &["run", "--format", "json"],
        &["run", "line.yaml", "--bad", "--format", "json"],
        &["run", "line.yaml", "--seed", "x", "--format", "json"],
        &[
            "run",
            "line.yaml",
            "--seed",
            "1",
            "--seed",
            "2",
            "--format",
            "json",
        ],
        &[
            "run",
            "line.yaml",
            "--seed",
            "9007199254740992",
            "--format",
            "json",
        ],
        &["run", "line.yaml", "--horizon", "0s", "--format", "json"],
        &["run", "line.yaml", "--seed", "1..2000", "--format", "json"],
        &["run", "line.yaml", "--seed", "1..3", "--format", "json"],
        &["run", "line.yaml", "--threads", "0", "--format", "json"],
        &[
            "run",
            "line.yaml",
            "--max-memory-mib",
            "x",
            "--format",
            "json",
        ],
        &["run", "line.yaml", "--format", "json"],
        &["run", "unseeded.yaml", "--format", "json"],
        &["run", "exact.yaml", "--seed", "1", "--format", "json"],
        &["run", "bad.yaml", "--format", "json"],
        &["run", "line.yaml", "--store", "taken", "--format", "json"],
        &["validate", "nothing.yaml", "--format", "json"],
        &["schema", "nope", "--format", "json"],
    ];
    for args in cases {
        let outcome = dir.taktsim(args);
        let lines = outcome.lines();
        assert!(!lines.is_empty(), "{args:?}");
        for line in lines {
            assert!(checker.valid(&schema, &line), "{args:?}: {line}");
        }
    }
}

/// Each code on each command's list is reached, and nothing else is.
#[test]
fn documented_exit_codes() {
    let description = describe();
    let table: BTreeSet<i128> = description["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(code_of)
        .collect();
    assert_eq!(table, BTreeSet::from([0, 1, 2, 3, 4, 5, 6, 7, 101]));

    let dir = Dir::new().with_line();
    dir.write("unseeded.yaml", UNSEEDED);
    let line_id_run = dir.taktsim(&["run", "line.yaml", "--format", "json", "--store", "st"]);
    let id = line_id_run.json()["runs"][0]["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // What an engine that changed its behaviour would find.
    let stored = dir
        .path()
        .join("st")
        .join("runs")
        .join(&id)
        .join("result.json");
    let hash = line_id_run.json()["runs"][0]["trace"]["stream_hash"]
        .as_str()
        .unwrap()
        .to_owned();
    let altered = std::fs::read_to_string(&stored)
        .unwrap()
        .replace(&hash, &"0".repeat(64));
    std::fs::write(&stored, altered).unwrap();
    dir.write("file", "x");

    let scenarios: [(&str, &[&str], i32); 12] = [
        ("describe", &["describe"], 0),
        ("describe", &["describe", "--bad"], 1),
        ("schema", &["schema", "model"], 0),
        ("schema", &["schema", "nope"], 1),
        ("validate", &["validate", "line.yaml"], 0),
        ("validate", &["validate", "unseeded.yaml", "--bad"], 1),
        ("validate", &["validate", "nothing.yaml"], 7),
        ("run", &["run", "line.yaml"], 0),
        ("run", &["run", "unseeded.yaml"], 1),
        (
            "run",
            &["run", "line.yaml", "--store", "st", "--no-reuse"],
            2,
        ),
        (
            "run",
            &["run", "line.yaml", "--max-events", "2", "--no-reuse"],
            3,
        ),
        ("run", &["run", "line.yaml", "--store", "file"], 7),
    ];
    let mut reached: BTreeSet<(&str, i128)> = BTreeSet::new();
    for (name, args, want) in scenarios {
        let outcome = dir.taktsim(args);
        assert_eq!(outcome.code, want, "{args:?}: {}", outcome.stderr);
        reached.insert((name, i128::from(want)));
    }
    let documented: BTreeSet<(&str, i128)> = ["describe", "schema", "validate", "run"]
        .into_iter()
        .flat_map(|name| {
            command(&description, name)["exit_codes"]
                .as_array()
                .unwrap()
                .iter()
                .map(move |c| (name, code_of(c)))
        })
        .collect();
    // Exit 4 needs a signal: `a_signal_exits_4_with_the_finished_runs`, under unix. On
    // Windows the library's flag is tested instead, and the gap is stated in the PR.
    let writing_stdout: BTreeSet<(&str, i128)> =
        BTreeSet::from([("describe", 7), ("schema", 7), ("run", 4)]);
    let not_reached: BTreeSet<_> = documented.difference(&reached).copied().collect();
    assert_eq!(not_reached, writing_stdout, "the rest are covered here");
    let not_documented: Vec<_> = reached.difference(&documented).collect();
    assert_eq!(not_documented.len(), 0, "{not_documented:?}");
}

/// Stdout that cannot be written is an I/O failure, not a success that said nothing.
#[cfg(target_os = "linux")]
#[test]
fn a_stdout_that_cannot_be_written_exits_7() {
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    for args in [&["describe"][..], &["schema", "model"][..]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_taktsim"))
            .args(args)
            .stdout(full.try_clone().unwrap())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{args:?}");
    }
}

/// `--threads` and `--max-memory-mib` change how fast, never a byte of stdout; zero or
/// a word is an argument error, and one run warns that they are unused.
#[test]
fn parallel_flags_change_no_byte() {
    let list = ["run", "line.yaml", "--seed", "1..4", "--format", "json"];
    let one = Dir::new().with_line();
    let serial = one.taktsim(&[&list[..], &["--threads", "1"]].concat());
    assert_eq!(serial.code, 0, "{}", serial.stderr);
    let many = Dir::new().with_line();
    let parallel =
        many.taktsim(&[&list[..], &["--threads", "4", "--max-memory-mib", "4096"]].concat());
    assert_eq!(parallel.code, 0, "{}", parallel.stderr);
    // One machine, so even `provenance.platform` is the same.
    assert_eq!(serial.stdout, parallel.stdout);
    assert!(serial.stderr.contains("run.threads"), "{}", serial.stderr);

    let dir = Dir::new().with_line();
    for args in [
        &["run", "line.yaml", "--threads", "0", "--format", "json"][..],
        &[
            "run",
            "line.yaml",
            "--max-memory-mib",
            "0",
            "--format",
            "json",
        ][..],
        &["run", "line.yaml", "--threads", "many", "--format", "json"][..],
    ] {
        let outcome = dir.taktsim(args);
        assert_eq!(outcome.code, 1, "{args:?}");
        assert_eq!(outcome.stdout, "");
        let line = &outcome.lines()[0];
        assert!(
            line["type"].as_str().unwrap().starts_with("args."),
            "{line}"
        );
        assert_eq!(line["path"], args[2], "{line}");
    }
    let unused = dir.taktsim(&["run", "line.yaml", "--threads", "2", "--format", "json"]);
    assert_eq!(unused.code, 0, "{}", unused.stderr);
    let warned = unused
        .lines()
        .into_iter()
        .any(|line| line["type"] == "run.flag_unused" && line["path"] == "--threads");
    assert!(warned, "{}", unused.stderr);
}

#[test]
fn event_limit_is_partial_and_exits_3() {
    let dir = Dir::new().with_line();
    let outcome = dir.taktsim(&["run", "line.yaml", "--max-events", "5", "--format", "json"]);
    assert_eq!(outcome.code, 3, "{}", outcome.stderr);
    let result = outcome.json();
    assert_eq!(result["complete"], false);
    assert_eq!(result["stop_reason"], "event_limit");
    assert_eq!(result["experiment_id"], Json::Null);
    assert_eq!(result["runs"][0]["run_id"], Json::Null);
    assert_eq!(result["runs"][0]["trace"], Json::Null);
    assert_eq!(result["runs"][0]["seed"], Json::from(42));
    let runs = dir.path().join(".taktsim").join("runs");
    assert_eq!(std::fs::read_dir(runs).unwrap().count(), 0);
}

/// `schema all` and `schema model` print the committed bytes; a component type
/// prints its entry; each document has its schema.
#[test]
fn schema_prints_what_is_committed() {
    let dir = Dir::new();
    let committed = std::fs::read_to_string(repo(&["schemas", "model", "1.0.json"])).unwrap();
    for name in ["all", "model"] {
        assert_eq!(dir.taktsim(&["schema", name]).stdout, committed, "{name}");
    }
    let description = describe();
    assert_eq!(
        dir.taktsim(&["schema", "error"]).json(),
        description["error_schema"]
    );
    assert_eq!(
        dir.taktsim(&["schema", "describe"]).json(),
        description["schema"]
    );
    assert_eq!(
        dir.taktsim(&["schema", "result"]).json(),
        command(&description, "run")["result_schema"]
    );
    assert_eq!(
        dir.taktsim(&["schema", "validation"]).json(),
        command(&description, "validate")["result_schema"]
    );
    for name in ["Buffer", "Machine", "Sink", "Source"] {
        let schema = dir.taktsim(&["schema", name]).json();
        assert_eq!(schema["properties"]["type"]["const"], name);
        let checker = Checker { root: &schema };
        let entry = Json::object([("id", Json::from("x")), ("type", Json::from(name))]);
        // A Sink needs nothing more; a Buffer and a Machine do.
        assert_eq!(
            checker.valid(&schema, &entry),
            matches!(name, "Sink" | "Source"),
            "{name}"
        );
    }
    let unknown = dir.taktsim(&["schema", "Conveyor", "--format", "json"]);
    assert_eq!(unknown.code, 1);
    assert_eq!(unknown.stdout, "");
    let line = &unknown.lines()[0];
    assert_eq!(line["type"], "args.unknown_schema");
    assert!(line["allowed"].as_str().unwrap().contains("Machine"));
}

/// A real SIGINT cuts the running replication; the finished ones count, the document
/// says `cancelled` and the exit code is 4 (ADR-0025 §9).
#[cfg(unix)]
#[allow(clippy::unwrap_used)] // Test helper.
fn interrupted(dir: &Dir, args: &[&str]) -> support::Outcome {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let child = Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .args(args)
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Once a replication is in the store, the handler is installed and the next one
    // runs.
    let runs = dir.path().join(".taktsim").join("runs");
    let started = Instant::now();
    while std::fs::read_dir(&runs).map_or(0, Iterator::count) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "no run finished"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let output = child.wait_with_output().unwrap();
    support::Outcome {
        code: output.status.code().unwrap(),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// A list long enough to be interrupted in a debug build.
#[cfg(unix)]
const SLOW: [&str; 9] = [
    "run",
    "line.yaml",
    "--seed",
    "1..3",
    "--horizon",
    "300h",
    "--format",
    "json",
    "--no-reuse",
];

#[cfg(unix)]
#[test]
fn a_signal_exits_4_with_the_finished_runs() {
    let dir = Dir::new().with_line();
    let outcome = interrupted(&dir, &SLOW);
    assert_eq!(outcome.code, 4, "{}", outcome.stderr);
    let result = outcome.json();
    assert_eq!(result["complete"], false);
    assert_eq!(result["stop_reason"], "cancelled");
    let counted = result["runs"].as_array().unwrap().len();
    assert!((1..3).contains(&counted), "{counted}");
    // Not kept: a cancelled document is not determined by the model and the run set.
    assert!(!dir.path().join(".taktsim").join("experiments").exists());
}

/// Running the same command again reuses the prefix and gives the bytes of a run that
/// was never interrupted.
#[cfg(unix)]
#[test]
fn interrupted_then_resumed_equals_uninterrupted() {
    let dir = Dir::new().with_line();
    let first = interrupted(&dir, &SLOW);
    assert_eq!(first.code, 4, "{}", first.stderr);
    let resumed = dir.taktsim(&SLOW[..8]);
    assert_eq!(resumed.code, 0, "{}", resumed.stderr);
    let fresh = Dir::new().with_line();
    let whole = fresh.taktsim(&SLOW[..8]);
    assert_eq!(whole.code, 0, "{}", whole.stderr);
    assert_eq!(resumed.stdout, whole.stdout);
}
