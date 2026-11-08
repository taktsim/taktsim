//! Runs the real `taktsim` binary: its stdout, stderr and exit code, as snapshots,
//! with what differs by platform or version masked.

mod support;

use std::process::Command;

use support::{Dir, EXACT, UNSEEDED};

fn taktsim(args: &[&str]) -> std::io::Result<std::process::Output> {
    Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .args(args)
        .output()
}

#[test]
fn version_prints_package_version() {
    for flag in ["--version", "-V"] {
        let out = taktsim(&[flag]).unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            format!("taktsim {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn help_prints_usage() {
    for flag in ["--help", "-h"] {
        let out = taktsim(&[flag]).unwrap();
        assert!(out.status.success());
        assert!(
            String::from_utf8(out.stdout)
                .unwrap()
                .starts_with("usage: taktsim")
        );
    }
}

/// Each command has its own help, for people; the JSON stays for agents.
#[test]
fn every_command_has_help() {
    for command in ["describe", "schema", "validate", "run"] {
        let out = taktsim(&[command, "--help"]).unwrap();
        assert!(out.status.success(), "{command}");
        assert!(
            String::from_utf8(out.stdout).unwrap().contains("--format"),
            "{command}"
        );
    }
}

/// The placeholder binary exited 2 here and printed its usage. ADR-0019's code for
/// invalid arguments is 1, and an agent reads a typed error, so this asks for more:
/// exit 1, nothing on stdout, and in JSON one line with a path, what was found, what
/// is allowed and an example.
#[test]
fn invalid_invocations_exit_1_with_a_typed_error_and_an_empty_stdout() {
    let dir = Dir::new();
    for args in [
        &[][..],
        &["run"][..],
        &["--version", "extra"][..],
        &["no-such-command"][..],
        &["run", "m.yaml", "--no-such-flag"][..],
    ] {
        let text = dir.taktsim(args);
        assert_eq!(text.code, 1, "{args:?}");
        assert_eq!(text.stdout, "", "{args:?}");
        assert!(
            text.stderr.starts_with("error[args."),
            "{args:?}: {}",
            text.stderr
        );

        let mut with_json = args.to_vec();
        with_json.extend(["--format", "json"]);
        let json = dir.taktsim(&with_json);
        assert_eq!(json.code, 1, "{args:?}");
        assert_eq!(json.stdout, "", "{args:?}");
        let lines = json.lines();
        assert_eq!(lines.len(), 1, "{args:?}: {}", json.stderr);
        assert_eq!(lines[0]["kind"], "error");
        for key in ["type", "message", "path", "found", "allowed", "example"] {
            assert!(
                lines[0][key].as_str().is_some(),
                "{args:?} {key}: {}",
                json.stderr
            );
        }
    }
}

/// One snapshot of an invocation in a fresh directory with `line.yaml`.
#[track_caller]
fn snapshot(name: &str, args: &[&str]) {
    let dir = Dir::new().with_line();
    dir.write("unseeded.yaml", UNSEEDED);
    dir.write("exact.yaml", EXACT);
    dir.write(
        "broken.yaml",
        "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 8h}\n\
         components:\n  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 10}\n  \
         - {id: sink, type: Sink}\nflow:\n  - [source, cut, sink]\n",
    );
    let outcome = dir.taktsim(args);
    insta::assert_snapshot!(name, dir.masked(&outcome));
}

#[test]
fn validate_snapshots() {
    snapshot("validate_text", &["validate", "line.yaml"]);
    snapshot(
        "validate_json",
        &["validate", "line.yaml", "--format", "json"],
    );
    snapshot("validate_invalid_text", &["validate", "broken.yaml"]);
    snapshot(
        "validate_invalid_json",
        &["validate", "broken.yaml", "--format", "json"],
    );
    snapshot("validate_missing_file", &["validate", "nothing.yaml"]);
}

#[test]
fn run_snapshots() {
    snapshot("run_text", &["run", "line.yaml"]);
    snapshot("run_json", &["run", "line.yaml", "--format", "json"]);
    snapshot(
        "run_seed_and_horizon",
        &["run", "line.yaml", "--seed", "7", "--horizon", "1h"],
    );
    snapshot("run_exact_text", &["run", "exact.yaml"]);
    snapshot("run_unused_seed", &["run", "exact.yaml", "--seed", "7"]);
    snapshot("run_no_seed", &["run", "unseeded.yaml", "--format", "json"]);
    snapshot(
        "run_event_limit",
        &["run", "line.yaml", "--max-events", "5", "--format", "json"],
    );
    snapshot("run_invalid_model", &["run", "broken.yaml"]);
}

#[test]
fn argument_error_snapshots() {
    snapshot("args_no_command", &[]);
    snapshot("args_unknown_command", &["simulate"]);
    snapshot(
        "args_unknown_flag",
        &["run", "line.yaml", "--sed", "1", "--format", "json"],
    );
    snapshot(
        "args_bad_duration",
        &["run", "line.yaml", "--horizon", "5", "--format", "json"],
    );
    snapshot("args_bad_format", &["run", "line.yaml", "--format", "yaml"]);
    snapshot(
        "args_seed_twice",
        &["run", "line.yaml", "--seed", "1", "--seed", "2"],
    );
    snapshot(
        "schema_unknown",
        &["schema", "Conveyor", "--format", "json"],
    );
}

#[test]
fn a_second_run_is_reused_and_says_so() {
    let dir = Dir::new().with_line();
    let first = dir.taktsim(&["run", "line.yaml", "--format", "json"]);
    assert_eq!(first.code, 0, "{}", first.stderr);
    assert_eq!(first.lines()[0]["reused"], false);
    let second = dir.taktsim(&["run", "line.yaml", "--format", "json"]);
    assert_eq!(second.code, 0);
    assert_eq!(second.lines()[0]["reused"], true);
    assert_eq!(second.stdout, first.stdout, "the same document");
    // With a limit too: nothing runs, so no limit is reached.
    let third = dir.taktsim(&["run", "line.yaml", "--format", "json", "--max-events", "1"]);
    assert_eq!(third.code, 0);
    assert_eq!(third.stdout, first.stdout);
}

#[test]
fn the_store_is_found_upward_and_can_be_named() {
    let dir = Dir::new().with_line();
    std::fs::create_dir_all(dir.path().join("a").join("b")).unwrap();
    assert_eq!(dir.taktsim(&["run", "line.yaml"]).code, 0);
    assert!(dir.path().join(".taktsim").join("store.json").is_file());
    let below = std::process::Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .args(["run", "../../line.yaml", "--format", "json"])
        .current_dir(dir.path().join("a").join("b"))
        .output()
        .unwrap();
    assert!(below.status.success());
    let progress = String::from_utf8(below.stderr).unwrap();
    assert!(progress.contains("\"reused\":true"), "{progress}");
    assert!(!dir.path().join("a").join("b").join(".taktsim").exists());
    let named = dir.taktsim(&["run", "line.yaml", "--store", "elsewhere"]);
    assert_eq!(named.code, 0);
    assert!(dir.path().join("elsewhere").join("store.json").is_file());
}

#[test]
fn describe_and_schema_print_json_whatever_the_format() {
    let dir = Dir::new();
    for args in [
        &["describe"][..],
        &["describe", "--format", "text"][..],
        &["schema", "all"][..],
        &["schema", "Machine", "--format", "text"][..],
    ] {
        let outcome = dir.taktsim(args);
        assert_eq!(outcome.code, 0, "{args:?}");
        assert!(outcome.json().as_object().is_some(), "{args:?}");
        assert_eq!(outcome.stderr, "", "{args:?}");
    }
}

/// `validate` reads only.
#[test]
fn validate_writes_nothing() {
    let dir = Dir::new().with_line();
    assert_eq!(dir.taktsim(&["validate", "line.yaml"]).code, 0);
    assert!(!dir.path().join(".taktsim").exists());
}
