//! Writes the model's JSON Schema under a `schemas/` directory (ADR-0020
//! §11): `model/1.0.json`, generated from the decoders.
//!
//! ```sh
//! cargo run -p taktsim-model --example schemas -- schemas
//! ```
//!
//! `ci/check-schemas.sh` runs it into a temporary directory and fails if
//! the committed files differ. The catalog is empty: no component type
//! exists before M5, which generates the schema with its own catalog.

use std::error::Error;
use std::path::PathBuf;

use taktsim_model::{Catalog, FORMAT_VERSION, model_schema};

fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: schemas <schemas directory>")?;
    let dir = root.join("model");
    std::fs::create_dir_all(&dir)?;
    let mut text = model_schema(&Catalog::new()).to_pretty();
    text.push('\n');
    std::fs::write(dir.join(format!("{FORMAT_VERSION}.json")), text)?;
    Ok(())
}
