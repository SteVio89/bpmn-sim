use thiserror::Error;

/// Errors raised while reading BPMN files. Always name the file and, where known, the element.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("{file}: invalid XML: {message}")]
    Xml { file: String, message: String },
    #[error("{file}: not a BPMN 2.0 document (root element is not bpmn:definitions)")]
    NotBpmn { file: String },
    #[error("{file}: element '{element}': {message}")]
    Element {
        file: String,
        element: String,
        message: String,
    },
    #[error("process id '{process}' is defined in both '{first}' and '{second}'")]
    DuplicateProcess {
        process: String,
        first: String,
        second: String,
    },
    #[error("{file}: not a DMN document (root element is not dmn:definitions)")]
    NotDmn { file: String },
    #[error("decision id '{decision}' is defined in both '{first}' and '{second}'")]
    DuplicateDecision {
        decision: String,
        first: String,
        second: String,
    },
    #[error("file '{file}' was loaded twice")]
    DuplicateFile { file: String },
    #[error("{path}: {message}")]
    Io { path: String, message: String },
}
