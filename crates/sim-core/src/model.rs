//! Internal model. Every element is identified by a qualified id `<file>#<element id>` (spec §3.2).

use std::collections::BTreeMap;
use std::fmt;

/// Qualified id: file name plus element id (ids are only unique within one file).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QId {
    pub file: String,
    pub id: String,
}

impl QId {
    pub fn new(file: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            id: id.into(),
        }
    }
}

impl fmt::Display for QId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.file, self.id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Task,
    User,
    Manual,
    Service,
    Send,
    Receive,
    Script,
    BusinessRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventDef {
    None,
    Timer,
    /// Optional message name (resolved from `messageRef`).
    Message(Option<String>),
    /// Optional signal name (resolved from `signalRef`).
    Signal(Option<String>),
    Link(String),
    Error,
    Escalation,
    Terminate,
    /// An event definition the simulator does not support (e.g. conditional, compensation).
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Task(TaskKind),
    SubProcess,
    CallActivity,
    StartEvent,
    EndEvent,
    IntermediateCatchEvent,
    IntermediateThrowEvent,
    BoundaryEvent,
    ExclusiveGateway,
    ParallelGateway,
    InclusiveGateway,
    EventBasedGateway,
    /// A flow element the simulator does not support; carries the BPMN element name.
    Unsupported(String),
}

impl NodeKind {
    pub fn is_event(&self) -> bool {
        matches!(
            self,
            NodeKind::StartEvent
                | NodeKind::EndEvent
                | NodeKind::IntermediateCatchEvent
                | NodeKind::IntermediateThrowEvent
                | NodeKind::BoundaryEvent
        )
    }
    pub fn is_gateway(&self) -> bool {
        matches!(
            self,
            NodeKind::ExclusiveGateway
                | NodeKind::ParallelGateway
                | NodeKind::InclusiveGateway
                | NodeKind::EventBasedGateway
        )
    }
}

#[derive(Debug, Clone)]
pub struct FlowNode {
    pub id: QId,
    pub name: Option<String>,
    pub kind: NodeKind,
    pub event_defs: Vec<EventDef>,
    /// Process this node belongs to.
    pub process: QId,
    /// Enclosing embedded subprocess, if any.
    pub parent: Option<QId>,
    /// Name of the lane that references this node (`flowNodeRef`).
    pub lane: Option<String>,
    /// Boundary events only.
    pub attached_to: Option<QId>,
    pub cancel_activity: bool,
    /// Call activities: called process id (`calledElement` or `zeebe:calledElement`).
    pub called_element: Option<String>,
    /// Business rule tasks: decision id (`zeebe:calledDecision`).
    pub called_decision: Option<String>,
    /// Business rule tasks: `zeebe:calledDecision resultVariable`.
    pub result_variable: Option<String>,
    /// Gateways and activities: `default` flow.
    pub default_flow: Option<QId>,
    pub incoming: Vec<QId>,
    pub outgoing: Vec<QId>,
}

impl FlowNode {
    /// Display label: name if present, else id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id.id)
    }
}

#[derive(Debug, Clone)]
pub struct SequenceFlow {
    pub id: QId,
    pub name: Option<String>,
    pub source: QId,
    pub target: QId,
    /// `conditionExpression` text, if any.
    pub condition: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MessageFlow {
    pub id: QId,
    pub name: Option<String>,
    /// May reference a flow node or a participant (black-box pool).
    pub source: QId,
    pub target: QId,
}

#[derive(Debug, Clone)]
pub struct Participant {
    pub id: QId,
    pub name: Option<String>,
    /// `None` for black-box pools.
    pub process: Option<QId>,
}

#[derive(Debug, Clone)]
pub struct Process {
    pub id: QId,
    pub name: Option<String>,
    pub lanes: Vec<String>,
}

/// One parsed BPMN file. Collections are `BTreeMap`s so iteration order is deterministic.
#[derive(Debug, Clone, Default)]
pub struct BpmnFile {
    pub name: String,
    pub processes: BTreeMap<QId, Process>,
    pub participants: BTreeMap<QId, Participant>,
    pub nodes: BTreeMap<QId, FlowNode>,
    pub sequence_flows: BTreeMap<QId, SequenceFlow>,
    pub message_flows: BTreeMap<QId, MessageFlow>,
    /// Signal id -> signal name.
    pub signals: BTreeMap<String, String>,
    /// Message id -> message name.
    pub messages: BTreeMap<String, String>,
    /// Lane id -> lane name.
    pub lanes: BTreeMap<QId, String>,
    /// BPMNDI layout of the file's first diagram (only used for drawing).
    pub diagram: Diagram,
}

/// Rectangle in diagram coordinates (BPMNDI `dc:Bounds`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diagram {
    /// Element (flow node, participant or lane) -> its shape.
    pub shapes: BTreeMap<QId, Bounds>,
    /// Sequence or message flow -> waypoints.
    pub edges: BTreeMap<QId, Vec<(f64, f64)>>,
    /// Element -> bounds of its name label (`bpmndi:BPMNLabel`), where the modeller gave one.
    pub labels: BTreeMap<QId, Bounds>,
}

impl serde::Serialize for QId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
