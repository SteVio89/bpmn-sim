//! Scenario file (TOML) and its binding to a loaded project (spec §4).
//!
//! `Scenario` is the file as written; `Scenario::bind` resolves element ids, checks the
//! parameters against the model and produces a `BoundModel` that the engine runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::calendar::{Calendar, CalendarSpec, SimClock};
use crate::dist::{Dist, DistSpec, parse_duration};
use crate::model::*;
use crate::project::Project;

#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("scenario file: {0}")]
    Toml(String),
    #[error("scenario: {0}")]
    Invalid(String),
    #[error("scenario has {} problem(s):\n  - {}", .0.len(), .0.join("\n  - "))]
    Many(Vec<String>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelSection>,
    #[serde(default)]
    pub simulation: SimulationSection,
    #[serde(default)]
    pub arrivals: Vec<ArrivalSpec>,
    #[serde(default)]
    pub resources: BTreeMap<String, ResourceSpec>,
    #[serde(default)]
    pub activities: BTreeMap<String, ActivitySpec>,
    #[serde(default)]
    pub gateways: BTreeMap<String, GatewaySpec>,
    // Sections of later milestones: accepted, reported as "not supported yet".
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub calendars: BTreeMap<String, CalendarSpec>,
    /// Catch events whose trigger comes from outside the model (delay until they fire).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, EventSpec>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub boundary: BTreeMap<String, BoundarySpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<MessagesSection>,
    /// Decision id -> how its inputs are sampled.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub decisions: BTreeMap<String, DecisionSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    pub label: Option<String>,
    /// Input expression (variable name) -> distribution of its value.
    #[serde(default)]
    pub inputs: BTreeMap<String, InputSpec>,
}

