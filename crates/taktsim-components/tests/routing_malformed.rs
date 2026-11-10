//! The corpus of malformed routed models (ADR-0023 §1, §8, §13): every file in
//! `data/routing_malformed/` fails to load with exactly the errors its `# expect:`
//! lines list, in order, and each error has a field path, what was found, what is
//! allowed and an example, as far as its kind says them.

mod support;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use support::{document, load, registry, try_load};
use taktsim_components::{BuildError, ComponentType, Flow, FlowMessage, FlowShared, Ports, Wiring};
use taktsim_core::{Component, ComponentError, Context, Delivery};
use taktsim_model::{
    ComponentParams, Encode, Fields, Format, Json, LoadErrorKind, LoadErrors, Loader,
};

/// Each error as `path @ line:column Kind`.
fn summary(errors: &LoadErrors) -> Vec<String> {
    errors
        .iter()
        .map(|e| {
            let kind = format!("{:?}", e.kind());
            let name = kind
                .split(|c: char| !c.is_ascii_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_owned();
            let at = e
                .location()
                .map_or_else(|| "-".to_owned(), |l| l.to_string());
            format!("{} @ {at} {name}", e.path())
        })
        .collect()
}

#[allow(clippy::unwrap_used)] // Test helper: the corpus is in the repository.
fn corpus() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("routing_malformed");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_malformed_routed_model_fails_with_the_expected_errors() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    let paths = corpus();
    assert_eq!(paths.len(), 41);
    let mut failures = Vec::new();
    for path in &paths {
        let text = std::fs::read_to_string(path).unwrap();
        let expected: Vec<&str> = text
            .lines()
            .filter_map(|l| l.strip_prefix("# expect: "))
            .collect();
        let actual = match loader.load_file(path) {
            Ok(_) => vec!["(loaded)".to_owned()],
            Err(errors) => summary(&errors),
        };
        if expected.is_empty() || actual != expected {
            let name = path.file_name().unwrap().to_string_lossy();
            let lines: Vec<String> = actual.iter().map(|l| format!("# expect: {l}")).collect();
            failures.push(format!("{name}:\n{}", lines.join("\n")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_rule_says_what_was_found_what_is_allowed_and_an_example() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    for path in corpus() {
        let errors = loader.load_file(&path).unwrap_err();
        for e in errors.iter() {
            let at = path.display();
            assert!(!e.path().to_string().is_empty(), "{at}: {e}");
            if let LoadErrorKind::NotAllowed {
                found,
                allowed,
                example,
            } = e.kind()
            {
                assert!(!found.is_empty() && !allowed.is_empty(), "{at}: {e}");
                assert!(!example.is_empty(), "{at}: {e}");
            }
            // What a person or an agent reads is one line, with single spaces.
            let kind = e.kind();
            for text in [kind.found(), kind.allowed(), kind.example()]
                .into_iter()
                .flatten()
            {
                assert!(
                    !text.contains("  ") && !text.contains('\n'),
                    "{at}: {text:?}"
                );
            }
        }
    }
}

#[test]
fn a_loop_back_to_a_buffer_names_the_fact_and_the_fix() {
    let registry = registry();
    let loader = Loader::new(registry.catalog());
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("routing_malformed")
        .join("loop-return-to-a-buffer.yaml");
    let errors = loader.load_file(&path).unwrap_err();
    let kind = errors.first().kind();
    let found = kind.found().unwrap();
    assert!(
        found.ends_with("whose type `Buffer` does not state `takes_returns`"),
        "{found:?}"
    );
    let allowed = kind.allowed().unwrap();
    assert!(
        allowed.starts_with(
            "a return target with a return lane; for a Buffer, send the way back to the \
             machine after the buffer"
        ),
        "{allowed:?}"
    );
}

const TWO_SINKS: &str = "  - {id: source, type: Source, interarrival: 1min}\n  \
                         - {id: a, type: Sink}\n  - {id: b, type: Sink}";

/// M5's rule stands for two unconditional edges, in every spelling (ADR-0023 §1).
#[test]
fn second_edge_from_a_component_is_rejected() {
    for flow in [
        "  - [source, a]\n  - [source, b]",
        "  - {from: source, to: a}\n  - {from: source, to: b}",
        "  - {from: source, to: a}\n  - [source, b]",
    ] {
        let errors = try_load(&document("1h", TWO_SINKS, flow)).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors}");
        assert_eq!(
            errors.first().kind().example().as_deref(),
            Some("{from: cut, to: [weld-1, weld-2]}"),
            "{errors}"
        );
    }
    // A second edge after a condition is first-match.
    load(&document(
        "1h",
        TWO_SINKS,
        "  - {from: source, to: a, when: \"priority > 0\"}\n  - {from: source, to: b}",
    ));
}

#[test]
fn no_edge_after_an_unconditional_one() {
    let errors = try_load(&document(
        "1h",
        TWO_SINKS,
        "  - {from: source, to: a}\n  - {from: source, to: b, when: \"priority > 0\"}",
    ))
    .unwrap_err();
    // The second can never be used: that is the error, whatever it says.
    assert_eq!(errors.len(), 1, "{errors}");
    assert_eq!(errors.first().path().to_string(), "flow[1].from");
}

#[test]
fn a_repeat_after_a_condition_is_how_to_write_first_match() {
    load(&document(
        "1h",
        TWO_SINKS,
        "  - {from: source, to: a, when: \"priority > 1\"}\n  \
         - {from: source, to: b, when: \"priority == 1\"}\n  - {from: source, to: a}",
    ));
}

#[test]
fn loop_without_an_exit_is_rejected() {
    let body = "  - {id: source, type: Source, interarrival: 1min}\n  \
                - {id: m, type: Machine, cycle_time: 1min}\n  \
                - {id: q, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}";
    for flow in [
        // A condition is no exit: a matching part would circle for ever.
        "  - [source, m, q]\n  - {from: q, to: m, when: \"priority > 0\"}\n  - {from: q, to: sink}",
        // Nor is a first-free edge back.
        "  - [source, m, q]\n  - {from: q, to: [m, sink]}",
        // Nor a preferred one.
        "  - [source, m, q]\n  - {from: q, to: [m, sink], choose: preferred}",
    ] {
        let errors = try_load(&document("1h", body, flow)).unwrap_err();
        assert_eq!(errors.len(), 1, "{errors}");
        assert_eq!(
            errors.first().kind().example().as_deref(),
            Some("{from: qc, split: {weld: 5%, pack: 95%}}"),
            "{errors}"
        );
    }
    // A split with a target outside is the way to write it, also behind a condition.
    load(&document(
        "1h",
        body,
        "  - [source, m, q]\n  - {from: q, split: {m: 5%, sink: 95%}}",
    ));
    load(&document(
        "1h",
        body,
        "  - [source, m, q]\n  - {from: q, to: sink, when: \"priority > 0\"}\n  \
         - {from: q, split: {m: 5%, sink: 95%}}",
    ));
}

#[test]
fn due_in_on_an_on_demand_source_is_rejected() {
    let yaml = |source: &str| {
        document(
            "1h",
            &format!(
                "  - {{id: source, type: Source{source}}}\n  \
                 - {{id: m, type: Machine, cycle_time: 1min}}\n  - {{id: sink, type: Sink}}"
            ),
            "  - [source, m, sink]",
        )
    };
    let errors = try_load(&yaml(", due_in: 8h")).unwrap_err();
    assert_eq!(errors.first().path().to_string(), "components[0].due_in");
    assert_eq!(
        errors.first().kind().example().as_deref(),
        Some("{id: source, type: Source, interarrival: 40s, due_in: 8h}")
    );
    load(&yaml(", interarrival: 1min, due_in: 8h"));
    // Zero is allowed, and a distribution.
    load(&yaml(", interarrival: 1min, due_in: 0s"));
    load(&yaml(
        ", interarrival: 1min, due_in: {dist: uniform, min: 2h, max: 8h}",
    ));
}

#[test]
fn too_many_leaving_edges() {
    let conditional = |n: usize| {
        let mut flow = String::new();
        for _ in 0..n - 1 {
            flow.push_str("  - {from: source, to: a, when: \"priority > 0\"}\n");
        }
        flow.push_str("  - {from: source, to: b}");
        document("1h", TWO_SINKS, &flow)
    };
    load(&conditional(1024));
    let errors = try_load(&conditional(1025)).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    assert_eq!(errors.first().path().to_string(), "flow[1024].from");
    assert!(
        errors.first().to_string().contains("1025 edges"),
        "{errors}"
    );
    // And targets on one edge.
    let many = |n: usize| {
        let mut components = String::from("  - {id: source, type: Source, interarrival: 1min}\n");
        let mut list = Vec::new();
        for i in 0..n {
            let _ = writeln!(components, "  - {{id: s{i}, type: Sink}}");
            list.push(format!("s{i}"));
        }
        document(
            "1h",
            components.trim_end(),
            &format!("  - {{from: source, to: [{}]}}", list.join(", ")),
        )
    };
    load(&many(1024));
    let errors = try_load(&many(1025)).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    assert_eq!(errors.first().path().to_string(), "flow[0].to[1024]");
}

#[test]
fn constant_zero_variant_cycle_time() {
    let yaml = |times: &str| {
        document(
            "1h",
            &format!(
                "  - {{id: source, type: Source, variants: {{red: 50%, blue: 50%}}}}\n  \
                 - {{id: m, type: Machine, cycle_time: 1min, cycle_time_by_variant: {times}}}\n  \
                 - {{id: sink, type: Sink}}"
            ),
            "  - [source, m, sink]",
        )
    };
    let errors = try_load(&yaml("{red: 0s}")).unwrap_err();
    assert_eq!(
        errors.first().path().to_string(),
        "components[1].cycle_time_by_variant.red"
    );
    load(&yaml("{red: 2min}"));
    load(&yaml("{red: {dist: exponential, mean: 2min}, blue: 1s}"));
}

/// A station of a domain's own, stating no facts: it passes a plain edge and may be
/// given no rule.
#[derive(Debug)]
struct Plain;

impl Encode for Plain {
    fn encode(&self) -> Json {
        Json::object([])
    }
}

impl ComponentParams for Plain {
    const TYPE: &'static str = "Plain";
    const EXAMPLE: &'static str = "{id: p, type: Plain}";

    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

struct Quiet;

impl Component<FlowMessage> for Quiet {
    fn handle(
        &mut self,
        _: Delivery<'_, FlowMessage>,
        _: &mut Context<'_, FlowMessage>,
    ) -> Result<(), ComponentError> {
        Ok(())
    }
}

impl ComponentType<Flow> for Plain {
    const STATION: bool = true;
    const PORTS: Ports = Ports::Both;

    fn build(
        &self,
        _: &Wiring<'_, FlowShared>,
    ) -> Result<Box<dyn Component<FlowMessage>>, BuildError> {
        Ok(Box::new(Quiet))
    }
}

#[test]
fn a_rule_on_a_type_without_the_facts_is_rejected() {
    let registry = taktsim_components::standard()
        .unwrap()
        .with::<Plain>()
        .unwrap();
    let loader = Loader::new(registry.catalog());
    let body = "  - {id: source, type: Source, interarrival: 1min}\n  \
                - {id: p, type: Plain}\n  - {id: q, type: Plain}\n  \
                - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}";
    let rejected = |flow: &str, control: &str, fact: &str, rule: &str| {
        let text = format!("{}{control}", document("1h", body, flow));
        let errors = loader
            .load_str(&text, Format::Yaml)
            .err()
            .unwrap_or_else(|| panic!("{flow} loaded"));
        assert!(
            errors.iter().any(|e| {
                let text = e.to_string();
                text.contains("type `Plain`")
                    && text.contains(&format!("`{fact}`"))
                    && text.contains(&format!("`{rule}`"))
            }),
            "{flow}: {errors}"
        );
    };
    let chain = "  - [source, p, q, m, sink]";
    // As a holder: a leaving `when`, `split` or status rule needs the route table.
    rejected(
        "  - [source, p]\n  - {from: p, to: q, when: \"priority > 0\"}\n  - {from: p, to: m}\n  - [q, m, sink]",
        "",
        "routes",
        "when",
    );
    rejected(
        "  - [source, p]\n  - {from: p, split: {q: 50%, m: 50%}}\n  - [q, m, sink]",
        "",
        "routes",
        "split",
    );
    rejected(
        "  - [source, p]\n  - {from: p, to: [q, m], choose: preferred}\n  - [q, m, sink]",
        "",
        "routes",
        "preferred",
    );
    // As a target of a status rule.
    rejected(
        "  - {from: source, to: [p, m], choose: preferred}\n  - [p, q, m, sink]",
        "",
        "status",
        "preferred",
    );
    rejected(
        "  - {from: source, to: [p, m], choose: shortest_queue}\n  - [p, q, m, sink]",
        "",
        "status",
        "shortest_queue",
    );
    rejected(
        "  - {from: source, to: [p, m], choose: lowest_utilization}\n  - [p, q, m, sink]",
        "",
        "status",
        "lowest_utilization",
    );
    // As a station with a rule, and upstream of one.
    rejected(
        chain,
        "control: {dispatch: {p: fifo}}\n",
        "takes_rule",
        "fifo",
    );
    rejected(
        chain,
        "control: {dispatch: {p: shortest_processing_time}}\n",
        "mean_time",
        "shortest_processing_time",
    );
    rejected(
        chain,
        "control: {dispatch: {p: minimum_setup}}\n",
        "setup",
        "minimum_setup",
    );
    rejected(chain, "control: {dispatch: {m: fifo}}\n", "routes", "fifo");
    // In a loop: C is the sum of the places of its components.
    let with_gate = "  - {id: source, type: Source, interarrival: 1min}\n  \
                     - {id: m, type: Machine, cycle_time: 1min}\n  - {id: p, type: Plain}\n  \
                     - {id: g, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}";
    let text = document(
        "1h",
        with_gate,
        "  - [source, m, p, g]\n  - {from: g, split: {m: 5%, sink: 95%}}",
    );
    let errors = loader.load_str(&text, Format::Yaml).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    let message = errors.first().to_string();
    assert!(
        message.contains("type `Plain`") && message.contains("`places`"),
        "{message}"
    );
}

#[test]
fn a_rule_on_a_sink_or_a_buffer_says_which_fact_it_lacks() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min}\n  \
         - {id: b, type: Buffer, capacity: 2 parts}\n  \
         - {id: m, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [b, m], choose: lowest_utilization}\n  - {from: [b, m], to: sink}",
    );
    let errors = try_load(&yaml).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors}");
    assert!(errors.first().to_string().contains("`work`"), "{errors}");
    // Status rules on what always has room are fine, and on a Buffer too.
    for rule in ["preferred", "shortest_queue"] {
        load(&document(
            "1h",
            "  - {id: source, type: Source, interarrival: 1min}\n  \
             - {id: b, type: Buffer, capacity: 2 parts}\n  - {id: sink, type: Sink}",
            &format!(
                "  - {{from: source, to: [b, sink], choose: {rule}}}\n  - {{from: b, to: sink}}"
            ),
        ));
    }
}

