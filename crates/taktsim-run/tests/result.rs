//! The result document (ADR-0024 §3, §4): it round-trips, matches its schema,
//! ignores what a later version adds, and every number states its accuracy.

mod support;

#[path = "../../taktsim-model/tests/support/schema_checker.rs"]
mod schema_checker;

use std::collections::BTreeSet;

use schema_checker::{ANNOTATIONS, Checker, VALIDATION, keywords};
use support::{TempDir, drawing_model, exact_model, model, run_text};
use taktsim_model::{Encode, Format, Json, decode_str};
use taktsim_run::{
    Accuracy, AccuracyKind, AccuracyTarget, Diagnostic, Metric, MetricValue, Over, ResultDocument,
    RunRequest, SamplingInterval, Scalar, ValidationReport, diagnostic_schema, result_schema,
    validation_schema,
};
use taktsim_stats::{ConfidenceInterval, ConfidenceLevel};

#[allow(clippy::unwrap_used)] // Test helper.
fn sample_documents() -> Vec<ResultDocument> {
    let dir = TempDir::new();
    let store = dir.store();
    let limited = RunRequest {
        max_events: Some(3),
        ..RunRequest::new()
    };
    vec![
        run_text(&drawing_model(", seed: 1"), &RunRequest::new(), &store)
            .unwrap()
            .result,
        run_text(&exact_model(""), &RunRequest::new(), &store)
            .unwrap()
            .result,
        run_text(&drawing_model(", seed: 2"), &limited, &store)
            .unwrap()
            .result,
    ]
}

/// A document with everything a later milestone fills in.
#[allow(clippy::unwrap_used)] // Test helper.
fn full_document() -> ResultDocument {
    let mut document = sample_documents().remove(0);
    let ci =
        ConfidenceInterval::from_values(&[98.0, 100.5, 102.0], ConfidenceLevel::DEFAULT).unwrap();
    let mut sampling = Accuracy::from(&ci);
    if let AccuracyKind::Sampling(interval) = &mut sampling.kind {
        interval.stopping = Some("sequential".to_owned());
    }
    sampling.target = Some(AccuracyTarget {
        relative: Some(0.02),
    });
    sampling.reached = Some(false);
    document.metrics.push(Metric {
        name: "state_shares(cut)".to_owned(),
        unit: "1".to_owned(),
        over: Over::Window {
            from: "1h".parse().unwrap(),
            to: "8h".parse().unwrap(),
        },
        observables: vec![
            "state_shares(cut)".to_owned(),
            "state_shares(cut, weld)".to_owned(),
        ],
        values: vec![
            MetricValue {
                value: Scalar::Number(0.75),
                labels: Some(
                    [
                        ("resource".to_owned(), "cut".to_owned()),
                        ("state".to_owned(), "busy".to_owned()),
                    ]
                    .into(),
                ),
                accuracy: vec![
                    sampling,
                    Accuracy::none("model_error", "unknown"),
                    Accuracy::censored(3.5),
                    Accuracy {
                        kind: AccuracyKind::PartlyUndefined {
                            undefined: 1,
                            of: 3,
                            detail: "one run delivered nothing".to_owned(),
                        },
                        target: None,
                        reached: None,
                    },
                ],
            },
            MetricValue {
                value: Scalar::Flag(true),
                labels: None,
                accuracy: vec![Accuracy {
                    kind: AccuracyKind::Unknown {
                        kind: "a_later_kind".to_owned(),
                    },
                    target: None,
                    reached: None,
                }],
            },
        ],
    });
    document
}

#[test]
fn result_round_trips() {
    let mut documents = sample_documents();
    documents.push(full_document());
    for document in documents {
        let text = document.to_text();
        let read: ResultDocument = decode_str(&text, Format::Json).unwrap();
        assert_eq!(read, document);
        assert_eq!(read.to_text(), text, "the same bytes after reading");
    }
}

