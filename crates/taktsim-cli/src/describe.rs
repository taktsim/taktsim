//! The CLI's self-description (ADR-0019 item 1, ADR-0024 §10). It walks the parser
//! for commands, arguments, types, defaults and allowed values; what the parser
//! does not hold comes from one exhaustive `match` on [`Command`], so a new command
//! does not compile until it has its facts.

use clap::CommandFactory;
use taktsim_model::{Decode, Decoder, Encode, Fields, Json, Node, Record, Schemas, record_schema};
use taktsim_run::{diagnostic_schema, result_schema, validation_schema};

use crate::args::{Cli, Command};

/// The format of the document.
pub(crate) const DESCRIBE_FORMAT: &str = "taktsim-describe/1";

/// The type a value of this `value_name` has: a closed table, so an argument with a
/// new name fails `every_value_name_has_a_type`.
pub(crate) const fn value_type(value_name: &str) -> Option<ValueType> {
    match value_name.as_bytes() {
        b"DURATION" => Some(ValueType {
            word: "duration",
            allowed: "a duration such as `42s`, `12min`, `0.5h` or `8h`",
            example: "8h",
        }),
        b"SEEDS" => Some(ValueType {
            word: "seeds",
            allowed: "a whole number from 0 to 9007199254740991, or a range such as `1..20`; repeat the flag for more",
            example: "42",
        }),
        b"RUNS" => Some(ValueType {
            word: "integer",
            allowed: "a whole number from 1 to 100000",
            example: "100",
        }),
        b"SECONDS" => Some(ValueType {
            word: "integer",
            allowed: "a whole number of seconds from 1",
            example: "600",
        }),
        b"COUNT" => Some(ValueType {
            word: "integer",
            allowed: "a whole number from 0",
            example: "1000",
        }),
        b"PATH" => Some(ValueType {
            word: "path",
            allowed: "a directory",
            example: ".taktsim",
        }),
        b"MODEL" => Some(ValueType {
            word: "path",
            allowed: "a model file, `.yaml`, `.yml` or `.json`",
            example: "line.yaml",
        }),
        b"FORMAT" => Some(ValueType {
            word: "enum",
            allowed: "`text` or `json`",
            example: "json",
        }),
        b"TYPE" => Some(ValueType {
            word: "string",
            allowed: "`all`, `model`, a component type, `result`, `validation`, `error` or `describe`",
            example: "Machine",
        }),
        _ => None,
    }
}

/// What a value's type promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ValueType {
    pub(crate) word: &'static str,
    pub(crate) allowed: &'static str,
    pub(crate) example: &'static str,
}

/// The exit codes of ADR-0019 item 3.
const EXIT_CODES: [(u8, &str); 9] = [
    (0, "success"),
    (
        1,
        "invalid input: arguments, model, data, unknown run id; nothing ran",
    ),
    (
        2,
        "run failed: invariant or conservation violation, engine error",
    ),
    (
        3,
        "budget exhausted: partial result printed, with the stop reason",
    ),
    (
        4,
        "interrupted: partial result printed, with the stop reason",
    ),
    (
        5,
        "check not passed; the result itself is complete and valid",
    ),
    (6, "a person's confirmation is required"),
    (
        7,
        "I/O failure: a file or the run store cannot be read or written, or stdout cannot be written",
    ),
    (101, "internal bug: the single invariant-violation path"),
];

/// One code a command can give, with the error types of that code.
#[derive(Debug, Clone, PartialEq)]
struct CommandExit {
    code: u32,
    error_types: Vec<String>,
}

impl Encode for CommandExit {
    fn encode(&self) -> Json {
        Json::object([])
            .with("code", &self.code)
            .with("error_types", &self.error_types)
    }
}

impl Record for CommandExit {
    const EXPECTED: &'static str = "an exit code with its error types";
    const EXAMPLE: &'static str = "{code: 1, error_types: [args.*]}";
    const SCHEMA_NAME: Option<&'static str> = Some("CommandExit");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let code = fields.required("code");
        let error_types = fields.required("error_types");
        Some(Self {
            code: code?,
            error_types: error_types?,
        })
    }
}

