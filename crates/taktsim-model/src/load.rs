//! The loader: text → typed, checked, normalized model with its hash
//! (ADR-0002's pipeline; ADR-0020).
//!
//! 1. Parse the YAML subset or JSON into a tree with locations.
//! 2. Decode the model file strictly (unknown keys rejected, every error
//!    with its field path and location), reading component parameters with
//!    the catalog's component types.
//! 3. Resolve `include:`: read, hash and decode each file.
//! 4. Check what spans the document: ids unique, every reference names a
//!    component, the flow graph, the observables.
//! 5. Normalize, canonicalize (RFC 8785) and hash (SHA-256).

use std::collections::BTreeMap;
use std::path::Path;

use crate::component::{Catalog, Component};
use crate::decode::{Decode, Decoder};
use crate::document::{
    Document, Edge, MainFile, Observable, Parts, Precision, RawExpr, read_main, read_parts,
};
use crate::error::{
    FieldPath, LoadError, LoadErrorKind, LoadErrors, Location, Place, Segment, suggest,
};
use crate::expr::{Expected, ExprKind, TypeEnv, check, parse};
use crate::hash::ModelHash;
use crate::include::{DirResolver, Included, Resolver};
use crate::jcs::canonicalize;
use crate::syntax::{self, Format, Node};

/// A model that loaded: its typed document, canonical form and hash.
///
/// ```
/// use taktsim_model::{Catalog, ComponentParams, Encode, Fields, Format, Json, Loader};
///
/// #[derive(Debug)]
/// struct Station {}
///
/// impl Encode for Station {
///     fn encode(&self) -> Json {
///         Json::object([])
///     }
/// }
///
/// impl ComponentParams for Station {
///     const TYPE: &'static str = "Station";
///     const EXAMPLE: &'static str = "{id: a, type: Station}";
///     fn decode(_: &mut Fields<'_, '_>) -> Option<Self> {
///         Some(Self {})
///     }
/// }
///
/// let catalog = Catalog::new().with::<Station>()?;
/// let yaml = "format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: terminating, horizon: 0.5h}\n\
///             components: [{id: a, type: Station}, {id: b, type: Station}]\nflow: [[a, b]]\n";
/// let json = r#"{"flow": [{"from": "a", "to": ["b"]}], "format": "1.0",
///     "experiment": {"horizon": "30min", "type": "terminating"}, "model": {"name": "m"},
///     "components": [{"type": "Station", "id": "a"}, {"id": "b", "type": "Station"}]}"#;
/// let loader = Loader::new(&catalog);
/// let a = loader.load_str(yaml, Format::Yaml)?;
/// let b = loader.load_str(json, Format::Json)?;
/// assert_eq!(a.hash(), b.hash());
/// assert!(a.canonical_json().starts_with(r#"{"components":[{"id":"a","type":"Station"}"#));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct Model {
    document: Document,
    canonical: String,
    hash: ModelHash,
    includes: Vec<Included>,
}

impl Model {
    /// The typed document.
    ///
    /// ```
    /// fn name(model: &taktsim_model::Model) -> &str {
    ///     model.document().model().name()
    /// }
    /// ```
    #[must_use]
    pub const fn document(&self) -> &Document {
        &self.document
    }

    /// The model hash: SHA-256 of [`canonical_json`](Self::canonical_json).
    ///
    /// ```
    /// fn hash(model: &taktsim_model::Model) -> String {
    ///     model.hash().to_string()
    /// }
    /// ```
    #[must_use]
    pub const fn hash(&self) -> ModelHash {
        self.hash
    }

    /// The normalized model as canonical JSON (RFC 8785): includes
    /// resolved, shorthands expanded, quantities in canonical text, absent
    /// and empty optional sections left out. It is itself a valid model
    /// file with the same hash.
    ///
    /// ```
    /// fn size(model: &taktsim_model::Model) -> usize {
    ///     model.canonical_json().len()
    /// }
    /// ```
    #[must_use]
    pub fn canonical_json(&self) -> &str {
        &self.canonical
    }

