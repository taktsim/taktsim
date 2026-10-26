//! The loader's contract with component types (M5) and with the CLI (M7):
//! what happens when a component type's code misbehaves, and that every
//! error variant gives a message, what was found, what is allowed and,
//! where one exists, an example.

mod support;

use std::io;

use schemars::JsonSchema;
use serde::Serialize;
use serde::ser::{Error as _, Serializer};
use support::summary;
use taktsim_core::random::DistributionError;
use taktsim_model::expr::parse;
use taktsim_model::{
    Ambiguity, Catalog, ComponentParams, Fields, Format, Found, IncludePathProblem, JsonProblem,
    LoadErrorKind, Loader, Location, NormalizeProblem, Place, Resolver,
};
use taktsim_units::UnitError;

const MODEL: &str = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 8h}\n\
                     components: [{id: a, type: Odd}, {id: b, type: Odd}]\nflow: [[a, b]]\n";

/// Loads `MODEL` with the component type `P` as `Odd`.
#[allow(clippy::unwrap_used)] // Test helper: `P` is a valid type.
fn errors_with<P: ComponentParams>() -> Vec<String> {
    let catalog = Catalog::new().with::<P>().unwrap();
    match Loader::new(&catalog).load_str(MODEL, Format::Yaml) {
        Ok(_) => vec!["(loaded)".to_owned()],
        Err(errors) => errors
            .iter()
            .map(|e| format!("{} {}", summary_one(e), e.kind()))
            .collect(),
    }
}

fn summary_one(e: &taktsim_model::LoadError) -> String {
    format!(
        "{} @ {}",
        e.path(),
        e.location().map(|l| l.to_string()).unwrap_or_default()
    )
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(transparent)]
struct NotAMap(u32);

impl ComponentParams for NotAMap {
    const TYPE: &'static str = "Odd";
    const EXAMPLE: &'static str = "{id: a, type: Odd}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self(1))
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct Reserved {
    id: u32,
}

impl ComponentParams for Reserved {
    const TYPE: &'static str = "Odd";
    const EXAMPLE: &'static str = "{id: a, type: Odd}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self { id: 1 })
    }
}

#[derive(Debug, JsonSchema)]
struct Failing;

impl Serialize for Failing {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom("no normalized form"))
    }
}

impl ComponentParams for Failing {
    const TYPE: &'static str = "Odd";
    const EXAMPLE: &'static str = "{id: a, type: Odd}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct Huge {
    n: u64,
}

impl ComponentParams for Huge {
    const TYPE: &'static str = "Odd";
    const EXAMPLE: &'static str = "{id: a, type: Odd}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self { n: u64::MAX })
    }
}

#[derive(Debug, Serialize, JsonSchema)]
struct Silent;

impl ComponentParams for Silent {
    const TYPE: &'static str = "Odd";
    const EXAMPLE: &'static str = "{id: a, type: Odd, size: 1}";
    fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
        None
    }
}

#[test]
fn parameters_without_a_normalized_form_are_errors_not_panics() {
    assert_eq!(
        errors_with::<NotAMap>(),
        [
            "components[0] @ 4:14 the parameters of component type `Odd` cannot be normalized: \
          the parameters did not serialize to a mapping"
        ]
    );
    assert_eq!(
        errors_with::<Reserved>(),
        [
            "components[0] @ 4:14 the parameters of component type `Odd` cannot be normalized: \
          the parameters serialize the reserved key `id`"
        ]
    );
    assert_eq!(
        errors_with::<Failing>(),
        [
            "components[0] @ 4:14 the parameters of component type `Odd` cannot be normalized: \
          serialization failed: no normalized form"
        ]
    );
    let huge = errors_with::<Huge>();
    assert_eq!(huge.len(), 1);
    assert!(huge[0].starts_with("(document) @ 1:1 the parameters of component type `(document)` cannot be normalized: the number"), "{huge:?}");
}

#[test]
fn a_component_type_that_rejects_without_a_reason_still_gives_one() {
    assert_eq!(
        errors_with::<Silent>(),
        [
            "components[0] @ 4:14 a mapping is not allowed; allowed: the parameters of Odd",
            "components[1] @ 4:34 a mapping is not allowed; allowed: the parameters of Odd",
        ]
    );
}

/// A resolver whose file is not UTF-8.
struct Latin1;