#[test]
fn labels_and_observable_round_trip() {
    let document = full_document();
    let read: ResultDocument = decode_str(&document.to_text(), Format::Json).unwrap();
    let metric = read.metrics.last().unwrap();
    assert_eq!(
        metric.observables,
        ["state_shares(cut)", "state_shares(cut, weld)"]
    );
    let labels = metric.values[0].labels.as_ref().unwrap();
    assert_eq!(labels["state"], "busy");
    assert_eq!(labels["resource"], "cut");
    assert_eq!(metric.values[1].labels, None);
}

/// Every keyword the schemas use is one the checker validates or an annotation.
#[test]
fn the_schemas_use_only_known_keywords() {
    for schema in [result_schema(), validation_schema(), diagnostic_schema()] {
        let mut used = BTreeSet::new();
        keywords(&schema, &mut used);
        for word in used {
            assert!(
                VALIDATION.contains(&word.as_str()) || ANNOTATIONS.contains(&word.as_str()),
                "{word}"
            );
        }
    }
}

#[test]
fn emitted_results_match_the_schema() {
    let schema = result_schema();
    let checker = Checker { root: &schema };
    let mut documents = sample_documents();
    documents.push(full_document());
    for document in &documents {
        assert!(
            checker.valid(&schema, &document.encode()),
            "{}",
            document.to_text()
        );
    }
    // And what it refuses.
    let good = sample_documents().remove(0).encode();
    let without = |key: &str| {
        let Json::Object(mut map) = good.clone() else {
            panic!("an object");
        };
        map.remove(key);
        Json::Object(map)
    };
    for key in [
        "complete",
        "format",
        "stop_reason",
        "provenance",
        "runs",
        "metrics",
        "unreported",
    ] {
        assert!(!checker.valid(&schema, &without(key)), "{key} is required");
    }
    // A key of a later version is allowed.
    let later = good.clone().with("a_later_key", &1_u32);
    assert!(checker.valid(&schema, &later));
    // A value without an accuracy statement is not a result.
    let text = sample_documents().remove(0).to_text();
    let mut bare: Json = decode_str::<Raw>(&text, Format::Json).unwrap().0;
    if let Json::Object(top) = &mut bare
        && let Some(Json::Array(metrics)) = top.get_mut("metrics")
        && let Some(Json::Object(metric)) = metrics.first_mut()
        && let Some(Json::Array(values)) = metric.get_mut("values")
        && let Some(Json::Object(value)) = values.first_mut()
    {
        value.insert("accuracy".to_owned(), Json::Array(vec![]));
    } else {
        panic!("the shape of a result changed");
    }
    assert!(!checker.valid(&schema, &bare));
}

/// The schema of `over` accepts exactly what the reader reads (ADR-0024 §2: one
/// definition for reader and schema): every mix of `from`, `to` and `at`, with and
/// without a key of a later version.
#[test]
fn the_schema_of_over_accepts_what_the_reader_reads() {
    let schema = result_schema();
    let checker = Checker { root: &schema };
    let over = &schema["$defs"]["Over"];
    assert_ne!(
        *over,
        Json::Null,
        "`Over` is a definition of the result schema"
    );
    let keys = ["from", "to", "at"];
    for mask in 0..(1_u8 << keys.len()) {
        for later in [false, true] {
            let mut object = Json::object([]);
            for (i, key) in keys.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    object = object.with(key, &Json::from("8h"));
                }
            }
            if later {
                object = object.with("a_later_key", &1_u32);
            }
            let text = object.to_string();
            let read = decode_str::<Over>(&text, Format::Json).is_ok();
            assert_eq!(checker.valid(over, &object), read, "{text}");
        }
    }
}

/// Any JSON, as read.
struct Raw(Json);

impl taktsim_model::Decode for Raw {
    const EXPECTED: &'static str = "anything";
    const EXAMPLE: &'static str = "{}";