    /// The included files with the SHA-256 of their content, in `include:`
    /// order: the provenance of a model loaded from several files.
    ///
    /// ```
    /// fn files(model: &taktsim_model::Model) -> Vec<&str> {
    ///     model.includes().iter().map(|i| i.path()).collect()
    /// }
    /// ```
    #[must_use]
    pub fn includes(&self) -> &[Included] {
        &self.includes
    }
}

/// Loads models with the component types and observables of a catalog.
///
/// ```
/// use taktsim_model::{Catalog, Format, LoadErrorKind, Loader};
///
/// let catalog = Catalog::new();
/// let errors = Loader::new(&catalog)
///     .load_str("format: \"1.0\"\nmodel: {name: m}\nexperiment: {type: steady_state, horizon: 8h}\n", Format::Yaml)
///     .unwrap_err();
/// assert!(errors.iter().any(|e| e.kind() == &LoadErrorKind::SteadyStateUnsupported));
/// ```
#[derive(Clone, Copy)]
pub struct Loader<'a> {
    catalog: &'a Catalog,
    resolver: Option<&'a dyn Resolver>,
}

impl std::fmt::Debug for Loader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loader")
            .field("catalog", self.catalog)
            .field("resolver", &self.resolver.is_some())
            .finish()
    }
}

/// A file being loaded: its name for errors, and its text for locating
/// errors inside expressions.
struct Source {
    name: Option<String>,
    text: String,
}

