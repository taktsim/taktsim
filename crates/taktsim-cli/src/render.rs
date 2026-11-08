//! Text for people, from the same typed values as the JSON (ADR-0019 item 2), and
//! the exit codes, from `complete` and the failure class, never from a word in a
//! `stop_reason`.

use std::fmt::Write as _;

use taktsim_model::Encode;
use taktsim_run::{
    Accuracy, AccuracyKind, Diagnostic, FailureClass, Metric, Over, ResultDocument, Scalar,
    ValidationReport,
};

use crate::args::Format;

/// The exit code of a failure (ADR-0019 item 3).
pub(crate) const fn exit_for_failure(class: FailureClass) -> u8 {
    match class {
        FailureClass::Invalid => 1,
        FailureClass::RunFailed => 2,
        FailureClass::Io => 7,
    }
}

/// The exit code of a result that was printed: `complete` decides.
pub(crate) const fn exit_for_result(complete: bool) -> u8 {
    if complete { 0 } else { 3 }
}

/// A line of stderr in `format`.
pub(crate) fn line(diagnostic: &Diagnostic, format: Format) -> String {
    match format {
        Format::Json => {
            let mut text = diagnostic.encode().to_string();
            text.push('\n');
            text
        }
        Format::Text => diagnostic_text(diagnostic),
    }
}

/// A diagnostic for a person: its type, where, what, and what to do.
fn diagnostic_text(d: &Diagnostic) -> String {
    let mut text = format!("{}[{}]", d.kind.as_str(), d.type_name);
    if let Some(path) = &d.path {
        let _ = write!(text, " {path}");
    }
    let _ = writeln!(text, ": {}", d.message);
    match (&d.file, d.line, d.column) {
        (Some(file), Some(line), Some(column)) => {
            let _ = writeln!(text, "  at: {file}:{line}:{column}");
        }
        (Some(file), _, _) => {
            let _ = writeln!(text, "  at: {file}");
        }
        (None, _, _) => {}
    }
    for (label, value) in [
        ("found", &d.found),
        ("allowed", &d.allowed),
        ("example", &d.example),
    ] {
        if let Some(value) = value {
            let _ = writeln!(text, "  {label}: {value}");
        }
    }
    if let Some(time) = d.time {
        let _ = writeln!(text, "  time: {time}");
    }
    if let Some(items) = d.items_alive {
        let _ = writeln!(text, "  items alive: {items}");
    }
    if !d.recent.is_empty() {
        let _ = writeln!(text, "  last records:");
        for line in &d.recent {
            let _ = writeln!(text, "    {line}");
        }
    }
    text
}

/// What the numbers of one metric are worth, in a few words.
fn accuracy_text(accuracy: &[Accuracy]) -> String {
    let parts: Vec<String> = accuracy
        .iter()
        .map(|statement| match &statement.kind {
            AccuracyKind::Sampling(interval) => format!(
                "± {} ({} level, {} replications)",
                interval.half_width, interval.level, interval.replications
            ),
            AccuracyKind::None { reason, .. } => format!("no error bound: {reason}"),
            AccuracyKind::Unknown { kind } => format!("no statement read from `{kind}`"),
        })
        .collect();
    parts.join("; ")
}

fn metric_text(text: &mut String, metric: &Metric) {
    let over = match metric.over {
        Over::Window { from, to } => format!("{from} to {to}"),
        Over::At { at } => format!("at {at}"),
    };
    for value in &metric.values {
        let number = match value.value {
            Scalar::Integer(n) => n.to_string(),
            Scalar::Number(x) => x.to_string(),
            Scalar::Flag(b) => b.to_string(),
        };
        let labels = value.labels.as_ref().map_or_else(String::new, |labels| {
            let parts: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!(" [{}]", parts.join(", "))
        });
        let _ = writeln!(
            text,
            "  {}{labels} = {number} {} ({over}; {})",
            metric.name,
            metric.unit,
            accuracy_text(&value.accuracy)
        );
    }
}

/// A result for a person.
pub(crate) fn result_text(result: &ResultDocument) -> String {
    let mut text = String::new();
    let done = if result.complete {
        "complete"
    } else {
        "partial: these numbers must not be compared with a complete result"
    };
    let _ = writeln!(text, "{done} ({})", result.stop_reason);
    let provenance = &result.provenance;
    let _ = writeln!(
        text,
        "engine: {} {}\npackage: {} {}\nmodel hash: {}\nsystem hash: {}\nplatform: {}",
        provenance.engine,
        provenance.engine_version,
        provenance.package,
        provenance.package_version,
        provenance.model_hash,
        provenance.system_hash,
        provenance.platform
    );
    if let Some(id) = &result.experiment_id {
        let _ = writeln!(text, "experiment: {id}");
    }
    for run in &result.runs {
        let seed = run
            .seed
            .map_or_else(|| "none".to_owned(), |seed| seed.to_string());
        let _ = writeln!(
            text,
            "run: seed {seed}, ended at {}, {} events, {} records",
            run.end_time, run.events, run.records
        );
        if let Some(id) = &run.run_id {
            let _ = writeln!(text, "  id: {id}");
        }
        if let Some(trace) = &run.trace {
            let _ = writeln!(text, "  trace: {}", trace.path);
        }
    }
    let _ = writeln!(text, "metrics:");
    for metric in &result.metrics {
        metric_text(&mut text, metric);
    }
    if !result.unreported.is_empty() {
        let _ = writeln!(text, "not reported:");
        for missing in &result.unreported {
            let _ = writeln!(text, "  {} ({})", missing.observable, missing.reason);
        }
    }
    text
}

