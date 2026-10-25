//! Model document types, loader, expression language and JSON Schema
//! generation. Roadmap M4 and M6.
//!
//! Part of [taktsim](https://github.com/taktsim/taktsim). So far it holds
//! the expression language ([`expr`], ADR-0009); the model document types
//! and the loader arrive with roadmap M4.
//!
//! ```
//! use taktsim_model::expr::{TypeEnv, condition};
//!
//! assert!(condition("30s < 1min", &TypeEnv::new()).is_ok());
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod expr;
