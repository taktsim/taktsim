//! The run store (ADR-0019 item 5, ADR-0024 §8): `runs/<id>/` directories, written
//! in `tmp/` and renamed into place, so no half-written run is ever visible.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use taktsim_model::{Fields, Format, Record, decode_str};

use crate::diagnostic::{Diagnostic, Failure, FailureClass};
use crate::id::RunId;
use crate::result::ResultDocument;

/// The name of a store's directory.
const STORE_DIR: &str = ".taktsim";
/// The layout this build reads and writes.
const STORE_FORMAT: u64 = 1;
/// The result of a run.
pub(crate) const RESULT_FILE: &str = "result.json";
/// The canonical normalized model of a run.
pub(crate) const MODEL_FILE: &str = "model.json";
/// The trace of a run.
pub(crate) const TRACE_FILE: &str = "trace.takt";

/// What `path` failing means, as a failure of exit code 7. There is no valid value
/// to offer as an example (ADR-0024 §9).
pub(crate) fn io_failure(type_name: &str, path: &Path, error: &io::Error) -> Failure {
    let mut line = Diagnostic::error(type_name, format!("{}: {error}", path.display()))
        .at("--store")
        .found(error.kind().to_string())
        .allowed("a readable and writable directory");
    line.file = Some(path.display().to_string());
    Failure::one(FailureClass::Io, line)
}

/// A store's own file.
struct StoreFile {
    store_format: u64,
}

impl Record for StoreFile {
    const EXPECTED: &'static str = "`{\"store_format\": 1}`";
    const EXAMPLE: &'static str = "{\"store_format\": 1}";
    const OPEN: bool = true;

    fn read(fields: &mut Fields<'_, '_>) -> Option<Self> {
        Some(Self {
            store_format: fields.required("store_format")?,
        })
    }
}