/// What the parser does not hold about a command.
struct Facts {
    writes: &'static str,
    long_running: bool,
    budget: Option<Json>,
    needs_person: bool,
    input_schemas: &'static [(&'static str, &'static str)],
    result_schema: Json,
    exits: &'static [(u32, &'static [&'static str])],
    example: &'static str,
}

/// The codes every command can give.
const COMMON_ARGS: &[&str] = &["args.*"];
const WRITE_ERROR: &[&str] = &["io.stdout"];
/// The invariant-violation path of ADR-0007, which every command can reach: the
/// panic hook's `internal.panic`, and a catalog the binary cannot build
/// (`internal.catalog`).
const INTERNAL: (u32, &[&str]) = (101, &["internal.*"]);

/// The exit codes of `run`, with the error types of each.
const RUN_EXITS: &[(u32, &[&str])] = &[
    (0, &[]),
    (
        1,
        &[
            "args.*",
            "model.*",
            "run.no_seed",
            "run.seed_out_of_range",
            "run.duplicate_seed",
            "run.no_budget",
            "run.budget_too_small",
            "run.budget_too_large",
            "run.seed_list_with_precision",
            "run.build",
        ],
    ),
    (
        2,
        &[
            "run.component_failed",
            "run.invariant_violated",
            "run.conservation_violated",
            "run.item_not_received",
            "run.engine",
            "run.failed",
            "run.metrics_differ",
            "run.stalled",
            "run.no_draw_claim_false",
            "run.store_differs",
            "run.stored_run_differs",
            "run.trace_format",
            "run.replay_differs",
            "run.kpi_conservation",
            "run.kpi_undeclared_state",
            "run.kpi_no_end",
            "run.windows_differ",
            "internal.no_trace",
            "internal.experiment_id",
        ],
    ),
    (3, &[]),
    (4, &[]),
    (
        7,
        &[
            "io.stdout",
            "model.io",
            "store.format",
            "store.not_a_store",
            "store.read",
            "store.result",
            "store.write",
        ],
    ),
    INTERNAL,
];

/// The facts of a command. Exhaustive, so adding a command does not compile until
/// it has them.
fn facts(command: &Command) -> Facts {
    match command {
        Command::Describe => Facts {
            writes: "nothing",
            long_running: false,
            budget: None,
            needs_person: false,
            input_schemas: &[],
            result_schema: Json::object([
                (
                    "description",
                    Json::from("This document; its schema is its key `schema`."),
                ),
                ("type", Json::from("object")),
            ]),
            exits: &[(0, &[]), (1, COMMON_ARGS), (7, WRITE_ERROR), INTERNAL],
            example: "taktsim describe",
        },
        Command::Schema { .. } => Facts {
            writes: "nothing",
            long_running: false,
            budget: None,
            needs_person: false,
            input_schemas: &[],
            result_schema: Json::object([
                ("description", Json::from("A JSON Schema, draft 2020-12.")),
                ("type", Json::from("object")),
            ]),
            exits: &[(0, &[]), (1, COMMON_ARGS), (7, WRITE_ERROR), INTERNAL],
            example: "taktsim schema Machine",
        },
        Command::Validate { .. } => Facts {
            writes: "nothing",
            long_running: false,
            budget: None,
            needs_person: false,
            input_schemas: &[("<model>", "taktsim schema model")],
            result_schema: validation_schema(),
            exits: &[
                (0, &[]),
                (1, &["args.*", "model.*"]),
                (7, &["io.stdout", "model.io"]),
                INTERNAL,
            ],
            example: "taktsim validate line.yaml --format json",
        },
        Command::Run(_) => Facts {
            writes: "a run in the run store",
            long_running: true,
            budget: Some(run_budget()),
            needs_person: false,
            input_schemas: &[("<model>", "taktsim schema model")],
            result_schema: result_schema(),
            exits: RUN_EXITS,
            example: "taktsim run line.yaml --seed 42 --horizon 8h --format json",
        },
    }
}

/// What `run` may spend, and when each flag is required (ADR-0025 §4).
fn run_budget() -> Json {
    Json::object([
        (
            "flags",
            Json::Array(vec![Json::from("--max-runs"), Json::from("--max-seconds")]),
        ),
        (
            "required",
            Json::from(format!(
                "both, with `experiment.precision`, and for a list of more than {} seeds; a shorter list is its own budget",
                taktsim_run::MAX_SHORT_LIST
            )),
        ),
        ("short_list", Json::from(taktsim_run::MAX_SHORT_LIST)),
        ("max_seeds", Json::from(taktsim_run::MAX_SEEDS)),
    ])
}

/// One value of each command, to take the facts of each. A command the parser has
/// and this list does not fails `every_command_has_its_facts`.
pub(crate) fn every_command() -> Vec<Command> {
    vec![
        Command::Describe,
        Command::Schema {
            name: String::new(),
        },
        Command::Validate {
            model: std::path::PathBuf::new(),
        },
        Command::Run(crate::args::RunArgs {
            model: std::path::PathBuf::new(),
            seed: Vec::new(),
            horizon: None,
            store: None,
            max_events: None,
            max_runs: None,
            max_seconds: None,
            no_reuse: false,
        }),
    ]
}

/// The word the parser knows a command by. Exhaustive, as `facts`.
pub(crate) const fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Describe => "describe",
        Command::Schema { .. } => "schema",
        Command::Validate { .. } => "validate",
        Command::Run(_) => "run",
    }
}

