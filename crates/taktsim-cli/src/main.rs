//! The `taktsim` command line: a thin layer over `taktsim-run` (ADR-0019,
//! ADR-0024). Argument parsing and rendering live here, and no other logic.
//! `describe` lists every command, its inputs, outputs and errors; an agent that
//! knows only this binary's name starts there.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod args;
mod describe;
mod parse_error;
mod render;

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::Parser;
use taktsim_components::{Flow, Registry, standard, standard_schema};
use taktsim_model::Json;
use taktsim_run::{
    Diagnostic, Failure, ModelSource, RunRequest, SCHEMA_NAMES, Store, schema_named, schema_names,
};

use crate::args::{Cli, Command, Format, RunArgs};
use crate::render::{exit_for_failure, exit_for_result};

/// The exit code of an internal bug: Rust's panic code (ADR-0007, ADR-0019).
const INTERNAL: u8 = 101;
/// The exit code of an I/O failure.
const IO_FAILURE: u8 = 7;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let format = parse_error::format_in(&args);
    install_panic_hook(format);
    #[cfg(feature = "test-panic")]
    panic_if_asked();
    let code = match Cli::try_parse_from(parse_error::attach_hyphen_values(&args)) {
        Ok(cli) => run_command(&cli),
        Err(error) => parse_error::handle(&error, &args, format),
    };
    ExitCode::from(code)
}

/// The invariant-violation path: with `--format json`, a panic is one more line of
/// JSON on stderr; the exit code is 101 either way (ADR-0024 §9).
fn install_panic_hook(format: Format) {
    if format == Format::Json {
        std::panic::set_hook(Box::new(|info| {
            let line = Diagnostic::error("internal.panic", info.to_string());
            write_stderr(&render::line(&line, Format::Json));
        }));
    }
}

/// With the `test-panic` feature only: no input reaches a panic, so the test of the
/// hook asks for one.
#[cfg(feature = "test-panic")]
fn panic_if_asked() {
    assert!(
        std::env::var_os("TAKTSIM_TEST_PANIC").is_none(),
        "a panic asked for by TAKTSIM_TEST_PANIC"
    );
}

/// Writes to stderr. If stderr itself is gone there is nowhere to say so.
pub(crate) fn write_stderr(text: &str) {
    let _ = std::io::stderr().write_all(text.as_bytes());
}

/// Writes to stdout: the exit code of a failure if it cannot be written, after an
/// `io.stdout` line on stderr.
pub(crate) fn print(text: &str, format: Format) -> u8 {
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(error) => {
            let line = Diagnostic::error("io.stdout", format!("stdout: {error}"))
                .at("<stdout>")
                .found(error.kind().to_string())
                .allowed("a stdout that can be written");
            write_stderr(&render::line(&line, format));
            IO_FAILURE
        }
    }
}

/// The standard catalog; a catalog rule broken here is a bug, not bad input.
fn registry(format: Format) -> Result<Registry<Flow>, u8> {
    standard().map_err(|error| {
        let line = Diagnostic::error("internal.catalog", error.to_string());
        write_stderr(&render::line(&line, format));
        INTERNAL
    })
}

fn run_command(cli: &Cli) -> u8 {
    match &cli.command {
        Command::Describe => print(&describe::describe_text(), cli.format),
        Command::Schema { name } => schema(name, cli.format),
        Command::Validate { model } => validate(model, cli.format),
        Command::Run(args) => run(args, cli.format),
    }
}

/// Each error line of `failure`, then its exit code.
fn fail(failure: &Failure, format: Format) -> u8 {
    for line in &failure.diagnostics {
        write_stderr(&render::line(line, format));
    }
    exit_for_failure(failure.class)
}

/// A schema as it is printed: keys sorted, `\n` line ends.
fn schema_text(schema: &Json) -> String {
    let mut text = schema.to_pretty();
    text.push('\n');
    text
}

