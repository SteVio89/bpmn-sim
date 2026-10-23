//! BPMN 2.0 XML parser. Elements are matched by namespace URI, never by prefix (spec §3.1).
//! Vendor extensions, data objects, annotations etc. are ignored; only `zeebe:calledElement`
//! and `zeebe:calledDecision` are read.

use std::collections::BTreeMap;

use roxmltree::{Document, Node};

use crate::error::ParseError;
use crate::model::*;

const BPMN_NS: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";
const ZEEBE_NS: &str = "http://camunda.org/schema/zeebe/1.0";
const BPMNDI_NS: &str = "http://www.omg.org/spec/BPMN/20100524/DI";
const DC_NS: &str = "http://www.omg.org/spec/DD/20100524/DC";
const DI_NS: &str = "http://www.omg.org/spec/DD/20100524/DI";

/// Parse one BPMN file. `file` is the file name used in qualified ids and error messages.
pub fn parse_file(file: &str, xml: &str) -> Result<BpmnFile, ParseError> {
    let doc = Document::parse(xml).map_err(|e| ParseError::Xml {
        file: file.to_string(),
        message: e.to_string(),
    })?;
    let root = doc.root_element();
    if !is_bpmn(root, "definitions") {
        return Err(ParseError::NotBpmn {
            file: file.to_string(),
        });
    }
    let mut p = Parser {
        file,
        out: BpmnFile {
            name: file.to_string(),
            ..BpmnFile::default()
        },
    };
    p.read_definitions(root)?;
    Ok(p.out)
}

fn is_bpmn(n: Node, local: &str) -> bool {
    n.is_element() && n.tag_name().name() == local && n.tag_name().namespace() == Some(BPMN_NS)
}

fn bpmn_children<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children()
        .filter(|c| c.is_element() && c.tag_name().namespace() == Some(BPMN_NS))
}

struct Parser<'f> {
    file: &'f str,
    out: BpmnFile,
}

