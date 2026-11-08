//! Argument errors go through the same typed line as every other error, so a
//! misspelt flag is also a path-level JSON error, and exit 1, not the parser's 2
//! (ADR-0019 item 3, ADR-0024 §9).

use std::ffi::OsString;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use taktsim_run::Diagnostic;

use crate::args::Format;
use crate::describe::{built_parser, value_type};
use crate::render;

/// The format the raw arguments ask for: a failed parse has no value.
pub(crate) fn format_in(args: &[OsString]) -> Format {
    let mut format = Format::Text;
    let mut words = args.iter().filter_map(|word| word.to_str());
    while let Some(word) = words.next() {
        let value = if word == "--format" {
            words.next()
        } else {
            word.strip_prefix("--format=")
        };
        match value {
            Some("json") => format = Format::Json,
            Some("text") => format = Format::Text,
            Some(_) | None => {}
        }
    }
    format
}

/// What the parser said about `kind`, as text.
fn context(error: &clap::Error, kind: ContextKind) -> Vec<String> {
    match error.get(kind) {
        Some(ContextValue::String(text)) => vec![text.clone()],
        Some(ContextValue::Strings(texts)) => texts.clone(),
        Some(_) | None => Vec::new(),
    }
}

/// The subcommand the raw arguments name, if they name one.
fn subcommand_in(args: &[OsString]) -> Option<clap::Command> {
    let parser = built_parser();
    let words: Vec<&str> = args.iter().filter_map(|w| w.to_str()).collect();
    words
        .iter()
        .find_map(|word| parser.find_subcommand(word).cloned())
}

/// The flags of a command, for what is allowed.
fn flags_of(command: &clap::Command) -> Vec<String> {
    command
        .get_arguments()
        .filter_map(clap::Arg::get_long)
        .filter(|long| !matches!(*long, "help" | "version"))
        .map(|long| format!("--{long}"))
        .collect()
}

/// `--horizon <DURATION>` as `--horizon`, `<MODEL>` as `<model>`.
fn path_of(argument: &str) -> String {
    let word = argument.split_whitespace().next().unwrap_or(argument);
    if word.starts_with('-') {
        word.to_owned()
    } else {
        word.to_lowercase()
    }
}

/// `--horizon <DURATION>` as `DURATION`.
fn value_name_of(argument: &str) -> Option<&str> {
    let start = argument.find('<')? + 1;
    let end = argument[start..].find('>')? + start;
    Some(&argument[start..end])
}

/// An invocation that works, for an example.
fn example_for(command: Option<&clap::Command>) -> String {
    let name = command.map_or("describe", clap::Command::get_name);
    match name {
        "run" => "taktsim run line.yaml --seed 42".to_owned(),
        "validate" => "taktsim validate line.yaml".to_owned(),
        "schema" => "taktsim schema Machine".to_owned(),
        other => format!("taktsim {other}"),
    }
}

