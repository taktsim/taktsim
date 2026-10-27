//! `include:`: reusing parts of a model from other files, with content
//! hashing (spec §4 Rules, ADR-0002, ADR-0020).
//!
//! ```yaml
//! include:
//!   - cells/welding.yaml
//!   - {path: cells/paint.yaml, sha256: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08}
//! ```
//!
//! An included file holds any of `components`, `flow` and `observe`; their
//! entries come before the model file's own, in `include:` order. Paths are
//! relative to the model file, use `/`, and stay in its directory or below.
//! Every included file is hashed (SHA-256 of its bytes) and the hash is
//! recorded with the model; a `sha256` pin makes a changed file an error.
//! The model hash covers the resolved content, not the file names, so
//! moving components between files does not change it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::decode::{Decode, Decoder, Fields, Record, Schemas};
use crate::error::{IncludePathProblem, LoadErrorKind, Place, Segment};
use crate::json::Json;
use crate::syntax::{Format, Node, Value};

/// Reads the files a model includes.
///
/// [`Loader::load_file`](crate::Loader::load_file) reads them from the
/// model's directory ([`DirResolver`]); a map from path to text serves
/// models held in memory, such as in tests or in the browser.
///
/// ```
/// use std::collections::BTreeMap;
/// use taktsim_model::Resolver;
///
/// let files = BTreeMap::from([("cells/a.yaml".to_owned(), "observe: []".to_owned())]);
/// assert_eq!(files.read("cells/a.yaml")?, b"observe: []");
/// assert!(files.read("cells/b.yaml").is_err());
/// # Ok::<(), std::io::Error>(())
/// ```
pub trait Resolver {
    /// The bytes of the file at `path`, a checked relative path with `/`
    /// separators.
    ///
    /// # Errors
    ///
    /// The I/O error if the file cannot be read.
    fn read(&self, path: &str) -> io::Result<Vec<u8>>;
}

impl Resolver for BTreeMap<String, String> {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        self.get(path)
            .map(|text| text.as_bytes().to_vec())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

/// Reads included files from a directory: the model file's directory.
///
/// ```
/// use taktsim_model::{DirResolver, Resolver};
///
/// let resolver = DirResolver::new("models");
/// assert!(resolver.read("does/not/exist.yaml").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirResolver {
    root: PathBuf,
}

impl DirResolver {
    /// Resolves paths against `root`.
    ///
    /// ```
    /// let resolver = taktsim_model::DirResolver::new(".");
    /// ```
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Resolver for DirResolver {
    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        // Join segment by segment, so the platform's separator is used.
        let full = path.split('/').fold(self.root.clone(), |p, s| p.join(s));
        std::fs::read(full)
    }
}

/// One entry of `include:`.
#[derive(Debug, Clone)]
pub(crate) struct IncludeSpec {
    pub(crate) path: String,
    pub(crate) sha256: Option<String>,
    pub(crate) place: Place,
    pub(crate) field: crate::error::FieldPath,
}

impl IncludeSpec {
    /// The format of the included file, from its extension (checked when
    /// the path was read).
    pub(crate) fn format(&self) -> Format {
        Format::from_path(Path::new(&self.path)).unwrap_or(Format::Yaml)
    }
}

/// A file a model included, with the SHA-256 of its bytes.
///
/// ```
/// use std::collections::BTreeMap;
/// use taktsim_model::{Catalog, Format, Loader};
///
/// # let catalog = Catalog::new();
/// let files = BTreeMap::from([("more.yaml".to_owned(), "observe: []\n".to_owned())]);
/// let loader = Loader::new(&catalog).with_resolver(&files);
/// // A model without components does not load, but its includes are read.
/// let errors = loader.load_str("format: \"1.0\"\ninclude: [more.yaml]\n", Format::Yaml).unwrap_err();
/// assert!(errors.iter().all(|e| e.file().is_none()));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Included {
    path: String,
    sha256: String,
}

impl Included {
    pub(crate) const fn new(path: String, sha256: String) -> Self {
        Self { path, sha256 }
    }

