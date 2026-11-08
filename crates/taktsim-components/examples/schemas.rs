//! Writes the model's JSON Schema under a `schemas/` directory (ADR-0020 §11):
//! `model/1.0.json`, generated from the decoders of the standard catalog. It
//! calls [`standard`], so it picks up every registered type with no list of its
//! own (ADR-0022 §11).
//!
//! ```sh
//! cargo run -p taktsim-components --example schemas -- schemas
//! ```
//!
//! `ci/check-schemas.sh` runs it into a temporary directory and fails if the
//! committed files differ.

use std::error::Error;
use std::path::PathBuf;

use taktsim_components::standard;
use taktsim_model::{FORMAT_VERSION, model_schema};

fn main() -> Result<(), Box<dyn Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: schemas <schemas directory>")?;
    let dir = root.join("model");
    std::fs::create_dir_all(&dir)?;
    let mut text = model_schema(standard()?.catalog())
        .with(
            "description",
            &"A taktsim model (spec §4) of the standard production catalog: Source, Machine, \
              Buffer, Sink. Generated from the Rust types; do not edit.",
        )
        .to_pretty();
    text.push('\n');
    std::fs::write(dir.join(format!("{FORMAT_VERSION}.json")), text)?;
    Ok(())
}
