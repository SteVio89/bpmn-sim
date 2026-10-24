//! A project is a set of BPMN files that reference each other (spec §3.2).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dmn::{DmnFile, parse_dmn};
use crate::error::ParseError;
use crate::model::*;
use crate::parser::parse_file;

#[derive(Debug, Default)]
pub struct Project {
    /// Files by name; `BTreeMap` for deterministic order.
    pub files: BTreeMap<String, BpmnFile>,
    /// Process id (global, unique across files) -> qualified id.
    pub processes: BTreeMap<String, QId>,
    /// Loaded DMN files.
    pub dmn: BTreeMap<String, DmnFile>,
    /// Decision id (global, unique across DMN files) -> file name.
    pub decisions: BTreeMap<String, String>,
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
        if path.extension().is_some_and(|x| x == "dmn") {
            self.add_dmn_source(&name, &xml)
        } else {
            self.add_source(&name, &xml)
        }
    }

    /// Add one DMN file. Decision ids must be unique across DMN files.
    pub fn add_dmn_source(&mut self, name: &str, xml: &str) -> Result<(), ParseError> {
        if self.dmn.contains_key(name) {
            return Err(ParseError::DuplicateFile { file: name.into() });
        }
        let file = parse_dmn(name, xml)?;
        for id in file.decisions.keys() {
            if let Some(existing) = self.decisions.get(id) {
                return Err(ParseError::DuplicateDecision {
                    decision: id.clone(),
                    first: existing.clone(),
                    second: name.into(),
                });
            }
        }
        for id in file.decisions.keys() {
            self.decisions.insert(id.clone(), name.to_string());
        }
        self.dmn.insert(name.to_string(), file);
        Ok(())
    }

    /// Load every `*.bpmn` and `*.dmn` file in a folder (not recursive), in name order.
    pub fn add_dir(&mut self, dir: &Path) -> Result<(), ParseError> {
        let io = |e: std::io::Error| ParseError::Io {
            path: dir.display().to_string(),
            message: e.to_string(),
        };
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map_err(io)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "bpmn" || x == "dmn"))
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

impl Project {
    pub fn flow(&self, id: &QId) -> Option<&SequenceFlow> {
        self.files.get(&id.file)?.sequence_flows.get(id)
    }

    /// A message flow at a call activity or subprocess (BPMN §9.3) only shows on the outside
    /// what elements inside it send or receive. The simulation sends nothing along it: the
    /// elements inside send and receive along their own message flows.
    pub fn is_mirror_flow(&self, f: &MessageFlow) -> bool {
        [&f.source, &f.target].into_iter().any(|end| {
            self.node(end)
                .is_some_and(|n| matches!(n.kind, NodeKind::CallActivity | NodeKind::SubProcess))
        })
    }

    /// Name of the participant (pool) whose process contains `process`, if the file has a collaboration.
    pub fn participant_name(&self, process: &QId) -> Option<&str> {
        self.files
            .get(&process.file)?
            .participants
            .values()
            .find(|p| p.process.as_ref() == Some(process))
            .and_then(|p| p.name.as_deref())
    }

    /// The nodes that get a token when the top level of `process` (`scope` = `None`, as for a
    /// call) or the embedded subprocess `scope` starts (BPMN §13.3.4): its none start events,
    /// or, if it has no start event at all, every activity and gateway without incoming
    /// sequence flows. Event subprocesses never start this way.
    pub fn start_nodes(&self, process: &QId, scope: Option<&QId>) -> Vec<QId> {
        let children: Vec<&FlowNode> = self
            .nodes()
            .filter(|n| n.process == *process && n.parent.as_ref() == scope)
            .collect();
        if children.iter().any(|n| n.kind == NodeKind::StartEvent) {
            return children
                .iter()
                .filter(|n| {
                    n.kind == NodeKind::StartEvent
                        && n.event_defs.iter().all(|d| matches!(d, EventDef::None))
                })
                .map(|n| n.id.clone())
                .collect();
        }
        children
            .iter()
            .filter(|n| {
                !n.kind.is_event()
                    && !matches!(n.kind, NodeKind::Unsupported(_))
                    && !n.triggered_by_event
                    && n.incoming.is_empty()
            })
            .map(|n| n.id.clone())
            .collect()
    }

    /// The start events of the event subprocesses directly inside `container` (an embedded
    /// subprocess or event subprocess, `None` for the top level of `process`).
    pub fn event_sub_starts(&self, process: &QId, container: Option<&QId>) -> Vec<&FlowNode> {
        self.nodes()
            .filter(|n| n.process == *process && self.is_event_sub_start(n))
            .filter(|n| {
                n.parent
                    .as_ref()
                    .and_then(|p| self.node(p))
                    .is_some_and(|es| es.parent.as_ref() == container)
            })
            .collect()
    }

    /// Whether `n` is the start event of an event subprocess.
    pub fn is_event_sub_start(&self, n: &FlowNode) -> bool {
        n.kind == NodeKind::StartEvent
            && n.parent
                .as_ref()
                .and_then(|p| self.node(p))
                .is_some_and(|p| p.triggered_by_event)
    }

