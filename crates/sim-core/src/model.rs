//! Internal model. Every element is identified by a qualified id `<file>#<element id>` (spec §3.2).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Mutex;

/// Qualified id: file name plus element id (ids are only unique within one file).
///
/// Both parts are interned names: the engine copies ids into every token, event and record,
/// and a copy is just two pointers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QId {
    pub file: Name,
    pub id: Name,
}

impl QId {
    pub fn new(file: impl Into<Name>, id: impl Into<Name>) -> Self {
        Self {
            file: file.into(),
            id: id.into(),
        }
    }
}

/// An interned string, for the parts of a `QId`: copying one is free and two names are equal
/// exactly when they point to the same string. It is used like a `str` (it dereferences to one
/// and compares with `str` and `String`) and orders like one.
///
/// Every distinct name is stored once and kept for the rest of the program. That stays small:
/// it is the vocabulary of file names and element ids of the models loaded.
#[derive(Clone, Copy, Hash)]
pub struct Name(&'static str);

impl Name {
    pub fn new(s: &str) -> Self {
        static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
        let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&n) = names.get(s) {
            return Self(n);
        }
        let n: &'static str = Box::leak(s.into());
        names.insert(n);
        Self(n)
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl Default for Name {
    fn default() -> Self {
        Self::new("")
    }
}

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        // Interned: equal names are the same string.
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for Name {}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self == other {
            std::cmp::Ordering::Equal
        } else {
            self.0.cmp(other.0)
        }
    }
}

impl std::ops::Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        self.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.0
    }
}

impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.0
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Self {
        Self::new(s)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self::new(&s)
    }
}

impl From<&Name> for Name {
    fn from(s: &Name) -> Self {
        *s
    }
}

impl From<Name> for String {
    fn from(s: Name) -> Self {
        s.as_str().to_owned()
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for Name {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<Name> for str {
    fn eq(&self, other: &Name) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<Name> for &str {
    fn eq(&self, other: &Name) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<Name> for String {
    fn eq(&self, other: &Name) -> bool {
        self == other.as_str()
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self)
    }
}

impl serde::Serialize for Name {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self)
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
    /// Optional error code (resolved from `errorRef`); `None` catches every error.
    Error(Option<String>),
    /// Optional escalation code (resolved from `escalationRef`); `None` catches every escalation.
    Escalation(Option<String>),
    Terminate,
    /// An event definition the simulator does not support (e.g. conditional, compensation).
    Unsupported(String),
}

/// Which of `timeDate`, `timeDuration` and `timeCycle` a timer event definition has
/// (BPMN 2.0.2 §10.5.5). The scenario gives the time itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerKind {
    Date,
    Duration,
    /// `repeat`: the count of an ISO-8601 repeating interval `R<n>/...`; `None` without one
    /// (`R/...`, a plain duration or a cron expression): unbounded.
    Cycle {
        repeat: Option<u32>,
    },
}

impl TimerKind {
    /// Read a `timeCycle` value: `R5/PT1H` repeats 5 times, everything else is unbounded.
    pub fn cycle(text: &str) -> Self {
        let t = text.trim();
        let repeat = t
            .strip_prefix('R')
            .and_then(|rest| rest.split_once('/'))
            .and_then(|(n, _)| n.parse::<u32>().ok());
        TimerKind::Cycle { repeat }
    }
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
    /// Boundary events: `cancelActivity`; start events: `isInterrupting` (both default true).
    pub cancel_activity: bool,
    /// Call activities: called process id (`calledElement` or `zeebe:calledElement`).
    pub called_element: Option<String>,
    /// Business rule tasks: decision id (`zeebe:calledDecision`).
    pub called_decision: Option<String>,
    /// Business rule tasks: `zeebe:calledDecision resultVariable`.
    pub result_variable: Option<String>,
    /// Gateways and activities: `default` flow.
    pub default_flow: Option<QId>,
    /// Activities: multi-instance or standard loop characteristics.
    pub looping: Option<Looping>,
    /// Subprocesses: `triggeredByEvent` (an event subprocess).
    pub triggered_by_event: bool,
    /// Receive tasks and event-based gateways: `instantiate`.
    pub instantiate: bool,
    /// Timer events: which kind of time the timer definition gives, if any.
    pub timer: Option<TimerKind>,
    pub incoming: Vec<QId>,
    pub outgoing: Vec<QId>,
}

impl FlowNode {
    /// Display label: name if present, else id.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id.id)
    }
}

/// `multiInstanceLoopCharacteristics` or `standardLoopCharacteristics` of an activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Looping {
    MultiInstance {
        sequential: bool,
        /// `loopCardinality` text.
        cardinality: Option<String>,
        /// `zeebe:loopCharacteristics inputCollection`.
        input_collection: Option<String>,
        /// `completionCondition` text.
        completion_condition: Option<String>,
    },
    Standard {
        test_before: bool,
        maximum: Option<u32>,
        /// `loopCondition` text.
        condition: Option<String>,
    },
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
    /// Error id -> error code (`errorCode`, else the name, else the id).
    pub errors: BTreeMap<String, String>,
    /// Escalation id -> escalation code (`escalationCode`, else the name, else the id).
    pub escalations: BTreeMap<String, String>,
    /// Lane id -> lane name.
    pub lanes: BTreeMap<QId, String>,
    /// BPMNDI layout of the file's first diagram (only used for drawing).
    pub diagram: Diagram,
    /// Data stores, data objects, text annotations and groups (only drawn, never simulated).
    pub artifacts: BTreeMap<QId, Artifact>,
    /// Associations (data input/output associations and plain ones); `true` = has an arrow
    /// at its target (only drawn).
    pub associations: BTreeMap<QId, bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKind {
    DataStore,
    DataObject,
    TextAnnotation,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub kind: ArtifactKind,
    /// The name, or the text of a text annotation.
    pub text: Option<String>,
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
