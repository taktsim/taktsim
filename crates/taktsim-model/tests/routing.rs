//! The routing words in the model crate (ADR-0023 §1, §2): what a catalog offers,
//! the normalized form, the hashes and the two closed shapes of an edge. What the
//! words mean is the domain's; `taktsim-components` tests that.

mod support;

use support::{catalog, data, summary};
use taktsim_model::expr::{Type, TypeEnv, condition};
use taktsim_model::{
    Catalog, ChoiceRule, DispatchRule, Format, Json, LoadErrorKind, Loader, Model, RoutingWords,
    model_schema,
};

fn routed() -> Catalog {
    catalog().with_routing(RoutingWords::ALL)
}

#[allow(clippy::panic)] // Test helper: a model that does not load fails the test.
fn load(text: &str) -> Model {
    let catalog = routed();
    match Loader::new(&catalog).load_str(text, Format::Yaml) {
        Ok(model) => model,
        Err(errors) => panic!("{text}\n{errors}"),
    }
}

fn errors(text: &str, catalog: &Catalog) -> Vec<String> {
    match Loader::new(catalog).load_str(text, Format::Yaml) {
        Ok(_) => vec!["(loaded)".to_owned()],
        Err(errors) => summary(&errors),
    }
}

#[allow(clippy::unwrap_used)] // Test helper: the fixture is in the repository.
fn fixture() -> String {
    std::fs::read_to_string(data("routing").join("m6-words.yaml")).unwrap()
}

/// A model with the given edges, on stations that accept anything.
fn with_flow(flow: &str) -> String {
    format!(
        "format: \"1.0\"\nmodel: {{name: m}}\nexperiment: {{type: terminating, horizon: 1h}}\n\
         components:\n  - {{id: a, type: Machine, cycle_time: 1min}}\n  \
         - {{id: b, type: Machine, cycle_time: 1min}}\n  - {{id: c, type: Machine, cycle_time: 1min}}\n\
         flow:\n{flow}\n"
    )
}

