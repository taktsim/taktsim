//! Model document types, loader, expression language and JSON Schema
//! generation. Roadmap M4 and M6.
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim). A model is a YAML
//! or JSON document (spec §4). The [`Loader`] reads one strictly (ADR-0002,
//! ADR-0020):
//!
//! - the YAML 1.2 subset of ADR-0002, or JSON, into a tree with lines and
//!   columns;
//! - typed decoding: unknown keys are errors, quantities carry units,
//!   distributions follow ADR-0003, and every error says where it is (file,
//!   line, column, field path), what was found, what is allowed and one
//!   valid example ([`LoadError`]); all errors are reported together;
//! - `include:` with content hashing; ids unique; every reference checked;
//!   observables type-checked with the expression language ([`expr`],
//!   ADR-0009);
//! - normalization and the model hash: SHA-256 of the canonical JSON
//!   (RFC 8785, [`jcs`]) of the normalized document, the same for every
//!   spelling of a model.
//!
//! Component types are not part of this crate: the caller registers them,
//! with the observables, in a [`Catalog`] (see [`ComponentParams`]).
//!
//! ```
//! use serde::Serialize;
//! use taktsim_model::{Catalog, ComponentParams, Distribution, Fields, Format, Loader};
//!
//! #[derive(Debug, Serialize, schemars::JsonSchema)]
//! struct Machine {
//!     cycle_time: Distribution,
//! }
//!
//! impl ComponentParams for Machine {
//!     const TYPE: &'static str = "Machine";
//!     const EXAMPLE: &'static str = "{id: cut, type: Machine, cycle_time: 42s}";
//!     fn decode(fields: &mut Fields<'_, '_>) -> Option<Self> {
//!         Some(Self { cycle_time: fields.required("cycle_time")? })
//!     }
//! }
//!
//! let catalog = Catalog::new().with::<Machine>()?;
//! let model = Loader::new(&catalog).load_str(
//!     r#"
//! format: "1.0"
//! model: {name: line-a}
//! experiment: {type: terminating, horizon: 8h}
//! components:
//!   - {id: cut, type: Machine, cycle_time: {dist: lognormal, mean: 42s, sigma: 0.08}}
//!   - {id: weld, type: Machine, cycle_time: 45s}
//! flow:
//!   - [cut, weld]
//! "#,
//!     Format::Yaml,
//! )?;
//! assert_eq!(model.document().components().len(), 2);
//! assert_eq!(model.hash().to_string().len(), 64);
//!
//! let errors = Loader::new(&catalog)
//!     .load_str(&model.canonical_json().replace("42s", "42"), Format::Json)
//!     .unwrap_err();
//! assert_eq!(errors.first().path().to_string(), "components[0].cycle_time.mean");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod component;
mod decode;
mod distribution;
mod document;
mod error;
pub mod expr;
mod hash;
mod include;
pub mod jcs;
mod load;
mod schema;
mod syntax;

pub use component::{Catalog, CatalogError, Component, ComponentParams, ComponentRef};
pub use decode::{Decode, Decoder, Fields};
pub use distribution::{DISTRIBUTIONS, Distribution};
pub use document::{
    Document, Edge, Experiment, ExperimentKind, FORMAT_VERSION, ModelInfo, Observable, Precision,
    TimeUnit,
};
pub use error::{
    Ambiguity, FieldPath, Found, IncludePathProblem, JsonProblem, LoadError, LoadErrorKind,
    LoadErrors, Location, MAX_INTEGER, NormalizeProblem, Place, Segment,
};
pub use hash::{ModelHash, ParseHashError};
pub use include::{DirResolver, Included, Resolver};
pub use load::{Loader, Model, decode_str, parse};
pub use schema::model_schema;
pub use syntax::{Entry, Format, MAX_NESTING, Node, Value};