#[test]
fn a_loaded_routed_model_always_builds() {
    let yaml = document(
        "1h",
        "  - {id: source, type: Source, interarrival: 1min, variants: {red: 50%, blue: 50%}}\n  \
         - {id: a, type: Machine, cycle_time: 1min}\n  \
         - {id: b, type: Machine, cycle_time: 1min}\n  \
         - {id: c, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}",
        "  - {from: source, to: [a, b], choose: shortest_queue}\n  \
         - {from: [a, b], to: c, when: \"variant == 'red'\"}\n  \
         - {from: [a, b], split: {sink: 50%, c: 50%}}\n  - {from: c, to: sink}",
    );
    let model = load(&yaml);
    assert!(
        registry()
            .build(model.document(), taktsim_core::EngineConfig::new(1))
            .is_ok()
    );
}

/// A condition that does not parse or type-check is told what a condition may read,
/// with an example that loads in its place (ADR-0023 §2).
#[test]
fn a_bad_condition_gets_an_example_that_loads() {
    let components = "  - {id: source, type: Source, interarrival: 1min, variants: {basic: 80%, premium: 20%}}\n  \
                      - {id: a, type: Sink}\n  - {id: b, type: Sink}";
    let with = |when: &str| {
        document(
            "1h",
            components,
            &format!("  - {{from: source, to: a, when: \"{when}\"}}\n  - {{from: source, to: b}}"),
        )
    };
    for bad in [
        "variant = 'basic'",
        "variant == 'c'",
        "priority",
        "throughput(a)",
        "variant ==",
    ] {
        let errors = try_load(&with(bad)).unwrap_err();
        assert_eq!(errors.len(), 1, "{bad}: {errors}");
        let e = errors.first();
        assert_eq!(e.path().to_string(), "flow[0].when", "{bad}: {e}");
        assert!(
            matches!(e.kind(), LoadErrorKind::Condition { .. }),
            "{bad}: {e}"
        );
        let allowed = e.kind().allowed().unwrap();
        assert!(allowed.contains("`variant` and `priority`"), "{allowed}");
        let example = e.kind().example().unwrap();
        load(&with(&example));
    }
}