#[test]
fn every_word_loads_and_the_canonical_form_is_a_model_with_the_same_hash() {
    let model = load(&fixture());
    let document = model.document();
    assert_eq!(document.flow()[1].choose(), Some(ChoiceRule::ShortestQueue));
    assert_eq!(
        document.flow()[3]
            .when()
            .map(ToString::to_string)
            .as_deref(),
        Some("variant == 'premium'")
    );
    // Targets of a split, and its shares, in the order of their ids.
    let split = document.flow()[4].split().unwrap();
    assert_eq!(split[0].0.id(), "cut");
    assert!((split[1].1.as_percent() - 95.0).abs() < 1e-12);
    let to: Vec<&str> = document.flow()[4]
        .to()
        .iter()
        .map(taktsim_model::ComponentRef::id)
        .collect();
    assert_eq!(to, ["cut", "pack"]);
    assert_eq!(split[0].0.path().to_string(), "flow[4].split.cut");
    // Rules in the order of their stations.
    let rules: Vec<(&str, DispatchRule)> = document
        .control()
        .dispatch()
        .iter()
        .map(|(r, rule)| (r.id(), *rule))
        .collect();
    assert_eq!(
        rules,
        [
            ("weld-1", DispatchRule::MinimumSetup),
            ("weld-2", DispatchRule::EarliestDueDate)
        ]
    );
    assert!(
        model
            .canonical_json()
            .contains(r#"{"from":["qc"],"split":{"cut":"5%","pack":"95%"}}"#)
    );
    assert!(
        model
            .canonical_json()
            .contains(r#"{"from":["qc"],"to":["paint"],"when":"variant == 'premium'"}"#)
    );
    assert!(model.canonical_json().contains(
        r#""control":{"dispatch":{"weld-1":"minimum_setup","weld-2":"earliest_due_date"}}"#
    ));
    let catalog = routed();
    let again = Loader::new(&catalog)
        .load_str(model.canonical_json(), Format::Json)
        .unwrap();
    assert_eq!(again.hash(), model.hash());
    assert_eq!(again.system_hash(), model.system_hash());
    assert_eq!(again.canonical_json(), model.canonical_json());
}

#[test]
fn when_spelling_changes_no_hash() {
    let base = load(&fixture());
    let text = fixture();
    for spelled in [
        "variant=='premium'",
        "variant   ==  'premium'",
        "(variant == 'premium')",
        "((variant) == ('premium'))",
    ] {
        let other = load(&text.replace("variant == 'premium'", spelled));
        assert_eq!(other.hash(), base.hash(), "{spelled}");
        assert_eq!(other.system_hash(), base.system_hash(), "{spelled}");
    }
    // Another condition is another model.
    let other = load(&text.replace("'premium'", "'basic'"));
    assert_ne!(other.system_hash(), base.system_hash());
}

#[test]
fn reordering_split_keys_and_dispatch_keys_changes_no_hash() {
    let base = load(&fixture());
    let text = fixture();
    let other = load(
        &text
            .replace("{pack: 95%, cut: 5%}", "{cut: 5.0%, pack: 95%}")
            .replace(
                "{weld-2: earliest_due_date, weld-1: minimum_setup}",
                "{weld-1: minimum_setup, weld-2: earliest_due_date}",
            ),
    );
    assert_eq!(other.hash(), base.hash());
}

#[test]
fn system_hash_changes_with_dispatch() {
    let text = fixture();
    let base = load(&text);
    let other = load(&text.replace("weld-2: earliest_due_date", "weld-2: fifo"));
    assert_ne!(other.hash(), base.hash());
    assert_ne!(other.system_hash(), base.system_hash());
    // No dispatch at all: another system again, and the same as an empty section,
    // which is left out.
    let without = text.split("control:").next().unwrap();
    let none = load(without);
    assert_ne!(none.system_hash(), base.system_hash());
    let empty = load(&format!("{without}control: {{dispatch: {{}}}}\n"));
    assert_eq!(empty.hash(), none.hash());
    assert_eq!(empty.system_hash(), none.system_hash());
    assert!(!none.canonical_json().contains("control"));
}

#[test]
fn a_written_first_free_stays_written() {
    let plain = load(&with_flow("  - [a, b]\n  - {from: b, to: [c, a]}"));
    let written = load(&with_flow(
        "  - [a, b]\n  - {from: b, to: [c, a], choose: first_free}",
    ));
    assert_ne!(plain.hash(), written.hash());
    assert_ne!(plain.system_hash(), written.system_hash());
    assert!(
        written
            .canonical_json()
            .contains(r#""choose":"first_free""#)
    );
    assert_eq!(plain.document().flow()[1].choose(), None);
}

#[test]
fn the_spec_example_keeps_its_hashes_with_every_word_offered() {
    let text = std::fs::read_to_string(data("valid").join("spec-line-a.yaml")).unwrap();
    let with = load(&text);
    let catalog = catalog();
    let without = Loader::new(&catalog).load_str(&text, Format::Yaml).unwrap();
    assert_eq!(with.hash(), without.hash());
    assert_eq!(with.system_hash(), without.system_hash());
    assert_eq!(
        with.hash().to_string(),
        "56d63bb349f4eef8da1e437c48f4e01206eaee6315c97c5913637f56ff462dae"
    );
    assert_eq!(
        with.system_hash().to_string(),
        "18c3cbaad4e836fd4c29a17ba0a88ff553dfb516a69886db10ed2efb09d89904"
    );
}

#[test]
fn routing_words_need_a_catalog_that_offers_them() {
    let text = fixture();
    let plain = catalog();
    assert_eq!(
        errors(&text, &plain),
        [
            "flow[1].choose @ 18:39 UnknownField",
            "flow[3].when @ 20:27 UnknownField",
            "flow[4].to @ 21:5 MissingField",
            "flow[4].split @ 21:16 UnknownField",
            "control @ 23:1 UnknownField",
        ]
    );
    // Each flag alone opens its own word.
    let only = |words: RoutingWords| catalog().with_routing(words);
    let e = errors(
        &text,
        &only(RoutingWords {
            when: true,
            ..RoutingWords::NONE
        }),
    );
    assert!(!e.iter().any(|l| l.starts_with("flow[3].when")), "{e:?}");
    assert!(e.iter().any(|l| l.starts_with("flow[1].choose")), "{e:?}");
    let e = errors(
        &text,
        &only(RoutingWords {
            choose: true,
            ..RoutingWords::NONE
        }),
    );
    assert!(!e.iter().any(|l| l.starts_with("flow[1].choose")), "{e:?}");
    assert!(e.iter().any(|l| l.starts_with("flow[3].when")), "{e:?}");
    let e = errors(
        &text,
        &only(RoutingWords {
            split: true,
            ..RoutingWords::NONE
        }),
    );
    assert!(!e.iter().any(|l| l.starts_with("flow[4]")), "{e:?}");
    assert!(e.iter().any(|l| l.starts_with("control")), "{e:?}");
    let e = errors(
        &text,
        &only(RoutingWords {
            dispatch: true,
            ..RoutingWords::NONE
        }),
    );
    assert!(!e.iter().any(|l| l.starts_with("control")), "{e:?}");
    assert!(e.iter().any(|l| l.starts_with("flow[3].when")), "{e:?}");
    assert_eq!(errors(&text, &routed()), ["(loaded)"]);
}

#[test]
fn a_catalog_without_routing_does_not_offer_it() {
    let schema = model_schema(&catalog());
    assert_eq!(schema["properties"]["control"], Json::Null);
    let entry = schema["$defs"]["FlowEntry"].to_string();
    for word in ["when", "choose", "split"] {
        assert!(!entry.contains(&format!("\"{word}\"")), "{word}: {entry}");
    }
    for name in ["ChoiceRule", "DispatchRule", "Control"] {
        assert_eq!(schema["$defs"][name], Json::Null, "{name}");
    }
    // With every word, an edge has exactly two closed shapes.
    let schema = model_schema(&routed());
    assert_eq!(schema["properties"]["control"]["$ref"], "#/$defs/Control");
    let forms = &schema["$defs"]["FlowEntry"]["oneOf"][1]["oneOf"];
    let shape = |form: &Json| {
        let keys: Vec<&str> = form["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(form["additionalProperties"], false);
        keys.join(" ")
    };
    assert_eq!(shape(&forms[0]), "choose from to when");
    assert_eq!(shape(&forms[1]), "from split when");
}

#[test]
fn split_with_to_or_choose_is_an_error_at_that_key() {
    let e = errors(
        &with_flow("  - {from: a, to: b, split: {b: 50%, c: 50%}}"),
        &routed(),
    );
    assert_eq!(e, ["flow[0].to @ 9:19 NotAllowed"]);
    let e = errors(
        &with_flow("  - {from: a, split: {b: 50%, c: 50%}, choose: preferred}"),
        &routed(),
    );
    assert_eq!(e, ["flow[0].choose @ 9:48 NotAllowed"]);
    let e = errors(&with_flow("  - {from: a}"), &routed());
    assert_eq!(e, ["flow[0].to @ 9:5 MissingField"]);
    let e = errors(&with_flow("  - {from: a, split: [b, c]}"), &routed());
    assert_eq!(e, ["flow[0].split @ 9:22 WrongType"]);
    let e = errors(
        &with_flow("  - {from: a, split: {\"b b\": 50%, c: 50%}}"),
        &routed(),
    );
    assert_eq!(e, ["flow[0].split[\"b b\"] @ 9:23 InvalidId"]);
    let e = errors(
        &with_flow("  - {from: a, split: {b: 0.5, c: 50%}}"),
        &routed(),
    );
    assert_eq!(e, ["flow[0].split.b @ 9:26 BareShare"]);
    let e = errors(
        &with_flow("  - {from: a, split: {x: 50%, c: 50%}}"),
        &routed(),
    );
    assert_eq!(e, ["flow[0].split.x @ 9:23 UnknownComponent"]);
    // A key is suggested among the words of its form.
    let catalog = routed();
    let errors = Loader::new(&catalog)
        .load_str(&with_flow("  - {from: a, to: b, wen: x}"), Format::Yaml)
        .unwrap_err();
    let LoadErrorKind::UnknownField {
        allowed,
        suggestion,
        ..
    } = errors.first().kind()
    else {
        panic!("{errors}");
    };
    assert_eq!(allowed, &["from", "to", "when", "choose", "split"]);
    assert_eq!(suggestion.as_deref(), Some("when"));
}

#[test]
fn a_repeated_edge_needs_a_condition_on_the_earlier_one() {
    let first_match = "  - {from: a, to: b, when: \"variant == 'x'\"}\n  - {from: a, to: c, when: \"variant == 'y'\"}\n  - {from: a, to: b}";
    assert_eq!(errors(&with_flow(first_match), &routed()), ["(loaded)"]);
    // Without a condition on the earlier one, as ever.
    let e = errors(
        &with_flow("  - {from: a, to: b}\n  - {from: a, to: b}"),
        &routed(),
    );
    assert_eq!(e, ["flow[1].to @ 10:19 DuplicateEdge"]);
    let e = errors(
        &with_flow(
            "  - {from: a, to: b, when: \"variant == 'x'\"}\n  - {from: a, to: b}\n  - {from: a, to: b}",
        ),
        &routed(),
    );
    assert_eq!(e, ["flow[2].to @ 11:19 DuplicateEdge"]);
}

#[test]
fn a_condition_is_parsed_with_its_path_and_a_limit() {
    let e = errors(
        &with_flow("  - {from: a, to: b, when: \"variant == \"}"),
        &routed(),
    );
    assert_eq!(e, ["flow[0].when @ 9:39 Expression"]);
    let long = "x".repeat(4097);
    let e = errors(
        &with_flow(&format!("  - {{from: a, to: b, when: \"{long}\"}}")),
        &routed(),
    );
    assert_eq!(e, ["flow[0].when @ 9:28 NotAllowed"]);
    let fits = format!("{}{}", " ".repeat(4096 - 3), "x>1");
    assert_eq!(fits.len(), 4096);
    let e = errors(
        &with_flow(&format!("  - {{from: a, to: b, when: \"{fits}\"}}")),
        &routed(),
    );
    assert_eq!(e, ["(loaded)"]);
    // It must be text at all.
    let e = errors(&with_flow("  - {from: a, to: b, when: 1}"), &routed());
    assert_eq!(e, ["flow[0].when @ 9:28 WrongType"]);
}

#[test]
fn control_reads_only_what_it_may() {
    let base = with_flow("  - [a, b]\n  - [b, c]");
    let with_control = |control: &str| format!("{base}control: {control}\n");
    assert_eq!(
        errors(&with_control("{dispatch: {nobody: fifo}}"), &routed()),
        ["control.dispatch.nobody @ 11:22 UnknownComponent"]
    );
    assert_eq!(
        errors(&with_control("{dispatch: {a: oldest}}"), &routed()),
        ["control.dispatch.a @ 11:25 UnknownValue"]
    );
    assert_eq!(
        errors(&with_control("{dispatch: [a, fifo]}"), &routed()),
        ["control.dispatch @ 11:21 WrongType"]
    );
    // `release` waits for v0.2.
    assert_eq!(
        errors(&with_control("{release: {policy: conwip}}"), &routed()),
        ["control.release @ 11:11 UnknownField"]
    );
    assert_eq!(
        errors(&with_control("fifo"), &routed()),
        ["control @ 11:10 WrongType"]
    );
    assert_eq!(
        errors(&with_control("{dispatch: {a: fifo}}"), &routed()),
        ["(loaded)"]
    );
}

#[test]
fn control_is_in_the_model_file_only() {
    let files = std::collections::BTreeMap::from([(
        "more.yaml".to_owned(),
        "control: {dispatch: {a: fifo}}\n".to_owned(),
    )]);
    let catalog = routed();
    let text = format!(
        "{}include: [more.yaml]\n",
        with_flow("  - [a, b]\n  - [b, c]")
    );
    let errors = Loader::new(&catalog)
        .with_resolver(&files)
        .load_str(&text, Format::Yaml)
        .unwrap_err();
    assert_eq!(errors.first().path().to_string(), "control");
    assert!(matches!(
        errors.first().kind(),
        LoadErrorKind::UnknownField { .. }
    ));
}

#[test]
fn every_word_of_a_rule_reads_back() {
    for rule in ChoiceRule::ALL {
        let read: ChoiceRule = taktsim_model::decode_str(rule.as_str(), Format::Yaml).unwrap();
        assert_eq!(read, rule);
    }
    for rule in DispatchRule::ALL {
        let read: DispatchRule = taktsim_model::decode_str(rule.as_str(), Format::Yaml).unwrap();
        assert_eq!(read, rule);
    }
    let words: Vec<&str> = ChoiceRule::ALL.iter().map(|r| r.as_str()).collect();
    assert_eq!(
        words,
        [
            "first_free",
            "shortest_queue",
            "lowest_utilization",
            "preferred"
        ]
    );
    let words: Vec<&str> = DispatchRule::ALL.iter().map(|r| r.as_str()).collect();
    assert_eq!(
        words,
        [
            "fifo",
            "earliest_due_date",
            "shortest_processing_time",
            "minimum_setup",
            "priority_class"
        ]
    );
}

/// The environment of a `when` (ADR-0023 §2), through every production a
/// condition on a part can use. `expr_syntax.rs` and `expr_types.rs` test the
/// language; these close what they leave to this context: `variant` as an enum
/// with the model's names and `priority` as an int, bare.
#[test]
fn conditions_over_variant_and_priority() {
    let env = TypeEnv::new()
        .with_enum("variant", ["basic", "premium"])
        .unwrap()
        .with_path("variant", Type::Enum("variant".to_owned()))
        .unwrap()
        .with_path("priority", Type::Int)
        .unwrap();
    let unit = |variant: &'static str, priority: i64| {
        std::collections::BTreeMap::from([
            (
                "variant".to_owned(),
                taktsim_model::expr::Value::Text(variant),
            ),
            (
                "priority".to_owned(),
                taktsim_model::expr::Value::Int(priority),
            ),
        ])
    };
    let (premium, basic) = (unit("premium", 2), unit("basic", 0));
    for (text, on_premium, on_basic) in [
        ("variant == 'premium'", true, false),
        ("variant != 'premium'", false, true),
        ("variant in ['premium', 'basic']", true, true),
        ("not (variant in ['premium'])", false, true),
        ("priority > 0", true, false),
        ("priority >= 0", true, true),
        ("priority < 1", false, true),
        ("priority <= 2", true, true),
        ("priority == 2", true, false),
        ("priority in [0, 1]", false, true),
        ("priority + 1 > 2", true, false),
        ("priority * 2 == 4", true, false),
        ("-priority < 0", true, false),
        ("variant == 'premium' and priority > 1", true, false),
        ("variant == 'basic' or priority > 1", true, true),
        ("not (variant == 'premium')", false, true),
        ("true", true, true),
    ] {
        let cond = condition(text, &env).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(cond.eval(&premium).unwrap(), on_premium, "{text}");
        assert_eq!(cond.eval(&basic).unwrap(), on_basic, "{text}");
    }
    for bad in [
        "variant",               // not a bool
        "priority",              // not a bool
        "variant == 'premum'",   // not a declared name
        "variant == 1",          // another type
        "priority == 'premium'", // another type
        "priority > 0.5",        // a float compared with an int
        "quality == 'ok'",       // no such path
        "throughput(sink) > 1",  // no observables
        "unit.priority > 1",     // the bare names only
        "variant < 'premium'",   // an enum has no order
    ] {
        assert!(condition(bad, &env).is_err(), "{bad}");
    }
}
