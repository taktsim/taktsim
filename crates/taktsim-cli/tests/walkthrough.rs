//! The scripted walk-through of ADR-0019 answer 6: an agent that knows only the
//! binary's name. Every command and flag it uses comes from `describe`'s output,
//! and the test fails if it needs a word not printed there.

mod support;

use support::Dir;
use taktsim_model::Json;

/// What `describe` printed, and nothing else.
struct Manual(Json);

impl Manual {
    #[allow(clippy::unwrap_used, clippy::panic)] // Test helper: an unknown word fails the test.
    fn command(&self, purpose_word: &str) -> &Json {
        self.0["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["purpose"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
                    .contains(purpose_word)
            })
            .unwrap_or_else(|| panic!("no command says `{purpose_word}`"))
    }
}

/// The word of an argument of `command`, whose help says `help_word`.
#[allow(clippy::unwrap_used, clippy::panic)] // Test helper: an unknown word fails the test.
fn flag<'a>(command: &'a Json, help_word: &str) -> &'a str {
    command["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| {
            a["help"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains(help_word)
        })
        .and_then(|a| a["name"].as_str())
        .unwrap_or_else(|| panic!("no flag says `{help_word}`"))
}

/// The JSON format, as an agent reads it from a flag's allowed values.
#[allow(clippy::unwrap_used)] // Test helper.
fn json_format(command: &Json) -> [&str; 2] {
    let format = command["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["allowed"].as_array().is_some_and(|v| !v.is_empty()))
        .unwrap();
    let json = format["allowed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| **v == "json")
        .and_then(Json::as_str)
        .unwrap();
    [format["name"].as_str().unwrap(), json]
}

/// It wants to know what a model looks like: `validate` names the schema's command.
#[allow(clippy::unwrap_used)] // Test helper.
fn learns_the_model_format(dir: &Dir, manual: &Manual) {
    let schema = manual.command("json schema");
    let schema_name = schema["name"].as_str().unwrap();
    let validate = manual.command("check a model");
    let reference = validate["input_schemas"][0]["schema_command"]
        .as_str()
        .unwrap();
    let mut words = reference.split_whitespace().skip(1);
    assert_eq!(
        words.next(),
        Some(schema_name),
        "the reference is that command"
    );
    let what = words.next().unwrap();
    let model_schema = dir.taktsim(&[schema_name, what]).json();
    assert_eq!(model_schema["properties"]["format"]["const"], "1.0");
    // And what each component takes: the names are in the schema.
    let machine = dir.taktsim(&[schema_name, "Machine"]).json();
    assert!(machine["properties"]["cycle_time"].as_object().is_some());
}

/// It writes a model and validates it, then has a broken one, and reads what to fix
/// from the error.
#[allow(clippy::unwrap_used)] // Test helper.
fn validates_and_fixes(dir: &Dir, manual: &Manual) {
    dir.write("line.yaml", support::LINE);
    let validate = manual.command("check a model");
    let name = validate["name"].as_str().unwrap();
    let [format_flag, json] = json_format(validate);
    let checked = dir.taktsim(&[name, "line.yaml", format_flag, json]);
    assert_eq!(checked.code, 0, "{}", checked.stderr);
    assert_eq!(checked.json()["valid"], true);

    dir.write(
        "broken.yaml",
        &support::LINE.replace("cycle_time: 45s", "cycle_time: 45"),
    );
    let refused = dir.taktsim(&[name, "broken.yaml", format_flag, json]);
    assert_eq!(refused.code, 1);
    let error = &refused.lines()[0];
    assert_eq!(error["path"], "components[3].cycle_time");
    let fix = error["example"].as_str().unwrap();
    dir.write(
        "broken.yaml",
        &support::LINE.replace("cycle_time: 45s", &format!("cycle_time: {fix}")),
    );
    let fixed = dir.taktsim(&[name, "broken.yaml", format_flag, json]);
    assert_eq!(
        fixed.code, 0,
        "the example is a valid value: {}",
        fixed.stderr
    );
}

/// It runs the model with flags from their help, bounds a run, and tells a partial
/// result from a complete one by `complete`.
#[allow(clippy::unwrap_used)] // Test helper.
fn runs_and_reads_the_result(dir: &Dir, manual: &Manual) {
    let run = manual.command("run a model");
    assert_eq!(run["writes"], "a run in the run store");
    assert_eq!(run["long_running"], false);
    assert_eq!(run["needs_person"], false);
    let name = run["name"].as_str().unwrap();
    let [format_flag, json] = json_format(run);
    let (seed, horizon) = (flag(run, "seed"), flag(run, "horizon"));
    let base = [
        name,
        "line.yaml",
        seed,
        "42",
        horizon,
        "1h",
        format_flag,
        json,
    ];
    let ran = dir.taktsim(&base);
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let result = ran.json();
    assert_eq!(result["complete"], true, "`complete` is what it decides by");
    assert_eq!(
        run["result_schema"]["properties"]["format"]["const"], result["format"],
        "it is the document `describe` promised"
    );

    let (limit, again) = (flag(run, "events"), flag(run, "compare"));
    let mut bounded = base.to_vec();
    bounded.extend([limit, "3", again]);
    let partial = dir.taktsim(&bounded);
    let documented: Vec<&Json> = run["exit_codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| &c["code"])
        .collect();
    let code = Json::from(u64::try_from(partial.code).unwrap());
    assert!(documented.contains(&&code), "{code}");
    assert_eq!(partial.json()["complete"], false);

    // Running the same thing again reuses it, and an example just works.
    let reused = dir.taktsim(&base);
    assert_eq!(reused.stdout, ran.stdout);
    assert_eq!(reused.lines()[0]["reused"], true);
    let example: Vec<&str> = run["example"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .skip(1)
        .collect();
    assert_eq!(dir.taktsim(&example).code, 0);
}

#[test]
fn walkthrough_uses_only_describe() {
    let dir = Dir::new();
    // All an agent knows.
    let described = dir.taktsim(&["describe"]);
    assert_eq!(described.code, 0);
    let manual = Manual(described.json());
    learns_the_model_format(&dir, &manual);
    validates_and_fixes(&dir, &manual);
    runs_and_reads_the_result(&dir, &manual);
}
