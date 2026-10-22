//! Model validation before simulating (spec §3.3). Scenario-dependent checks
//! (gateway probabilities) belong to the scenario loader and arrive with M1.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::*;
use crate::project::Project;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub element: QId,
    pub name: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        match &self.name {
            Some(n) => write!(f, "{sev}: {} ({n}): {}", self.element, self.message),
            None => write!(f, "{sev}: {}: {}", self.element, self.message),
        }
    }
}

/// Validate all loaded files. Issues are sorted by (element, message) so output is reproducible.
pub fn validate(project: &Project) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut push = |severity, n: &FlowNode, message: String| {
        issues.push(Issue {
            severity,
            element: n.id.clone(),
            name: n.name.clone(),
            message,
        })
    };

    let reachable = reachable_nodes(project);

    for n in project.nodes() {
        let is_link_throw = n.event_defs.iter().any(|d| matches!(d, EventDef::Link(_)))
            && n.kind == NodeKind::IntermediateThrowEvent;

        if !n.kind.is_event() && n.incoming.is_empty() {
            push(Severity::Warning, n, "has no incoming sequence flow".into());
        }
        if n.kind != NodeKind::EndEvent && !is_link_throw && n.outgoing.is_empty() {
            push(Severity::Warning, n, "has no outgoing sequence flow".into());
        }

        if n.kind == NodeKind::IntermediateCatchEvent && !reachable.contains(&n.id) {
            push(
                Severity::Warning,
                n,
                "intermediate catch event can never be reached".into(),
            );
        }

        if is_link_throw {
            for def in &n.event_defs {
                if let EventDef::Link(name) = def
                    && link_catch(project, &n.process, name).is_none()
                {
                    push(
                        Severity::Error,
                        n,
                        format!(
                            "link throw event has no link catch event named '{name}' in its process"
                        ),
                    );
                }
            }
        }

        if n.kind == NodeKind::CallActivity {
            match &n.called_element {
                None => push(
                    Severity::Error,
                    n,
                    "call activity has no called process id".into(),
                ),
                Some(p) if !project.processes.contains_key(p) => push(
                    Severity::Error,
                    n,
                    format!("called process '{p}' is not defined in any loaded file"),
                ),
                Some(_) => {}
            }
        }

        if let NodeKind::Unsupported(kind) = &n.kind {
            push(
                Severity::Warning,
                n,
                format!("unsupported element '{kind}'"),
            );
        }
        for def in &n.event_defs {
            if let EventDef::Unsupported(kind) = def {
                push(
                    Severity::Warning,
                    n,
                    format!("unsupported event definition '{kind}'"),
                );
            }
        }
    }

    issues.sort_by(|a, b| (&a.element, &a.message).cmp(&(&b.element, &b.message)));
    issues
}

fn link_catch<'a>(project: &'a Project, process: &QId, name: &str) -> Option<&'a FlowNode> {
    project.nodes().find(|n| {
        n.process == *process
            && n.kind == NodeKind::IntermediateCatchEvent
            && n.event_defs
                .iter()
                .any(|d| matches!(d, EventDef::Link(l) if l == name))
    })
}

/// Nodes reachable from any start event via sequence flows, link jumps and boundary events.
fn reachable_nodes(project: &Project) -> BTreeSet<QId> {
    let flows: BTreeMap<&QId, &SequenceFlow> = project
        .files
        .values()
        .flat_map(|f| f.sequence_flows.iter())
        .collect();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<QId> = project
        .nodes()
        .filter(|n| n.kind == NodeKind::StartEvent)
        .map(|n| n.id.clone())
        .collect();

    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(node) = project.node(&id) else {
            continue;
        };
        for f in node.outgoing.iter().filter_map(|f| flows.get(f)) {
            stack.push(f.target.clone());
        }
        for def in &node.event_defs {
            if let EventDef::Link(name) = def
                && node.kind == NodeKind::IntermediateThrowEvent
                && let Some(c) = link_catch(project, &node.process, name)
            {
                stack.push(c.id.clone());
            }
        }
        // Boundary events are reachable as soon as their activity is.
        stack.extend(
            project
                .nodes()
                .filter(|b| b.attached_to.as_ref() == Some(&id))
                .map(|b| b.id.clone()),
        );
    }
    seen
}