/// A run store: a directory of finished runs by id. Opening one does not create
/// it; the first write does.
///
/// ```
/// use taktsim_run::Store;
///
/// let dir = std::env::temp_dir().join(format!("taktsim-doc-store-{}", std::process::id()));
/// let store = Store::open(&dir)?;
/// assert_eq!(store.root(), dir);
/// assert!(!dir.exists());
/// # Ok::<(), taktsim_run::Failure>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store in `root`.
    ///
    /// # Errors
    ///
    /// A [`Failure`] of class `Io` if `root` holds a `store.json` that cannot be
    /// read, or of a layout this build does not know.
    ///
    /// ```
    /// use taktsim_run::{FailureClass, Store};
    ///
    /// let dir = std::env::temp_dir().join(format!("taktsim-doc-open-{}", std::process::id()));
    /// std::fs::create_dir_all(&dir)?;
    /// std::fs::write(dir.join("store.json"), r#"{"store_format": 99}"#)?;
    /// assert_eq!(Store::open(&dir).unwrap_err().class, FailureClass::Io);
    /// std::fs::remove_dir_all(&dir)?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn open(root: &Path) -> Result<Self, Failure> {
        let store = Self {
            root: root.to_path_buf(),
        };
        store.check_format()?;
        Ok(store)
    }

    /// The nearest `.taktsim/` upward from `start`, like `.git`; if none exists,
    /// `.taktsim/` in `start`.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open).
    ///
    /// ```
    /// use taktsim_run::Store;
    ///
    /// let top = std::env::temp_dir().join(format!("taktsim-doc-discover-{}", std::process::id()));
    /// let below = top.join("a").join("b");
    /// std::fs::create_dir_all(&below)?;
    /// assert_eq!(Store::discover(&below)?.root(), below.join(".taktsim"));
    /// std::fs::create_dir_all(top.join(".taktsim"))?;
    /// assert_eq!(Store::discover(&below)?.root(), top.join(".taktsim"));
    /// std::fs::remove_dir_all(&top)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn discover(start: &Path) -> Result<Self, Failure> {
        let found = start
            .ancestors()
            .map(|dir| dir.join(STORE_DIR))
            .find(|dir| dir.is_dir());
        Self::open(&found.unwrap_or_else(|| start.join(STORE_DIR)))
    }

    /// Where the store is.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// let store = taktsim_run::Store::open(Path::new("no/such/store"))?;
    /// assert_eq!(store.root(), Path::new("no/such/store"));
    /// # Ok::<(), taktsim_run::Failure>(())
    /// ```
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn check_format(&self) -> Result<(), Failure> {
        let path = self.root.join("store.json");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_failure("store.read", &path, &e)),
        };
        let found = decode_str::<StoreFile>(&text, Format::Json)
            .map(|file| file.store_format)
            .ok();
        if found == Some(STORE_FORMAT) {
            return Ok(());
        }
        let mut line = Diagnostic::error(
            "store.format",
            format!(
                "{} is of a store layout this build does not read",
                path.display()
            ),
        )
        .at("--store")
        .found(text.trim().to_owned())
        .allowed(format!("`store_format` {STORE_FORMAT}"));
        line.file = Some(path.display().to_string());
        Err(Failure::one(FailureClass::Io, line))
    }

    /// Creates the store's own files on the first write. Each is written beside its
    /// place and renamed in, so a process starting on the same new store reads the
    /// whole file or none (ADR-0024 §8).
    fn ensure(&self) -> Result<(), Failure> {
        for dir in [self.root.join("runs"), self.root.join("tmp")] {
            fs::create_dir_all(&dir).map_err(|e| io_failure("store.write", &dir, &e))?;
        }
        let write_new = |name: &str, text: &str| {
            let path = self.root.join(name);
            if path.exists() {
                return Ok(());
            }
            let temp = self.temp_dir()?;
            let written = temp.file(name);
            fs::write(&written, text).map_err(|e| io_failure("store.write", &written, &e))?;
            match fs::rename(&written, &path) {
                Ok(()) => Ok(()),
                // Another process put the same bytes there first.
                Err(_) if path.is_file() => Ok(()),
                Err(e) => Err(io_failure("store.write", &path, &e)),
            }
        };
        write_new("store.json", "{\"store_format\": 1}\n")?;
        write_new(".gitignore", "*\n")
    }

    fn run_dir(&self, id: &RunId) -> PathBuf {
        self.root.join("runs").join(id.as_str())
    }

    /// The stored file `name` of the run `id`.
    pub(crate) fn run_file(&self, id: &RunId, name: &str) -> PathBuf {
        self.run_dir(id).join(name)
    }

    /// The stored result of the run `id`, if it is in the store.
    pub(crate) fn read_result(&self, id: &RunId) -> Result<Option<StoredResult>, Failure> {
        let path = self.run_file(id, RESULT_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_failure("store.read", &path, &e)),
        };
        match decode_str(&text, Format::Json) {
            Ok(document) => Ok(Some(StoredResult { document, text })),
            Err(errors) => {
                let mut line = Diagnostic::error(
                    "store.result",
                    format!("{} is not a result this build reads", path.display()),
                )
                .at("--store")
                .found(errors.first().to_string())
                .allowed(crate::RESULT_FORMAT);
                line.file = Some(path.display().to_string());
                Err(Failure::one(FailureClass::Io, line))
            }
        }
    }

    /// A new directory for a run in progress.
    pub(crate) fn begin(&self) -> Result<TempRun, Failure> {
        self.ensure()?;
        self.temp_dir()
    }

    /// A new directory below `tmp/`, which exists.
    fn temp_dir(&self) -> Result<TempRun, Failure> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = self
                .root
                .join("tmp")
                .join(format!("{}-{n}", std::process::id()));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(TempRun { dir }),
                // Left by an earlier process with the same number.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_failure("store.write", &dir, &e)),
            }
        }
    }

    /// Renames `temp` into place as the run `id`, or says that it is there already:
    /// a stored run is never overwritten.
    pub(crate) fn commit(&self, temp: &TempRun, id: &RunId) -> Result<Commit, Failure> {
        let target = self.run_dir(id);
        match fs::rename(&temp.dir, &target) {
            Ok(()) => Ok(Commit::Stored),
            Err(_) if target.join(RESULT_FILE).is_file() => Ok(Commit::AlreadyThere),
            Err(e) => Err(io_failure("store.write", &target, &e)),
        }
    }
}

/// A stored result: its value, and its bytes as they are in the store. A reader
/// ignores keys it does not know, so only the bytes are sure to hold all of them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoredResult {
    pub(crate) document: ResultDocument,
    pub(crate) text: String,
}

/// What [`Store::commit`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Commit {
    Stored,
    AlreadyThere,
}

/// A run in progress; dropped, it leaves nothing behind.
#[derive(Debug)]
pub(crate) struct TempRun {
    dir: PathBuf,
}

impl TempRun {
    /// The file `name` of this run.
    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for TempRun {
    fn drop(&mut self) {
        // Gone after a commit; nothing to do about a failure here.
        let _ = fs::remove_dir_all(&self.dir);
    }
}
