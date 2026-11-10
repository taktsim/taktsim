//! Independent tests of M8's exit codes through the real binary (ADR-0025 §4, §5, §9),
//! written from the ADR and `taktsim describe` only. Each expected code is the row of
//! ADR-0025 §5's ending table, or §4's budget table, that the command falls in.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[allow(clippy::unwrap_used)] // Test helper.
fn workdir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hand-derived-exit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[allow(clippy::unwrap_used)] // Test helper.
fn write_model(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("model.yaml");
    std::fs::write(&path, text).unwrap();
    path
}

#[allow(clippy::expect_used)] // Test helper.
fn taktsim(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("the binary runs")
}

const RANDOM: &str = "format: \"1.0\"\nmodel: {name: random}\n\
    experiment: {type: terminating, horizon: 1h}\n\
    components:\n\
    - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
    - {id: cut, type: Machine, cycle_time: {dist: exponential, mean: 45s}}\n\
    - {id: buf, type: Buffer, capacity: 100 parts}\n\
    - {id: weld, type: Machine, cycle_time: 40s}\n\
    - {id: sink, type: Sink}\n\
    flow: [[source, cut, buf, weld, sink]]\n";

/// A short list finishes: exit 0, `seeds_finished`, one entry per seed.
#[test]
fn list_finishes_with_exit_0() {
    let dir = workdir("list");
    write_model(&dir, RANDOM);
    let out = taktsim(
        &dir,
        &["run", "model.yaml", "--seed", "1..4", "--format", "json"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("\"stop_reason\": \"seeds_finished\""),
        "{stdout}"
    );
    assert_eq!(stdout.matches("\"seed\": ").count(), 4, "{stdout}");
}

/// A replication stopped by `--max-events`: exit 3, `event_limit`.
#[test]
fn event_limit_exits_3() {
    let dir = workdir("events");
    write_model(&dir, RANDOM);
    let out = taktsim(
        &dir,
        &[
            "run",
            "model.yaml",
            "--seed",
            "1..3",
            "--max-events",
            "20",
            "--format",
            "json",
        ],
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("\"stop_reason\": \"event_limit\"")
    );
}

/// `--seed 0..9007199254740991` is 2^53 seeds: refused at once, exit 1,
/// `run.budget_too_large`, before anything is expanded (a 2^53-entry list would need
/// 64 PiB, so a test that returns at all shows nothing was allocated).
#[test]
fn huge_range_exits_1_at_once() {
    let dir = workdir("huge");
    write_model(&dir, RANDOM);
    let out = taktsim(
        &dir,
        &[
            "run",
            "model.yaml",
            "--seed",
            "0..9007199254740991",
            "--format",
            "json",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("\"type\":\"run.budget_too_large\"")
    );
    assert!(!dir.join(".taktsim").join("runs").exists());
}

/// 1,001 seeds without budget flags: exit 1, `run.no_budget`, one line per flag.
#[test]
fn long_list_without_budget_exits_1() {
    let dir = workdir("long");
    write_model(&dir, RANDOM);
    let out = taktsim(
        &dir,
        &["run", "model.yaml", "--seed", "1..1001", "--format", "json"],
    );
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr.matches("\"type\":\"run.no_budget\"").count(),
        2,
        "{stderr}"
    );
}

/// A model that draws nothing with `--seed 1..5`: one run, exit 0, no interval, so no
/// `±` in the text (ADR-0025 §12, `no_draw_model_never_gets_an_interval`).
#[test]
fn no_draw_list_prints_no_interval() {
    let dir = workdir("no-draw");
    write_model(
        &dir,
        "format: \"1.0\"\nmodel: {name: fixed}\nexperiment: {type: terminating, horizon: 1h}\n\
         components: [{id: source, type: Source, interarrival: 50s}, {id: cut, type: Machine, cycle_time: 42s}, {id: sink, type: Sink}]\n\
         flow: [[source, cut, sink]]\n",
    );
    let out = taktsim(&dir, &["run", "model.yaml", "--seed", "1..5"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains('±'), "{stdout}");
    assert_eq!(stdout.matches("run: ").count(), 1, "{stdout}");
}

/// A model that draws nothing is one run whatever `--seed` names (ADR-0025 §4, last
/// row), and each unused flag is a warning: the binary answers as the library does,
/// since it checks the list only once the model is loaded.
#[test]
fn no_draw_model_takes_any_seed_list_as_one_run() {
    let dir = workdir("no-draw-any");
    write_model(
        &dir,
        "format: \"1.0\"\nmodel: {name: fixed}\nexperiment: {type: terminating, horizon: 1h}\n\
         components: [{id: source, type: Source}, {id: cut, type: Machine, cycle_time: 1min}, {id: sink, type: Sink}]\n\
         flow: [[source, cut, sink]]\n",
    );
    for args in [
        &["--seed", "1..20", "--max-runs", "5"][..],
        &["--seed", "1..2000"][..],
        &["--seed", "0..9007199254740991"][..],
    ] {
        let mut all = vec!["run", "model.yaml", "--format", "json"];
        all.extend(args);
        let out = taktsim(&dir, &all);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(stderr.contains("\"type\":\"run.seed_unused\""), "{stderr}");
        assert!(!stderr.contains("\"kind\":\"error\""), "{stderr}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert_eq!(stdout.matches("\"run_id\": ").count(), 1, "{stdout}");
    }
    let out = taktsim(
        &dir,
        &[
            "run",
            "model.yaml",
            "--seed",
            "1..20",
            "--max-runs",
            "5",
            "--format",
            "json",
        ],
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("\"type\":\"run.flag_unused\"")
            && stderr.contains("\"path\":\"--max-runs\""),
        "{stderr}"
    );
}

/// A precision model with a seed list: exit 1 with `run.seed_list_with_precision`
/// (ADR-0025 §4), whatever the budget, not a budget refusal whose example still fails.
#[test]
fn precision_with_a_seed_list_names_that_first() {
    let dir = workdir("precision-list");
    write_model(
        &dir,
        &RANDOM.replace(
            "horizon: 1h}",
            "horizon: 1h, precision: {\"throughput(sink)\": 5%}}",
        ),
    );
    for budget in [
        &["--max-runs", "10", "--max-seconds", "60"][..],
        &["--max-runs", "20", "--max-seconds", "60"][..],
        &[][..],
    ] {
        let mut all = vec!["run", "model.yaml", "--seed", "1..20", "--format", "json"];
        all.extend(budget);
        let out = taktsim(&dir, &all);
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(out.status.code(), Some(1), "{budget:?}: {stderr}");
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        assert!(
            stderr.contains("\"type\":\"run.seed_list_with_precision\""),
            "{budget:?}: {stderr}"
        );
    }
}

/// A real SIGINT while long replications run: exit 4, `cancelled` (ADR-0025 §9). Each
/// replication has about 260,000 parts, many seconds each, so the signal sent after
/// one second always arrives while they run.
#[cfg(unix)]
#[test]
fn sigint_exits_4() {
    let dir = workdir("sigint");
    write_model(
        &dir,
        "format: \"1.0\"\nmodel: {name: slow}\nexperiment: {type: terminating, horizon: 72h}\n\
         components:\n\
         - {id: source, type: Source, interarrival: {dist: exponential, mean: 1s}}\n\
         - {id: queue, type: Buffer, capacity: 1000 parts}\n\
         - {id: server, type: Machine, cycle_time: {dist: exponential, mean: 500ms}}\n\
         - {id: sink, type: Sink}\n\
         flow: [[source, queue, server, sink]]\n",
    );
    let child = Command::new(env!("CARGO_BIN_EXE_taktsim"))
        .current_dir(&dir)
        .args(["run", "model.yaml", "--seed", "1..3", "--format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(1));
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("\"stop_reason\": \"cancelled\""),
        "{stdout}"
    );
    assert!(stdout.contains("\"complete\": false"), "{stdout}");
}