/// Prints the schema called `name`; `taktsim-run` picks every one but `describe`,
/// which is this binary's own (ADR-0024 §10).
fn schema(name: &str, format: Format) -> u8 {
    let model = match standard_schema() {
        Ok(model) => model,
        Err(error) => {
            let line = Diagnostic::error("internal.catalog", error.to_string());
            write_stderr(&render::line(&line, format));
            return INTERNAL;
        }
    };
    let schema = if name == "describe" {
        Some(describe::describe_schema())
    } else {
        schema_named(&model, name)
    };
    if let Some(schema) = schema {
        return print(&schema_text(&schema), format);
    }
    let mut names = schema_names(&model);
    names.insert(SCHEMA_NAMES.len(), "describe");
    let line = Diagnostic::error("args.unknown_schema", format!("no schema of `{name}`"))
        .at("<type>")
        .found(name)
        .allowed(names.join(", "))
        .example("Machine");
    write_stderr(&render::line(&line, format));
    1
}

fn validate(model: &Path, format: Format) -> u8 {
    let registry = match registry(format) {
        Ok(registry) => registry,
        Err(code) => return code,
    };
    let report = taktsim_run::validate(&registry, ModelSource::Path(model));
    let text = match format {
        Format::Json => report.to_text(),
        Format::Text => render::validation_text(&report),
    };
    let printed = print(&text, format);
    for line in &report.errors {
        write_stderr(&render::line(line, format));
    }
    // Exit 1 says the report is on stdout; if it is not, that comes first.
    if printed != 0 {
        return printed;
    }
    report.failure_class().map_or(0, exit_for_failure)
}

/// The store `--store` names, or the nearest one upward from here.
fn open_store(args: &RunArgs) -> Result<Store, Failure> {
    if let Some(dir) = &args.store {
        return Store::open(dir);
    }
    let here = std::env::current_dir().map_err(|e| {
        Failure::new(
            taktsim_run::FailureClass::Io,
            vec![
                Diagnostic::error("store.read", format!("the current directory: {e}"))
                    .at("--store")
                    .found(e.kind().to_string())
                    .allowed("a readable current directory"),
            ],
        )
    })?;
    Store::discover(&here)
}

fn run(args: &RunArgs, format: Format) -> u8 {
    let registry = match registry(format) {
        Ok(registry) => registry,
        Err(code) => return code,
    };
    let store = match open_store(args) {
        Ok(store) => store,
        Err(failure) => return fail(&failure, format),
    };
    let seeds: Vec<u64> = args.seed.iter().copied().collect();
    let request = RunRequest {
        seeds: &seeds,
        budget: taktsim_run::Budget::none(),
        threads: None,
        max_memory_mib: None,
        horizon: args.horizon,
        max_events: args.max_events,
        reuse: !args.no_reuse,
        cancel: None,
    };
    let response =
        match taktsim_run::run(&registry, ModelSource::Path(&args.model), &request, &store) {
            Ok(response) => response,
            Err(failure) => return fail(&failure, format),
        };
    for warning in &response.warnings {
        write_stderr(&render::line(warning, format));
    }
    let mut progress = Diagnostic::progress(
        "run.store",
        format!(
            "{} in the store {}",
            if response.reused { "reused" } else { "ran" },
            store.root().display()
        ),
    );
    progress.store = Some(store.root().display().to_string());
    progress.reused = Some(response.reused);
    write_stderr(&render::line(&progress, format));
    let text = match format {
        Format::Json => response.text.clone(),
        Format::Text => render::result_text(&response.result),
    };
    match print(&text, format) {
        0 => exit_for_result(response.ended),
        code => code,
    }
}

#[cfg(test)]
mod tests {
    use taktsim_model::Encode;

    use super::*;

    /// The line a panic writes: `internal.panic`, which `error` allows.
    #[test]
    fn a_panic_line_is_one_line_of_the_error_format() {
        let line = render::line(
            &Diagnostic::error("internal.panic", "at engine.rs: violated"),
            Format::Json,
        );
        assert_eq!(line.lines().count(), 1);
        assert!(line.contains("\"type\":\"internal.panic\""), "{line}");
        let schema = taktsim_run::diagnostic_schema();
        assert_eq!(schema["properties"]["type"]["type"], Json::from("string"));
        let _ = Diagnostic::error("internal.panic", "x").encode();
    }
}