    /// The outgoing flows of an activity or event (not a gateway) that has conditional
    /// outgoing flows: those are taken when their condition holds, the default flow when none
    /// holds, every other flow always. `None` without a conditional outgoing flow.
    pub fn activity_split(&self, n: &FlowNode) -> Option<ActivitySplit> {
        if n.kind.is_gateway() {
            return None;
        }
        let mut split = ActivitySplit::default();
        for f in &n.outgoing {
            let condition = self.flow(f).and_then(|fl| fl.condition.clone());
            if n.default_flow.as_ref() == Some(f) {
                split.default = Some(f.clone());
            } else if let Some(c) = condition {
                split.conditional.push((f.clone(), c));
            } else {
                split.always.push(f.clone());
            }
        }
        if split.conditional.is_empty() {
            return None;
        }
        Some(split)
    }
}

/// See `Project::activity_split`. Flows are in model order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivitySplit {
    pub always: Vec<QId>,
    /// Conditional flows with their condition text.
    pub conditional: Vec<(QId, String)>,
    pub default: Option<QId>,
}

/// Which files a main file needs: itself plus, directly or indirectly, every `*.bpmn` in its
/// folder that defines a process called by a call activity and every `*.dmn` that defines a
/// decision used by a business rule task (spec §3.2). Files are not searched recursively.
/// The main file comes first, the rest in name order. References that nothing in the folder
/// defines are left for validation to report; unrelated unreadable files are ignored.
pub fn discover_files(main: &Path) -> Result<Vec<PathBuf>, ParseError> {
    let io = |e: std::io::Error| ParseError::Io {
        path: main.display().to_string(),
        message: e.to_string(),
    };
    let dir = main
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let main_name = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = std::fs::read_to_string(main).map_err(io)?;
    let main_file = parse_file(&main_name, &text)?;

    // Candidates in the folder (parse failures are skipped: they may be unrelated).
    let mut bpmn: BTreeMap<String, (PathBuf, BpmnFile)> = BTreeMap::new();
    let mut dmn: BTreeMap<String, (PathBuf, DmnFile)> = BTreeMap::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(io)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == main_name {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        match p.extension().and_then(|x| x.to_str()) {
            Some("bpmn") => {
                if let Ok(f) = parse_file(&name, &text) {
                    bpmn.insert(name, (p, f));
                }
            }
            Some("dmn") => {
                if let Ok(f) = parse_dmn(&name, &text) {
                    dmn.insert(name, (p, f));
                }
            }
            _ => {}
        }
    }

    let mut selected: Vec<String> = Vec::new();
    let mut selected_dmn: Vec<String> = Vec::new();
    let mut queue: Vec<BpmnFile> = vec![main_file.clone()];
    let mut defines: BTreeMap<String, Vec<String>> = BTreeMap::new(); // process id -> files
    for f in std::iter::once(&main_file).chain(bpmn.values().map(|(_, f)| f)) {
        for pid in f.processes.keys() {
            defines
                .entry(pid.id.clone())
                .or_default()
                .push(f.name.clone());
        }
    }
    while let Some(file) = queue.pop() {
        for n in file.nodes.values() {
            if let Some(called) = &n.called_element
                && let Some(files) = defines.get(called)
            {
                if files.len() > 1 {
                    return Err(ParseError::DuplicateProcess {
                        process: called.clone(),
                        first: files[0].clone(),
                        second: files[1].clone(),
                    });
                }
                for f in files {
                    if *f != main_name && !selected.contains(f) {
                        selected.push(f.clone());
                        if let Some((_, parsed)) = bpmn.get(f) {
                            queue.push(parsed.clone());
                        }
                    }
                }
            }
            if let Some(decision) = &n.called_decision {
                let owners: Vec<&String> = dmn
                    .iter()
                    .filter(|(_, (_, d))| d.decisions.contains_key(decision))
                    .map(|(name, _)| name)
                    .collect();
                if owners.len() > 1 {
                    return Err(ParseError::DuplicateDecision {
                        decision: decision.clone(),
                        first: owners[0].clone(),
                        second: owners[1].clone(),
                    });
                }
                if let Some(o) = owners.first()
                    && !selected_dmn.contains(o)
                {
                    selected_dmn.push((*o).clone());
                }
            }
        }
    }
    selected.sort();
    selected_dmn.sort();
    let mut out = vec![main.to_path_buf()];
    out.extend(
        selected
            .iter()
            .filter_map(|n| bpmn.get(n).map(|(p, _)| p.clone())),
    );
    out.extend(
        selected_dmn
            .iter()
            .filter_map(|n| dmn.get(n).map(|(p, _)| p.clone())),
    );
    Ok(out)
}

impl Project {
    /// Add a main file together with everything it references (see `discover_files`); files
    /// that are already loaded are skipped.
    pub fn add_main(&mut self, main: &Path) -> Result<(), ParseError> {
        for p in discover_files(main)? {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !self.files.contains_key(&name) && !self.dmn.contains_key(&name) {
                self.add_path(&p)?;
            }
        }
        Ok(())
    }

    /// Open a main file together with everything it references.
    pub fn open_main(main: &Path) -> Result<Self, ParseError> {
        let mut project = Self::new();
        project.add_main(main)?;
        Ok(project)
    }
}