/// Any JSON value.
#[derive(Debug, Clone, PartialEq)]
struct AnyJson(Json);

impl Encode for AnyJson {
    fn encode(&self) -> Json {
        self.0.clone()
    }
}

impl Decode for AnyJson {
    const EXPECTED: &'static str = "any JSON value";
    const EXAMPLE: &'static str = "{}";

    fn decode(node: &Node, _: &mut Decoder) -> Option<Self> {
        Some(Self(node.to_json()))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([("description", Json::from("Any JSON value."))])
    }
}

/// One argument or flag of a command.
#[derive(Debug, Clone, PartialEq)]
struct ArgumentInfo {
    name: String,
    help: String,
    value_type: String,
    required: bool,
    positional: bool,
    takes_value: bool,
    default: Option<String>,
    allowed: Vec<String>,
}

impl Encode for ArgumentInfo {
    fn encode(&self) -> Json {
        Json::object([])
            .with("name", &self.name)
            .with("help", &self.help)
            .with("type", &self.value_type)
            .with("required", &self.required)
            .with("positional", &self.positional)
            .with("takes_value", &self.takes_value)
            .with_optional("default", self.default.as_ref())
            .with("allowed", &self.allowed)
    }
}

impl Record for ArgumentInfo {
    const EXPECTED: &'static str = "an argument of a command";
    const EXAMPLE: &'static str = "{name: --seed, type: seed, required: false}";
    const SCHEMA_NAME: Option<&'static str> = Some("Argument");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let name = fields.required("name");
        let help = fields.required("help");
        let value_type = fields.required("type");
        let required = fields.required("required");
        let positional = fields.required("positional");
        let takes_value = fields.required("takes_value");
        let default = fields.optional("default");
        let allowed = fields.required("allowed");
        Some(Self {
            name: name?,
            help: help?,
            value_type: value_type?,
            required: required?,
            positional: positional?,
            takes_value: takes_value?,
            default: default?,
            allowed: allowed?,
        })
    }
}

/// An input document of a command, by reference.
#[derive(Debug, Clone, PartialEq)]
struct InputSchema {
    argument: String,
    schema_command: String,
}

impl Encode for InputSchema {
    fn encode(&self) -> Json {
        Json::object([])
            .with("argument", &self.argument)
            .with("schema_command", &self.schema_command)
    }
}

impl Record for InputSchema {
    const EXPECTED: &'static str = "an input document by reference";
    const EXAMPLE: &'static str = "{argument: <model>, schema_command: taktsim schema model}";
    const SCHEMA_NAME: Option<&'static str> = Some("InputSchema");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let argument = fields.required("argument");
        let schema_command = fields.required("schema_command");
        Some(Self {
            argument: argument?,
            schema_command: schema_command?,
        })
    }
}

/// One command.
#[derive(Debug, Clone, PartialEq)]
struct CommandInfo {
    name: String,
    purpose: String,
    writes: String,
    long_running: bool,
    budget: Option<AnyJson>,
    needs_person: bool,
    arguments: Vec<ArgumentInfo>,
    input_schemas: Vec<InputSchema>,
    result_schema: AnyJson,
    exit_codes: Vec<CommandExit>,
    example: String,
}

impl Encode for CommandInfo {
    fn encode(&self) -> Json {
        Json::object([])
            .with("name", &self.name)
            .with("purpose", &self.purpose)
            .with("writes", &self.writes)
            .with("long_running", &self.long_running)
            .with_optional("budget", self.budget.as_ref())
            .with("needs_person", &self.needs_person)
            .with("arguments", &self.arguments)
            .with("input_schemas", &self.input_schemas)
            .with("result_schema", &self.result_schema)
            .with("exit_codes", &self.exit_codes)
            .with("example", &self.example)
    }
}

