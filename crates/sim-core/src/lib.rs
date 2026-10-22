//! Core library of the BPMN simulator: model, parser and validation.
//! No UI dependencies.

pub mod dist;
pub mod engine;
pub mod error;
pub mod model;
pub mod parser;
pub mod project;
pub mod scenario;
pub mod stats;
pub mod validate;

pub use engine::{Engine, run_replications};
pub use error::ParseError;
pub use model::*;
pub use project::Project;
pub use scenario::{BoundModel, Scenario, ScenarioError};
pub use stats::*;
pub use validate::{Issue, Severity, validate};