    /// The path as written in `include:`.
    ///
    /// ```
    /// fn path(i: &taktsim_model::Included) -> &str {
    ///     i.path()
    /// }
    /// ```
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The SHA-256 of the file's bytes, as 64 lowercase hex digits.
    ///
    /// ```
    /// fn hash(i: &taktsim_model::Included) -> &str {
    ///     i.sha256()
    /// }
    /// ```
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Checks an include path: relative, `/` separators, portable file names,
/// no `..`, a `.yaml`, `.yml` or `.json` file.
pub(crate) fn check_path(path: &str) -> Result<(), IncludePathProblem> {
    if path.contains('\\') {
        return Err(IncludePathProblem::Backslash);
    }
    if path.starts_with('/') || path.as_bytes().get(1) == Some(&b':') {
        return Err(IncludePathProblem::Absolute);
    }
    for segment in path.split('/') {
        match segment {
            "" | "." => return Err(IncludePathProblem::EmptySegment),
            ".." => return Err(IncludePathProblem::ParentDirectory),
            _ => {}
        }
        if let Some(c) = segment
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(IncludePathProblem::Character(c));
        }
    }
    if Format::from_path(Path::new(path)).is_none() {
        return Err(IncludePathProblem::Extension);
    }
    Ok(())
}

/// An include path.
struct IncludePath(String);

impl Decode for IncludePath {
    const EXPECTED: &'static str = "a relative path to a .yaml, .yml or .json file";
    const EXAMPLE: &'static str = "cells/welding.yaml";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let path = String::decode(node, cx)?;
        if let Err(problem) = check_path(&path) {
            cx.error(node.start(), LoadErrorKind::IncludePath { path, problem });
            return None;
        }
        Some(Self(path))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("string")),
            (
                "description",
                Json::from(
                    "A path relative to the model file, with `/` separators, in its directory or \
                     below, to a .yaml, .yml or .json file.",
                ),
            ),
            (
                "pattern",
                Json::from(format!(
                    r"^(?:{SEGMENT}/)*[A-Za-z0-9._-]*\.(?:[Yy][Aa]?[Mm][Ll]|[Jj][Ss][Oo][Nn])$"
                )),
            ),
            (
                "examples",
                Json::Array(vec![Json::from("cells/welding.yaml")]),
            ),
        ])
    }
}

/// One segment of an include path in the schema's pattern: letters,
/// digits, `.`, `_`, `-`, but not `.` or `..` alone.
const SEGMENT: &str = r"(?:[A-Za-z0-9_-]|\.[A-Za-z0-9_-]|\.\.[A-Za-z0-9._-])[A-Za-z0-9._-]*";

/// A pinned hash: 64 lowercase hex digits.
struct Sha256(String);

impl Decode for Sha256 {
    const EXPECTED: &'static str = "a SHA-256 as 64 lowercase hex digits";
    const EXAMPLE: &'static str =
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let hash = String::decode(node, cx)?;
        if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            cx.not_allowed(node, hash, Self::EXPECTED, Self::EXAMPLE);
            return None;
        }
        Some(Self(hash))
    }

    fn schema(_: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("string")),
            ("pattern", Json::from("^[0-9a-f]{64}$")),
        ])
    }
}

/// An entry of `include:` written as a mapping.
struct IncludeMap {
    path: IncludePath,
    sha256: Option<Sha256>,
}

impl Record for IncludeMap {
    const EXPECTED: &'static str = IncludeEntry::EXPECTED;
    const EXAMPLE: &'static str = "{path: cells/welding.yaml, sha256: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08}";

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        let path = fields.required("path");
        let sha256 = fields.optional("sha256");
        Some(Self {
            path: path?,
            sha256: sha256?,
        })
    }
}

/// One entry of `include:`: a path, or `{path, sha256}`.
struct IncludeEntry(IncludeSpec);

