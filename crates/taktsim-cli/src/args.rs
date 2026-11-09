//! The command line's arguments: the parser's types, which `describe` walks.

use std::num::{NonZeroU32, NonZeroU64};
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use taktsim_run::SeedSpec;
use taktsim_units::SimDuration;

/// Simulate production systems from models in text files.
#[derive(Debug, Parser)]
#[command(
    name = "taktsim",
    version,
    subcommand_required = true,
    disable_help_subcommand = true,
    help_template = "usage: {usage}\n\n{about}\n\n{all-args}\n"
)]
pub(crate) struct Cli {
    /// What to do.
    #[command(subcommand)]
    pub(crate) command: Command,
    /// How to print the result: `json` is one document on stdout and JSON Lines on
    /// stderr.
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "text",
        value_name = "FORMAT"
    )]
    pub(crate) format: Format,
}

/// The commands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Print every command with its inputs, outputs and errors, as one JSON document.
    Describe,
    /// Print a JSON Schema: `all` or `model`, a component type, `result`, `validation`, `error` or `describe`.
    Schema {
        /// What to print the schema of.
        #[arg(value_name = "TYPE")]
        name: String,
    },
    /// Check a model without running it.
    Validate {
        /// The model file, `.yaml`, `.yml` or `.json`.
        #[arg(value_name = "MODEL")]
        model: PathBuf,
    },
    /// Run a model and keep the run in the run store.
    Run(RunArgs),
}

/// The arguments of `run`.
#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// The model file, `.yaml`, `.yml` or `.json`.
    #[arg(value_name = "MODEL")]
    pub(crate) model: PathBuf,
    /// The seed, over `model.seed`; a model that draws nothing has none. Repeat it, or
    /// give a range such as `1..20`, for an experiment over several seeds; with
    /// `experiment.precision`, one seed is the first of a sequence.
    #[arg(long, value_name = "SEEDS")]
    pub(crate) seed: Vec<SeedSpec>,
    /// Another horizon: a new version of the model, with its own hash.
    #[arg(long, value_name = "DURATION")]
    pub(crate) horizon: Option<SimDuration>,
    /// The run store; otherwise the nearest `.taktsim/` upward, else one here.
    #[arg(long, value_name = "PATH")]
    pub(crate) store: Option<PathBuf>,
    /// The most events to process; a run stopped by it is partial and exits 3.
    #[arg(long, value_name = "COUNT")]
    pub(crate) max_events: Option<u64>,
    /// The most replications of an experiment; required with `experiment.precision`
    /// and for more than 1000 seeds.
    #[arg(long, value_name = "RUNS")]
    pub(crate) max_runs: Option<NonZeroU32>,
    /// The most wall-clock seconds of an experiment; required with
    /// `experiment.precision` and for more than 1000 seeds. Only how many replications
    /// count depends on it.
    #[arg(long, value_name = "SECONDS")]
    pub(crate) max_seconds: Option<NonZeroU64>,
    /// Run even if the store has this run, and compare; it is never overwritten.
    #[arg(long)]
    pub(crate) no_reuse: bool,
}

/// How to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// For people.
    Text,
    /// One JSON document on stdout, JSON Lines on stderr.
    Json,
}