/// The commands, for what is allowed.
fn commands() -> String {
    built_parser()
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The typed line of a parse error.
fn diagnostic(error: &clap::Error, args: &[OsString]) -> Diagnostic {
    let kind = error.kind();
    let command = subcommand_in(args);
    let argument = context(error, ContextKind::InvalidArg);
    let argument = argument.first().map(String::as_str);
    let value = context(error, ContextKind::InvalidValue);
    let value = value.first().map(String::as_str);
    let valid = context(error, ContextKind::ValidValue);
    let example = example_for(command.as_ref());
    let no_command = kind == ErrorKind::MissingSubcommand
        || kind == ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand;
    if kind == ErrorKind::InvalidSubcommand || no_command {
        let found = context(error, ContextKind::InvalidSubcommand);
        let found = found.first().map_or("nothing", String::as_str);
        let (type_name, message) = if kind == ErrorKind::InvalidSubcommand {
            ("args.unknown_command", format!("no command `{found}`"))
        } else {
            ("args.missing_command", "a command is missing".to_owned())
        };
        return Diagnostic::error(type_name, message)
            .at("<command>")
            .found(found)
            .allowed(commands())
            .example("taktsim describe");
    }
    if kind == ErrorKind::UnknownArgument {
        let found = argument.unwrap_or("an unknown argument");
        let allowed = command.as_ref().map_or_else(
            || "--help, --version, --format".to_owned(),
            |c| flags_of(c).join(", "),
        );
        return Diagnostic::error("args.unknown_argument", format!("no argument `{found}`"))
            .at(path_of(found))
            .found(found)
            .allowed(allowed)
            .example(example);
    }
    if kind == ErrorKind::MissingRequiredArgument {
        let missing = argument.unwrap_or("<argument>");
        let name = value_name_of(missing).unwrap_or(missing);
        let allowed = value_type(name).map_or("a value", |t| t.allowed);
        return Diagnostic::error(
            "args.missing_argument",
            format!("the required argument {} is missing", path_of(missing)),
        )
        .at(path_of(missing))
        .found("nothing")
        .allowed(allowed)
        .example(example);
    }
    if kind == ErrorKind::ArgumentConflict {
        let repeated = argument.unwrap_or("an argument");
        return Diagnostic::error(
            "args.repeated",
            format!("{} can be given once", path_of(repeated)),
        )
        .at(path_of(repeated))
        .found("more than one value")
        .allowed("one value")
        .example(example);
    }
    if kind == ErrorKind::InvalidValue || kind == ErrorKind::ValueValidation {
        let named = argument.unwrap_or("<argument>");
        let found = value.filter(|v| !v.is_empty()).unwrap_or("nothing");
        let promise = value_name_of(named).and_then(value_type);
        let allowed = if valid.is_empty() {
            promise.map_or_else(|| "another value".to_owned(), |t| t.allowed.to_owned())
        } else {
            valid.join(", ")
        };
        let good = valid
            .first()
            .map(String::as_str)
            .or_else(|| promise.map(|t| t.example))
            .unwrap_or("1");
        let message = if found == "nothing" {
            format!("{} needs a value", path_of(named))
        } else {
            format!("`{found}` is not a value of {}", path_of(named))
        };
        return Diagnostic::error("args.invalid_value", message)
            .at(path_of(named))
            .found(found)
            .allowed(allowed)
            .example(good);
    }
    Diagnostic::error("args.invalid", "the arguments cannot be read")
        .at(argument.map_or_else(|| "<arguments>".to_owned(), path_of))
        .found(argument.or(value).unwrap_or("these arguments"))
        .allowed("what `taktsim describe` lists")
        .example(example)
}

/// `--version` takes nothing beside it; a flag or a command after it is a mistake
/// the parser's early exit would otherwise hide.
fn version_is_alone(args: &[OsString]) -> bool {
    args.len() == 2
}

/// Prints what the parser has to say, and returns the exit code.
pub(crate) fn handle(error: &clap::Error, args: &[OsString], format: Format) -> u8 {
    let kind = error.kind();
    if kind == ErrorKind::DisplayHelp {
        return u8::from(error.print().is_err()) * 7;
    }
    if kind == ErrorKind::DisplayVersion {
        if version_is_alone(args) {
            return u8::from(error.print().is_err()) * 7;
        }
        let line = Diagnostic::error("args.unknown_argument", "`--version` stands alone")
            .at("--version")
            .found(
                args.iter()
                    .skip(1)
                    .map(|word| word.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
            .allowed("`--version` with nothing beside it")
            .example("taktsim --version");
        crate::write_stderr(&render::line(&line, format));
        return 1;
    }
    crate::write_stderr(&render::line(&diagnostic(error, args), format));
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_format_comes_from_the_raw_arguments() {
        assert_eq!(format_in(&words(&["taktsim"])), Format::Text);
        assert_eq!(
            format_in(&words(&["taktsim", "--format", "json"])),
            Format::Json
        );
        assert_eq!(
            format_in(&words(&["taktsim", "--format=json", "bad"])),
            Format::Json
        );
        assert_eq!(
            format_in(&words(&["taktsim", "--format", "yaml"])),
            Format::Text
        );
        assert_eq!(
            format_in(&words(&["taktsim", "--format", "json", "--format", "text"])),
            Format::Text
        );
        assert_eq!(format_in(&words(&["taktsim", "--format"])), Format::Text);
    }

    #[test]
    fn a_path_is_the_flag_or_the_argument() {
        assert_eq!(path_of("--horizon <DURATION>"), "--horizon");
        assert_eq!(path_of("<MODEL>"), "<model>");
        assert_eq!(value_name_of("--horizon <DURATION>"), Some("DURATION"));
        assert_eq!(value_name_of("--no-reuse"), None);
    }
}
