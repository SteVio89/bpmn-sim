//! Core library of the BPMN simulator: model, parser and validation.
//! No UI dependencies.

pub mod error;
pub mod model;
pub mod parser;
pub mod project;
pub mod validate;

pub use error::ParseError;
pub use model::*;
pub use project::Project;
pub use validate::{Issue, Severity, validate};
