//! Runs the real `taktsim` binary in a temporary directory, for the tests of the
//! CLI, with what differs between targets masked in what it prints.

#![allow(dead_code, clippy::unwrap_used, clippy::panic)] // Each test crate uses a part.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use regex::Regex;
use taktsim_model::{Decode, Decoder, Format, Json, Node, Schemas, decode_str};

/// A directory removed when dropped, the binary's current directory.
pub struct Dir(PathBuf);

impl Dir {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "taktsim-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes a file below the directory.
    pub fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap();
    }

    /// Writes the model `line.yaml` the examples in `describe` name.
    pub fn with_line(self) -> Self {
        self.write("line.yaml", LINE);
        self
    }

    /// The binary, run here with these arguments.
    pub fn taktsim(&self, args: &[&str]) -> Outcome {
        let output = Command::new(env!("CARGO_BIN_EXE_taktsim"))
            .args(args)
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Outcome {
            code: output.status.code().unwrap(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }

    /// What is printed, with the temporary directory and everything that differs
    /// by target or version masked, and backslashes in paths normalized.
    pub fn masked(&self, outcome: &Outcome) -> String {
        let mut text = format!(
            "exit: {}\n--- stdout ---\n{}--- stderr ---\n{}",
            outcome.code, outcome.stdout, outcome.stderr
        );
        let canonical = std::fs::canonicalize(&self.0).unwrap();
        let verbatim = canonical.display().to_string();
        let plain = verbatim.trim_start_matches(r"\\?\").to_owned();
        for form in [
            verbatim.clone(),
            plain.clone(),
            self.0.display().to_string(),
        ] {
            text = text.replace(&form.replace('\\', r"\\"), "<tmp>");
            text = text.replace(&form, "<tmp>");
        }
        let store = Regex::new(r"<tmp>(\\\\|\\)\.taktsim").unwrap();
        let text = store.replace_all(&text, "<tmp>/.taktsim").into_owned();
        let platform = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
        let version = env!("CARGO_PKG_VERSION");
        let text = text
            .replace(&platform, "<platform>")
            .replace(
                &format!("\"engine_version\": \"{version}\""),
                "\"engine_version\": \"<version>\"",
            )
            .replace(
                &format!("\"package_version\": \"{version}\""),
                "\"package_version\": \"<version>\"",
            )
            .replace(
                &format!("engine: taktsim-core {version}"),
                "engine: taktsim-core <version>",
            )
            .replace(
                &format!("package: taktsim-components {version}"),
                "package: taktsim-components <version>",
            );
        // The ids cover the versions, and their definition is pinned elsewhere.
        let run_id = Regex::new(r"taktsim-core-[0-9a-f]{64}").unwrap();
        let text = run_id.replace_all(&text, "<run id>");
        let experiment = Regex::new(r"exp-[0-9a-f]{64}").unwrap();
        experiment
            .replace_all(&text, "<experiment id>")
            .into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What a run of the binary did.
#[derive(Debug)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    /// Stdout as JSON.
    pub fn json(&self) -> Json {
        parse(&self.stdout)
    }

    /// Each line of stderr as JSON.
    pub fn lines(&self) -> Vec<Json> {
        self.stderr.lines().map(parse).collect()
    }
}

/// Any JSON, as read.
struct Raw(Json);

impl Decode for Raw {
    const EXPECTED: &'static str = "anything";
    const EXAMPLE: &'static str = "{}";

    fn decode(node: &Node, _: &mut Decoder) -> Option<Self> {
        Some(Self(node.to_json()))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([])
    }
}

/// Parses `text` as JSON.
pub fn parse(text: &str) -> Json {
    match decode_str::<Raw>(text, Format::Json) {
        Ok(raw) => raw.0,
        Err(errors) => panic!("not JSON: {errors}\n{text}"),
    }
}

/// A path below the repository.
pub fn repo(parts: &[&str]) -> PathBuf {
    parts.iter().fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        |p, q| p.join(q),
    )
}

/// The model the examples in `describe` run: it draws, and states a seed.
pub const LINE: &str = "format: \"1.0\"\n\
model: {name: line-a, seed: 42}\n\
experiment: {type: terminating, horizon: 8h}\n\
components:\n\
  - {id: source, type: Source}\n\
  - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}}\n\
  - {id: buf1, type: Buffer, capacity: 5 parts}\n\
  - {id: weld, type: Machine, cycle_time: 45s}\n\
  - {id: sink, type: Sink}\n\
flow:\n\
  - [source, cut, buf1, weld, sink]\n";

/// A model that draws, with a precision target: a sequential experiment.
pub const PRECISE: &str = "format: \"1.0\"\n\
model: {name: precise, seed: 1}\n\
experiment: {type: terminating, horizon: 1h, precision: {\"throughput(sink)\": 0.1%}}\n\
components:\n\
  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
  - {id: sink, type: Sink}\n\
flow:\n\
  - [source, sink]\n";

/// A model that draws and states no seed.
pub const UNSEEDED: &str = "format: \"1.0\"\n\
model: {name: unseeded}\n\
experiment: {type: terminating, horizon: 1h}\n\
components:\n\
  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n\
  - {id: sink, type: Sink}\n\
flow:\n\
  - [source, sink]\n";

/// A model that draws nothing.
pub const EXACT: &str = "format: \"1.0\"\n\
model: {name: exact}\n\
experiment: {type: terminating, horizon: 1h}\n\
components:\n\
  - {id: source, type: Source}\n\
  - {id: cut, type: Machine, cycle_time: 1min}\n\
  - {id: sink, type: Sink}\n\
flow:\n\
  - [source, cut, sink]\n";