    fn decode(node: &taktsim_model::Node, _: &mut taktsim_model::Decoder) -> Option<Self> {
        Some(Self(node.to_json()))
    }

    fn schema(_: &mut taktsim_model::Schemas) -> Json {
        Json::object([])
    }
}

#[test]
fn unknown_keys_are_ignored() {
    let document = sample_documents().remove(0);
    let text = document.to_text();
    // At every level the document has keys.
    let later = text
        .replacen(
            "\"complete\": true,",
            "\"complete\": true, \"later_top\": [1, {}],",
            1,
        )
        .replace(
            "\"engine\": ",
            "\"later_provenance\": {\"x\": 1}, \"engine\": ",
        )
        .replace("\"events\": ", "\"later_run\": null, \"events\": ")
        .replace("\"unit\": ", "\"later_metric\": true, \"unit\": ")
        .replace("\"value\": ", "\"later_value\": \"x\", \"value\": ")
        .replace("\"reason\": ", "\"later_accuracy\": 1, \"reason\": ");
    assert_ne!(later, text);
    let read: ResultDocument = decode_str(&later, Format::Json).unwrap();
    assert_eq!(read, document);
}

#[test]
fn a_new_major_format_is_refused() {
    let text = sample_documents()
        .remove(0)
        .to_text()
        .replace("taktsim-result/1", "taktsim-result/2");
    let errors = decode_str::<ResultDocument>(&text, Format::Json).unwrap_err();
    assert_eq!(errors.first().path().to_string(), "format");
}

#[test]
fn every_value_has_accuracy() {
    for document in sample_documents() {
        assert_ne!(document.metrics.len(), 0);
        for metric in &document.metrics {
            assert_ne!(metric.values.len(), 0, "{}", metric.name);
            for value in &metric.values {
                assert_ne!(value.accuracy.len(), 0, "{}", metric.name);
            }
        }
    }
    // A reader refuses a value without one.
    let text = sample_documents()
        .remove(0)
        .to_text()
        .replace("\"accuracy\": [", "\"accuracy_was\": [");
    assert!(decode_str::<ResultDocument>(&text, Format::Json).is_err());
}

#[test]
fn unknown_kind_reads_as_no_statement() {
    let read: Accuracy = decode_str("{kind: a_later_kind, whatever: 1}", Format::Yaml).unwrap();
    assert_eq!(
        read.kind,
        AccuracyKind::Unknown {
            kind: "a_later_kind".to_owned()
        }
    );
    assert_eq!(read.encode()["kind"], "a_later_kind");
    // `target` and `reached` are read on every kind.
    let read: Accuracy = decode_str(
        "{kind: a_later_kind, target: {relative: 0.02}, reached: true}",
        Format::Yaml,
    )
    .unwrap();
    assert_eq!(read.reached, Some(true));
    assert_eq!(read.target.and_then(|t| t.relative), Some(0.02));
    // A known kind without its keys is an error, not a silent no statement.
    assert!(decode_str::<Accuracy>("{kind: none}", Format::Yaml).is_err());
    assert!(decode_str::<Accuracy>("{kind: sampling}", Format::Yaml).is_err());
    assert!(decode_str::<Accuracy>("{reason: r}", Format::Yaml).is_err());
}