impl Resolver for Latin1 {
    fn read(&self, _: &str) -> io::Result<Vec<u8>> {
        Ok(b"observe: [caf\xe9]\n".to_vec())
    }
}

#[test]
fn an_included_file_that_is_not_utf8() {
    let catalog = support::catalog();
    let text = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 8h}\ninclude: [x.yaml]\n";
    let errors = Loader::new(&catalog)
        .with_resolver(&Latin1)
        .load_str(text, Format::Yaml)
        .unwrap_err();
    assert_eq!(errors.first().file(), Some("x.yaml"));
    assert_eq!(
        errors.first().kind(),
        &LoadErrorKind::NotUtf8 { offset: 13 }
    );
}

#[test]
fn expression_errors_in_scalars_with_escapes_point_at_the_scalar() {
    let catalog = support::catalog();
    let model = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 8h}\n\
                 components: [{id: s, type: Sink}, {id: t, type: Sink}]\nflow: [[s, t]]\n\
                 observe:\n  - \"thr\\x6Fughput(snk)\"\n  - >-\n    throughput(\n    tt)\n";
    let errors = Loader::new(&catalog)
        .load_str(model, Format::Yaml)
        .unwrap_err();
    // Exact columns where the text is as written; the scalar's start
    // where escapes or folding change it (for a block scalar, the start of
    // its content).
    assert_eq!(
        summary(&errors),
        ["observe[0] @ 7:5 Expression", "observe[1] @ 9:5 Expression"]
    );
}

/// One value of every syntax-level variant.
#[allow(clippy::unwrap_used)] // Test helper: the inputs are fixed.
fn syntax_kinds() -> Vec<LoadErrorKind> {
    let at = Location::new(1, 1);
    vec![
        LoadErrorKind::Io {
            error: io::ErrorKind::NotFound,
        },
        LoadErrorKind::NotUtf8 { offset: 3 },
        LoadErrorKind::Yaml {
            problem: "bad".into(),
        },
        LoadErrorKind::Json {
            problem: JsonProblem::TrailingComma,
        },
        LoadErrorKind::TooDeep { limit: 64 },
        LoadErrorKind::Empty,
        LoadErrorKind::MultipleDocuments,
        LoadErrorKind::Anchor,
        LoadErrorKind::Alias,
        LoadErrorKind::Tag { tag: "!x".into() },
        LoadErrorKind::MergeKey,
        LoadErrorKind::ComplexKey,
        LoadErrorKind::NonStringKey {
            found: Found::Int(1),
        },
        LoadErrorKind::DuplicateKey {
            key: "a".into(),
            first: at,
        },
        LoadErrorKind::Ambiguous {
            text: "yes".into(),
            reading: Ambiguity::Boolean,
        },
        LoadErrorKind::Ambiguous {
            text: "012".into(),
            reading: Ambiguity::Number,
        },
        LoadErrorKind::Ambiguous {
            text: "2026-10-26".into(),
            reading: Ambiguity::Timestamp,
        },
        LoadErrorKind::IntegerRange {
            text: "9007199254740992".into(),
        },
        LoadErrorKind::NotFinite {
            text: "1e999".into(),
        },
    ]
}