impl Record for CommandInfo {
    const EXPECTED: &'static str = "a command";
    const EXAMPLE: &'static str = "{name: run, purpose: Run a model.}";
    const SCHEMA_NAME: Option<&'static str> = Some("Command");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let name = fields.required("name");
        let purpose = fields.required("purpose");
        let writes = fields.required("writes");
        let long_running = fields.required("long_running");
        let budget = fields.optional("budget");
        let needs_person = fields.required("needs_person");
        let arguments = fields.required("arguments");
        let input_schemas = fields.required("input_schemas");
        let result_schema = fields.required("result_schema");
        let exit_codes = fields.required("exit_codes");
        let example = fields.required("example");
        Some(Self {
            name: name?,
            purpose: purpose?,
            writes: writes?,
            long_running: long_running?,
            budget: budget?,
            needs_person: needs_person?,
            arguments: arguments?,
            input_schemas: input_schemas?,
            result_schema: result_schema?,
            exit_codes: exit_codes?,
            example: example?,
        })
    }
}

/// One line of the table of exit codes.
#[derive(Debug, Clone, PartialEq)]
struct ExitCodeInfo {
    code: u32,
    meaning: String,
}

impl Encode for ExitCodeInfo {
    fn encode(&self) -> Json {
        Json::object([])
            .with("code", &self.code)
            .with("meaning", &self.meaning)
    }
}

impl Record for ExitCodeInfo {
    const EXPECTED: &'static str = "an exit code and its meaning";
    const EXAMPLE: &'static str = "{code: 0, meaning: success}";
    const SCHEMA_NAME: Option<&'static str> = Some("ExitCode");
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let code = fields.required("code");
        let meaning = fields.required("meaning");
        Some(Self {
            code: code?,
            meaning: meaning?,
        })
    }
}

/// The document `taktsim describe` prints.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Describe {
    package_version: String,
    exit_codes: Vec<ExitCodeInfo>,
    error_schema: AnyJson,
    schema: AnyJson,
    commands: Vec<CommandInfo>,
}

impl Encode for Describe {
    fn encode(&self) -> Json {
        Json::object([])
            .with("format", &DESCRIBE_FORMAT)
            .with("package_version", &self.package_version)
            .with("exit_codes", &self.exit_codes)
            .with("error_schema", &self.error_schema)
            .with("schema", &self.schema)
            .with("commands", &self.commands)
    }
}

impl Record for Describe {
    const EXPECTED: &'static str = "the description of the CLI";
    const EXAMPLE: &'static str = "{format: taktsim-describe/1}";
    const DESCRIPTION: Option<&'static str> = Some(
        "Every command of the CLI, its inputs, outputs and errors (ADR-0019). Generated from its types; do not edit.",
    );
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let format = fields.required::<String>("format");
        let package_version = fields.required("package_version");
        let exit_codes = fields.required("exit_codes");
        let error_schema = fields.required("error_schema");
        let schema = fields.required("schema");
        let commands = fields.required("commands");
        format?;
        Some(Self {
            package_version: package_version?,
            exit_codes: exit_codes?,
            error_schema: error_schema?,
            schema: schema?,
            commands: commands?,
        })
    }
}

/// The schema of the document itself.
pub(crate) fn describe_schema() -> Json {
    record_schema::<Describe>().with("title", &DESCRIBE_FORMAT)
}

/// The type word of one argument of the parser.
fn argument_info(arg: &clap::Arg) -> ArgumentInfo {
    let takes_value = arg.get_action().takes_values();
    let value_name = arg
        .get_value_names()
        .and_then(|names| names.first())
        .map(ToString::to_string);
    let positional = arg.is_positional();
    let name = match (arg.get_long(), &value_name, positional) {
        (Some(long), _, _) => format!("--{long}"),
        (None, Some(value), _) => format!("<{}>", value.to_lowercase()),
        (None, None, _) => arg.get_id().to_string(),
    };
    let value_type = if takes_value {
        value_name
            .as_deref()
            .and_then(value_type)
            .map_or("unknown", |t| t.word)
    } else {
        "boolean"
    };
    let default = arg
        .get_default_values()
        .first()
        .map(|d| d.to_string_lossy().into_owned())
        .filter(|_| takes_value);
    ArgumentInfo {
        name,
        help: arg.get_help().map_or_else(String::new, ToString::to_string),
        value_type: value_type.to_owned(),
        required: arg.is_required_set(),
        positional,
        takes_value,
        default,
        allowed: arg
            .get_possible_values()
            .iter()
            .filter(|_| takes_value)
            .map(|value| value.get_name().to_owned())
            .collect(),
    }
}