impl<'a> Loader<'a> {
    /// A loader for models made of `catalog`'s component types.
    ///
    /// ```
    /// let catalog = taktsim_model::Catalog::new();
    /// let loader = taktsim_model::Loader::new(&catalog);
    /// ```
    #[must_use]
    pub const fn new(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            resolver: None,
        }
    }

    /// Reads included files with `resolver`, also for models given as
    /// text. Without one, [`load_file`](Self::load_file) reads them from
    /// the model's directory and [`load_str`](Self::load_str) rejects
    /// `include:`.
    ///
    /// ```
    /// use std::collections::BTreeMap;
    ///
    /// let catalog = taktsim_model::Catalog::new();
    /// let files: BTreeMap<String, String> = BTreeMap::new();
    /// let loader = taktsim_model::Loader::new(&catalog).with_resolver(&files);
    /// ```
    #[must_use]
    pub const fn with_resolver(mut self, resolver: &'a dyn Resolver) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Loads a model from `text`.
    ///
    /// # Errors
    ///
    /// Every problem found, in reading order.
    ///
    /// ```
    /// use taktsim_model::{Catalog, Format, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_str("{}", Format::Json).unwrap_err();
    /// let paths: Vec<String> = errors.iter().map(|e| e.path().to_string()).collect();
    /// assert_eq!(paths, ["format", "model", "experiment", "components", "flow"]);
    /// ```
    pub fn load_str(&self, text: &str, format: Format) -> Result<Model, LoadErrors> {
        let source = Source {
            name: None,
            text: text.to_owned(),
        };
        self.load(source, format, self.resolver, None)
    }

    /// Loads the model file at `path`; its format is chosen by its
    /// extension, and included files are read from its directory unless a
    /// resolver is set.
    ///
    /// # Errors
    ///
    /// Every problem found, in reading order; [`LoadErrorKind::Io`] if a
    /// file cannot be read.
    ///
    /// ```
    /// use std::path::Path;
    /// use taktsim_model::{Catalog, LoadErrorKind, Loader};
    ///
    /// let errors = Loader::new(&Catalog::new()).load_file(Path::new("missing.yaml")).unwrap_err();
    /// assert!(errors.first().kind().is_io());
    /// assert_eq!(errors.first().file(), Some("missing.yaml"));
    /// ```
    pub fn load_file(&self, path: &Path) -> Result<Model, LoadErrors> {
        let name = path.display().to_string();
        let single = |kind| {
            LoadErrors::single(LoadError::new(
                Some(name.clone()),
                None,
                FieldPath::default(),
                kind,
            ))
        };
        let Some(format) = Format::from_path(path) else {
            return Err(single(LoadErrorKind::NotAllowed {
                found: name.clone(),
                allowed: "a file ending in .yaml, .yml or .json".to_owned(),
                example: "line-a.yaml".to_owned(),
            }));
        };
        let bytes =
            std::fs::read(path).map_err(|e| single(LoadErrorKind::Io { error: e.kind() }))?;
        let text = String::from_utf8(bytes).map_err(|e| {
            single(LoadErrorKind::NotUtf8 {
                offset: e.utf8_error().valid_up_to(),
            })
        })?;
        let dir = DirResolver::new(path.parent().unwrap_or_else(|| Path::new("")));
        let resolver = self.resolver.unwrap_or(&dir);
        let own_name = path.file_name().and_then(|n| n.to_str());
        let source = Source {
            name: Some(name.clone()),
            text,
        };
        self.load(source, format, Some(resolver), own_name)
    }

    fn load(
        &self,
        main: Source,
        format: Format,
        resolver: Option<&dyn Resolver>,
        own_name: Option<&str>,
    ) -> Result<Model, LoadErrors> {
        let mut run = Run {
            catalog: self.catalog,
            sources: vec![],
            errors: vec![],
        };
        let result = run.load(main, format, resolver, own_name);
        let Run {
            sources,
            mut errors,
            ..
        } = run;
        // Reading order: by file (the model, then its includes), then by
        // location; errors without one first. The sort is stable, so ties
        // keep the order they were found in.
        let file_index = |e: &LoadError| {
            sources
                .iter()
                .position(|s| s.name.as_deref() == e.file())
                .unwrap_or(usize::MAX)
        };
        errors.sort_by_key(|e| (file_index(e), e.sort_key()));
        match (result, LoadErrors::from_vec(errors)) {
            (Some(model), None) => Ok(model),
            (_, Some(errors)) => Err(errors),
            (None, None) => Err(LoadErrors::single(LoadError::new(
                None,
                None,
                FieldPath::default(),
                LoadErrorKind::Empty,
            ))),
        }
    }
}

/// The lists of the model file and its includes, joined.
struct Merged {
    components: Vec<Component>,
    flow: Vec<Edge>,
    /// Where each edge was written (see `Parts::flow_places`).
    flow_places: Vec<crate::document::EdgePlaces>,
    observe: Vec<RawExpr>,
}

/// The state of one load.
struct Run<'a> {
    catalog: &'a Catalog,
    sources: Vec<Source>,
    errors: Vec<LoadError>,
}