/// A later kind may use `interval`, `reason` or `detail` in a shape of its own: the
/// reader rule says it reads as no statement, so it must not fail the document, and
/// the schema must accept what the reader reads (ADR-0024 §4).
#[test]
fn an_unknown_kind_may_use_a_known_key_in_another_shape() {
    let schema = result_schema();
    let checker = Checker { root: &schema };
    let accuracy = &schema["$defs"]["Accuracy"];
    for text in [
        r#"{"kind": "discretization", "detail": {"order": 2}}"#,
        r#"{"kind": "discretization", "interval": 0.5}"#,
        r#"{"kind": "discretization", "reason": ["a", "b"]}"#,
    ] {
        let read: Accuracy = decode_str(text, Format::Json).unwrap();
        assert_eq!(
            read.kind,
            AccuracyKind::Unknown {
                kind: "discretization".to_owned()
            },
            "{text}"
        );
        let json = decode_str::<Raw>(text, Format::Json).unwrap().0;
        assert!(checker.valid(accuracy, &json), "{text}");
    }
    // The known kinds still type their own keys, in the reader and the schema.
    for text in [
        r#"{"kind": "none", "reason": "r", "detail": {"order": 2}}"#,
        r#"{"kind": "sampling", "interval": 0.5}"#,
    ] {
        assert!(
            decode_str::<Accuracy>(text, Format::Json).is_err(),
            "{text}"
        );
        let json = decode_str::<Raw>(text, Format::Json).unwrap().0;
        assert!(!checker.valid(accuracy, &json), "{text}");
    }
    // In a whole stored document too.
    let text = sample_documents().remove(0).to_text().replacen(
        "\"accuracy\": [",
        "\"accuracy\": [{\"kind\": \"discretization\", \"detail\": {\"order\": 2}},",
        1,
    );
    let read: ResultDocument = decode_str(&text, Format::Json).unwrap();
    assert_eq!(read.metrics[0].values[0].accuracy.len(), 2);
}

/// A float with no fraction of its own reads back as the same float, also above
/// 2^53 − 1, where the reader refuses an integer: the document is read back on
/// reuse, so it must read as what was written (ADR-0024 §3).
#[test]
fn a_whole_float_reads_back_as_a_float() {
    let mut document = sample_documents().remove(0);
    let interval = SamplingInterval {
        mean: 1e17,
        half_width: 2.0,
        std_dev: 0.0,
        replications: 20,
        level: 1.0,
    };
    let accuracy = Accuracy {
        kind: AccuracyKind::Sampling(interval),
        target: Some(AccuracyTarget {
            relative: Some(1.0),
        }),
        reached: Some(true),
    };
    document.metrics.push(Metric {
        name: "throughput(sink)".to_owned(),
        unit: "items/h".to_owned(),
        over: Over::Window {
            from: "0s".parse().unwrap(),
            to: "8h".parse().unwrap(),
        },
        observable: None,
        values: [2.0, -4.0, 0.0, 1e17, 1e300]
            .into_iter()
            .map(|x| MetricValue {
                value: Scalar::Number(x),
                labels: None,
                accuracy: vec![accuracy.clone()],
            })
            .collect(),
    });
    let text = document.to_text();
    let read: ResultDocument = decode_str(&text, Format::Json).unwrap();
    assert_eq!(read, document);
    assert_eq!(read.to_text(), text);
    let schema = result_schema();
    let json = decode_str::<Raw>(&text, Format::Json).unwrap().0;
    assert!(Checker { root: &schema }.valid(&schema, &json));
}

#[test]
fn single_run_states_no_interval() {
    let dir = TempDir::new();
    let result = run_text(
        &drawing_model(", seed: 1"),
        &RunRequest::new(),
        &dir.store(),
    )
    .unwrap()
    .result;
    for metric in &result.metrics {
        let [accuracy] = metric.values[0].accuracy.as_slice() else {
            panic!("{}: one statement", metric.name);
        };
        let AccuracyKind::None { reason, detail } = &accuracy.kind else {
            panic!("{}: no interval from one run", metric.name);
        };
        assert_eq!(reason, "single_replication");
        assert!(detail.len() > 1);
    }
}