/// A decision input: a number distribution (`dist = ...`, plain numbers, no time units),
/// a categorical choice (`choice = { gold = 0.2, silver = 0.8 }`) or a boolean (`p_true = 0.3`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputSpec {
    Choice(ChoiceSpec),
    Chance(ChanceSpec),
    Number(NumberSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceSpec {
    pub choice: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChanceSpec {
    pub p_true: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "dist", rename_all = "lowercase", deny_unknown_fields)]
pub enum NumberSpec {
    Fixed { value: f64 },
    Uniform { min: f64, max: f64 },
    Normal { mean: f64, stdev: f64 },
    Exponential { mean: f64 },
    Triangular { min: f64, mode: f64, max: f64 },
    Lognormal { mean: f64, stdev: f64 },
}

impl NumberSpec {
    fn to_dist(&self) -> Result<Dist, String> {
        use crate::dist::Seconds as S;
        Dist::new(&match *self {
            NumberSpec::Fixed { value } => DistSpec::Fixed { value: S(value) },
            NumberSpec::Uniform { min, max } => DistSpec::Uniform {
                min: S(min),
                max: S(max),
            },
            NumberSpec::Normal { mean, stdev } => DistSpec::Normal {
                mean: S(mean),
                stdev: S(stdev),
            },
            NumberSpec::Exponential { mean } => DistSpec::Exponential { mean: S(mean) },
            NumberSpec::Triangular { min, mode, max } => DistSpec::Triangular {
                min: S(min),
                mode: S(mode),
                max: S(max),
            },
            NumberSpec::Lognormal { mean, stdev } => DistSpec::Lognormal {
                mean: S(mean),
                stdev: S(stdev),
            },
        })
    }
}

/// How a decision input is drawn.
#[derive(Debug, Clone)]
pub enum Sampler {
    Number(Dist),
    Choice(Vec<(String, f64)>),
    Chance(f64),
}

impl InputSpec {
    fn sampler(&self) -> Result<Sampler, String> {
        match self {
            InputSpec::Number(n) => n.to_dist().map(Sampler::Number),
            InputSpec::Chance(c) if (0.0..=1.0).contains(&c.p_true) => {
                Ok(Sampler::Chance(c.p_true))
            }
            InputSpec::Chance(_) => Err("p_true must be between 0 and 1".into()),
            InputSpec::Choice(c) => {
                let sum: f64 = c.choice.values().sum();
                if c.choice.is_empty()
                    || c.choice.values().any(|p| *p < 0.0)
                    || (sum - 1.0).abs() > 1e-9
                {
                    return Err(format!(
                        "choice probabilities must be non-negative and sum to 1 (they sum to {sum})"
                    ));
                }
                Ok(Sampler::Choice(
                    c.choice.iter().map(|(k, p)| (k.clone(), *p)).collect(),
                ))
            }
        }
    }
}

/// A business rule task's decision, ready to evaluate.
#[derive(Debug, Clone)]
pub struct BoundDecision {
    pub decision: String,
    pub table: crate::dmn::Table,
    pub result_variable: Option<String>,
    pub samplers: BTreeMap<String, Sampler>,
}

/// A split decided by its flows' condition expressions.
#[derive(Debug, Clone)]
pub struct ConditionCfg {
    pub flows: Vec<(QId, crate::feel::Expr)>,
    pub default: Option<QId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSpec {
    pub label: Option<String>,
    pub delay: DistSpec,
}

/// Timer boundary: `delay` (+ optional `probability`, default 1). Error/escalation boundary:
/// `probability` and `delay` (time after the activity was activated). Message/signal boundary: none.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySpec {
    pub label: Option<String>,
    pub probability: Option<f64>,
    pub delay: Option<DistSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessagesSection {
    /// Keep messages that arrive before the receiver waits (otherwise they are lost, spec §5.3).
    #[serde(default = "default_true")]
    pub buffer_unconsumed: bool,
    /// Black-box participant -> delay until it replies along its outgoing message flows.
    #[serde(default)]
    pub black_box_reply: BTreeMap<String, DistSpec>,
}

fn default_true() -> bool {
    true
}

impl Default for MessagesSection {
    fn default() -> Self {
        Self {
            buffer_unconsumed: true,
            black_box_reply: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignalScope {
    /// Signals reach only instances of the same case (default, spec §5.4).
    #[default]
    Case,
    Global,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationSection {
    #[serde(default = "default_seed")]
    pub seed: u64,
    #[serde(default = "default_replications")]
    pub replications: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases: Option<u64>,
    /// Stop the run at this simulation time (e.g. "365d"). With `cases` too, whichever comes first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<String>,
    /// Real date and time of simulation time 0 (UTC), default Monday 2024-01-01 00:00. It decides
    /// which weekday the calendars start on and the timestamps of exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default)]
    pub warm_up_cases: u64,
    #[serde(default = "default_time_unit")]
    pub time_unit: TimeUnit,
    #[serde(default, skip_serializing_if = "is_default_scope")]
    pub signal_scope: SignalScope,
    /// Record every token step of the first N cases (for the animated token replay).
    #[serde(default = "default_trace_cases")]
    pub trace_cases: u64,
}

fn default_trace_cases() -> u64 {
    100
}

fn is_default_scope(s: &SignalScope) -> bool {
    *s == SignalScope::Case
}

fn default_seed() -> u64 {
    1
}
fn default_replications() -> u32 {
    1
}
fn default_time_unit() -> TimeUnit {
    TimeUnit::Minutes
}

impl Default for SimulationSection {
    fn default() -> Self {
        Self {
            seed: default_seed(),
            replications: default_replications(),
            cases: None,
            horizon: None,
            start: None,
            warm_up_cases: 0,
            time_unit: default_time_unit(),
            signal_scope: SignalScope::Case,
            trace_cases: default_trace_cases(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl TimeUnit {
    pub fn seconds(self) -> f64 {
        match self {
            TimeUnit::Seconds => 1.0,
            TimeUnit::Minutes => 60.0,
            TimeUnit::Hours => 3600.0,
            TimeUnit::Days => 86_400.0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TimeUnit::Seconds => "seconds",
            TimeUnit::Minutes => "minutes",
            TimeUnit::Hours => "hours",
            TimeUnit::Days => "days",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrivalSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub inter_arrival: DistSpec,
    pub triggers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSpec {
    pub count: u32,
    #[serde(default)]
    pub cost_per_hour: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivitySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub duration: DistSpec,
    #[serde(default)]
    pub fixed_cost: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewaySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub probabilities: BTreeMap<String, f64>,
}

impl Scenario {
    pub fn from_toml(text: &str) -> Result<Self, ScenarioError> {
        toml::from_str(text).map_err(|e| ScenarioError::Toml(e.to_string()))
    }
}

// ---------------------------------------------------------------- bound model

#[derive(Debug, Clone)]
pub struct SimParams {
    pub seed: u64,
    pub replications: u32,
    pub cases: u64,
    pub warm_up_cases: u64,
    pub time_unit: TimeUnit,
    /// Stop at this simulation time.
    pub horizon: Option<f64>,
    pub clock: SimClock,
    pub trace_cases: u64,
}

#[derive(Debug, Clone)]
pub struct Arrival {
    pub label: Option<String>,
    pub inter_arrival: Dist,
    /// Start events, grouped per process in the order of first appearance.
    pub triggers: Vec<QId>,
}

#[derive(Debug, Clone)]
pub struct ActivityCfg {
    pub duration: Dist,
    pub fixed_cost: f64,
    /// Resource pool name; `None` = no resource needed.
    pub pool: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PoolDef {
    pub count: u32,
    pub cost_per_hour: f64,
    pub calendar: Option<Calendar>,
}

/// Everything the engine needs, fully resolved and validated.
#[derive(Debug, Clone)]
pub struct BoundModel<'p> {
    pub project: &'p Project,
    pub sim: SimParams,
    pub arrivals: Vec<Arrival>,
    pub pools: BTreeMap<String, PoolDef>,
    pub activities: BTreeMap<QId, ActivityCfg>,
    /// XOR split -> (outgoing flow, probability), in model order.
    pub gateways: BTreeMap<QId, Vec<(QId, f64)>>,
    /// Inclusive (OR) split -> independent flow probabilities and the default flow.
    pub inclusive: BTreeMap<QId, InclusiveCfg>,
    /// Splits whose flows' conditions decide (no `[gateways]` entry, every flow has a condition).
    pub conditions: BTreeMap<QId, ConditionCfg>,
    /// Activities and events with conditional outgoing flows.
    pub activity_splits: BTreeMap<QId, ActivitySplitCfg>,
    /// Business rule task -> its evaluable decision.
    pub decisions: BTreeMap<QId, BoundDecision>,
    pub buffer_unconsumed: bool,
    pub signal_scope: SignalScope,
    /// Black-box participant -> reply delay.
    pub black_box_reply: BTreeMap<QId, Dist>,
    /// Delay of externally triggered catch events (timer events, optionally message/signal).
    pub events: BTreeMap<QId, Dist>,
    pub boundaries: BTreeMap<QId, BoundaryCfg>,
    /// Processes that take part in the run (triggered, or reached by messages/signals).
    pub active: Vec<QId>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InclusiveCfg {
    /// Non-default outgoing flows and their probabilities, in model order.
    pub flows: Vec<(QId, f64)>,
    pub default: Option<QId>,
}

/// Conditional outgoing flows of an activity or event: `always` flows are always taken, the
/// conditional ones by `rule`, the default flow when no conditional flow is taken.
#[derive(Debug, Clone)]
pub struct ActivitySplitCfg {
    pub always: Vec<QId>,
    pub rule: SplitRule,
    pub default: Option<QId>,
}

#[derive(Debug, Clone)]
pub enum SplitRule {
    /// The flows' condition expressions.
    Conditions(Vec<(QId, crate::feel::Expr)>),
    /// `[gateways]` probabilities, each flow drawn independently.
    Probabilities(Vec<(QId, f64)>),
}

#[derive(Debug, Clone)]
pub struct BoundaryCfg {
    pub probability: f64,
    pub delay: Option<Dist>,
}

/// Resolve a scenario key to an element: bare ids must be unique across files (spec §4).
fn resolve(project: &Project, key: &str, what: &str) -> Result<QId, String> {
    if let Some((file, id)) = key.split_once('#') {
        let q = QId::new(file, id);
        return if project.node(&q).is_some() {
            Ok(q)
        } else {
            Err(format!("{what} '{key}' does not exist in the loaded model"))
        };
    }
    let matches: Vec<&FlowNode> = project.nodes().filter(|n| n.id.id == key).collect();
    match matches.as_slice() {
        [] => Err(format!("{what} '{key}' does not exist in the loaded model")),
        [one] => Ok(one.id.clone()),
        many => Err(format!(
            "{what} '{key}' is ambiguous, it exists in {}; use a qualified id like '{}'",
            many.iter()
                .map(|n| n.id.file.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            many[0].id
        )),
    }
}

impl Scenario {
    pub fn bind<'p>(&self, project: &'p Project) -> Result<BoundModel<'p>, ScenarioError> {
        let mut errors: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        if let Some(m) = &self.model {
            let loaded = |f: &String| project.files.contains_key(f) || project.dmn.contains_key(f);
            if let Some(main) = &m.main
                && !project.files.contains_key(main)
            {
                errors.push(format!("model.main '{main}' is not a loaded BPMN file"));
            }
            for f in m.files.iter().flatten().filter(|f| !loaded(f)) {
                errors.push(format!("model.files lists '{f}', which is not loaded"));
            }
        }

        let sim = &self.simulation;
        let horizon = match sim.horizon.as_deref().map(parse_duration) {
            None => None,
            Some(Ok(h)) if h > 0.0 => Some(h),
            Some(Ok(_)) => {
                errors.push("simulation.horizon must be positive".into());
                None
            }
            Some(Err(e)) => {
                errors.push(format!("simulation.horizon: {e}"));
                None
            }
        };
        let clock = match sim.start.as_deref().map(SimClock::parse) {
            None => SimClock::default(),
            Some(Ok(c)) => c,
            Some(Err(e)) => {
                errors.push(format!("simulation.{e}"));
                SimClock::default()
            }
        };
        let cases = match (sim.cases, horizon) {
            (Some(c), _) if c > 0 => c,
            (None, Some(_)) => u64::MAX,
            _ => {
                errors.push(
                    "set simulation.cases to a positive number (or a simulation.horizon)".into(),
                );
                0
            }
        };
        if sim.replications == 0 {
            errors.push("simulation.replications must be at least 1".into());
        }
        if sim.warm_up_cases >= cases && cases > 0 {
            errors.push("simulation.warm_up_cases must be smaller than simulation.cases".into());
        }

        let mut calendars = BTreeMap::new();
        for (name, spec) in &self.calendars {
            match Calendar::new(name, spec, clock) {
                Ok(c) => {
                    calendars.insert(name.clone(), c);
                }
                Err(e) => errors.push(e),
            }
        }

        let mut pools = BTreeMap::new();
        for (name, r) in &self.resources {
            if r.count == 0 {
                errors.push(format!("resource '{name}': count must be at least 1"));
            }
            let calendar = match &r.calendar {
                None => None,
                Some(c) => {
                    let found = calendars.get(c).cloned();
                    if found.is_none() && !self.calendars.contains_key(c) {
                        errors.push(format!(
                            "resource '{name}': calendar '{c}' is not defined under [calendars]"
                        ));
                    }
                    found
                }
            };
            pools.insert(
                name.clone(),
                PoolDef {
                    count: r.count,
                    cost_per_hour: r.cost_per_hour,
                    calendar,
                },
            );
        }

        // Arrivals and the processes they instantiate.
        let mut arrivals = Vec::new();
        let mut processes: Vec<QId> = Vec::new();
        if self.arrivals.is_empty() {
            errors.push("at least one [[arrivals]] entry is required".into());
        }
        for (i, a) in self.arrivals.iter().enumerate() {
            let label = a.label.clone().unwrap_or_else(|| format!("arrivals[{i}]"));
            let inter_arrival = Dist::new(&a.inter_arrival)
                .map_err(|e| errors.push(format!("{label}: inter_arrival: {e}")))
                .ok();
            let mut triggers = Vec::new();
            for t in &a.triggers {
                match resolve(project, t, "trigger") {
                    Ok(q) => {
                        let Some(n) = project.node(&q) else { continue };
                        if n.kind != NodeKind::StartEvent {
                            errors.push(format!("{label}: trigger '{q}' is not a start event"));
                        } else if !n
                            .event_defs
                            .iter()
                            .all(|d| matches!(d, EventDef::None | EventDef::Timer))
                        {
                            errors.push(format!(
                                "{label}: trigger '{q}' is a message/signal/other start event; arrivals can only fire none or timer start events"
                            ));
                        } else {
                            if !processes.contains(&n.process) {
                                processes.push(n.process.clone());
                            }
                            triggers.push(q);
                        }
                    }
                    Err(e) => errors.push(format!("{label}: {e}")),
                }
            }
            if triggers.is_empty() {
                errors.push(format!("{label}: no valid triggers"));
            }
            if let Some(inter_arrival) = inter_arrival {
                arrivals.push(Arrival {
                    label: a.label.clone(),
                    inter_arrival,
                    triggers,
                });
            }
        }

        // Message-related configuration and the processes reached through messages.
        let messages = self.messages.clone().unwrap_or_default();
        let mut black_box_reply = BTreeMap::new();
        for (key, spec) in &messages.black_box_reply {
            match resolve_participant(project, key) {
                Ok(q) => match Dist::new(spec) {
                    Ok(d) => {
                        black_box_reply.insert(q, d);
                    }
                    Err(e) => errors.push(format!("[messages.black_box_reply.{key}]: {e}")),
                },
                Err(e) => errors.push(format!("[messages.black_box_reply]: {e}")),
            }
        }
        // Fixpoint: a message flow from an active process (or a replying black box) to a message
        // start event activates the receiving process.
        loop {
            let mut grew = false;
            for f in project
                .files
                .values()
                .flat_map(|f| f.message_flows.values())
            {
                let source_active = match project.node(&f.source) {
                    Some(n) => processes.contains(&n.process),
                    None => black_box_reply.contains_key(&f.source),
                };
                let Some(t) = project.node(&f.target) else {
                    continue;
                };
                if source_active
                    && t.kind == NodeKind::StartEvent
                    && t.event_defs
                        .iter()
                        .any(|d| matches!(d, EventDef::Message(_)))
                    && !processes.contains(&t.process)
                {
                    processes.push(t.process.clone());
                    grew = true;
                }
            }
            // Called processes of call activities in active processes.
            let called: Vec<QId> = project
                .nodes()
                .filter(|n| n.kind == NodeKind::CallActivity && processes.contains(&n.process))
                .filter_map(|n| n.called_element.as_ref())
                .filter_map(|c| project.processes.get(c).cloned())
                .collect();
            for p in called {
                if !processes.contains(&p) {
                    processes.push(p);
                    grew = true;
                }
            }
            // Signal start events of processes whose signal is thrown by an active process.
            let thrown: Vec<&str> = project
                .nodes()
                .filter(|n| processes.contains(&n.process))
                .flat_map(|n| n.event_defs.iter())
                .filter_map(|d| match d {
                    EventDef::Signal(name) => Some(name.as_deref().unwrap_or_default()),
                    _ => None,
                })
                .collect();
            let to_add: Vec<QId> = project
                .nodes()
                .filter(|n| n.kind == NodeKind::StartEvent && !processes.contains(&n.process))
                .filter(|n| {
                    n.event_defs.iter().any(|d| {
                        matches!(d, EventDef::Signal(name) if thrown.contains(&name.as_deref().unwrap_or_default()))
                    })
                })
                .map(|n| n.process.clone())
                .collect();
            for p in to_add {
                if !processes.contains(&p) {
                    processes.push(p);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        for f in project
            .files
            .values()
            .flat_map(|f| f.message_flows.values())
        {
            let src = project.node(&f.source);
            let active = match src {
                Some(n) => processes.contains(&n.process),
                None => black_box_reply.contains_key(&f.source),
            };
            if !active {
                continue;
            }
            if let Some(n) = src
                && !sends_messages(n)
            {
                errors.push(format!(
                    "message flow {}: source '{}' is not a send task or message throw/end event",
                    f.id, f.source
                ));
            }
            match project.node(&f.target) {
                Some(t) => {
                    if !receives_messages(t) {
                        errors.push(format!(
                            "message flow {}: target '{}' cannot receive a message",
                            f.id, f.target
                        ));
                    } else if !processes.contains(&t.process) && t.kind != NodeKind::StartEvent {
                        warnings.push(format!(
                            "message flow {}: target '{}' is in a process that is never instantiated, the message will stay undelivered",
                            f.id, f.target
                        ));
                    }
                }
                None => {
                    let is_black_box = project
                        .files
                        .get(&f.target.file)
                        .and_then(|file| file.participants.get(&f.target))
                        .is_some_and(|p| p.process.is_none());
                    if !is_black_box {
                        errors.push(format!(
                            "message flow {}: target '{}' is a pool with a process; point the flow at a message start or catch event instead",
                            f.id, f.target
                        ));
                    }
                }
            }
        }

        // Element support and per-element configuration for every instantiated process.
        let nodes: Vec<&FlowNode> = project
            .nodes()
            .filter(|n| processes.contains(&n.process))
            .collect();
        for n in &nodes {
            if let Some(why) = unsupported(n) {
                errors.push(format!("{} ({}): {why}", n.id, n.label()));
            }
        }

        // Link events, subprocess starts, catch-event delays and boundary events.
        for n in &nodes {
            if n.kind == NodeKind::IntermediateThrowEvent {
                for d in &n.event_defs {
                    if let EventDef::Link(name) = d
                        && !nodes.iter().any(|c| {
                            c.process == n.process
                                && c.kind == NodeKind::IntermediateCatchEvent
                                && c.event_defs
                                    .iter()
                                    .any(|d| matches!(d, EventDef::Link(l) if l == name))
                        })
                    {
                        errors.push(format!(
                            "{} ({}): no link catch event named '{name}' in this process",
                            n.id,
                            n.label()
                        ));
                    }
                }
            }
            if n.kind == NodeKind::CallActivity {
                match n
                    .called_element
                    .as_ref()
                    .and_then(|c| project.processes.get(c))
                {
                    None => errors.push(format!(
                        "{} ({}): called process {} is not defined in any loaded file",
                        n.id,
                        n.label(),
                        n.called_element.as_deref().unwrap_or("(none)")
                    )),
                    Some(p) => {
                        if !project.nodes().any(|c| {
                            c.process == *p
                                && c.parent.is_none()
                                && c.kind == NodeKind::StartEvent
                                && c.event_defs.iter().all(|d| matches!(d, EventDef::None))
                        }) {
                            errors.push(format!(
                                "{} ({}): called process '{}' has no none start event",
                                n.id,
                                n.label(),
                                p.id
                            ));
                        }
                    }
                }
            }
            if n.kind == NodeKind::SubProcess
                && !nodes.iter().any(|c| {
                    c.parent.as_ref() == Some(&n.id)
                        && c.kind == NodeKind::StartEvent
                        && c.event_defs.iter().all(|d| matches!(d, EventDef::None))
                })
            {
                errors.push(format!(
                    "{} ({}): embedded subprocess has no none start event",
                    n.id,
                    n.label()
                ));
            }
        }

        // Call activities must not (indirectly) call their own process.
        for start in &processes {
            let mut seen: Vec<QId> = Vec::new();
            let mut stack = vec![start.clone()];
            while let Some(p) = stack.pop() {
                for n in project
                    .nodes()
                    .filter(|n| n.process == p && n.kind == NodeKind::CallActivity)
                {
                    let Some(next) = n
                        .called_element
                        .as_ref()
                        .and_then(|c| project.processes.get(c))
                    else {
                        continue;
                    };
                    if next == start {
                        errors.push(format!(
                            "{}: recursive call, process '{}' (indirectly) calls itself",
                            n.id, start.id
                        ));
                    } else if !seen.contains(next) {
                        seen.push(next.clone());
                        stack.push(next.clone());
                    }
                }
            }
        }

        let mut events = BTreeMap::new();
        let mut event_specs: BTreeMap<QId, &EventSpec> = BTreeMap::new();
        for (key, spec) in &self.events {
            match resolve(project, key, "event") {
                Ok(q) => match project.node(&q).map(|n| &n.kind) {
                    Some(NodeKind::IntermediateCatchEvent | NodeKind::Task(TaskKind::Receive)) => {
                        event_specs.insert(q, spec);
                    }
                    _ => errors.push(format!(
                        "[events.{key}]: '{q}' is not an intermediate catch event or receive task"
                    )),
                },
                Err(e) => errors.push(format!("[events]: {e}")),
            }
        }
        for n in &nodes {
            let is_timer = n.kind == NodeKind::IntermediateCatchEvent
                && n.event_defs.iter().any(|d| matches!(d, EventDef::Timer));
            match event_specs.get(&n.id) {
                Some(spec) => match Dist::new(&spec.delay) {
                    Ok(d) => {
                        events.insert(n.id.clone(), d);
                    }
                    Err(e) => errors.push(format!("[events] {}: delay: {e}", n.id)),
                },
                None if is_timer => errors.push(format!(
                    "{} ({}): timer event needs an [events] entry with a delay",
                    n.id,
                    n.label()
                )),
                None => {}
            }
        }

        let mut boundary_specs: BTreeMap<QId, &BoundarySpec> = BTreeMap::new();
        for (key, spec) in &self.boundary {
            match resolve(project, key, "boundary event") {
                Ok(q) if project.node(&q).map(|n| &n.kind) == Some(&NodeKind::BoundaryEvent) => {
                    boundary_specs.insert(q, spec);
                }
                Ok(q) => errors.push(format!("[boundary.{key}]: '{q}' is not a boundary event")),
                Err(e) => errors.push(format!("[boundary]: {e}")),
            }
        }
        let mut boundaries = BTreeMap::new();
        for n in nodes.iter().filter(|n| n.kind == NodeKind::BoundaryEvent) {
            let name = format!("{} ({})", n.id, n.label());
            let host = n.attached_to.as_ref().and_then(|a| project.node(a));
            if !matches!(
                host.map(|h| &h.kind),
                Some(NodeKind::Task(_) | NodeKind::SubProcess | NodeKind::CallActivity)
            ) {
                errors.push(format!(
                    "{name}: boundary events are only supported on tasks, call activities and embedded subprocesses"
                ));
            }
            let spec = boundary_specs.get(&n.id);
            let Some(def) = n.event_defs.first() else {
                continue;
            };
            let (needs_prob, needs_delay) = match def {
                EventDef::Timer => (false, true),
                EventDef::Error | EventDef::Escalation => (true, true),
                _ => (false, false),
            };
            if (needs_prob || needs_delay) && spec.is_none() {
                errors.push(format!("{name}: needs a [boundary] entry"));
                continue;
            }
            if needs_prob && spec.is_some_and(|s| s.probability.is_none()) {
                errors.push(format!("{name}: [boundary] entry needs a probability"));
            }
            if needs_delay && spec.is_some_and(|s| s.delay.is_none()) {
                errors.push(format!("{name}: [boundary] entry needs a delay"));
            }
            if !(needs_prob || needs_delay) && spec.is_some() {
                warnings.push(format!("{name}: message/signal boundary events take no [boundary] parameters, the entry is ignored"));
            }
            let probability = spec.and_then(|s| s.probability).unwrap_or(1.0);
            if !(0.0..=1.0).contains(&probability) {
                errors.push(format!("{name}: probability must be between 0 and 1"));
            }
            let delay = match spec.and_then(|s| s.delay.as_ref()).filter(|_| needs_delay) {
                Some(d) => match Dist::new(d) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        errors.push(format!("{name}: delay: {e}"));
                        None
                    }
                },
                None => None,
            };
            boundaries.insert(n.id.clone(), BoundaryCfg { probability, delay });
        }

        let mut configured: BTreeMap<QId, &ActivitySpec> = BTreeMap::new();
        for (key, spec) in &self.activities {
            match resolve(project, key, "activity") {
                Ok(q) => {
                    if !matches!(project.node(&q).map(|n| &n.kind), Some(NodeKind::Task(_))) {
                        errors.push(format!("[activities.{key}]: '{q}' is not a task"));
                    } else if configured.insert(q.clone(), spec).is_some() {
                        errors.push(format!("[activities.{key}]: '{q}' is configured twice"));
                    }
                }
                Err(e) => errors.push(format!("[activities]: {e}")),
            }
        }

        let mut activities = BTreeMap::new();
        for n in nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Task(k) if k != TaskKind::Receive))
        {
            let Some(spec) = configured.get(&n.id) else {
                errors.push(format!(
                    "{} ({}): no [activities] entry with a duration",
                    n.id,
                    n.label()
                ));
                continue;
            };
            let duration = match Dist::new(&spec.duration) {
                Ok(d) => d,
                Err(e) => {
                    errors.push(format!("{}: duration: {e}", n.id));
                    continue;
                }
            };
            let pool = spec.resource.clone().or_else(|| {
                n.lane
                    .clone()
                    .or_else(|| project.participant_name(&n.process).map(str::to_string))
            });
            if let Some(p) = &pool
                && !pools.contains_key(p)
            {
                errors.push(format!(
                    "{} ({}): resource pool '{p}' is not defined under [resources]",
                    n.id,
                    n.label()
                ));
            }
            activities.insert(
                n.id.clone(),
                ActivityCfg {
                    duration,
                    fixed_cost: spec.fixed_cost,
                    pool,
                },
            );
        }

        // Decisions of business rule tasks (DMN evaluation, spec §5.9).
        for key in self.decisions.keys() {
            if !project.decisions.contains_key(key) {
                errors.push(format!(
                    "[decisions.{key}]: no loaded DMN file defines this decision"
                ));
            }
        }
        let mut decisions = BTreeMap::new();
        let mut produced: Vec<String> = Vec::new();
        let mut pending = Vec::new();
        for n in nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Task(TaskKind::BusinessRule))
        {
            let Some(id) = &n.called_decision else {
                continue;
            };
            let Some(decision) = project
                .decisions
                .get(id)
                .and_then(|f| project.dmn.get(f))
                .and_then(|f| f.decisions.get(id))
            else {
                warnings.push(format!(
                    "{}: decision '{id}' is not loaded; the task runs without evaluating it",
                    n.id
                ));
                continue;
            };
            let Some(raw) = &decision.table else {
                errors.push(format!("{}: decision '{id}' is not a decision table", n.id));
                continue;
            };
            match crate::dmn::Table::compile(id, raw) {
                Ok(table) => {
                    for o in &table.outputs {
                        produced.push(o.clone());
                        if let Some(r) = &n.result_variable {
                            produced.push(format!("{r}.{o}"));
                        }
                    }
                    produced.extend(n.result_variable.clone());
                    pending.push((n.id.clone(), id.clone(), table, n.result_variable.clone()));
                }
                Err(e) => errors.push(format!("{}: {e}", n.id)),
            }
        }
        // Inputs are process variables: a distribution given under any decision serves every
        // decision with an input of that name (it is sampled once per instance).
        let mut global: BTreeMap<String, (String, Sampler)> = BTreeMap::new();
        for (id, spec) in &self.decisions {
            for (input, s) in &spec.inputs {
                match s.sampler() {
                    Ok(sampler) => {
                        if let Some((first, _)) = global.get(input) {
                            warnings.push(format!(
                                "decision input '{input}' is defined under both [decisions.{first}] and [decisions.{id}]; the first is used"
                            ));
                        } else {
                            global.insert(input.clone(), (id.clone(), sampler));
                        }
                    }
                    Err(e) => errors.push(format!("[decisions.{id}.inputs.{input}]: {e}")),
                }
            }
        }
        let all_inputs: Vec<String> = pending
            .iter()
            .flat_map(|(_, _, t, _)| t.inputs.clone())
            .collect();
        for (id, spec) in &self.decisions {
            for k in spec.inputs.keys().filter(|k| !all_inputs.contains(k)) {
                warnings.push(format!("[decisions.{id}.inputs]: '{k}' is not an input of any decision used and is ignored"));
            }
        }
        for (task, id, table, result_variable) in pending {
            let mut samplers = BTreeMap::new();
            for input in &table.inputs {
                match global.get(input) {
                    Some((_, s)) => {
                        samplers.insert(input.clone(), s.clone());
                    }
                    // Values set by another decision need no sampler.
                    None if produced.contains(input) => {}
                    None => errors.push(format!(
                        "{task}: decision '{id}' needs a distribution for its input '{input}' under [decisions.\"{id}\".inputs]"
                    )),
                }
            }
            decisions.insert(
                task,
                BoundDecision {
                    decision: id,
                    table,
                    result_variable,
                    samplers,
                },
            );
        }

        // Split probabilities: XOR splits need one per flow summing to 1; inclusive (OR) splits
        // take each flow independently with its probability, except the default flow.
        let mut gateways = BTreeMap::new();
        let mut inclusive = BTreeMap::new();
        let mut conditions = BTreeMap::new();
        let mut gw_specs: BTreeMap<QId, &GatewaySpec> = BTreeMap::new();
        for (key, spec) in &self.gateways {
            match resolve(project, key, "gateway") {
                Ok(q) => match project.node(&q) {
                    Some(n)
                        if matches!(
                            n.kind,
                            NodeKind::ExclusiveGateway | NodeKind::InclusiveGateway
                        ) || project.activity_split(n).is_some() =>
                    {
                        gw_specs.insert(q, spec);
                    }
                    _ => errors.push(format!(
                        "[gateways.{key}]: '{q}' is not an exclusive or inclusive gateway or an element with conditional outgoing flows"
                    )),
                },
                Err(e) => errors.push(format!("[gateways]: {e}")),
            }
        }
        for n in nodes.iter().filter(|n| {
            matches!(
                n.kind,
                NodeKind::ExclusiveGateway | NodeKind::InclusiveGateway
            ) && n.outgoing.len() > 1
        }) {
            let is_or = n.kind == NodeKind::InclusiveGateway;
            let conditional: Vec<(QId, Option<&String>)> = n
                .outgoing
                .iter()
                .filter(|f| n.default_flow.as_ref() != Some(*f))
                .map(|f| {
                    (
                        f.clone(),
                        project.flow(f).and_then(|fl| fl.condition.as_ref()),
                    )
                })
                .collect();
            let all_conditions = conditional.iter().all(|(_, c)| c.is_some());
            if !gw_specs.contains_key(&n.id) && all_conditions {
                let mut flows = Vec::new();
                for (f, c) in conditional {
                    match c.map(|c| crate::feel::Expr::parse(c)) {
                        Some(Ok(e)) => flows.push((f, e)),
                        Some(Err(e)) => {
                            errors.push(format!("{} condition of flow {}: {e}", n.id, f.id))
                        }
                        None => {}
                    }
                }
                conditions.insert(
                    n.id.clone(),
                    ConditionCfg {
                        flows,
                        default: n.default_flow.clone(),
                    },
                );
                continue;
            }
            if gw_specs.contains_key(&n.id) && all_conditions {
                warnings.push(format!(
                    "{}: the [gateways] probabilities are used instead of the flow conditions",
                    n.id
                ));
            }
            let Some(spec) = gw_specs.get(&n.id) else {
                errors.push(format!(
                    "{} ({}): {} split needs a [gateways] entry with probabilities (or a condition on every outgoing flow)",
                    n.id,
                    n.label(),
                    if is_or { "inclusive" } else { "XOR" }
                ));
                continue;
            };
            let default = n.default_flow.clone().filter(|_| is_or);
            let mut by_flow: BTreeMap<QId, f64> = BTreeMap::new();
            for (k, p) in &spec.probabilities {
                let q = match k.split_once('#') {
                    Some((f, i)) => QId::new(f, i),
                    None => QId::new(&n.id.file, k),
                };
                if !n.outgoing.contains(&q) {
                    errors.push(format!(
                        "{}: '{k}' is not an outgoing flow of this gateway",
                        n.id
                    ));
                } else if default.as_ref() == Some(&q) {
                    errors.push(format!(
                        "{}: '{k}' is the default flow of an inclusive gateway and takes no probability (it is used when no other flow is chosen)",
                        n.id
                    ));
                } else if !(0.0..=1.0).contains(p) {
                    errors.push(format!(
                        "{}: probability of '{k}' must be between 0 and 1",
                        n.id
                    ));
                } else {
                    by_flow.insert(q, *p);
                }
            }
            let mut list = Vec::new();
            for f in n.outgoing.iter().filter(|f| default.as_ref() != Some(*f)) {
                match by_flow.get(f) {
                    Some(p) => list.push((f.clone(), *p)),
                    None => errors.push(format!(
                        "{}: no probability for outgoing flow '{}'",
                        n.id, f.id
                    )),
                }
            }
            if is_or {
                inclusive.insert(
                    n.id.clone(),
                    InclusiveCfg {
                        flows: list,
                        default,
                    },
                );
            } else {
                let sum: f64 = list.iter().map(|(_, p)| p).sum();
                if list.len() == n.outgoing.len() && (sum - 1.0).abs() > 1e-9 {
                    errors.push(format!("{}: probabilities sum to {sum}, expected 1", n.id));
                }
                gateways.insert(n.id.clone(), list);
            }
        }

        // Conditional flows leaving an activity or event: decided by their conditions, or by
        // independent probabilities from a `[gateways]` entry (like an inclusive split).
        let mut activity_splits = BTreeMap::new();
        for n in &nodes {
            let Some(split) = project.activity_split(n) else {
                continue;
            };
            let rule = match gw_specs.get(&n.id) {
                Some(spec) => {
                    let mut by_flow: BTreeMap<QId, f64> = BTreeMap::new();
                    for (k, p) in &spec.probabilities {
                        let q = match k.split_once('#') {
                            Some((f, i)) => QId::new(f, i),
                            None => QId::new(&n.id.file, k),
                        };
                        if !split.conditional.iter().any(|(f, _)| *f == q) {
                            errors.push(format!(
                                "{}: '{k}' is not a conditional outgoing flow of this element",
                                n.id
                            ));
                        } else if !(0.0..=1.0).contains(p) {
                            errors.push(format!(
                                "{}: probability of '{k}' must be between 0 and 1",
                                n.id
                            ));
                        } else {
                            by_flow.insert(q, *p);
                        }
                    }
                    let mut list = Vec::new();
                    for (f, _) in &split.conditional {
                        match by_flow.get(f) {
                            Some(p) => list.push((f.clone(), *p)),
                            None => errors.push(format!(
                                "{}: no probability for conditional flow '{}'",
                                n.id, f.id
                            )),
                        }
                    }
                    SplitRule::Probabilities(list)
                }
                None => {
                    let mut flows = Vec::new();
                    for (f, c) in &split.conditional {
                        match crate::feel::Expr::parse(c) {
                            Ok(e) => flows.push((f.clone(), e)),
                            Err(e) => {
                                errors.push(format!("{} condition of flow {}: {e}", n.id, f.id))
                            }
                        }
                    }
                    SplitRule::Conditions(flows)
                }
            };
            activity_splits.insert(
                n.id.clone(),
                ActivitySplitCfg {
                    always: split.always,
                    rule,
                    default: split.default,
                },
            );
        }

        // Event-based gateways lead to catch events (message, signal, timer) or receive tasks.
        for n in nodes
            .iter()
            .filter(|n| n.kind == NodeKind::EventBasedGateway)
        {
            for f in &n.outgoing {
                let target = project.flow(f).and_then(|fl| project.node(&fl.target));
                let ok = target.is_some_and(|t| match &t.kind {
                    NodeKind::Task(TaskKind::Receive) => true,
                    NodeKind::IntermediateCatchEvent => matches!(
                        t.event_defs.first(),
                        Some(EventDef::Message(_) | EventDef::Signal(_) | EventDef::Timer)
                    ),
                    _ => false,
                });
                if !ok {
                    errors.push(format!(
                        "{} ({}): flow '{}' of an event-based gateway must lead to a message, signal or timer catch event or a receive task",
                        n.id,
                        n.label(),
                        f.id
                    ));
                }
            }
        }

        if !errors.is_empty() {
            return Err(if errors.len() == 1 {
                ScenarioError::Invalid(errors.remove(0))
            } else {
                ScenarioError::Many(errors)
            });
        }
        Ok(BoundModel {
            project,
            sim: SimParams {
                seed: sim.seed,
                replications: sim.replications,
                cases,
                warm_up_cases: sim.warm_up_cases,
                time_unit: sim.time_unit,
                horizon,
                clock,
                trace_cases: sim.trace_cases,
            },
            arrivals,
            pools,
            activities,
            gateways,
            inclusive,
            conditions,
            activity_splits,
            decisions,
            buffer_unconsumed: messages.buffer_unconsumed,
            signal_scope: sim.signal_scope,
            black_box_reply,
            events,
            boundaries,
            active: processes,
            warnings,
        })
    }
}

/// Elements that can send a message along an outgoing message flow.
pub fn sends_messages(n: &FlowNode) -> bool {
    match &n.kind {
        NodeKind::Task(TaskKind::Send) => true,
        NodeKind::IntermediateThrowEvent | NodeKind::EndEvent => n
            .event_defs
            .iter()
            .any(|d| matches!(d, EventDef::Message(_))),
        _ => false,
    }
}

/// Elements a message flow may end at.
pub fn receives_messages(n: &FlowNode) -> bool {
    match &n.kind {
        NodeKind::Task(TaskKind::Receive) => true,
        NodeKind::IntermediateCatchEvent | NodeKind::StartEvent | NodeKind::BoundaryEvent => n
            .event_defs
            .iter()
            .any(|d| matches!(d, EventDef::Message(_))),
        _ => false,
    }
}

fn resolve_participant(project: &Project, key: &str) -> Result<QId, String> {
    let all = || project.files.values().flat_map(|f| f.participants.values());
    if let Some((file, id)) = key.split_once('#') {
        let q = QId::new(file, id);
        return if all().any(|p| p.id == q) {
            Ok(q)
        } else {
            Err(format!(
                "participant '{key}' does not exist in the loaded model"
            ))
        };
    }
    let m: Vec<&Participant> = all().filter(|p| p.id.id == key).collect();
    match m.as_slice() {
        [] => Err(format!(
            "participant '{key}' does not exist in the loaded model"
        )),
        [one] => Ok(one.id.clone()),
        many => Err(format!(
            "participant '{key}' is ambiguous, it exists in {}; use a qualified id",
            many.iter()
                .map(|p| p.id.file.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Elements the engine cannot execute yet; `None` if supported.
fn unsupported(n: &FlowNode) -> Option<String> {
    let todo = |what: &str, m: &str| Some(format!("{what} is not supported yet ({m})"));
    let single = |ok: fn(&EventDef) -> bool| n.event_defs.len() == 1 && ok(&n.event_defs[0]);
    match &n.kind {
        NodeKind::Task(_)
        | NodeKind::ExclusiveGateway
        | NodeKind::ParallelGateway
        | NodeKind::SubProcess => None,
        NodeKind::StartEvent
            if n.parent.is_none()
                && (n
                    .event_defs
                    .iter()
                    .all(|d| matches!(d, EventDef::None | EventDef::Timer))
                    || single(|d| matches!(d, EventDef::Message(_) | EventDef::Signal(_)))) =>
        {
            None
        }
        NodeKind::StartEvent if n.parent.is_some() && single(|d| matches!(d, EventDef::None)) => {
            None
        }
        NodeKind::EndEvent
            if single(|d| {
                matches!(
                    d,
                    EventDef::None
                        | EventDef::Message(_)
                        | EventDef::Signal(_)
                        | EventDef::Terminate
                        | EventDef::Error
                        | EventDef::Escalation
                )
            }) =>
        {
            None
        }
        NodeKind::IntermediateThrowEvent
            if single(|d| {
                matches!(
                    d,
                    EventDef::None
                        | EventDef::Message(_)
                        | EventDef::Signal(_)
                        | EventDef::Link(_)
                        | EventDef::Escalation
                )
            }) =>
        {
            None
        }
        NodeKind::IntermediateCatchEvent
            if single(|d| {
                matches!(
                    d,
                    EventDef::Message(_)
                        | EventDef::Signal(_)
                        | EventDef::Timer
                        | EventDef::Link(_)
                )
            }) =>
        {
            None
        }
        NodeKind::BoundaryEvent
            if single(|d| {
                matches!(
                    d,
                    EventDef::Timer
                        | EventDef::Error
                        | EventDef::Escalation
                        | EventDef::Message(_)
                        | EventDef::Signal(_)
                )
            }) =>
        {
            None
        }
        NodeKind::StartEvent
        | NodeKind::EndEvent
        | NodeKind::IntermediateThrowEvent
        | NodeKind::IntermediateCatchEvent
        | NodeKind::BoundaryEvent => todo("event with this event definition", "not planned"),
        NodeKind::CallActivity => None,
        NodeKind::InclusiveGateway | NodeKind::EventBasedGateway => None,
        NodeKind::Unsupported(k) => Some(format!("'{k}' is not supported")),
    }
}

impl Scenario {
    /// A complete starting scenario for a loaded model: every task, XOR split and resource pool
    /// gets an entry with placeholder values, using qualified ids (spec §4).
    /// Arrivals trigger every none/timer start event of `main` (default: first file).
    pub fn skeleton(project: &Project, main: Option<&str>) -> Scenario {
        let main = main
            .map(str::to_string)
            .or_else(|| project.files.keys().next().cloned());
        let triggers: Vec<String> = project
            .nodes()
            .filter(|n| {
                n.kind == NodeKind::StartEvent
                    && Some(&n.id.file) == main.as_ref()
                    && n.event_defs
                        .iter()
                        .all(|d| matches!(d, EventDef::None | EventDef::Timer))
            })
            .map(|n| n.id.to_string())
            .collect();

        let fixed = |secs: f64| DistSpec::Fixed {
            value: crate::dist::Seconds(secs),
        };
        let mut resources = BTreeMap::new();
        let mut activities = BTreeMap::new();
        let mut gateways = BTreeMap::new();
        let mut events = BTreeMap::new();
        let mut boundary = BTreeMap::new();
        let mut skeleton_decisions = BTreeMap::new();
        for n in project.nodes() {
            if let Some(table) = n
                .called_decision
                .as_ref()
                .and_then(|d| {
                    project
                        .decisions
                        .get(d)
                        .and_then(|f| project.dmn.get(f))
                        .and_then(|f| f.decisions.get(d))
                })
                .and_then(|d| d.table.as_ref())
            {
                let id = n.called_decision.clone().unwrap_or_default();
                skeleton_decisions
                    .entry(id)
                    .or_insert_with(|| DecisionSpec {
                        label: None,
                        inputs: table
                            .inputs
                            .iter()
                            .map(|i| {
                                (
                                    i.clone(),
                                    InputSpec::Number(NumberSpec::Uniform {
                                        min: 0.0,
                                        max: 100.0,
                                    }),
                                )
                            })
                            .collect(),
                    });
            }
            let conditioned = n
                .outgoing
                .iter()
                .filter(|f| n.default_flow.as_ref() != Some(*f))
                .all(|f| project.flow(f).is_some_and(|fl| fl.condition.is_some()));
            if n.kind.is_gateway() && n.outgoing.len() > 1 && conditioned {
                continue; // decided by its conditions
            }
            match &n.kind {
                NodeKind::Task(TaskKind::Receive) => {}
                NodeKind::Task(_) => {
                    let pool = n
                        .lane
                        .clone()
                        .or_else(|| project.participant_name(&n.process).map(str::to_string));
                    if let Some(p) = pool {
                        resources.entry(p).or_insert(ResourceSpec {
                            count: 1,
                            cost_per_hour: 0.0,
                            calendar: None,
                        });
                    }
                    activities.insert(
                        n.id.to_string(),
                        ActivitySpec {
                            label: n.name.clone(),
                            duration: fixed(600.0),
                            fixed_cost: 0.0,
                            resource: None,
                        },
                    );
                }
                NodeKind::IntermediateCatchEvent
                    if n.event_defs.iter().any(|d| matches!(d, EventDef::Timer)) =>
                {
                    events.insert(
                        n.id.to_string(),
                        EventSpec {
                            label: n.name.clone(),
                            delay: fixed(3600.0),
                        },
                    );
                }
                NodeKind::BoundaryEvent => {
                    let spec = match n.event_defs.first() {
                        Some(EventDef::Timer) => Some(BoundarySpec {
                            label: n.name.clone(),
                            probability: None,
                            delay: Some(fixed(3600.0)),
                        }),
                        Some(EventDef::Error | EventDef::Escalation) => Some(BoundarySpec {
                            label: n.name.clone(),
                            probability: Some(0.1),
                            delay: Some(fixed(600.0)),
                        }),
                        _ => None,
                    };
                    if let Some(spec) = spec {
                        boundary.insert(n.id.to_string(), spec);
                    }
                }
                NodeKind::InclusiveGateway if n.outgoing.len() > 1 => {
                    gateways.insert(
                        n.id.to_string(),
                        GatewaySpec {
                            label: n.name.clone(),
                            probabilities: n
                                .outgoing
                                .iter()
                                .filter(|f| n.default_flow.as_ref() != Some(*f))
                                .map(|f| (f.id.clone(), 0.5))
                                .collect(),
                        },
                    );
                }
                NodeKind::ExclusiveGateway if n.outgoing.len() > 1 => {
                    let p = 1.0 / n.outgoing.len() as f64;
                    gateways.insert(
                        n.id.to_string(),
                        GatewaySpec {
                            label: n.name.clone(),
                            probabilities: n.outgoing.iter().map(|f| (f.id.clone(), p)).collect(),
                        },
                    );
                }
                _ => {}
            }
        }
        Scenario {
            model: main.map(|m| ModelSection {
                main: Some(m),
                files: None,
            }),
            simulation: SimulationSection {
                cases: Some(100),
                ..SimulationSection::default()
            },
            arrivals: vec![ArrivalSpec {
                label: Some("Arrivals".into()),
                inter_arrival: fixed(3600.0),
                triggers,
            }],
            resources,
            activities,
            gateways,
            calendars: BTreeMap::new(),
            events,
            boundary,
            decisions: skeleton_decisions,
            messages: None,
        }
    }

    pub fn to_toml(&self) -> Result<String, ScenarioError> {
        toml::to_string_pretty(self).map_err(|e| ScenarioError::Toml(e.to_string()))
    }
}
