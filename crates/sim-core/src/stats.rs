//! Result types and statistics of one replication (spec §6).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{Name, QId};

/// Descriptive statistics. Percentiles use linear interpolation between closest ranks
/// (R type 7). Everything is 0 when `count` is 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub std_dev: f64,
    pub min: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        let n = v.len() as f64;
        let mean = v.iter().sum::<f64>() / n;
        let std_dev = if v.len() > 1 {
            (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
        } else {
            0.0
        };
        Self {
            count: v.len(),
            mean,
            std_dev,
            min: v[0],
            median: percentile(&v, 0.5),
            p90: percentile(&v, 0.9),
            max: v[v.len() - 1],
        }
    }
}

/// `sorted` must be non-empty and ascending.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// One finished task execution; the basis of the event log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityRecord {
    pub case: u64,
    pub instance: u64,
    /// Token that ran the task (tells the instances of a multi-instance task apart).
    pub token: u64,
    pub activity: QId,
    pub resource: Option<Name>,
    /// Time the token arrived at the task.
    pub queued: f64,
    pub start: f64,
    pub end: f64,
    /// Working time actually spent (differs from `end - start` when a calendar paused the task).
    pub work: f64,
    pub cost: f64,
    /// Cut short by an interrupting boundary event or a terminate end event; `start` equals
    /// `end` if the task never got its resource.
    pub cancelled: bool,
}

impl ActivityRecord {
    pub fn waiting(&self) -> f64 {
        self.start - self.queued
    }
    pub fn processing(&self) -> f64 {
        self.work
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseRecord {
    pub id: u64,
    pub arrival: f64,
    pub completion: Option<f64>,
    pub cost: f64,
    /// Among the first `warm_up_cases` completed cases; left out of the statistics.
    pub warm_up: bool,
    /// The case failed through an incident (see `ReplicationResult::failed`).
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityStats {
    /// Completed runs; `waiting` and `processing` only cover these.
    pub runs: usize,
    pub cancelled: usize,
    pub waiting: Summary,
    pub processing: Summary,
    pub total_cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceStats {
    pub count: u32,
    pub tasks: u64,
    pub busy_time: f64,
    /// busy time / (count * observation window)
    pub utilisation: f64,
    pub cost: f64,
}

/// How a decision table evaluated (spec §5.9).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DecisionStats {
    pub evaluations: u64,
    /// No rule matched (outputs are null).
    pub no_match: u64,
    /// UNIQUE matched several rules / ANY rules disagreed (the first match was used).
    pub violations: u64,
    /// Output (as text) -> count.
    pub outcomes: BTreeMap<String, u64>,
}

/// One process instance: a pool of a case, a process started by a message or signal, or the
/// process a call activity called.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstanceRecord {
    /// The `instance` of activity records and trace events.
    pub id: u64,
    pub case: u64,
    pub process: QId,
    /// For a called process: the instance whose call activity called it, and that call activity.
    pub parent: Option<u64>,
    pub call: Option<QId>,
    pub start: f64,
    pub end: Option<f64>,
}

/// Completed runs of an embedded subprocess, event subprocess or call activity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScopeStats {
    /// Completed runs (cut-short runs are left out, like cancelled tasks).
    pub runs: usize,
    /// Waiting time of the tasks inside, summed per run.
    pub waiting: Summary,
    /// Working time of the tasks inside, summed per run.
    pub processing: Summary,
    /// From entering the subprocess (or calling the process) to leaving it.
    pub duration: Summary,
    /// Resource pools the tasks inside used.
    pub pools: std::collections::BTreeSet<String>,
}

/// Completed instances of one process (a call activity or message start creates its own).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProcessStats {
    pub instances: usize,
    pub cycle_time: Summary,
}

/// One step of a token, recorded for the first `simulation.trace_cases` cases (token replay).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraceEvent {
    pub time: f64,
    pub case: u64,
    pub instance: u64,
    pub token: u64,
    pub kind: TraceKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// The token reached `node`, over sequence flow `via` (none for start events, link jumps
    /// and boundary events).
    Arrive { node: QId, via: Option<QId> },
    /// The token was consumed (end event, merged at a join, cancelled).
    Gone,
}

/// A token still alive when the run ended (spec §5.8).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StuckToken {
    pub case: u64,
    pub process: QId,
    pub element: QId,
    pub name: Option<String>,
    pub state: String,
}