#[test]
fn sampling_wraps_the_interval() {
    let ci =
        ConfidenceInterval::from_values(&[98.0, 100.0, 102.0], ConfidenceLevel::DEFAULT).unwrap();
    let accuracy = Accuracy::from(&ci);
    let AccuracyKind::Sampling(interval) = accuracy.kind else {
        panic!("a sampling statement");
    };
    assert_eq!(
        interval,
        SamplingInterval {
            mean: ci.mean(),
            half_width: ci.half_width(),
            std_dev: ci.std_dev(),
            replications: 3,
            level: ConfidenceLevel::DEFAULT.level(),
            stopping: None,
        }
    );
    let json = Accuracy::from(&ci).encode();
    assert_eq!(json["kind"], "sampling");
    assert_eq!(json["interval"]["replications"], Json::from(3));
    let read: Accuracy = decode_str(&json.to_string(), Format::Json).unwrap();
    assert_eq!(read, Accuracy::from(&ci));
}

#[test]
fn over_is_a_window_or_a_reading() {
    for bad in ["{from: 0s}", "{to: 1h}", "{from: 0s, to: 1h, at: 1h}", "{}"] {
        assert!(decode_str::<Over>(bad, Format::Yaml).is_err(), "{bad}");
    }
    assert_eq!(
        decode_str::<Over>("{at: 1h, later: 1}", Format::Yaml).unwrap(),
        Over::At {
            at: "1h".parse().unwrap()
        }
    );
}

/// Each declared observable is in some metric's `observables` or in
/// `unreported`, so a reader can check that it got what it asked for.
#[test]
fn declared_observable_is_answered_or_unreported() {
    let yaml = "format: \"1.0\"\nmodel: {name: m, seed: 1}\n\
         experiment: {type: terminating, horizon: 1h, precision: {\"throughput(sink)\": 2%}}\n\
         components:\n  - {id: source, type: Source, interarrival: {dist: exponential, mean: 1min}}\n  \
         - {id: buf, type: Buffer, capacity: 5 parts}\n  - {id: cut, type: Machine, cycle_time: 1min}\n  - {id: sink, type: Sink}\n\
         flow:\n  - [source, buf, cut, sink]\n\
         observe:\n  - throughput(sink)\n  - wip(buf)\n  - state_shares(cut)\n";
    let dir = TempDir::new();
    let result = run_text(yaml, &RunRequest::new(), &dir.store())
        .unwrap()
        .result;
    let answered: BTreeSet<&str> = result
        .metrics
        .iter()
        .flat_map(|m| m.observables.iter().map(String::as_str))
        .collect();
    let declared = BTreeSet::from(["state_shares(cut)", "throughput(sink)", "wip(buf)"]);
    assert_eq!(answered, declared);
    assert_eq!(result.unreported.len(), 0, "every one has a value");
    // A model that declares nothing has an empty list, not a missing one.
    let plain = run_text(&exact_model(""), &RunRequest::new(), &dir.store()).unwrap();
    assert_eq!(plain.result.unreported.len(), 0);
    assert!(plain.result.to_text().contains("\"unreported\": []"));
}