/// One value of every variant after parsing.
#[allow(clippy::unwrap_used)] // Test helper: the inputs are fixed.
fn document_kinds() -> Vec<LoadErrorKind> {
    let at = Location::new(1, 1);
    let place = Place::new(Some("a.yaml".into()), at);
    vec![
        LoadErrorKind::WrongType {
            expected: "text",
            example: "a",
            found: Found::Null,
        },
        LoadErrorKind::MissingField {
            field: "a",
            expected: "text",
            example: "a",
        },
        LoadErrorKind::UnknownField {
            field: "b".into(),
            allowed: vec!["a".into()],
            suggestion: Some("a".into()),
        },
        LoadErrorKind::UnknownField {
            field: "zz".into(),
            allowed: vec!["a".into()],
            suggestion: None,
        },
        LoadErrorKind::UnknownValue {
            what: "component type",
            found: "X".into(),
            allowed: vec![],
            suggestion: None,
        },
        LoadErrorKind::Unit("42sec".parse::<taktsim_units::SimDuration>().unwrap_err()),
        LoadErrorKind::Unit("2.5 parts".parse::<taktsim_units::Count>().unwrap_err()),
        LoadErrorKind::Unit(
            "0.0000000001s"
                .parse::<taktsim_units::SimDuration>()
                .unwrap_err(),
        ),
        LoadErrorKind::Unit("x".parse::<taktsim_units::SimDuration>().unwrap_err()),
        LoadErrorKind::Unit(UnitError::ZeroDuration),
        LoadErrorKind::Distribution(DistributionError::NoPhases {
            example: "{dist: erlang, mean: 1min, k: 2}",
        }),
        LoadErrorKind::Expression {
            text: "a +".into(),
            error: parse("a +").unwrap_err(),
        },
        LoadErrorKind::NotAllowed {
            found: "0s".into(),
            allowed: "> 0s".into(),
            example: "8h".into(),
        },
        LoadErrorKind::UnsupportedFormat {
            found: "2.0".into(),
        },
        LoadErrorKind::InvalidId {
            what: "component id",
            found: "a b".into(),
        },
        LoadErrorKind::DuplicateId {
            id: "a".into(),
            first: place.clone(),
        },
        LoadErrorKind::CaseClash {
            id: "A".into(),
            other: "a".into(),
            first: place.clone(),
        },
        LoadErrorKind::UnknownComponent {
            id: "snk".into(),
            suggestion: Some("sink".into()),
        },
        LoadErrorKind::TooFew {
            what: "a chain",
            min: 2,
            found: 1,
            example: "[a, b]",
        },
        LoadErrorKind::Repeated { id: "a".into() },
        LoadErrorKind::SelfLoop { id: "a".into() },
        LoadErrorKind::DuplicateEdge {
            from: "a".into(),
            to: "b".into(),
            first: place.clone(),
        },
        LoadErrorKind::DuplicateObservable {
            text: "wip(b)".into(),
            first: place.clone(),
        },
        LoadErrorKind::NotAnObservable { text: "1".into() },
        LoadErrorKind::SteadyStateUnsupported,
    ]
}

/// One value of every variant about `include:`.
fn include_kinds() -> Vec<LoadErrorKind> {
    let place = Place::new(Some("a.yaml".into()), Location::new(1, 1));
    vec![
        LoadErrorKind::IncludePath {
            path: "../a.yaml".into(),
            problem: IncludePathProblem::ParentDirectory,
        },
        LoadErrorKind::IncludeUnavailable,
        LoadErrorKind::NestedInclude,
        LoadErrorKind::IncludeSelf {
            path: "a.yaml".into(),
        },
        LoadErrorKind::DuplicateInclude {
            path: "a.yaml".into(),
            first: place,
        },
        LoadErrorKind::IncludeHash {
            path: "a.yaml".into(),
            pinned: "ab".into(),
            actual: "cd".into(),
        },
        LoadErrorKind::Normalize {
            type_name: "X".into(),
            problem: NormalizeProblem::Number("1e30".into()),
        },
    ]
}

/// One value of every variant: adding a variant fails to compile until it
/// is listed in `listed`, and the test below fails until it is in one of
/// the lists.
fn every_kind() -> Vec<LoadErrorKind> {
    let kinds: Vec<LoadErrorKind> = syntax_kinds()
        .into_iter()
        .chain(document_kinds())
        .chain(include_kinds())
        .collect();
    for kind in &kinds {
        listed(kind);
    }
    kinds
}