impl Run<'_> {
    fn error(&mut self, place: &Place, path: FieldPath, kind: LoadErrorKind) {
        self.errors.push(LoadError::new(
            place.file().map(str::to_owned),
            Some(place.location()),
            path,
            kind,
        ));
    }

    /// Parses the text of source `index`, recording syntax errors.
    fn parse(&mut self, index: usize, format: Format) -> Option<Node> {
        let source = &self.sources[index];
        match syntax::parse(&source.text, format) {
            Ok(node) => Some(node),
            Err(errors) => {
                let name = source.name.clone();
                self.errors.extend(
                    errors
                        .into_iter()
                        .map(|e| LoadError::new(name.clone(), e.location, e.path, e.kind)),
                );
                None
            }
        }
    }

    fn load(
        &mut self,
        main: Source,
        format: Format,
        resolver: Option<&dyn Resolver>,
        own_name: Option<&str>,
    ) -> Option<Model> {
        let name = main.name.clone();
        self.sources.push(main);
        let root = self.parse(0, format)?;
        let mut cx = Decoder::new(name);
        let main = read_main(&root, &mut cx, self.catalog);
        self.errors.extend(cx.into_errors());
        let main = main?;

        let mut parts = Vec::new();
        let mut included = Vec::new();
        for spec in &main.includes {
            if resolver.is_none() {
                self.error(
                    &spec.place,
                    spec.field.clone(),
                    LoadErrorKind::IncludeUnavailable,
                );
                continue;
            }
            if own_name == Some(spec.path.as_str()) {
                let kind = LoadErrorKind::IncludeSelf {
                    path: spec.path.clone(),
                };
                self.error(&spec.place, spec.field.clone(), kind);
                continue;
            }
            let Some(resolver) = resolver else { continue };
            if let Some((fragment, sha256)) = self.include(spec, resolver) {
                parts.push(fragment);
                included.push(Included::new(spec.path.clone(), sha256));
            }
        }
        // If a file could not be included, the checks across files would
        // report what it may have held as missing.
        if included.len() != main.includes.len() {
            return None;
        }
        let MainFile {
            model,
            experiment,
            parts: own,
            start,
            ..
        } = main;
        parts.push(own);
        let main_place = Place::new(self.sources[0].name.clone(), start);
        let Merged {
            components,
            flow,
            flow_places,
            observe,
        } = self.merge(parts, &main_place)?;

        self.check_ids(&components);
        self.check_references(&components, &flow);
        self.check_flow(&flow, &flow_places);
        let env = self.environment(&components);
        let observe = self.observables(&observe, &env, Expected::Any);
        let (model, experiment) = (model?, experiment?);
        let targets: Vec<RawExpr> = experiment
            .precision
            .iter()
            .map(|p| p.observable.clone())
            .collect();
        let checked = self.observables(&targets, &env, Expected::Numeric)?;
        let mut precision: Vec<Precision> = checked
            .into_iter()
            .zip(&experiment.precision)
            .map(|(observable, raw)| Precision::new(observable, raw.target))
            .collect();
        precision.sort_by(|a, b| a.observable().text().cmp(b.observable().text()));
        let observe = observe?;
        if !self.errors.is_empty() {
            return None;
        }
        let document = Document {
            model,
            experiment: experiment.finish(precision),
            components,
            flow,
            observe,
        };
        self.hash(document, included, &main_place)
    }

    /// Normalizes, canonicalizes and hashes a checked document.
    fn hash(
        &mut self,
        document: Document,
        included: Vec<Included>,
        main_place: &Place,
    ) -> Option<Model> {
        let json = match document.to_json() {
            Ok(json) => json,
            Err((component, kind)) => {
                self.error(&component.place().clone(), component.path().clone(), kind);
                return None;
            }
        };
        let canonical = match canonicalize(&json) {
            Ok(text) => text,
            Err(e) => {
                let kind = LoadErrorKind::Normalize {
                    type_name: "(document)".to_owned(),
                    problem: crate::error::NormalizeProblem::Number(e.to_string()),
                };
                self.error(main_place, FieldPath::default(), kind);
                return None;
            }
        };
        Some(Model {
            hash: ModelHash::of(canonical.as_bytes()),
            canonical,
            document,
            includes: included,
        })
    }

    /// Reads, hashes and decodes one included file.
    fn include(
        &mut self,
        spec: &crate::include::IncludeSpec,
        resolver: &dyn Resolver,
    ) -> Option<(Parts, String)> {
        let name = Some(spec.path.clone());
        let file_error = |kind| LoadError::new(name.clone(), None, FieldPath::default(), kind);
        // Registered before reading, so errors follow `include:` order even
        // for a file that cannot be read.
        self.sources.push(Source {
            name: name.clone(),
            text: String::new(),
        });
        let index = self.sources.len() - 1;
        let bytes = match resolver.read(&spec.path) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.errors
                    .push(file_error(LoadErrorKind::Io { error: e.kind() }));
                return None;
            }
        };
        let sha256 = ModelHash::of(&bytes).to_string();
        if let Some(pinned) = &spec.sha256
            && *pinned != sha256
        {
            let kind = LoadErrorKind::IncludeHash {
                path: spec.path.clone(),
                pinned: pinned.clone(),
                actual: sha256,
            };
            self.error(&spec.place, spec.field.clone(), kind);
            return None;
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(e) => {
                let offset = e.utf8_error().valid_up_to();
                self.errors
                    .push(file_error(LoadErrorKind::NotUtf8 { offset }));
                return None;
            }
        };
        self.sources[index].text = text;
        let root = self.parse(index, spec.format())?;
        let mut cx = Decoder::new(name);
        let parts = read_fragment(&root, &mut cx, self.catalog);
        self.errors.extend(cx.into_errors());
        Some((parts?, sha256))
    }

    /// Joins the lists of every file, included files first; checks that
    /// the model has components and a flow.
    fn merge(&mut self, parts: Vec<Parts>, main: &Place) -> Option<Merged> {
        let mut components = Vec::new();
        let mut flow = Vec::new();
        let mut flow_places = Vec::new();
        let mut observe = Vec::new();
        let (mut has_components, mut has_flow, mut complete) = (false, false, true);
        for part in parts {
            has_components |= part.has_components;
            has_flow |= part.has_flow;
            complete &= part.complete;
            components.extend(part.components.unwrap_or_default());
            flow.extend(part.flow.unwrap_or_default());
            flow_places.extend(part.flow_places);
            observe.extend(part.observe.unwrap_or_default());
        }
        for (key, present, empty, expected, example) in [
            (
                "components",
                has_components,
                components.is_empty(),
                "a list of components",
                "[{id: cut, type: Machine, cycle_time: 42s}]",
            ),
            (
                "flow",
                has_flow,
                flow.is_empty(),
                "a list of chains and edges",
                "[[source, cut, sink]]",
            ),
        ] {
            let path = FieldPath::from(vec![Segment::Key(key.to_owned())]);
            if !present {
                let kind = LoadErrorKind::MissingField {
                    field: key,
                    expected,
                    example,
                };
                self.error(main, path, kind);
            } else if empty && complete {
                let kind = LoadErrorKind::TooFew {
                    what: if key == "flow" {
                        "the flow"
                    } else {
                        "the model's components"
                    },
                    min: 1,
                    found: 0,
                    example,
                };
                self.error(main, path, kind);
            }
        }
        // Without components or a flow, the checks across sections would
        // only repeat that error.
        let usable = complete && !components.is_empty() && !flow.is_empty();
        usable.then_some(Merged {
            components,
            flow,
            flow_places,
            observe,
        })
    }

    /// Ids are unique, also ignoring case.
    fn check_ids(&mut self, components: &[Component]) {
        let mut exact: BTreeMap<&str, &Component> = BTreeMap::new();
        let mut folded: BTreeMap<String, &Component> = BTreeMap::new();
        for c in components {
            let (place, path) = c.id_place();
            if let Some(first) = exact.get(c.id()) {
                let kind = LoadErrorKind::DuplicateId {
                    id: c.id().to_owned(),
                    first: first.id_place().0.clone(),
                };
                self.error(place, path, kind);
            } else if let Some(other) = folded.get(&c.id().to_ascii_lowercase()) {
                let kind = LoadErrorKind::CaseClash {
                    id: c.id().to_owned(),
                    other: other.id().to_owned(),
                    first: other.id_place().0.clone(),
                };
                self.error(place, path, kind);
            } else {
                exact.insert(c.id(), c);
                folded.insert(c.id().to_ascii_lowercase(), c);
            }
        }
    }

    /// Every reference names a component.
    fn check_references(&mut self, components: &[Component], flow: &[Edge]) {
        let known: Vec<&str> = components.iter().map(Component::id).collect();
        let references = flow
            .iter()
            .flat_map(|e| e.from().iter().chain(e.to()))
            .chain(components.iter().flat_map(Component::references));
        // A chain's inner ids belong to two edges: report each place once.
        let mut unknown: Vec<crate::component::ComponentRef> = Vec::new();
        for r in references {
            let reported = unknown
                .iter()
                .any(|u| u.place() == r.place() && u.path() == r.path());
            if !known.contains(&r.id()) && !reported {
                unknown.push(r.clone());
            }
        }
        for r in unknown {
            let kind = LoadErrorKind::UnknownComponent {
                id: r.id().to_owned(),
                suggestion: suggest(r.id(), known.iter().copied()),
            };
            self.error(r.place(), r.path().clone(), kind);
        }
    }

    /// No edge from a component to itself; each edge once.
    fn check_flow(&mut self, flow: &[Edge], places: &[Vec<(Place, FieldPath)>]) {
        let mut seen: BTreeMap<(&str, &str), Place> = BTreeMap::new();
        for (edge, places) in flow.iter().zip(places) {
            for (i, to) in edge.to().iter().enumerate() {
                // The place of each `to` reference follows the entry's.
                let Some((place, path)) = places.get(i + 1).or_else(|| places.first()).cloned()
                else {
                    continue;
                };
                for from in edge.from() {
                    if from.id() == to.id() {
                        let kind = LoadErrorKind::SelfLoop {
                            id: to.id().to_owned(),
                        };
                        self.error(&place, path.clone(), kind);
                    } else if let Some(first) = seen.get(&(from.id(), to.id())) {
                        let kind = LoadErrorKind::DuplicateEdge {
                            from: from.id().to_owned(),
                            to: to.id().to_owned(),
                            first: first.clone(),
                        };
                        self.error(&place, path.clone(), kind);
                    } else {
                        seen.insert((from.id(), to.id()), place.clone());
                    }
                }
            }
        }
    }

    /// The expression environment: the catalog's observables and the
    /// model's components (each id once).
    fn environment(&self, components: &[Component]) -> TypeEnv {
        let mut env = self.catalog.environment().clone();
        let mut added: Vec<&str> = Vec::new();
        for c in components {
            if added.contains(&c.id()) {
                continue;
            }
            // Ids have the form `with_component` requires and are unique
            // here, so it cannot fail; if it did, the id is left out and
            // references to it are reported as unknown.
            if let Ok(next) = env.clone().with_component(c.id(), c.type_name()) {
                env = next;
                added.push(c.id());
            }
        }
        env
    }

    /// Checks observables: each parses, is a call of a catalog observable
    /// on existing components of accepted types, has the expected type, and
    /// appears once (by canonical text).
    fn observables(
        &mut self,
        raw: &[RawExpr],
        env: &TypeEnv,
        expected: Expected,
    ) -> Option<Vec<Observable>> {
        let mut out = Vec::with_capacity(raw.len());
        let mut seen: BTreeMap<String, Place> = BTreeMap::new();
        let mut ok = true;
        for r in raw {
            let place = r.place.clone();
            let checked = parse(&r.text).and_then(|expr| {
                check(&expr, env, expected)?;
                Ok(expr)
            });
            let expr = match checked {
                Ok(expr) => expr,
                Err(error) => {
                    let at = self.expression_location(&place, &r.text, error.span().start);
                    let kind = LoadErrorKind::Expression {
                        text: r.text.clone(),
                        error,
                    };
                    self.error(
                        &Place::new(place.file().map(str::to_owned), at),
                        r.path.clone(),
                        kind,
                    );
                    ok = false;
                    continue;
                }
            };
            if !matches!(expr.clone().into_kind(), ExprKind::Call(_)) {
                let kind = LoadErrorKind::NotAnObservable {
                    text: r.text.clone(),
                };
                self.error(&place, r.path.clone(), kind);
                ok = false;
                continue;
            }
            let text = expr.to_string();
            if let Some(first) = seen.get(&text) {
                let kind = LoadErrorKind::DuplicateObservable {
                    text,
                    first: first.clone(),
                };
                self.error(&place, r.path.clone(), kind);
                ok = false;
                continue;
            }
            seen.insert(text.clone(), place.clone());
            out.push(Observable::new(text, expr, place));
        }
        ok.then_some(out)
    }

    /// The location of byte `offset` of the expression `text`, written as
    /// a scalar starting at `place`: exact when the scalar is plain or
    /// quoted without escapes on one line, otherwise the scalar's start.
    fn expression_location(&self, place: &Place, text: &str, offset: usize) -> Location {
        let start = place.location();
        let source = self
            .sources
            .iter()
            .find(|s| s.name.as_deref() == place.file());
        let exact = source.and_then(|s| {
            let line = s.text.split('\n').nth(start.line().checked_sub(1)?)?;
            let byte = line.char_indices().nth(start.column().checked_sub(1)?)?.0;
            let rest = &line[byte..];
            let quote = if rest.starts_with(text) {
                0
            } else if rest.starts_with(['"', '\'']) && rest[1..].starts_with(text) {
                1
            } else {
                return None;
            };
            let prefix = text.get(..offset)?;
            (!prefix.contains('\n')).then(|| {
                Location::new(
                    start.line(),
                    start.column() + quote + prefix.chars().count(),
                )
            })
        });
        exact.unwrap_or(start)
    }
}

