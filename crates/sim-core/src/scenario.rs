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
use crate::needs::calls_loaded_process;
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Multi-instance activity -> number of instances.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub multi_instance: BTreeMap<String, MultiInstanceSpec>,
    /// Activity with a standard loop -> how often it repeats.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub loops: BTreeMap<String, LoopSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultiInstanceSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub count: CountSpec,
}

/// A number of instances: `count = 3` or a distribution of plain numbers (rounded; a uniform
/// distribution draws whole numbers from `min` to `max` with equal chances).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CountSpec {
    Fixed(u32),
    Dist(NumberSpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Chance that the activity runs again after an iteration (and, with `testBefore`, that
    /// it runs at all).
    pub repeat_probability: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSpec {
    pub label: Option<String>,
    /// Input expression (variable name) -> distribution of its value.
    #[serde(default)]
    pub inputs: BTreeMap<String, InputSpec>,
}

/// A decision input: a number distribution (`dist = ...`, plain numbers, no time units),
/// a categorical choice (`choice = { gold = 0.2, silver = 0.8 }`) or a boolean (`p_true = 0.3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputSpec {
    Choice(ChoiceSpec),
    Chance(ChanceSpec),
    Number(NumberSpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceSpec {
    pub choice: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChanceSpec {
    pub p_true: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl Sampler {
    /// One value, as the engine draws it.
    pub fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> crate::feel::Value {
        use crate::feel::Value;
        use rand::RngExt;
        match self {
            Sampler::Number(dist) => Value::Num(dist.sample_raw(rng)),
            Sampler::Chance(p) => Value::Bool(rng.random::<f64>() < *p),
            Sampler::Choice(options) => {
                let r: f64 = rng.random();
                let mut acc = 0.0;
                let mut pick = options.last().map(|(k, _)| k.clone()).unwrap_or_default();
                for (k, p) in options {
                    acc += p;
                    if r < acc {
                        pick = k.clone();
                        break;
                    }
                }
                Value::Str(pick)
            }
        }
    }
}

impl InputSpec {
    /// How the input is drawn; an error if the spec is invalid (e.g. shares not adding up).
    pub fn sampler(&self) -> Result<Sampler, String> {
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

/// A split decided by its flows' condition expressions, or by a decision output
/// (`[gateways]` entry with `output`, turned into `output = value` conditions).
#[derive(Debug, Clone)]
pub struct ConditionCfg {
    pub flows: Vec<(QId, crate::feel::Expr)>,
    pub default: Option<QId>,
    /// The decision output the conditions compare, if the split follows one.
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSpec {
    pub label: Option<String>,
    pub delay: DistSpec,
}

/// Timer boundary: `delay` (+ optional `probability`, default 1). Error/escalation boundary:
/// `probability` and `delay` (time after the activity was activated). Message/signal boundary: none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySpec {
    pub label: Option<String>,
    pub probability: Option<f64>,
    pub delay: Option<DistSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrivalSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub inter_arrival: DistSpec,
    pub triggers: Vec<String>,
    /// Working hours (`[calendars]` name): a case that arrives while they are closed starts at
    /// the next opening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceSpec {
    pub count: u32,
    #[serde(default)]
    pub cost_per_hour: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewaySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub probabilities: BTreeMap<String, f64>,
    /// Instead of probabilities (XOR split with two flows): take `then` once this element has
    /// completed in the case, the other flow until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_completed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<String>,
    /// Instead of probabilities: the variable a decision sets (a business rule task's output)
    /// whose value picks the flow, by `values`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Outgoing flow -> the value(s) of `output` that take it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, OutputValues>,
}

impl GatewaySpec {
    /// Whether the split follows a decision output instead of probabilities.
    pub fn by_output(&self) -> bool {
        self.output.is_some() || !self.values.is_empty()
    }
}

/// A literal a decision output is compared with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputValue {
    Bool(bool),
    Num(f64),
    Str(String),
}

impl OutputValue {
    pub fn to_value(&self) -> crate::feel::Value {
        use crate::feel::Value;
        match self {
            OutputValue::Bool(b) => Value::Bool(*b),
            OutputValue::Num(n) => Value::Num(*n),
            OutputValue::Str(s) => Value::Str(s.clone()),
        }
    }

    /// The literal for a decision value; `None` for null and lists.
    pub fn from_value(v: &crate::feel::Value) -> Option<Self> {
        use crate::feel::Value;
        match v {
            Value::Bool(b) => Some(OutputValue::Bool(*b)),
            Value::Num(n) => Some(OutputValue::Num(*n)),
            Value::Str(s) => Some(OutputValue::Str(s.clone())),
            Value::Null | Value::List(_) => None,
        }
    }
}

/// One value or a list of values (`Flow_a = "x"` or `Flow_a = ["x", "y"]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputValues {
    One(OutputValue),
    Many(Vec<OutputValue>),
}

impl OutputValues {
    pub fn as_slice(&self) -> &[OutputValue] {
        match self {
            OutputValues::One(v) => std::slice::from_ref(v),
            OutputValues::Many(vs) => vs,
        }
    }

    /// One value as `One`, several as `Many`.
    pub fn from_vec(mut vs: Vec<OutputValue>) -> Self {
        if vs.len() == 1 {
            OutputValues::One(vs.remove(0))
        } else {
            OutputValues::Many(vs)
        }
    }
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
    /// Cases start at the next opening of these working hours.
    pub calendar: Option<Calendar>,
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
    /// XOR splits decided by whether an element has completed in the case.
    pub history: BTreeMap<QId, HistoryCfg>,
    /// Inclusive (OR) split -> independent flow probabilities and the default flow.
    pub inclusive: BTreeMap<QId, InclusiveCfg>,
    /// Splits whose flows' conditions decide (no `[gateways]` entry, every flow has a condition).
    pub conditions: BTreeMap<QId, ConditionCfg>,
    /// Activities and events with conditional outgoing flows.
    pub activity_splits: BTreeMap<QId, ActivitySplitCfg>,
    /// Multi-instance and looping activities.
    pub loops: BTreeMap<QId, LoopCfg>,
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

/// An XOR split that asks whether `element` has completed in the case: `then` if it has,
/// `otherwise` until then.
#[derive(Debug, Clone)]
pub struct HistoryCfg {
    pub element: QId,
    pub then: QId,
    pub otherwise: QId,
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
pub enum LoopCfg {
    /// Multi-instance: `count` instances, one after another or all at once.
    Multi { sequential: bool, count: Count },
    /// Standard loop: after each iteration (and, with `test_before`, before the first) the
    /// activity runs (again) with probability `repeat`, at most `maximum` times.
    Standard {
        repeat: f64,
        test_before: bool,
        maximum: Option<u32>,
    },
}

/// Where the number of instances of a multi-instance activity comes from.
#[derive(Debug, Clone)]
pub enum Count {
    Sample(CountDist),
    /// An instance variable set by a decision: the length of a list, or a number.
    Variable(String),
}

#[derive(Debug, Clone)]
pub enum CountDist {
    /// Rounded to the nearest whole number, at least 0.
    Rounded(Dist),
    /// Whole numbers from `.0` to `.1`, all equally likely.
    Uniform(u32, u32),
}

impl CountDist {
    pub fn sample<R: rand::Rng + ?Sized>(&self, rng: &mut R) -> u32 {
        use rand::RngExt;
        match *self {
            CountDist::Rounded(ref d) => d.sample(rng).round() as u32,
            CountDist::Uniform(lo, hi) => rng.random_range(lo..=hi),
        }
    }
}

impl CountSpec {
    fn to_dist(&self) -> Result<CountDist, String> {
        match self {
            CountSpec::Fixed(n) => Ok(CountDist::Rounded(Dist::Fixed(f64::from(*n)))),
            CountSpec::Dist(NumberSpec::Uniform { min, max }) => {
                let (lo, hi) = (min.ceil().max(0.0), max.floor());
                if hi < lo {
                    return Err(format!("uniform: no whole number between {min} and {max}"));
                }
                Ok(CountDist::Uniform(lo as u32, hi as u32))
            }
            CountSpec::Dist(n) => n.to_dist().map(CountDist::Rounded),
        }
    }
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

/// A scenario error or warning, with the element (or resource pool) it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub element: Option<QId>,
    pub pool: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Result of `Scenario::check`.
#[derive(Debug, Clone)]
pub struct Checked<'p> {
    pub bound: Option<BoundModel<'p>>,
    pub errors: Vec<Problem>,
    pub warnings: Vec<Problem>,
}

#[derive(Default)]
struct Problems(Vec<Problem>);

impl Problems {
    fn push(&mut self, message: String) {
        self.0.push(Problem {
            element: None,
            pool: None,
            message,
        });
    }
    fn at(&mut self, element: &QId, message: String) {
        self.0.push(Problem {
            element: Some(element.clone()),
            pool: None,
            message,
        });
    }
    fn pool(&mut self, pool: &str, message: String) {
        self.0.push(Problem {
            element: None,
            pool: Some(pool.to_string()),
            message,
        });
    }
}

impl Scenario {
    pub fn bind<'p>(&self, project: &'p Project) -> Result<BoundModel<'p>, ScenarioError> {
        let c = self.check(project);
        match c.bound {
            Some(b) => Ok(b),
            None => {
                let mut m: Vec<String> = c.errors.into_iter().map(|p| p.message).collect();
                Err(if m.len() == 1 {
                    ScenarioError::Invalid(m.remove(0))
                } else {
                    ScenarioError::Many(m)
                })
            }
        }
    }

    /// Like `bind`, but reports every error and warning with the element it is about; the
    /// bound model is there when there are no errors.
    pub fn check<'p>(&self, project: &'p Project) -> Checked<'p> {
        let mut errors = Problems::default();
        let mut warnings = Problems::default();

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
                errors.pool(name, format!("resource '{name}': count must be at least 1"));
            }
            let calendar =
                match &r.calendar {
                    None => None,
                    Some(c) => {
                        let found = calendars.get(c).cloned();
                        if found.is_none() && !self.calendars.contains_key(c) {
                            errors.pool(name, format!(
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
                            errors.at(&q, format!("{label}: trigger '{q}' is not a start event"));
                        } else if n.parent.is_some() {
                            // Earlier skeletons listed these; the subprocess starts them itself.
                            warnings.at(&q, format!(
                                "{label}: trigger '{q}' is a start event inside a subprocess, it is ignored"
                            ));
                        } else if !n
                            .event_defs
                            .iter()
                            .all(|d| matches!(d, EventDef::None | EventDef::Timer))
                        {
                            errors.at(&q, format!(
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
            let calendar = a.calendar.as_ref().and_then(|c| {
                let found = calendars.get(c).cloned();
                if found.is_none() && !self.calendars.contains_key(c) {
                    errors.push(format!(
                        "{label}: calendar '{c}' is not defined under [calendars]"
                    ));
                }
                found
            });
            if let Some(inter_arrival) = inter_arrival {
                arrivals.push(Arrival {
                    label: a.label.clone(),
                    inter_arrival,
                    triggers,
                    calendar,
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
                .filter(|f| !project.is_mirror_flow(f))
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
                    && t.parent.is_none()
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
                .filter(|n| {
                    n.kind == NodeKind::StartEvent
                        && n.parent.is_none()
                        && !processes.contains(&n.process)
                })
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
            if project.is_mirror_flow(f) {
                // Only a picture of what happens inside: fine towards a black box, but an element
                // of a modelled process at the other end never gets (or sends) this message.
                let is_scope =
                    |n: &&FlowNode| matches!(n.kind, NodeKind::CallActivity | NodeKind::SubProcess);
                let (src, tgt) = (project.node(&f.source), project.node(&f.target));
                if [src, tgt]
                    .into_iter()
                    .flatten()
                    .any(|n| processes.contains(&n.process))
                {
                    if let Some(s) = src.filter(|n| !is_scope(n)) {
                        warnings.at(&s.id, format!(
                            "message flow {}: ends at a call activity or subprocess, which only shows what happens inside it; the simulation sends nothing along it, so nothing inside receives the message of '{}'",
                            f.id, s.label()
                        ));
                    } else if let Some(t) = tgt.filter(|n| !is_scope(n)) {
                        warnings.at(&t.id, format!(
                            "message flow {}: starts at a call activity or subprocess, which only shows what happens inside it; the simulation sends nothing along it, so '{}' never gets this message",
                            f.id, t.label()
                        ));
                    }
                }
                continue;
            }
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
                errors.at(
                    &f.source,
                    format!(
                        "message flow {}: source '{}' cannot send a message: {}",
                        f.id,
                        f.source,
                        send_hint(n)
                    ),
                );
            }
            match project.node(&f.target) {
                Some(t) => {
                    if !receives_messages(t) {
                        errors.at(
                            &f.target,
                            format!(
                                "message flow {}: target '{}' cannot receive a message: {}",
                                f.id,
                                f.target,
                                receive_hint(t)
                            ),
                        );
                    } else if !processes.contains(&t.process)
                        && !(t.kind == NodeKind::StartEvent && t.parent.is_none())
                    {
                        warnings.at(&f.target, format!(
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
                        errors.at(&f.source, format!(
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
            if let Some(why) = unsupported(project, n) {
                errors.at(&n.id, format!("{} ({}): {why}", n.id, n.label()));
            }
        }

        // Elements with an [activities] entry (checked below, with the tasks).
        let configured_keys: Vec<QId> = self
            .activities
            .keys()
            .filter_map(|k| resolve(project, k, "activity").ok())
            .collect();

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
                        errors.at(
                            &n.id,
                            format!(
                                "{} ({}): no link catch event named '{name}' in this process",
                                n.id,
                                n.label()
                            ),
                        );
                    }
                }
            }
            if n.kind == NodeKind::CallActivity {
                match n
                    .called_element
                    .as_ref()
                    .and_then(|c| project.processes.get(c))
                {
                    // Runs as a task with its [activities] entry (checked with the tasks).
                    None if configured_keys.contains(&n.id) => {}
                    None => errors.at(
                        &n.id,
                        format!(
                            "{} ({}): called process {} is not defined in any loaded file; load the file that defines it, or give the call activity an [activities] entry with a duration to run it as a task",
                            n.id,
                            n.label(),
                            n.called_element.as_deref().unwrap_or("(none)")
                        ),
                    ),
                    Some(p) => {
                        if project.start_nodes(p, None).is_empty() {
                            errors.at(&n.id, format!(
                                "{} ({}): called process '{}' has no none start event (nor, without any start event, an activity or gateway without incoming flows)",
                                n.id,
                                n.label(),
                                p.id
                            ));
                        }
                    }
                }
            }
            if n.triggered_by_event {
                for why in event_sub_problems(project, n) {
                    errors.at(&n.id, format!("{} ({}): {why}", n.id, n.label()));
                }
            } else if n.kind == NodeKind::SubProcess
                && project.start_nodes(&n.process, Some(&n.id)).is_empty()
            {
                errors.at(&n.id, format!(
                    "{} ({}): embedded subprocess has no none start event (nor, without any start event, an activity or gateway without incoming flows)",
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
                        errors.at(
                            &n.id,
                            format!(
                                "{}: recursive call, process '{}' (indirectly) calls itself",
                                n.id, start.id
                            ),
                        );
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
                    Some(NodeKind::StartEvent)
                        if project.node(&q).is_some_and(|n| {
                            project.is_event_sub_start(n)
                                && matches!(
                                    n.event_defs.first(),
                                    Some(EventDef::Timer | EventDef::Message(_) | EventDef::Signal(_))
                                )
                        }) =>
                    {
                        event_specs.insert(q, spec);
                    }
                    // A timer start event that arrivals fire: it starts this long after the case.
                    Some(NodeKind::StartEvent)
                        if project.node(&q).is_some_and(|n| {
                            n.parent.is_none()
                                && matches!(n.event_defs.as_slice(), [EventDef::Timer])
                        }) =>
                    {
                        if !arrivals.iter().any(|a| a.triggers.contains(&q)) {
                            warnings.at(&q, format!(
                                "[events.{key}]: '{q}' is not a trigger of any arrival, the delay is not used"
                            ));
                        }
                        event_specs.insert(q, spec);
                    }
                    _ => errors.at(&q, format!(
                        "[events.{key}]: '{q}' is not an intermediate catch event, receive task, timer start event of an arrival or timer/message/signal start event of an event subprocess"
                    )),
                },
                Err(e) => errors.push(format!("[events]: {e}")),
            }
        }
        for n in &nodes {
            let is_timer = (n.kind == NodeKind::IntermediateCatchEvent
                || project.is_event_sub_start(n))
                && n.event_defs.iter().any(|d| matches!(d, EventDef::Timer));
            match event_specs.get(&n.id) {
                Some(spec) => match Dist::new(&spec.delay) {
                    Ok(d) => {
                        events.insert(n.id.clone(), d);
                    }
                    Err(e) => errors.at(&n.id, format!("[events] {}: delay: {e}", n.id)),
                },
                None if is_timer => errors.at(
                    &n.id,
                    format!(
                        "{} ({}): timer event needs an [events] entry with a delay",
                        n.id,
                        n.label()
                    ),
                ),
                None => {}
            }
        }

        // A token at a receive task or message catch event whose only senders are black boxes
        // that never answer waits forever. (A black box answers only messages it gets.)
        let sent_flows: Vec<&MessageFlow> = project
            .files
            .values()
            .flat_map(|f| f.message_flows.values())
            .filter(|f| !project.is_mirror_flow(f))
            .collect();
        let silent_black_box = |p: &QId| -> Option<String> {
            let name = project
                .files
                .get(&p.file)?
                .participants
                .get(p)
                .filter(|p| p.process.is_none())?
                .name
                .clone()
                .unwrap_or_else(|| p.id.clone());
            let contacted = sent_flows.iter().any(|g| {
                g.target == *p
                    && project
                        .node(&g.source)
                        .is_some_and(|s| processes.contains(&s.process))
            });
            if !contacted {
                Some(format!(
                    "black-box pool '{name}', which answers only messages it gets, and nothing in the model sends it one"
                ))
            } else if !black_box_reply.contains_key(p) {
                Some(format!(
                    "black-box pool '{name}', which has no reply in [messages.black_box_reply]"
                ))
            } else {
                None
            }
        };
        for n in &nodes {
            let waits = matches!(
                n.kind,
                NodeKind::Task(TaskKind::Receive) | NodeKind::IntermediateCatchEvent
            ) && receives_messages(n);
            // (After an event-based gateway another event can still move the token on.)
            let after_event_gateway = n.incoming.iter().any(|f| {
                project
                    .flow(f)
                    .and_then(|f| project.node(&f.source))
                    .is_some_and(|s| s.kind == NodeKind::EventBasedGateway)
            });
            if !waits || after_event_gateway || events.contains_key(&n.id) {
                continue;
            }
            let senders: Vec<&QId> = sent_flows
                .iter()
                .filter(|f| f.target == n.id)
                .map(|f| &f.source)
                .collect();
            let silent: Option<Vec<String>> = senders.iter().map(|s| silent_black_box(s)).collect();
            if let Some(silent) = silent.filter(|s| !s.is_empty()) {
                warnings.at(&n.id, format!(
                    "{} ({}): waits for a message only from {}, so every token stops here; give it an [events] entry with the delay until the message comes from outside the model, or a message flow to the pool and a reply in [messages.black_box_reply]",
                    n.id,
                    n.label(),
                    silent.join(" and from ")
                ));
            }
        }

        let mut boundary_specs: BTreeMap<QId, &BoundarySpec> = BTreeMap::new();
        for (key, spec) in &self.boundary {
            match resolve(project, key, "boundary event") {
                Ok(q) if project.node(&q).map(|n| &n.kind) == Some(&NodeKind::BoundaryEvent) => {
                    boundary_specs.insert(q, spec);
                }
                Ok(q) => errors.at(
                    &q,
                    format!("[boundary.{key}]: '{q}' is not a boundary event"),
                ),
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
                errors.at(&n.id, format!(
                    "{name}: boundary events are only supported on tasks, call activities and embedded subprocesses"
                ));
            }
            let spec = boundary_specs.get(&n.id);
            let Some(def) = n.event_defs.first() else {
                continue;
            };
            let (needs_prob, needs_delay) = match def {
                EventDef::Timer => (false, true),
                EventDef::Error(_) | EventDef::Escalation(_) => (true, true),
                _ => (false, false),
            };
            // On a subprocess or call activity, errors and escalations thrown inside trigger
            // the event; a [boundary] entry only adds failures the model does not show.
            if matches!(def, EventDef::Error(_) | EventDef::Escalation(_))
                && spec.is_none()
                && host.is_some_and(|h| !matches!(h.kind, NodeKind::Task(_)))
            {
                continue;
            }
            if (needs_prob || needs_delay) && spec.is_none() {
                errors.at(&n.id, format!("{name}: needs a [boundary] entry"));
                continue;
            }
            if needs_prob && spec.is_some_and(|s| s.probability.is_none()) {
                errors.at(
                    &n.id,
                    format!("{name}: [boundary] entry needs a probability"),
                );
            }
            if needs_delay && spec.is_some_and(|s| s.delay.is_none()) {
                errors.at(&n.id, format!("{name}: [boundary] entry needs a delay"));
            }
            if !(needs_prob || needs_delay) && spec.is_some() {
                warnings.at(&n.id, format!("{name}: message/signal boundary events take no [boundary] parameters, the entry is ignored"));
            }
            let probability = spec.and_then(|s| s.probability).unwrap_or(1.0);
            if !(0.0..=1.0).contains(&probability) {
                errors.at(
                    &n.id,
                    format!("{name}: probability must be between 0 and 1"),
                );
            }
            let delay = match spec.and_then(|s| s.delay.as_ref()).filter(|_| needs_delay) {
                Some(d) => match Dist::new(d) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        errors.at(&n.id, format!("{name}: delay: {e}"));
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
                    let Some(n) = project.node(&q) else { continue };
                    if n.kind == NodeKind::CallActivity && calls_loaded_process(project, n) {
                        warnings.at(&q, format!(
                            "[activities.{key}]: '{q}' calls a loaded process, which runs instead; the entry is ignored"
                        ));
                    } else if !matches!(n.kind, NodeKind::Task(_) | NodeKind::CallActivity) {
                        errors.at(&q, format!("[activities.{key}]: '{q}' is not a task"));
                    } else if configured.insert(q.clone(), spec).is_some() {
                        errors.at(&q, format!("[activities.{key}]: '{q}' is configured twice"));
                    }
                }
                Err(e) => errors.push(format!("[activities]: {e}")),
            }
        }

        let mut activities = BTreeMap::new();
        for n in nodes.iter().filter(|n| match n.kind {
            NodeKind::Task(k) => k != TaskKind::Receive,
            // A call activity whose process is not loaded runs as a task (or is an error).
            NodeKind::CallActivity => {
                !calls_loaded_process(project, n) && configured.contains_key(&n.id)
            }
            _ => false,
        }) {
            let Some(spec) = configured.get(&n.id) else {
                errors.at(
                    &n.id,
                    format!(
                        "{} ({}): no [activities] entry with a duration",
                        n.id,
                        n.label()
                    ),
                );
                continue;
            };
            let duration = match Dist::new(&spec.duration) {
                Ok(d) => d,
                Err(e) => {
                    errors.at(&n.id, format!("{}: duration: {e}", n.id));
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
                errors.at(
                    &n.id,
                    format!(
                        "{} ({}): resource pool '{p}' is not defined under [resources]",
                        n.id,
                        n.label()
                    ),
                );
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
                warnings.at(
                    &n.id,
                    format!(
                        "{}: decision '{id}' is not loaded; the task runs without evaluating it",
                        n.id
                    ),
                );
                continue;
            };
            let Some(raw) = &decision.table else {
                errors.at(
                    &n.id,
                    format!("{}: decision '{id}' is not a decision table", n.id),
                );
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
                Err(e) => errors.at(&n.id, format!("{}: {e}", n.id)),
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
                    None => errors.at(&task, format!(
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

        // Multi-instance activities and standard loops.
        let mut mi_specs: BTreeMap<QId, &MultiInstanceSpec> = BTreeMap::new();
        for (key, spec) in &self.multi_instance {
            match resolve(project, key, "activity") {
                Ok(q)
                    if matches!(
                        project.node(&q).and_then(|n| n.looping.as_ref()),
                        Some(Looping::MultiInstance { .. })
                    ) =>
                {
                    mi_specs.insert(q, spec);
                }
                Ok(q) => errors.at(
                    &q,
                    format!("[multi_instance.{key}]: '{q}' is not a multi-instance activity"),
                ),
                Err(e) => errors.push(format!("[multi_instance]: {e}")),
            }
        }
        let mut loop_specs: BTreeMap<QId, &LoopSpec> = BTreeMap::new();
        for (key, spec) in &self.loops {
            match resolve(project, key, "activity") {
                Ok(q)
                    if matches!(
                        project.node(&q).and_then(|n| n.looping.as_ref()),
                        Some(Looping::Standard { .. })
                    ) =>
                {
                    loop_specs.insert(q, spec);
                }
                Ok(q) => errors.at(
                    &q,
                    format!("[loops.{key}]: '{q}' is not an activity with a standard loop"),
                ),
                Err(e) => errors.push(format!("[loops]: {e}")),
            }
        }
        // A FEEL expression that is just a variable name (`= items`, `items`).
        let variable = |text: &str| {
            let t = text.trim().trim_start_matches('=').trim();
            (!t.is_empty()
                && t.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.'))
            .then(|| t.to_string())
        };
        let mut loops = BTreeMap::new();
        for n in &nodes {
            let name = format!("{} ({})", n.id, n.label());
            match &n.looping {
                None => {}
                Some(Looping::MultiInstance {
                    sequential,
                    cardinality,
                    input_collection,
                    completion_condition,
                }) => {
                    if completion_condition.is_some() {
                        warnings.at(
                            &n.id,
                            format!(
                                "{name}: the completion condition is ignored, all instances run"
                            ),
                        );
                    }
                    // The scenario first, then a number in the model, then a variable that a
                    // decision sets (the collection, or the cardinality expression).
                    let literal = cardinality
                        .as_deref()
                        .and_then(|c| c.trim().trim_start_matches('=').trim().parse::<u32>().ok());
                    let from_decision = input_collection
                        .iter()
                        .chain(cardinality)
                        .filter_map(|c| variable(c))
                        .find(|v| produced.contains(v));
                    let count = match (mi_specs.get(&n.id), literal, from_decision) {
                        (Some(spec), _, _) => match spec.count.to_dist() {
                            Ok(d) => Count::Sample(d),
                            Err(e) => {
                                errors.at(&n.id, format!("[multi_instance] {}: count: {e}", n.id));
                                continue;
                            }
                        },
                        (None, Some(k), _) => {
                            Count::Sample(CountDist::Rounded(Dist::Fixed(f64::from(k))))
                        }
                        (None, None, Some(v)) => Count::Variable(v),
                        (None, None, None) => {
                            errors.at(&n.id, format!(
                                "{name}: multi-instance activity needs a [multi_instance] entry with a count"
                            ));
                            continue;
                        }
                    };
                    loops.insert(
                        n.id.clone(),
                        LoopCfg::Multi {
                            sequential: *sequential,
                            count,
                        },
                    );
                }
                Some(Looping::Standard {
                    test_before,
                    maximum,
                    ..
                }) => {
                    let Some(spec) = loop_specs.get(&n.id) else {
                        errors.at(&n.id, format!(
                            "{name}: loop activity needs a [loops] entry with a repeat_probability"
                        ));
                        continue;
                    };
                    let p = spec.repeat_probability;
                    if !(0.0..=1.0).contains(&p) {
                        errors.at(
                            &n.id,
                            format!("{name}: repeat_probability must be between 0 and 1"),
                        );
                    } else if p >= 1.0 && maximum.is_none() {
                        errors.at(
                            &n.id,
                            format!(
                                "{name}: repeat_probability 1 without a loopMaximum never ends"
                            ),
                        );
                    }
                    loops.insert(
                        n.id.clone(),
                        LoopCfg::Standard {
                            repeat: p,
                            test_before: *test_before,
                            maximum: *maximum,
                        },
                    );
                }
            }
        }

        // Split probabilities: XOR splits need one per flow summing to 1; inclusive (OR) splits
        // take each flow independently with its probability, except the default flow.
        let mut gateways = BTreeMap::new();
        let mut history = BTreeMap::new();
        let mut inclusive = BTreeMap::new();
        let mut conditions = BTreeMap::new();
        let mut gw_specs: BTreeMap<QId, &GatewaySpec> = BTreeMap::new();
        let decision_outputs = crate::needs::decision_outputs(project);
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
                    _ => errors.at(&q, format!(
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
                            errors.at(&n.id, format!("{} condition of flow {}: {e}", n.id, f.id))
                        }
                        None => {}
                    }
                }
                conditions.insert(
                    n.id.clone(),
                    ConditionCfg {
                        flows,
                        default: n.default_flow.clone(),
                        output: None,
                    },
                );
                continue;
            }
            if gw_specs.contains_key(&n.id) && all_conditions {
                warnings.at(
                    &n.id,
                    format!(
                        "{}: the [gateways] entry is used instead of the flow conditions",
                        n.id
                    ),
                );
            }
            let Some(spec) = gw_specs.get(&n.id) else {
                errors.at(&n.id, format!(
                    "{} ({}): {} split needs a [gateways] entry with probabilities (or a condition on every outgoing flow)",
                    n.id,
                    n.label(),
                    if is_or { "inclusive" } else { "XOR" }
                ));
                continue;
            };
            if spec.by_output() {
                match output_cfg(n, spec, &decision_outputs) {
                    Ok((cfg, notes)) => {
                        for w in notes {
                            warnings.at(&n.id, format!("{} ({}): {w}", n.id, n.label()));
                        }
                        conditions.insert(n.id.clone(), cfg);
                    }
                    Err(e) => errors.at(&n.id, format!("{} ({}): {e}", n.id, n.label())),
                }
                continue;
            }
            if spec.if_completed.is_some() || spec.then.is_some() {
                match history_cfg(project, n, spec) {
                    Ok(cfg) => {
                        history.insert(n.id.clone(), cfg);
                    }
                    Err(e) => errors.at(&n.id, format!("{} ({}): {e}", n.id, n.label())),
                }
                continue;
            }
            let default = n.default_flow.clone().filter(|_| is_or);
            let mut by_flow: BTreeMap<QId, f64> = BTreeMap::new();
            for (k, p) in &spec.probabilities {
                let q = match k.split_once('#') {
                    Some((f, i)) => QId::new(f, i),
                    None => QId::new(&n.id.file, k),
                };
                if !n.outgoing.contains(&q) {
                    errors.at(
                        &n.id,
                        format!("{}: '{k}' is not an outgoing flow of this gateway", n.id),
                    );
                } else if default.as_ref() == Some(&q) {
                    errors.at(&n.id, format!(
                        "{}: '{k}' is the default flow of an inclusive gateway and takes no probability (it is used when no other flow is chosen)",
                        n.id
                    ));
                } else if !(0.0..=1.0).contains(p) {
                    errors.at(
                        &n.id,
                        format!("{}: probability of '{k}' must be between 0 and 1", n.id),
                    );
                } else {
                    by_flow.insert(q, *p);
                }
            }
            let mut list = Vec::new();
            for f in n.outgoing.iter().filter(|f| default.as_ref() != Some(*f)) {
                match by_flow.get(f) {
                    Some(p) => list.push((f.clone(), *p)),
                    None => errors.at(
                        &n.id,
                        format!("{}: no probability for outgoing flow '{}'", n.id, f.id),
                    ),
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
                    errors.at(
                        &n.id,
                        format!("{}: probabilities sum to {sum}, expected 1", n.id),
                    );
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
                Some(spec) if spec.by_output() => {
                    errors.at(
                        &n.id,
                        format!(
                            "{} ({}): output only works for an XOR or inclusive gateway; this element's flows have conditions",
                            n.id,
                            n.label()
                        ),
                    );
                    continue;
                }
                Some(spec) if spec.if_completed.is_some() || spec.then.is_some() => {
                    errors.at(
                        &n.id,
                        format!(
                            "{} ({}): if_completed only works for an XOR split with two outgoing flows",
                            n.id,
                            n.label()
                        ),
                    );
                    continue;
                }
                Some(spec) => {
                    let mut by_flow: BTreeMap<QId, f64> = BTreeMap::new();
                    for (k, p) in &spec.probabilities {
                        let q = match k.split_once('#') {
                            Some((f, i)) => QId::new(f, i),
                            None => QId::new(&n.id.file, k),
                        };
                        if !split.conditional.iter().any(|(f, _)| *f == q) {
                            errors.at(
                                &n.id,
                                format!(
                                    "{}: '{k}' is not a conditional outgoing flow of this element",
                                    n.id
                                ),
                            );
                        } else if !(0.0..=1.0).contains(p) {
                            errors.at(
                                &n.id,
                                format!("{}: probability of '{k}' must be between 0 and 1", n.id),
                            );
                        } else {
                            by_flow.insert(q, *p);
                        }
                    }
                    let mut list = Vec::new();
                    for (f, _) in &split.conditional {
                        match by_flow.get(f) {
                            Some(p) => list.push((f.clone(), *p)),
                            None => errors.at(
                                &n.id,
                                format!("{}: no probability for conditional flow '{}'", n.id, f.id),
                            ),
                        }
                    }
                    SplitRule::Probabilities(list)
                }
                None => {
                    let mut flows = Vec::new();
                    for (f, c) in &split.conditional {
                        match crate::feel::Expr::parse(c) {
                            Ok(e) => flows.push((f.clone(), e)),
                            Err(e) => errors
                                .at(&n.id, format!("{} condition of flow {}: {e}", n.id, f.id)),
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
                    errors.at(&n.id, format!(
                        "{} ({}): flow '{}' of an event-based gateway must lead to a message, signal or timer catch event or a receive task",
                        n.id,
                        n.label(),
                        f.id
                    ));
                }
            }
        }

        let warning_texts = warnings.0.iter().map(|w| w.message.clone()).collect();
        let bound = errors.0.is_empty().then_some(BoundModel {
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
            history,
            inclusive,
            conditions,
            activity_splits,
            loops,
            decisions,
            buffer_unconsumed: messages.buffer_unconsumed,
            signal_scope: sim.signal_scope,
            black_box_reply,
            events,
            boundaries,
            active: processes,
            warnings: warning_texts,
        });
        Checked {
            bound,
            errors: errors.0,
            warnings: warnings.0,
        }
    }
}

/// Why `n` cannot be the source of a message flow, and what to model instead.
fn send_hint(n: &FlowNode) -> String {
    let receiver = match n.kind {
        NodeKind::Task(TaskKind::Receive) => Some("a receive task"),
        _ if receives_messages(n) => Some("a message event that receives"),
        _ => None,
    };
    match receiver {
        Some(what) => format!(
            "it is {what}, which only receives; send from a send task or a message throw event next to it"
        ),
        None => "only send tasks and message throw/end events send; make it a send task, or add a send task or message throw event next to it".into(),
    }
}

/// Why `n` cannot be the target of a message flow, and what to model instead.
fn receive_hint(n: &FlowNode) -> String {
    let sender = match n.kind {
        NodeKind::Task(TaskKind::Send) => Some("a send task"),
        _ if sends_messages(n) => Some("a message event that sends"),
        _ => None,
    };
    match sender {
        Some(what) => format!(
            "it is {what}, which only sends; to wait for the answer, add a receive task or message catch event after it and let the message flow end there"
        ),
        None => "only receive tasks and message start, catch and boundary events receive; make it a receive task, or add a message catch event before it (or a message boundary event on it)".into(),
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

/// An XOR split decided by whether `if_completed` has completed in the case (`[gateways]`
/// entry with `if_completed` and `then`).
fn history_cfg(project: &Project, n: &FlowNode, spec: &GatewaySpec) -> Result<HistoryCfg, String> {
    if n.kind != NodeKind::ExclusiveGateway || n.outgoing.len() != 2 {
        return Err("if_completed only works for an XOR split with two outgoing flows".into());
    }
    if !spec.probabilities.is_empty() {
        return Err("give either probabilities or if_completed, not both".into());
    }
    let (Some(element), Some(then)) = (&spec.if_completed, &spec.then) else {
        return Err("if_completed and then go together: the element to ask about and the flow to take once it has completed".into());
    };
    let element = resolve(project, element, "if_completed: element")?;
    let then = match then.split_once('#') {
        Some((f, i)) => QId::new(f, i),
        None => QId::new(&n.id.file, then),
    };
    let Some(otherwise) = n.outgoing.iter().find(|f| **f != then).cloned() else {
        return Err("the two outgoing flows are the same".into());
    };
    if !n.outgoing.contains(&then) {
        return Err(format!(
            "then: '{}' is not an outgoing flow of this gateway",
            then.id
        ));
    }
    Ok(HistoryCfg {
        element,
        then,
        otherwise,
    })
}

/// A split that follows a decision output (`[gateways]` entry with `output` and `values`):
/// each flow becomes the condition `output = value` (or any of its values); with a list
/// (COLLECT) a flow is taken when the list holds one of its values. Also returns warnings
/// about values the decisions cannot give or flows no value leads to.
fn output_cfg(
    n: &FlowNode,
    spec: &GatewaySpec,
    outputs: &[crate::needs::DecisionOutput],
) -> Result<(ConditionCfg, Vec<String>), String> {
    use crate::feel::{Expr, Op, Value};
    if !spec.probabilities.is_empty() || spec.if_completed.is_some() || spec.then.is_some() {
        return Err("give either probabilities, if_completed or output, not several".into());
    }
    let Some(var) = &spec.output else {
        return Err(
            "values need output: the decision output (variable) they are compared with".into(),
        );
    };
    let setters = crate::needs::outputs_named(outputs, var);
    if setters.is_empty() {
        let mut known: Vec<&str> = outputs.iter().map(|o| o.variable()).collect();
        known.dedup();
        // A result variable of a table with several outputs holds them all.
        let parts: Vec<&str> = known
            .iter()
            .copied()
            .filter(|k| {
                k.strip_prefix(var.as_str())
                    .is_some_and(|r| r.starts_with('.'))
            })
            .collect();
        return Err(if !parts.is_empty() {
            format!(
                "output '{var}' holds several decision outputs; name one: {}",
                parts.join(", ")
            )
        } else if known.is_empty() {
            format!("output '{var}': no business rule task evaluates a loaded decision table")
        } else {
            format!(
                "output '{var}' is not set by any decision; decision outputs are: {}",
                known.join(", ")
            )
        });
    }
    let default = n.default_flow.clone();
    let mut by_flow: BTreeMap<QId, Vec<Value>> = BTreeMap::new();
    for (k, vs) in &spec.values {
        let q = match k.split_once('#') {
            Some((f, i)) => QId::new(f, i),
            None => QId::new(&n.id.file, k),
        };
        if !n.outgoing.contains(&q) {
            return Err(format!("'{k}' is not an outgoing flow of this gateway"));
        }
        if default.as_ref() == Some(&q) {
            return Err(format!(
                "'{k}' is the default flow and takes no value (it is taken when no other flow's value matches)"
            ));
        }
        if vs.as_slice().is_empty() {
            return Err(format!("no value for flow '{k}'"));
        }
        by_flow.insert(q, vs.as_slice().iter().map(OutputValue::to_value).collect());
    }
    let mut flows = Vec::new();
    for f in n.outgoing.iter().filter(|f| default.as_ref() != Some(*f)) {
        let Some(vs) = by_flow.get(f) else {
            return Err(format!("no value for outgoing flow '{}'", f.id));
        };
        let mut eqs: Vec<Expr> = vs
            .iter()
            .map(|v| Expr::Cmp(var.clone(), Op::Eq, v.clone()))
            .collect();
        let e = if eqs.len() == 1 {
            eqs.remove(0)
        } else {
            Expr::Or(eqs)
        };
        flows.push((f.clone(), e));
    }
    // What the decisions can give against what the flows take.
    let mut notes = Vec::new();
    let mapped: Vec<&Value> = by_flow.values().flatten().collect();
    for o in &setters {
        let Some(possible) = &o.values else {
            continue;
        };
        for (f, vs) in &by_flow {
            for v in vs.iter().filter(|v| !possible.contains(v)) {
                notes.push(format!(
                    "flow '{}': decision '{}' never gives {v} for '{var}'",
                    f.id, o.decision
                ));
            }
        }
        if default.is_none() && !o.list {
            for v in possible.iter().filter(|v| !mapped.contains(v)) {
                notes.push(format!(
                    "decision '{}' can give {v} for '{var}', which no flow takes; the token then stays at the gateway",
                    o.decision
                ));
            }
        }
    }
    Ok((
        ConditionCfg {
            flows,
            default,
            output: Some(var.clone()),
        },
        notes,
    ))
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
fn unsupported(project: &Project, n: &FlowNode) -> Option<String> {
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
        NodeKind::StartEvent if project.is_event_sub_start(n) => {
            if single(|d| {
                matches!(
                    d,
                    EventDef::Message(_)
                        | EventDef::Signal(_)
                        | EventDef::Timer
                        | EventDef::Error(_)
                        | EventDef::Escalation(_)
                )
            }) {
                None
            } else {
                todo(
                    "event subprocess start event with this event definition",
                    "not planned",
                )
            }
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
                        | EventDef::Error(_)
                        | EventDef::Escalation(_)
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
                        | EventDef::Escalation(_)
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
                        | EventDef::Error(_)
                        | EventDef::Escalation(_)
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

/// What is wrong with the structure of event subprocess `n` (BPMN §10.3.5, §13.5.4).
fn event_sub_problems(project: &Project, n: &FlowNode) -> Vec<String> {
    let mut problems = Vec::new();
    let starts: Vec<&FlowNode> = project
        .nodes()
        .filter(|c| c.parent.as_ref() == Some(&n.id) && c.kind == NodeKind::StartEvent)
        .collect();
    match starts.as_slice() {
        [s] => {
            if s.event_defs.iter().all(|d| matches!(d, EventDef::None)) {
                problems.push("the start event of an event subprocess needs a trigger".into());
            }
            if !s.cancel_activity && matches!(s.event_defs.first(), Some(EventDef::Error(_))) {
                problems.push("an error event subprocess is always interrupting".into());
            }
        }
        _ => problems.push("an event subprocess needs exactly one start event".into()),
    }
    if !n.incoming.is_empty() || !n.outgoing.is_empty() {
        problems.push("an event subprocess must not have sequence flows".into());
    }
    if project
        .nodes()
        .any(|b| b.attached_to.as_ref() == Some(&n.id))
    {
        problems.push("an event subprocess cannot have boundary events".into());
    }
    problems
}

impl Scenario {
    /// A complete starting scenario for a loaded model: every task, XOR split and resource pool
    /// gets an entry with placeholder values, using qualified ids (spec §4).
    /// Arrivals trigger every none/timer start event of `main` (default: first file).
    pub fn skeleton(project: &Project, main: Option<&str>) -> Scenario {
        let main = main
            .map(str::to_string)
            .or_else(|| project.files.keys().next().cloned());
        let mut s = Scenario {
            model: main.map(|m| ModelSection {
                main: Some(m),
                files: None,
            }),
            simulation: SimulationSection {
                cases: Some(100),
                ..SimulationSection::default()
            },
            ..Scenario::default()
        };
        s.update_from_model(project);
        s
    }

    pub fn to_toml(&self) -> Result<String, ScenarioError> {
        toml::to_string_pretty(self).map_err(|e| ScenarioError::Toml(e.to_string()))
    }
}