/// Why a case failed: an incident (BPMN 2.0.2 D-3, D-4). The instance where it happened, the
/// processes it called and the instances that called it halt with their tokens kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FailureReason {
    /// No outgoing condition is true and there is no default flow (`rule`: the rule ID).
    RuntimeException { rule: String },
    /// An error that no boundary event or event subprocess catches (R-10.5.1-03).
    UncaughtError { code: String },
}

impl std::fmt::Display for FailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailureReason::RuntimeException { rule } => write!(f, "runtime exception ({rule})"),
            FailureReason::UncaughtError { code } => {
                write!(f, "uncaught error '{code}' (R-10.5.1-03)")
            }
        }
    }
}

/// A case that failed (an incident). It never counts as completed; its tokens are listed here,
/// not as stuck tokens.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseFailure {
    pub case: u64,
    pub time: f64,
    /// Process and element where the (first) incident of the case happened.
    pub process: QId,
    pub element: QId,
    pub name: Option<String>,
    pub reason: FailureReason,
    /// Every token of the case left when the run ended.
    pub tokens: Vec<StuckToken>,
}

/// An escalation that nothing caught: it has no effect (D-4), but is reported.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UncaughtEscalation {
    pub time: f64,
    pub case: u64,
    pub process: QId,
    pub element: QId,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOutcome {
    /// A receiver was waiting and continued.
    Delivered,
    /// The target is a message start event: a new instance was created.
    StartedInstance,
    /// Nobody was waiting; stored until the receiver reaches the element.
    Buffered,
    /// Nobody was waiting and buffering is off (or the case was already complete).
    Lost,
    /// Sent to a black-box pool: only logged.
    BlackBox,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageRecord {
    pub time: f64,
    pub case: u64,
    pub flow: QId,
    pub source: QId,
    pub target: QId,
    pub outcome: MessageOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalRecord {
    pub time: f64,
    pub case: u64,
    pub name: String,
    /// Number of waiting tokens that received it; 0 means the signal was missed (not stored).
    pub receivers: usize,
    /// Instances created by signal start events.
    pub started: usize,
}

/// Buffered messages nobody ever picked up.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UndeliveredMessage {
    pub case: u64,
    pub target: QId,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplicationResult {
    pub seed: u64,
    /// Observation window: time of the last event processed.
    pub end_time: f64,
    pub cases_started: u64,
    pub cases_completed: u64,
    pub case_cycle_time: Summary,
    pub case_cost: Summary,
    pub activities: BTreeMap<QId, ActivityStats>,
    /// Embedded subprocesses, event subprocesses and call activities (of a loaded process):
    /// what happens inside them per run.
    pub scopes: BTreeMap<QId, ScopeStats>,
    /// Every process instance, in the order they were created.
    pub instances: Vec<InstanceRecord>,
    pub resources: BTreeMap<String, ResourceStats>,
    /// Tokens consumed per end event.
    pub end_events: BTreeMap<QId, u64>,
    /// Tokens left when nothing more could happen (deadlocks), outside failed cases.
    pub stuck: Vec<StuckToken>,
    /// Tokens still alive when the horizon cut the run off (instead of `stuck`), outside
    /// failed cases.
    pub unfinished: Vec<StuckToken>,
    /// Cases that failed through an incident (runtime exception, uncaught error).
    pub failed: Vec<CaseFailure>,
    /// Escalations nothing caught.
    pub uncaught_escalations: Vec<UncaughtEscalation>,
    pub horizon_reached: bool,
    /// Unix timestamp (UTC) of simulation time 0.
    pub start_epoch: i64,
    pub messages: Vec<MessageRecord>,
    pub messages_lost: u64,
    pub undelivered: Vec<UndeliveredMessage>,
    pub signals: Vec<SignalRecord>,
    /// How often each boundary event fired.
    pub boundary_fired: BTreeMap<QId, u64>,
    pub processes: BTreeMap<QId, ProcessStats>,
    /// Decision id -> evaluation statistics.
    pub decisions: BTreeMap<String, DecisionStats>,
    /// Token steps of the first `trace_cases` cases.
    pub trace: Vec<TraceEvent>,
    pub cases: Vec<CaseRecord>,
    pub records: Vec<ActivityRecord>,
}
