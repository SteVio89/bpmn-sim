//! Core library of the BPMN simulator: model, parser and validation.
//! No UI dependencies.

pub mod aggregate;
pub mod calendar;
pub mod diagram;
pub mod dist;
pub mod dmn;
pub mod engine;
pub mod error;
pub mod export;
pub mod feel;
pub mod manual;
pub mod model;
pub mod needs;
pub mod parser;
pub mod project;
pub mod scenario;
pub mod stats;
pub mod validate;

pub use aggregate::{AggregateResult, Estimate, aggregate};
pub use engine::{Engine, run_replications, run_replications_with};
pub use error::ParseError;
pub use model::*;
pub use project::{Project, discover_files};
pub use scenario::{BoundModel, Scenario, ScenarioError};
pub use stats::*;
pub use validate::{Issue, Severity, validate};