/// The parser, built, so global flags are on every command.
pub(crate) fn built_parser() -> clap::Command {
    let mut parser = Cli::command();
    parser.build();
    parser
}

/// The description of this CLI.
pub(crate) fn describe() -> Describe {
    let parser = built_parser();
    let known = every_command();
    let commands = parser
        .get_subcommands()
        .filter_map(|sub| {
            let command = known.iter().find(|c| command_name(c) == sub.get_name())?;
            let facts = facts(command);
            Some(CommandInfo {
                name: sub.get_name().to_owned(),
                purpose: sub
                    .get_about()
                    .map_or_else(String::new, ToString::to_string),
                writes: facts.writes.to_owned(),
                long_running: facts.long_running,
                budget: facts.budget.map(AnyJson),
                needs_person: facts.needs_person,
                arguments: sub
                    .get_arguments()
                    .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
                    .map(argument_info)
                    .collect(),
                input_schemas: facts
                    .input_schemas
                    .iter()
                    .map(|(argument, schema_command)| InputSchema {
                        argument: (*argument).to_owned(),
                        schema_command: (*schema_command).to_owned(),
                    })
                    .collect(),
                result_schema: AnyJson(facts.result_schema),
                exit_codes: facts
                    .exits
                    .iter()
                    .map(|(code, types)| CommandExit {
                        code: *code,
                        error_types: types.iter().map(|t| (*t).to_owned()).collect(),
                    })
                    .collect(),
                example: facts.example.to_owned(),
            })
        })
        .collect();
    Describe {
        package_version: env!("CARGO_PKG_VERSION").to_owned(),
        exit_codes: EXIT_CODES
            .iter()
            .map(|(code, meaning)| ExitCodeInfo {
                code: u32::from(*code),
                meaning: (*meaning).to_owned(),
            })
            .collect(),
        error_schema: AnyJson(diagnostic_schema()),
        schema: AnyJson(describe_schema()),
        commands,
    }
}

/// The document as printed and committed.
pub(crate) fn describe_text() -> String {
    let mut text = describe().encode().to_pretty();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use taktsim_model::{Format, decode_str};

    use super::*;

    #[test]
    fn every_command_has_its_facts() {
        let parser = built_parser();
        let known: BTreeSet<&str> = every_command().iter().map(command_name).collect();
        let parsed: BTreeSet<&str> = parser
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        assert_eq!(known, parsed);
        assert_eq!(describe().commands.len(), parsed.len());
    }

    #[test]
    fn every_value_name_has_a_type() {
        for sub in built_parser().get_subcommands() {
            for arg in sub.get_arguments() {
                if !arg.get_action().takes_values() {
                    continue;
                }
                for name in arg.get_value_names().into_iter().flatten() {
                    assert!(
                        value_type(name.as_str()).is_some(),
                        "{} {name}: add it to the table",
                        sub.get_name()
                    );
                }
            }
        }
        assert_eq!(value_type("NO SUCH NAME"), None);
        for info in describe().commands.iter().flat_map(|c| &c.arguments) {
            assert_ne!(info.value_type, "unknown", "{}", info.name);
        }
    }

    #[test]
    fn it_reads_back_as_itself() {
        let text = describe_text();
        let read: Describe = decode_str(&text, Format::Json).unwrap();
        assert_eq!(read, describe());
    }

    /// `run` is long-running from M8 on, and states its budget (ADR-0025 §9); nothing
    /// needs a person.
    #[test]
    fn only_run_is_long_running_and_nothing_needs_a_person() {
        for command in describe().commands {
            assert_eq!(
                command.long_running,
                command.name == "run",
                "{}",
                command.name
            );
            assert_eq!(
                command.budget.is_some(),
                command.long_running,
                "{}",
                command.name
            );
            assert!(!command.needs_person, "{}", command.name);
            assert!(!command.purpose.is_empty(), "{}", command.name);
            assert!(command.example.starts_with("taktsim "), "{}", command.name);
        }
    }
}