impl Decode for IncludeEntry {
    const EXPECTED: &'static str = "a path, or a mapping with `path` and `sha256`";
    const EXAMPLE: &'static str = "cells/welding.yaml";
    const SCHEMA_NAME: Option<&'static str> = Some("Include");

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        let place = Place::new(cx.file().map(str::to_owned), node.start());
        let (path, sha256) = match node.value() {
            Value::Str(_) => (IncludePath::decode(node, cx)?.0, None),
            Value::Map(_) => {
                let entry = IncludeMap::decode(node, cx)?;
                (entry.path.0, entry.sha256.map(|h| h.0))
            }
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::List(_) => {
                cx.wrong_type::<Self>(node);
                return None;
            }
        };
        Some(Self(IncludeSpec {
            path,
            sha256,
            place,
            field: cx.path().clone(),
        }))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            (
                "description",
                Json::from(
                    "A file whose components, flow and observables come before the model's own; \
                     optionally pinned to the SHA-256 of its content.",
                ),
            ),
            (
                "oneOf",
                Json::Array(vec![
                    schemas.of::<IncludePath>(),
                    schemas.of::<IncludeMap>(),
                ]),
            ),
        ])
    }
}

/// `include:`: a list of entries, each file once (ignoring case, so the
/// model reads the same on case-insensitive file systems).
pub(crate) struct Includes(pub(crate) Vec<IncludeSpec>);

impl Decode for Includes {
    const EXPECTED: &'static str = "a list of paths";
    const EXAMPLE: &'static str = "[cells/welding.yaml]";

    fn decode(node: &Node, cx: &mut Decoder) -> Option<Self> {
        if !matches!(node.value(), Value::List(_)) {
            cx.wrong_type::<Self>(node);
            return None;
        }
        let specs: Vec<IncludeSpec> = cx
            .list(node, IncludeEntry::decode)?
            .into_iter()
            .map(|e| e.0)
            .collect();
        let mut seen: BTreeMap<String, &IncludeSpec> = BTreeMap::new();
        let mut ok = true;
        for (i, spec) in specs.iter().enumerate() {
            if let Some(first) = seen.get(&spec.path.to_ascii_lowercase()) {
                let kind = LoadErrorKind::DuplicateInclude {
                    path: spec.path.clone(),
                    first: first.place.clone(),
                };
                let at = spec.place.location();
                cx.within(Segment::Index(i), |cx| cx.error(at, kind));
                ok = false;
            } else {
                seen.insert(spec.path.to_ascii_lowercase(), spec);
            }
        }
        ok.then_some(Self(specs))
    }

    fn schema(schemas: &mut Schemas) -> Json {
        Json::object([
            ("type", Json::from("array")),
            ("items", schemas.of::<IncludeEntry>()),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_portable_and_inside_the_model_directory() {
        for good in [
            "a.yaml",
            "cells/weld-1.yml",
            "x/y/z_2.json",
            "A.YAML",
            ".hidden.yaml",
        ] {
            assert_eq!(check_path(good), Ok(()), "{good}");
        }
        let cases = [
            ("/a.yaml", IncludePathProblem::Absolute),
            ("C:a.yaml", IncludePathProblem::Absolute),
            ("cells\\a.yaml", IncludePathProblem::Backslash),
            ("../a.yaml", IncludePathProblem::ParentDirectory),
            ("cells/../a.yaml", IncludePathProblem::ParentDirectory),
            ("./a.yaml", IncludePathProblem::EmptySegment),
            ("cells//a.yaml", IncludePathProblem::EmptySegment),
            ("a b.yaml", IncludePathProblem::Character(' ')),
            ("ä.yaml", IncludePathProblem::Character('ä')),
            ("a.txt", IncludePathProblem::Extension),
            ("cells/", IncludePathProblem::EmptySegment),
        ];
        for (path, problem) in cases {
            assert_eq!(check_path(path), Err(problem), "{path}");
        }
    }

    #[test]
    fn directories_resolve_segment_by_segment() {
        let dir = std::env::temp_dir().join(format!("taktsim-include-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("cells")).unwrap();
        std::fs::write(dir.join("cells").join("a.yaml"), "x").unwrap();
        let resolver = DirResolver::new(&dir);
        assert_eq!(resolver.read("cells/a.yaml").unwrap(), b"x");
        assert_eq!(
            resolver.read("cells/b.yaml").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