#[test]
fn nothing_in_a_result_depends_on_where_or_when_it_ran() {
    let a = TempDir::new();
    let b = TempDir::new();
    let yaml = drawing_model(", seed: 1");
    let one = run_text(&yaml, &RunRequest::new(), &a.store())
        .unwrap()
        .result;
    let two = run_text(&yaml, &RunRequest::new(), &b.store())
        .unwrap()
        .result;
    assert_eq!(one.to_text(), two.to_text());
    let text = one.to_text();
    assert!(!text.contains(a.path().to_str().unwrap()));
    let provenance = one.provenance;
    assert_eq!(
        provenance.platform,
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    );
    assert_eq!(provenance.engine, taktsim_core::ENGINE_NAME);
    assert_eq!(provenance.engine_version, taktsim_core::ENGINE_VERSION);
    assert_eq!(provenance.package, "taktsim-components");
    assert_eq!(provenance.package_version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn validation_reports_match_their_schema_and_round_trip() {
    let registry = taktsim_components::standard().unwrap();
    let good_text = exact_model("");
    let good = taktsim_run::validate(
        &registry,
        taktsim_run::ModelSource::Text(&good_text, Format::Yaml),
    );
    assert!(good.valid);
    assert!(good.model_hash.is_some() && good.system_hash.is_some());
    assert_eq!(good.failure_class(), None);
    let bad_text = model(
        "",
        "  - {id: source, type: Source}\n  - {id: cut, type: Machine, cycle_time: 10}\n  - {id: sink, type: Sink}",
        "  - [source, cut, sink]",
    );
    let bad = taktsim_run::validate(
        &registry,
        taktsim_run::ModelSource::Text(&bad_text, Format::Yaml),
    );
    assert!(!bad.valid);
    assert_eq!(bad.model_hash, None);
    assert_eq!(
        bad.errors[0].path.as_deref(),
        Some("components[1].cycle_time")
    );
    assert_eq!(bad.errors[0].type_name, "model.wrong_type");
    assert!(bad.errors[0].line.is_some() && bad.errors[0].column.is_some());
    assert_eq!(
        bad.failure_class(),
        Some(taktsim_run::FailureClass::Invalid)
    );

    let schema = validation_schema();
    let checker = Checker { root: &schema };
    for report in [&good, &bad] {
        assert!(checker.valid(&schema, &report.encode()));
        let read: ValidationReport = decode_str(&report.to_text(), Format::Json).unwrap();
        assert_eq!(&read, report);
    }
}

#[test]
fn diagnostic_lines_match_their_schema() {
    let schema = diagnostic_schema();
    let checker = Checker { root: &schema };
    let complete = Diagnostic::error("args.unknown_flag", "m")
        .at("--sed")
        .found("--sed")
        .allowed("--seed")
        .example("--seed 42");
    assert!(checker.valid(&schema, &complete.encode()));
    // An `args.*` error promises all of it.
    let bare = Diagnostic::error("args.unknown_flag", "m");
    assert!(!checker.valid(&schema, &bare.encode()));
    // A `model.*` error needs no example: the loader has none for every kind.
    let model = Diagnostic::error("model.anchor", "m")
        .at("components")
        .found("an anchor")
        .allowed("plain values");
    assert!(checker.valid(&schema, &model.encode()));
    // A run error says when and what happened last instead.
    let run = Diagnostic::error("run.stalled", "m").at_time("1h".parse().unwrap());
    assert!(checker.valid(&schema, &run.encode()));
    let read: Diagnostic = decode_str(&run.encode().to_string(), Format::Json).unwrap();
    assert_eq!(read, run);
}

/// `taktsim schema` and the Python bindings pick a schema by name in this crate, so
/// the two cannot drift (ADR-0024 §1, §10): every name listed gives a schema, and a
/// component type gives its entry with the `$defs` its references need.
#[test]
fn every_listed_schema_name_gives_a_schema() {
    let model = taktsim_components::standard_schema().unwrap();
    let names = taktsim_run::schema_names(&model);
    assert_eq!(
        names,
        [
            "all",
            "model",
            "result",
            "validation",
            "error",
            "Buffer",
            "Machine",
            "Sink",
            "Source"
        ]
    );
    for name in names {
        let schema = taktsim_run::schema_named(&model, name).unwrap();
        assert!(schema.as_object().is_some(), "{name}");
    }
    let pick = |name| taktsim_run::schema_named(&model, name);
    assert_eq!(pick("all"), Some(model.clone()));
    assert_eq!(pick("model"), Some(model.clone()));
    assert_eq!(pick("result"), Some(result_schema()));
    assert_eq!(pick("validation"), Some(validation_schema()));
    assert_eq!(pick("error"), Some(diagnostic_schema()));
    let source = pick("Source").unwrap();
    assert_eq!(source["properties"]["type"]["const"], "Source");
    assert_eq!(source["title"], "Source");
    assert_eq!(source["$defs"], model["$defs"]);
    assert_eq!(pick("Conveyor"), None);
    assert_eq!(pick("describe"), None, "the CLI's own");
}