/// A validation report for a person. The errors themselves go to stderr.
pub(crate) fn validation_text(report: &ValidationReport) -> String {
    match (report.valid, report.model_hash, report.system_hash) {
        (true, Some(model), Some(system)) => {
            format!("valid\nmodel hash: {model}\nsystem hash: {system}\n")
        }
        (true, _, _) => "valid\n".to_owned(),
        (false, _, _) => format!("invalid; errors: {}\n", report.errors.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_codes_of_adr_0019() {
        assert_eq!(exit_for_failure(FailureClass::Invalid), 1);
        assert_eq!(exit_for_failure(FailureClass::RunFailed), 2);
        assert_eq!(exit_for_failure(FailureClass::Io), 7);
        assert_eq!(exit_for_result(true), 0);
        assert_eq!(exit_for_result(false), 3);
    }

    #[test]
    fn a_diagnostic_says_all_it_has_in_text_and_as_one_json_line() {
        let mut d = Diagnostic::error("run.stalled", "items are left")
            .at("components[1]")
            .found("3 items")
            .allowed("none")
            .example("x")
            .at_time("1h".parse().unwrap());
        d.file = Some("m.yaml".to_owned());
        d.line = Some(4);
        d.column = Some(2);
        d.items_alive = Some(3);
        d.recent = vec!["1h event".to_owned()];
        let text = line(&d, Format::Text);
        for part in [
            "error[run.stalled] components[1]: items are left",
            "at: m.yaml:4:2",
            "found: 3 items",
            "allowed: none",
            "example: x",
            "time: 1h",
            "items alive: 3",
            "1h event",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        let json = line(&d, Format::Json);
        assert_eq!(json.lines().count(), 1);
        assert!(json.starts_with('{') && json.ends_with("}\n"));
        d.line = None;
        assert!(line(&d, Format::Text).contains("at: m.yaml\n"));
    }

    #[test]
    fn text_states_each_kind_of_accuracy() {
        let none = Accuracy::none("single_replication", "one run");
        assert_eq!(accuracy_text(&[none]), "no error bound: single_replication");
        let unknown = Accuracy {
            kind: AccuracyKind::Unknown {
                kind: "later".to_owned(),
            },
            target: None,
            reached: None,
        };
        assert!(accuracy_text(&[unknown]).contains("no statement"));
        let sampling = Accuracy {
            kind: AccuracyKind::Sampling(taktsim_run::SamplingInterval {
                mean: 1.0,
                half_width: 0.5,
                std_dev: 0.25,
                replications: 3,
                level: 0.95,
            }),
            target: None,
            reached: None,
        };
        assert_eq!(
            accuracy_text(&[sampling]),
            "± 0.5 (0.95 level, 3 replications)"
        );
    }

    #[test]
    fn text_shows_labels_flags_and_readings() {
        let mut text = String::new();
        metric_text(
            &mut text,
            &Metric {
                name: "share".to_owned(),
                unit: "1".to_owned(),
                over: Over::At {
                    at: "1h".parse().unwrap(),
                },
                observable: None,
                values: vec![
                    taktsim_run::MetricValue {
                        value: Scalar::Number(0.5),
                        labels: Some([("state".to_owned(), "busy".to_owned())].into()),
                        accuracy: vec![Accuracy::none("r", "d")],
                    },
                    taktsim_run::MetricValue {
                        value: Scalar::Flag(true),
                        labels: None,
                        accuracy: vec![Accuracy::none("r", "d")],
                    },
                ],
            },
        );
        assert!(
            text.contains("share [state=busy] = 0.5 1 (at 1h;"),
            "{text}"
        );
        assert!(text.contains("share = true 1"), "{text}");
    }

    #[test]
    fn a_report_says_valid_or_how_many_errors() {
        let mut report = ValidationReport {
            valid: true,
            errors: vec![],
            model_hash: None,
            system_hash: None,
        };
        assert_eq!(validation_text(&report), "valid\n");
        report.valid = false;
        assert!(validation_text(&report).starts_with("invalid"));
    }
}