/// Compiles only while `every_kind` can list every variant.
fn listed(kind: &LoadErrorKind) {
    match kind {
        LoadErrorKind::Io { .. }
        | LoadErrorKind::NotUtf8 { .. }
        | LoadErrorKind::Yaml { .. }
        | LoadErrorKind::Json { .. }
        | LoadErrorKind::TooDeep { .. }
        | LoadErrorKind::Empty
        | LoadErrorKind::MultipleDocuments
        | LoadErrorKind::Anchor
        | LoadErrorKind::Alias
        | LoadErrorKind::Tag { .. }
        | LoadErrorKind::MergeKey
        | LoadErrorKind::ComplexKey
        | LoadErrorKind::NonStringKey { .. }
        | LoadErrorKind::DuplicateKey { .. }
        | LoadErrorKind::Ambiguous { .. }
        | LoadErrorKind::IntegerRange { .. }
        | LoadErrorKind::NotFinite { .. }
        | LoadErrorKind::WrongType { .. }
        | LoadErrorKind::MissingField { .. }
        | LoadErrorKind::UnknownField { .. }
        | LoadErrorKind::UnknownValue { .. }
        | LoadErrorKind::Unit(_)
        | LoadErrorKind::Distribution(_)
        | LoadErrorKind::Expression { .. }
        | LoadErrorKind::NotAllowed { .. }
        | LoadErrorKind::UnsupportedFormat { .. }
        | LoadErrorKind::InvalidId { .. }
        | LoadErrorKind::DuplicateId { .. }
        | LoadErrorKind::CaseClash { .. }
        | LoadErrorKind::UnknownComponent { .. }
        | LoadErrorKind::TooFew { .. }
        | LoadErrorKind::Repeated { .. }
        | LoadErrorKind::SelfLoop { .. }
        | LoadErrorKind::DuplicateEdge { .. }
        | LoadErrorKind::DuplicateObservable { .. }
        | LoadErrorKind::NotAnObservable { .. }
        | LoadErrorKind::SteadyStateUnsupported
        | LoadErrorKind::IncludePath { .. }
        | LoadErrorKind::IncludeUnavailable
        | LoadErrorKind::NestedInclude
        | LoadErrorKind::IncludeSelf { .. }
        | LoadErrorKind::DuplicateInclude { .. }
        | LoadErrorKind::IncludeHash { .. }
        | LoadErrorKind::Normalize { .. } => {}
    }
}

#[test]
fn every_error_kind_has_a_message_what_was_found_and_what_is_allowed() {
    for kind in every_kind() {
        let message = kind.to_string();
        assert!(!message.is_empty(), "{kind:?}");
        assert!(kind.found().is_some_and(|f| !f.is_empty()), "{kind:?}");
        assert!(kind.allowed().is_some(), "{kind:?}");
        // An example, if given, is never empty.
        assert!(kind.example().is_none_or(|e| !e.is_empty()), "{kind:?}");
        assert_eq!(kind.is_io(), matches!(kind, LoadErrorKind::Io { .. }));
    }
}

#[test]
fn messages_name_the_fix() {
    let text = |k: LoadErrorKind| k.to_string();
    assert_eq!(
        text(LoadErrorKind::Ambiguous {
            text: "no".into(),
            reading: Ambiguity::Boolean
        }),
        "`no` is read as a boolean by some YAML parsers and as text by others; quote it for text, \
         or write a JSON boolean: true or false"
    );
    assert_eq!(
        text(LoadErrorKind::UnknownValue {
            what: "component type",
            found: "X".into(),
            allowed: vec![],
            suggestion: None
        }),
        "unknown component type `X`; allowed: none (no component types are registered)"
    );
    assert_eq!(
        text(LoadErrorKind::IncludeHash {
            path: "a.yaml".into(),
            pinned: "ab".into(),
            actual: "cd".into()
        }),
        "`a.yaml` has SHA-256 cd, but `include:` pins ab"
    );
    let hash = LoadErrorKind::IncludeHash {
        path: "a.yaml".into(),
        pinned: "ab".into(),
        actual: "cd".into(),
    };
    assert_eq!(
        hash.example().as_deref(),
        Some("{path: a.yaml, sha256: cd}")
    );
    for problem in [
        IncludePathProblem::Absolute,
        IncludePathProblem::Backslash,
        IncludePathProblem::ParentDirectory,
        IncludePathProblem::EmptySegment,
        IncludePathProblem::Character('*'),
        IncludePathProblem::Extension,
    ] {
        assert_ne!(problem.to_string(), "");
    }
    for problem in [
        JsonProblem::UnexpectedEnd,
        JsonProblem::UnexpectedChar('x'),
        JsonProblem::TrailingComma,
        JsonProblem::InvalidNumber,
        JsonProblem::InvalidEscape,
        JsonProblem::LoneSurrogate,
        JsonProblem::ControlCharacter,
        JsonProblem::InvalidLiteral,
        JsonProblem::TrailingContent,
    ] {
        assert_ne!(problem.to_string(), "");
    }
    for found in [
        Found::Null,
        Found::Bool(true),
        Found::Float(0.5),
        Found::List,
    ] {
        assert_ne!(found.to_string(), "");
    }
}