/// Reads an included file: any of `components`, `flow`, `observe`.
fn read_fragment(node: &Node, cx: &mut Decoder, catalog: &Catalog) -> Option<Parts> {
    /// For the error when the file is not a mapping.
    struct Fragment;
    impl Decode for Fragment {
        const EXPECTED: &'static str =
            "an included file: a mapping with any of `components`, `flow`, `observe`";
        const EXAMPLE: &'static str = "components: [{id: cut, type: Machine, cycle_time: 42s}]";
        fn decode(_: &Node, _: &mut Decoder) -> Option<Self> {
            None
        }
    }

    let mut fields = cx.fields::<Fragment>(node)?;
    if let Some(entry) = fields.entry("include") {
        let at = entry.key_start();
        fields.take("include");
        fields
            .decoder()
            .within(Segment::Key("include".to_owned()), |cx| {
                cx.error(at, LoadErrorKind::NestedInclude);
            });
    }
    let parts = read_parts(&mut fields, catalog);
    fields.finish();
    Some(parts)
}

/// Decodes a value of type `T` from `text`: for testing a component type's
/// parameters, or reading one value.
///
/// # Errors
///
/// Every problem found, with paths relative to the value.
///
/// ```
/// use taktsim_model::{Distribution, Format, decode_str};
///
/// let d: Distribution = decode_str("{dist: erlang, mean: 12min, k: 3}", Format::Yaml)?;
/// assert_eq!(d.name(), Some("erlang"));
/// # Ok::<(), taktsim_model::LoadErrors>(())
/// ```
pub fn decode_str<T: Decode>(text: &str, format: Format) -> Result<T, LoadErrors> {
    let node = syntax::parse(text, format).map_err(|errors| {
        let errors = errors
            .into_iter()
            .map(|e| LoadError::new(None, e.location, e.path, e.kind))
            .collect();
        LoadErrors::from_vec(errors).unwrap_or_else(|| LoadErrors::single(empty()))
    })?;
    let mut cx = Decoder::new(None);
    let value = T::decode(&node, &mut cx);
    match (value, LoadErrors::from_vec(cx.into_errors())) {
        (Some(value), None) => Ok(value),
        (_, Some(errors)) => Err(errors),
        (None, None) => Err(LoadErrors::single(empty())),
    }
}

/// The error for a value that could not be read without a reason given.
fn empty() -> LoadError {
    LoadError::new(None, None, FieldPath::default(), LoadErrorKind::Empty)
}
