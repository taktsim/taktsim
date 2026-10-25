//! Python bindings for taktsim: a thin one-to-one layer over the Rust API
//! (ADR-0005). `validate` and `run` arrive in roadmap M12.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use pyo3::prelude::*;

/// The `taktsim` Python module.
#[pymodule]
fn taktsim(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))
}
