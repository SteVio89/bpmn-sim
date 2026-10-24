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

    let implicit = implicit_starts(project);
    let reachable = reachable_nodes(project, &implicit);
    // Scopes without a start event may also leave out end events (BPMN §10.5.2, §10.5.3).
    let startless: BTreeSet<(&QId, Option<&QId>)> = implicit
        .iter()
        .filter_map(|id| project.node(id))
        .map(|n| (&n.process, n.parent.as_ref()))
        .collect();
    let incoming_msg: std::collections::BTreeSet<&QId> = project
        .files
        .values()
        .flat_map(|f| f.message_flows.values().map(|m| &m.target))
        .collect();
    let outgoing_msg: std::collections::BTreeSet<&QId> = project
        .files
        .values()
        .flat_map(|f| f.message_flows.values().map(|m| &m.source))
        .collect();

    for n in project.nodes() {
        let is_link_throw = n.event_defs.iter().any(|d| matches!(d, EventDef::Link(_)))
            && n.kind == NodeKind::IntermediateThrowEvent;

        let instantiating_gateway = n.kind == NodeKind::EventBasedGateway
            && (n.instantiate || (n.incoming.is_empty() && !implicit.contains(&n.id)));
        if instantiating_gateway {
            push(
                Severity::Warning,
                n,
                "event-based gateways that start a process are not supported; only start events start one".into(),
            );
        } else if !n.triggered_by_event {
            // (Event subprocesses have no sequence flows; their structure is checked on binding.)
            if !n.kind.is_event() && n.incoming.is_empty() && !implicit.contains(&n.id) {
                push(Severity::Warning, n, "has no incoming sequence flow".into());
            }
            if n.kind != NodeKind::EndEvent
                && !is_link_throw
                && n.outgoing.is_empty()
                && !startless.contains(&(&n.process, n.parent.as_ref()))
            {
                push(Severity::Warning, n, "has no outgoing sequence flow".into());
            }
        }
        if n.instantiate && n.kind == NodeKind::Task(TaskKind::Receive) {
            push(
                Severity::Warning,
                n,
                "receive tasks that start a process (instantiate) are not supported; only start events start one".into(),
            );
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
                    Severity::Warning,
                    n,
                    format!(
                        "called process '{p}' is not defined in any loaded file; the simulation runs the call activity as a task if the scenario gives it a duration"
                    ),
                ),
                Some(_) => {}
            }
        }

        if crate::scenario::receives_messages(n)
            && n.kind != NodeKind::StartEvent
            && !incoming_msg.contains(&n.id)
        {
            push(
                Severity::Warning,
                n,
                "waits for a message but has no incoming message flow".into(),
            );
        }
        if crate::scenario::sends_messages(n) && !outgoing_msg.contains(&n.id) {
            push(
                Severity::Warning,
                n,
                "sends a message but has no outgoing message flow".into(),
            );
        }

        if let Some(d) = &n.called_decision
            && !project.decisions.contains_key(d)
        {
            push(
                Severity::Warning,
                n,
                format!("decision '{d}' is not defined in any loaded DMN file"),
            );
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

    issues.extend(mirror_consistency(project));
    issues.sort_by(|a, b| (&a.element, &a.message).cmp(&(&b.element, &b.message)));
    issues
}

/// Direction of the messages between a call activity or subprocess and another party.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Dir {
    ToParty,
    FromParty,
}

