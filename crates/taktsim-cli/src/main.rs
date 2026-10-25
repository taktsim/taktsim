//! The `taktsim` command line. `validate` and `run` arrive in roadmap M7;
//! until then it only answers `--version` and `--help`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// The CLI is the program's I/O boundary, so it may print.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

const USAGE: &str =
    "usage: taktsim [--version | --help]\n\nCommands arrive in v0.1 (validate, run).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version" | "-V"] => {
            println!("taktsim {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
