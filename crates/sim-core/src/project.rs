//! A project is a set of BPMN files that reference each other (spec §3.2).

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::ParseError;
use crate::model::*;
use crate::parser::parse_file;

#[derive(Debug, Default)]
pub struct Project {
    /// Files by name; `BTreeMap` for deterministic order.
    pub files: BTreeMap<String, BpmnFile>,
    /// Process id (global, unique across files) -> qualified id.
    pub processes: BTreeMap<String, QId>,
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one file from its XML text. Fails if a process id already exists in another file.
    pub fn add_source(&mut self, name: &str, xml: &str) -> Result<(), ParseError> {
        if self.files.contains_key(name) {
            return Err(ParseError::DuplicateFile { file: name.into() });
        }
        let file = parse_file(name, xml)?;
        for pid in file.processes.keys() {
            if let Some(existing) = self.processes.get(&pid.id) {
                return Err(ParseError::DuplicateProcess {
                    process: pid.id.clone(),
                    first: existing.file.clone(),
                    second: name.into(),
                });
            }
        }
        for pid in file.processes.keys() {
            self.processes.insert(pid.id.clone(), pid.clone());
        }
        self.files.insert(name.to_string(), file);
        Ok(())
    }

    pub fn add_path(&mut self, path: &Path) -> Result<(), ParseError> {
        let io = |e: std::io::Error| ParseError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        };
        let xml = std::fs::read_to_string(path).map_err(io)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.add_source(&name, &xml)
    }

    /// Load every `*.bpmn` file in a folder (not recursive), in name order.
    /// Discovery from a single main file by following call activities comes with M4.
    pub fn add_dir(&mut self, dir: &Path) -> Result<(), ParseError> {
        let io = |e: std::io::Error| ParseError::Io {
            path: dir.display().to_string(),
            message: e.to_string(),
        };
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "bpmn"))
            .collect();
        paths.sort();
        for p in paths {
            self.add_path(&p)?;
        }
        Ok(())
    }

    pub fn load_dir(dir: &Path) -> Result<Self, ParseError> {
        let mut project = Self::new();
        project.add_dir(dir)?;
        Ok(project)
    }

    pub fn node(&self, id: &QId) -> Option<&FlowNode> {
        self.files.get(&id.file)?.nodes.get(id)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &FlowNode> {
        self.files.values().flat_map(|f| f.nodes.values())
    }

    pub fn process(&self, process_id: &str) -> Option<&Process> {
        let q = self.processes.get(process_id)?;
        self.files.get(&q.file)?.processes.get(q)
    }
}