/// The message flows at a call activity or subprocess show on the outside who the elements
/// inside it exchange messages with. Warns where the two levels disagree: a party (matched by
/// pool name, as the same pool has a different id in every file) that only one level shows.
fn mirror_consistency(project: &Project) -> Vec<Issue> {
    let flows: Vec<&MessageFlow> = project
        .files
        .values()
        .flat_map(|f| f.message_flows.values())
        .collect();
    let mut issues = Vec::new();
    for scope in project.nodes() {
        let (inner_name, this) = match scope.kind {
            NodeKind::CallActivity
                if scope
                    .called_element
                    .as_ref()
                    .is_some_and(|c| project.processes.contains_key(c)) =>
            {
                ("the called process", "this call activity")
            }
            NodeKind::SubProcess => ("the subprocess", "this subprocess"),
            _ => continue,
        };
        // (party key, direction) -> (party name, element labels or flow ids)
        type Seen = BTreeMap<(String, Dir), (String, BTreeSet<String>)>;
        let (mut inner, mut outer) = (Seen::new(), Seen::new());
        for f in &flows {
            for (end, other, dir) in [
                (&f.source, &f.target, Dir::ToParty),
                (&f.target, &f.source, Dir::FromParty),
            ] {
                if *other == scope.id {
                    continue;
                }
                let Some(party) = party_name(project, other) else {
                    continue;
                };
                let (seen, what) = if *end == scope.id {
                    (&mut outer, f.id.id.clone())
                } else if let Some(n) = project.node(end)
                    && is_inside(project, scope, n)
                {
                    (&mut inner, n.label().to_string())
                } else {
                    continue;
                };
                let key = party
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                seen.entry((key, dir))
                    .or_insert_with(|| (party.trim().to_string(), BTreeSet::new()))
                    .1
                    .insert(what);
            }
        }
        // An expanded subprocess is drawn in place, so its own message flows already show who it
        // talks to; only a collapsed one (or one with flows at its border) is another level.
        if scope.kind == NodeKind::SubProcess
            && outer.is_empty()
            && !drawn_collapsed(project, scope)
        {
            continue;
        }
        let list = |items: &BTreeSet<String>| {
            items
                .iter()
                .map(|i| format!("'{i}'"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut warn = |message: String| {
            issues.push(Issue {
                severity: Severity::Warning,
                element: scope.id.clone(),
                name: scope.name.clone(),
                message,
            })
        };
        for (k, (party, elements)) in &inner {
            if outer.contains_key(k) {
                continue;
            }
            let elements = list(elements);
            warn(match k.1 {
                Dir::ToParty => format!(
                    "{inner_name} sends messages to '{party}' ({elements}), but no message flow goes from {this} to '{party}'; show that pool and the message flow here too"
                ),
                Dir::FromParty => format!(
                    "{inner_name} receives messages from '{party}' ({elements}), but no message flow from '{party}' ends at {this}; show that pool and the message flow here too"
                ),
            });
        }
        for (k, (party, ids)) in &outer {
            if inner.contains_key(k) {
                continue;
            }
            let ids = list(ids);
            warn(match k.1 {
                Dir::ToParty => format!(
                    "message flow {ids} goes from {this} to '{party}', but nothing in {inner_name} sends messages to '{party}'"
                ),
                Dir::FromParty => format!(
                    "message flow {ids} from '{party}' ends at {this}, but nothing in {inner_name} receives messages from '{party}'"
                ),
            });
        }
    }
    issues
}

/// Whether `n` is inside the call activity or subprocess `scope` (at any depth).
fn is_inside(project: &Project, scope: &FlowNode, n: &FlowNode) -> bool {
    if scope.kind == NodeKind::CallActivity {
        return scope
            .called_element
            .as_ref()
            .and_then(|c| project.processes.get(c))
            == Some(&n.process);
    }
    let mut parent = n.parent.as_ref();
    while let Some(p) = parent {
        if *p == scope.id {
            return true;
        }
        parent = project.node(p).and_then(|x| x.parent.as_ref());
    }
    false
}

/// Whether the subprocess `s` is drawn collapsed: it has a shape in the file's (first) diagram,
/// but nothing inside it has one (its content is on a diagram of its own).
fn drawn_collapsed(project: &Project, s: &FlowNode) -> bool {
    let Some(file) = project.files.get(&s.id.file) else {
        return false;
    };
    let shapes = &file.diagram.shapes;
    shapes.contains_key(&s.id)
        && !file
            .nodes
            .values()
            .any(|n| shapes.contains_key(&n.id) && is_inside(project, s, n))
}

/// Name (else id) of the pool at the end `end` of a message flow: the participant itself, or
/// the participant of the element's process.
fn party_name(project: &Project, end: &QId) -> Option<String> {
    let participants = &project.files.get(&end.file)?.participants;
    let p = match project.node(end) {
        Some(n) => participants
            .values()
            .find(|p| p.process.as_ref() == Some(&n.process))?,
        None => participants.get(end)?,
    };
    Some(p.name.clone().unwrap_or_else(|| p.id.id.clone()))
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

/// Activities and gateways that start their subprocess or called process because it has no
/// start event (`Project::start_nodes`).
fn implicit_starts(project: &Project) -> BTreeSet<QId> {
    let scopes = project
        .nodes()
        .filter_map(|n| n.called_element.as_ref())
        .filter_map(|c| project.processes.get(c))
        .map(|p| (p, None))
        .chain(
            project
                .nodes()
                .filter(|n| n.kind == NodeKind::SubProcess)
                .map(|n| (&n.process, Some(&n.id))),
        );
    scopes
        .flat_map(|(p, scope)| project.start_nodes(p, scope))
        .filter(|id| {
            project
                .node(id)
                .is_some_and(|n| n.kind != NodeKind::StartEvent)
        })
        .collect()
}

/// Nodes reachable from any start event (or implicit start) via sequence flows, link jumps
/// and boundary events.
fn reachable_nodes(project: &Project, implicit: &BTreeSet<QId>) -> BTreeSet<QId> {
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
        .chain(implicit.iter().cloned())
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