impl Parser<'_> {
    fn err(&self, element: &str, message: impl Into<String>) -> ParseError {
        ParseError::Element {
            file: self.file.to_string(),
            element: element.to_string(),
            message: message.into(),
        }
    }

    fn qid(&self, id: &str) -> QId {
        QId::new(self.file, id)
    }

    fn required_id(&self, n: Node) -> Result<String, ParseError> {
        n.attribute("id").map(str::to_string).ok_or_else(|| {
            self.err(
                &format!("<{}>", n.tag_name().name()),
                "element has no 'id' attribute",
            )
        })
    }

    fn required_attr(&self, n: Node, id: &str, attr: &str) -> Result<String, ParseError> {
        n.attribute(attr)
            .map(str::to_string)
            .ok_or_else(|| self.err(id, format!("missing attribute '{attr}'")))
    }

    fn read_definitions(&mut self, root: Node) -> Result<(), ParseError> {
        // Messages, signals, errors and escalations first: event definitions refer to them.
        for c in bpmn_children(root) {
            let kind = c.tag_name().name();
            if matches!(kind, "message" | "signal" | "error" | "escalation") {
                let id = self.required_id(c)?;
                let name = c.attribute("name").unwrap_or(&id).to_string();
                // Errors and escalations are matched by code, also across files.
                let code = |attr: &str| {
                    c.attribute(attr)
                        .filter(|v| !v.is_empty())
                        .map_or_else(|| name.clone(), str::to_string)
                };
                match kind {
                    "message" => self.out.messages.insert(id, name),
                    "signal" => self.out.signals.insert(id, name.clone()),
                    "error" => self.out.errors.insert(id, code("errorCode")),
                    _ => self.out.escalations.insert(id, code("escalationCode")),
                };
            }
        }
        for c in bpmn_children(root) {
            match c.tag_name().name() {
                "process" => self.read_process(c)?,
                "collaboration" => self.read_collaboration(c)?,
                _ => {}
            }
        }
        self.read_diagram(root);
        self.check_references()
    }

    /// Shapes and edges of the first `bpmndi:BPMNDiagram`. Layout problems never fail parsing:
    /// shapes without usable bounds are skipped.
    fn read_diagram(&mut self, root: Node) {
        let in_ns = |n: &Node, ns: &str, local: &str| {
            n.is_element() && n.tag_name().name() == local && n.tag_name().namespace() == Some(ns)
        };
        let Some(diagram) = root.children().find(|c| in_ns(c, BPMNDI_NS, "BPMNDiagram")) else {
            return;
        };
        let num = |n: &Node, a: &str| n.attribute(a).and_then(|v| v.parse::<f64>().ok());
        for plane in diagram
            .children()
            .filter(|c| in_ns(c, BPMNDI_NS, "BPMNPlane"))
        {
            for el in plane.children().filter(|c| c.is_element()) {
                let Some(target) = el.attribute("bpmnElement") else {
                    continue;
                };
                let q = self.qid(target);
                let bounds_of = |n: &Node| {
                    let b = n.children().find(|c| in_ns(c, DC_NS, "Bounds"))?;
                    Some(Bounds {
                        x: num(&b, "x")?,
                        y: num(&b, "y")?,
                        width: num(&b, "width")?,
                        height: num(&b, "height")?,
                    })
                };
                // Where a modeller placed the element's name (`bpmndi:BPMNLabel`), if anywhere.
                if let Some(label) = el
                    .children()
                    .find(|c| in_ns(c, BPMNDI_NS, "BPMNLabel"))
                    .and_then(|l| bounds_of(&l))
                {
                    self.out.diagram.labels.insert(q.clone(), label);
                }
                if in_ns(&el, BPMNDI_NS, "BPMNShape") {
                    let b = el.children().find(|c| in_ns(c, DC_NS, "Bounds"));
                    if let Some(b) = b
                        && let (Some(x), Some(y), Some(width), Some(height)) = (
                            num(&b, "x"),
                            num(&b, "y"),
                            num(&b, "width"),
                            num(&b, "height"),
                        )
                    {
                        self.out.diagram.shapes.insert(
                            q,
                            Bounds {
                                x,
                                y,
                                width,
                                height,
                            },
                        );
                    }
                } else if in_ns(&el, BPMNDI_NS, "BPMNEdge") {
                    let points: Vec<(f64, f64)> = el
                        .children()
                        .filter(|c| in_ns(c, DI_NS, "waypoint"))
                        .filter_map(|w| Some((num(&w, "x")?, num(&w, "y")?)))
                        .collect();
                    if points.len() >= 2 {
                        self.out.diagram.edges.insert(q, points);
                    }
                }
            }
        }
    }

    fn read_collaboration(&mut self, n: Node) -> Result<(), ParseError> {
        for c in bpmn_children(n) {
            if self.read_artifact(c) {
                continue;
            }
            match c.tag_name().name() {
                "participant" => {
                    let id = self.required_id(c)?;
                    let participant = Participant {
                        id: self.qid(&id),
                        name: c.attribute("name").map(str::to_string),
                        process: c.attribute("processRef").map(|p| self.qid(p)),
                    };
                    self.out
                        .participants
                        .insert(participant.id.clone(), participant);
                }
                "messageFlow" => {
                    let id = self.required_id(c)?;
                    let source = self.required_attr(c, &id, "sourceRef")?;
                    let target = self.required_attr(c, &id, "targetRef")?;
                    let flow = MessageFlow {
                        id: self.qid(&id),
                        name: c.attribute("name").map(str::to_string),
                        source: self.qid(&source),
                        target: self.qid(&target),
                    };
                    self.out.message_flows.insert(flow.id.clone(), flow);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_process(&mut self, n: Node) -> Result<(), ParseError> {
        let id = self.required_id(n)?;
        let pid = self.qid(&id);

        let mut lane_of: BTreeMap<String, String> = BTreeMap::new();
        let mut lanes = Vec::new();
        let mut lane_ids = Vec::new();
        for ls in bpmn_children(n).filter(|c| c.tag_name().name() == "laneSet") {
            self.read_lanes(ls, &mut lane_of, &mut lanes, &mut lane_ids);
        }
        for (id, name) in lane_ids {
            let q = self.qid(&id);
            self.out.lanes.insert(q, name);
        }

        let mut flows = Vec::new();
        self.read_scope(n, &pid, None, &mut flows)?;

        for (fid, f) in &flows {
            let source = self.qid(&self.required_attr(*f, fid, "sourceRef")?);
            let target = self.qid(&self.required_attr(*f, fid, "targetRef")?);
            let flow = SequenceFlow {
                id: self.qid(fid),
                name: f.attribute("name").map(str::to_string),
                source,
                target,
                condition: bpmn_children(*f)
                    .find(|c| c.tag_name().name() == "conditionExpression")
                    .and_then(|c| c.text())
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
            };
            self.out.sequence_flows.insert(flow.id.clone(), flow);
        }

        // Lanes, then incoming/outgoing wiring for the nodes of this process.
        for node in self.out.nodes.values_mut().filter(|nd| nd.process == pid) {
            node.lane = lane_of.get(&node.id.id).cloned();
        }
        let wiring: Vec<(QId, QId, QId)> = flows
            .iter()
            .filter_map(|(fid, _)| self.out.sequence_flows.get(&self.qid(fid)))
            .map(|f| (f.id.clone(), f.source.clone(), f.target.clone()))
            .collect();
        for (fid, source, target) in wiring {
            if let Some(s) = self.out.nodes.get_mut(&source) {
                s.outgoing.push(fid.clone());
            }
            if let Some(t) = self.out.nodes.get_mut(&target) {
                t.incoming.push(fid);
            }
        }

        self.out.processes.insert(
            pid.clone(),
            Process {
                id: pid,
                name: n.attribute("name").map(str::to_string),
                lanes,
            },
        );
        Ok(())
    }

    /// Pre-order walk: a nested lane overrides the lane of its parent for the nodes it lists.
    fn read_lanes(
        &self,
        set: Node,
        lane_of: &mut BTreeMap<String, String>,
        lanes: &mut Vec<String>,
        lane_ids: &mut Vec<(String, String)>,
    ) {
        for lane in bpmn_children(set).filter(|c| c.tag_name().name() == "lane") {
            let name = lane
                .attribute("name")
                .or_else(|| lane.attribute("id"))
                .unwrap_or_default()
                .to_string();
            lanes.push(name.clone());
            if let Some(id) = lane.attribute("id") {
                lane_ids.push((id.to_string(), name.clone()));
            }
            for r in bpmn_children(lane).filter(|c| c.tag_name().name() == "flowNodeRef") {
                if let Some(text) = r.text() {
                    lane_of.insert(text.trim().to_string(), name.clone());
                }
            }
            for child in bpmn_children(lane).filter(|c| c.tag_name().name() == "childLaneSet") {
                self.read_lanes(child, lane_of, lanes, lane_ids);
            }
        }
    }

    fn read_scope<'a, 'i>(
        &mut self,
        scope: Node<'a, 'i>,
        process: &QId,
        parent: Option<&QId>,
        flows: &mut Vec<(String, Node<'a, 'i>)>,
    ) -> Result<(), ParseError> {
        for c in bpmn_children(scope) {
            let local = c.tag_name().name();
            if local == "sequenceFlow" {
                flows.push((self.required_id(c)?, c));
                continue;
            }
            if self.read_artifact(c) {
                continue;
            }
            let Some(kind) = node_kind(local) else {
                continue;
            };
            let id = self.required_id(c)?;
            let qid = self.qid(&id);
            // Data associations of an activity (drawn as dotted arrows).
            for a in bpmn_children(c).filter(|a| {
                matches!(
                    a.tag_name().name(),
                    "dataInputAssociation" | "dataOutputAssociation"
                )
            }) {
                if let Some(aid) = a.attribute("id") {
                    let q = self.qid(aid);
                    self.out.associations.insert(q, true);
                }
            }
            if self.out.nodes.contains_key(&qid) {
                return Err(self.err(&id, "duplicate element id"));
            }
            let node = self.read_node(c, &id, kind, process, parent)?;
            self.out.nodes.insert(qid.clone(), node);
            if local == "subProcess" {
                self.read_scope(c, process, Some(&qid), flows)?;
            }
        }
        Ok(())
    }

    /// Remember data stores, data objects, text annotations, groups and associations for
    /// drawing; returns whether `c` was one of them.
    fn read_artifact(&mut self, c: Node) -> bool {
        let kind = match c.tag_name().name() {
            "dataStoreReference" => ArtifactKind::DataStore,
            "dataObjectReference" => ArtifactKind::DataObject,
            "textAnnotation" => ArtifactKind::TextAnnotation,
            "group" => ArtifactKind::Group,
            "association" => {
                if let Some(id) = c.attribute("id") {
                    let directed =
                        matches!(c.attribute("associationDirection"), Some("One" | "Both"));
                    self.out.associations.insert(self.qid(id), directed);
                }
                return true;
            }
            _ => return false,
        };
        let Some(id) = c.attribute("id") else {
            return true;
        };
        let text = if kind == ArtifactKind::TextAnnotation {
            bpmn_children(c)
                .find(|t| t.tag_name().name() == "text")
                .and_then(|t| t.text())
                .map(|t| t.trim().to_string())
        } else {
            c.attribute("name").map(str::to_string)
        };
        self.out
            .artifacts
            .insert(self.qid(id), Artifact { kind, text });
        true
    }

    fn read_node(
        &self,
        n: Node,
        id: &str,
        kind: NodeKind,
        process: &QId,
        parent: Option<&QId>,
    ) -> Result<FlowNode, ParseError> {
        let kind = match (&kind, n.tag_name().name()) {
            (NodeKind::Task(_), name) => NodeKind::Task(task_kind(name)),
            _ => kind,
        };
        let mut event_defs = Vec::new();
        if kind.is_event() {
            for d in bpmn_children(n).filter(|d| d.tag_name().name().ends_with("EventDefinition")) {
                event_defs.push(self.event_def(d));
            }
            if event_defs.is_empty() {
                event_defs.push(EventDef::None);
            }
        }

        let attached_to = if kind == NodeKind::BoundaryEvent {
            Some(self.qid(&self.required_attr(n, id, "attachedToRef")?))
        } else {
            None
        };

        let called_element = if kind == NodeKind::CallActivity {
            n.attribute("calledElement")
                .map(str::to_string)
                .or_else(|| zeebe_attr(n, "calledElement", "processId"))
        } else {
            None
        };
        let (called_decision, result_variable) = if kind == NodeKind::Task(TaskKind::BusinessRule) {
            (
                zeebe_attr(n, "calledDecision", "decisionId"),
                zeebe_attr(n, "calledDecision", "resultVariable"),
            )
        } else {
            (None, None)
        };

        Ok(FlowNode {
            id: self.qid(id),
            name: n.attribute("name").map(str::to_string),
            kind,
            event_defs,
            process: process.clone(),
            parent: parent.cloned(),
            lane: None,
            attached_to,
            cancel_activity: n.attribute("cancelActivity") != Some("false"),
            called_element,
            called_decision,
            result_variable,
            default_flow: n.attribute("default").map(|d| self.qid(d)),
            incoming: Vec::new(),
            outgoing: Vec::new(),
        })
    }

    fn event_def(&self, d: Node) -> EventDef {
        match d.tag_name().name() {
            "timerEventDefinition" => EventDef::Timer,
            "messageEventDefinition" => EventDef::Message(d.attribute("messageRef").map(|r| {
                self.out
                    .messages
                    .get(r)
                    .cloned()
                    .unwrap_or_else(|| r.to_string())
            })),
            "signalEventDefinition" => EventDef::Signal(d.attribute("signalRef").map(|r| {
                self.out
                    .signals
                    .get(r)
                    .cloned()
                    .unwrap_or_else(|| r.to_string())
            })),
            "linkEventDefinition" => {
                EventDef::Link(d.attribute("name").unwrap_or_default().to_string())
            }
            "errorEventDefinition" => EventDef::Error(d.attribute("errorRef").map(|r| {
                self.out
                    .errors
                    .get(r)
                    .cloned()
                    .unwrap_or_else(|| r.to_string())
            })),
            "escalationEventDefinition" => {
                EventDef::Escalation(d.attribute("escalationRef").map(|r| {
                    self.out
                        .escalations
                        .get(r)
                        .cloned()
                        .unwrap_or_else(|| r.to_string())
                }))
            }
            "terminateEventDefinition" => EventDef::Terminate,
            other => EventDef::Unsupported(other.to_string()),
        }
    }

    /// Dangling references are errors that name the file and the referring element.
    fn check_references(&self) -> Result<(), ParseError> {
        for f in self.out.sequence_flows.values() {
            for (end, what) in [(&f.source, "sourceRef"), (&f.target, "targetRef")] {
                if !self.out.nodes.contains_key(end) {
                    return Err(self.err(
                        &f.id.id,
                        format!("{what} '{}' is not a flow element of this file", end.id),
                    ));
                }
            }
        }
        for n in self.out.nodes.values() {
            if let Some(a) = &n.attached_to
                && !self.out.nodes.contains_key(a)
            {
                return Err(self.err(&n.id.id, format!("attachedToRef '{}' not found", a.id)));
            }
        }
        for f in self.out.message_flows.values() {
            for (end, what) in [(&f.source, "sourceRef"), (&f.target, "targetRef")] {
                if !self.out.nodes.contains_key(end) && !self.out.participants.contains_key(end) {
                    return Err(self.err(
                        &f.id.id,
                        format!(
                            "{what} '{}' is neither a flow element nor a participant",
                            end.id
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn zeebe_attr(n: Node, element: &str, attr: &str) -> Option<String> {
    bpmn_children(n)
        .filter(|c| c.tag_name().name() == "extensionElements")
        .flat_map(|ext| ext.children())
        .find(|c| {
            c.is_element()
                && c.tag_name().namespace() == Some(ZEEBE_NS)
                && c.tag_name().name() == element
        })
        .and_then(|c| c.attribute(attr))
        .map(str::to_string)
}

fn task_kind(local: &str) -> TaskKind {
    match local {
        "userTask" => TaskKind::User,
        "manualTask" => TaskKind::Manual,
        "serviceTask" => TaskKind::Service,
        "sendTask" => TaskKind::Send,
        "receiveTask" => TaskKind::Receive,
        "scriptTask" => TaskKind::Script,
        "businessRuleTask" => TaskKind::BusinessRule,
        _ => TaskKind::Task,
    }
}

/// `None` for elements that are not flow nodes (data objects, annotations, ...): those are ignored.
fn node_kind(local: &str) -> Option<NodeKind> {
    Some(match local {
        "task" | "userTask" | "manualTask" | "serviceTask" | "sendTask" | "receiveTask"
        | "scriptTask" | "businessRuleTask" => NodeKind::Task(TaskKind::Task),
        "subProcess" => NodeKind::SubProcess,
        "callActivity" => NodeKind::CallActivity,
        "startEvent" => NodeKind::StartEvent,
        "endEvent" => NodeKind::EndEvent,
        "intermediateCatchEvent" => NodeKind::IntermediateCatchEvent,
        "intermediateThrowEvent" => NodeKind::IntermediateThrowEvent,
        "boundaryEvent" => NodeKind::BoundaryEvent,
        "exclusiveGateway" => NodeKind::ExclusiveGateway,
        "parallelGateway" => NodeKind::ParallelGateway,
        "inclusiveGateway" => NodeKind::InclusiveGateway,
        "eventBasedGateway" => NodeKind::EventBasedGateway,
        "complexGateway" | "transaction" | "adHocSubProcess" | "implicitThroughEvent" => {
            NodeKind::Unsupported(local.to_string())
        }
        _ => return None,
    })
}
