//! Scenario file (TOML) and its binding to a loaded project (spec §4).
//!
//! `Scenario` is the file as written; `Scenario::bind` resolves element ids, checks the
//! parameters against the model and produces a `BoundModel` that the engine runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dist::{Dist, DistSpec};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendars: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<toml::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<toml::Value>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub horizon: Option<String>,
    #[serde(default)]
    pub warm_up_cases: u64,
    #[serde(default = "default_time_unit")]
    pub time_unit: TimeUnit,
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
            warm_up_cases: 0,
            time_unit: default_time_unit(),
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
    pub warnings: Vec<String>,
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

        for (name, present) in [
            ("[calendars]", self.calendars.is_some()),
            ("[events]", self.events.is_some()),
            ("[boundary]", self.boundary.is_some()),
            ("[messages]", self.messages.is_some()),
            ("simulation.horizon", self.simulation.horizon.is_some()),
        ] {
            if present {
                warnings.push(format!("{name} is not supported yet and is ignored"));
            }
        }
        if self.model.as_ref().is_some_and(|m| m.files.is_some()) {
            warnings.push("model.files is ignored; the loaded files are used".into());
        }

        let sim = &self.simulation;
        let cases = match sim.cases {
            Some(c) if c > 0 => c,
            _ => {
                errors.push("simulation.cases must be set to a positive number".into());
                0
            }
        };
        if sim.replications == 0 {
            errors.push("simulation.replications must be at least 1".into());
        }
        if sim.warm_up_cases >= cases && cases > 0 {
            errors.push("simulation.warm_up_cases must be smaller than simulation.cases".into());
        }

        let mut pools = BTreeMap::new();
        for (name, r) in &self.resources {
            if r.count == 0 {
                errors.push(format!("resource '{name}': count must be at least 1"));
            }
            if r.calendar.is_some() {
                warnings.push(format!(
                    "resource '{name}': calendars are not supported yet"
                ));
            }
            pools.insert(
                name.clone(),
                PoolDef {
                    count: r.count,
                    cost_per_hour: r.cost_per_hour,
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
        for n in nodes.iter().filter(|n| matches!(n.kind, NodeKind::Task(_))) {
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

        // XOR split probabilities.
        let mut gateways = BTreeMap::new();
        let mut gw_specs: BTreeMap<QId, &GatewaySpec> = BTreeMap::new();
        for (key, spec) in &self.gateways {
            match resolve(project, key, "gateway") {
                Ok(q) => {
                    if project.node(&q).map(|n| &n.kind) != Some(&NodeKind::ExclusiveGateway) {
                        errors.push(format!(
                            "[gateways.{key}]: '{q}' is not an exclusive gateway"
                        ));
                    } else {
                        gw_specs.insert(q, spec);
                    }
                }
                Err(e) => errors.push(format!("[gateways]: {e}")),
            }
        }
        for n in nodes
            .iter()
            .filter(|n| n.kind == NodeKind::ExclusiveGateway && n.outgoing.len() > 1)
        {
            let Some(spec) = gw_specs.get(&n.id) else {
                errors.push(format!(
                    "{} ({}): XOR split needs a [gateways] entry with probabilities",
                    n.id,
                    n.label()
                ));
                continue;
            };
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
            for f in &n.outgoing {
                match by_flow.get(f) {
                    Some(p) => list.push((f.clone(), *p)),
                    None => errors.push(format!(
                        "{}: no probability for outgoing flow '{}'",
                        n.id, f.id
                    )),
                }
            }
            let sum: f64 = list.iter().map(|(_, p)| p).sum();
            if list.len() == n.outgoing.len() && (sum - 1.0).abs() > 1e-9 {
                errors.push(format!("{}: probabilities sum to {sum}, expected 1", n.id));
            }
            gateways.insert(n.id.clone(), list);
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
            },
            arrivals,
            pools,
            activities,
            gateways,
            warnings,
        })
    }
}

/// Elements the M1 engine cannot execute yet; `None` if supported.
fn unsupported(n: &FlowNode) -> Option<String> {
    let plain = n.event_defs.iter().all(|d| matches!(d, EventDef::None));
    let todo = |what: &str, m: &str| Some(format!("{what} is not supported yet ({m})"));
    match &n.kind {
        NodeKind::Task(TaskKind::Send | TaskKind::Receive) => todo("send/receive task", "M3"),
        NodeKind::Task(_) | NodeKind::ExclusiveGateway | NodeKind::ParallelGateway => None,
        NodeKind::StartEvent
            if n.event_defs
                .iter()
                .all(|d| matches!(d, EventDef::None | EventDef::Timer)) =>
        {
            None
        }
        NodeKind::EndEvent | NodeKind::IntermediateThrowEvent | NodeKind::StartEvent if plain => {
            None
        }
        NodeKind::StartEvent | NodeKind::EndEvent | NodeKind::IntermediateThrowEvent => {
            todo("event with a non-none event definition", "M3/M4")
        }
        NodeKind::IntermediateCatchEvent => todo("intermediate catch event", "M3/M4"),
        NodeKind::BoundaryEvent => todo("boundary event", "M4"),
        NodeKind::SubProcess => todo("embedded subprocess", "M4"),
        NodeKind::CallActivity => todo("call activity", "M4"),
        NodeKind::InclusiveGateway | NodeKind::EventBasedGateway => {
            todo("inclusive/event-based gateway", "M6")
        }
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
        for n in project.nodes() {
            match &n.kind {
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
            calendars: None,
            events: None,
            boundary: None,
            messages: None,
        }
    }

    pub fn to_toml(&self) -> Result<String, ScenarioError> {
        toml::to_string_pretty(self).map_err(|e| ScenarioError::Toml(e.to_string()))
    }
}
