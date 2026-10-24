//! What each model element needs from a scenario, and filling a scenario from the model.
//!
//! `Scenario::skeleton`, "update from model" and the configuration view of the desktop app all
//! use `needs`, so they agree on which elements take parameters.

use std::collections::BTreeMap;

use crate::dist::{DistSpec, Seconds};
use crate::dmn::{HitPolicy, InputDescription, InputKind, Table};
use crate::feel::Value;
use crate::model::*;
use crate::project::Project;
use crate::scenario::*;

/// A parameter an element takes from the scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    /// Task duration (`[activities]`).
    Duration,
    /// Probabilities of the outgoing flows of an XOR or inclusive split (`[gateways]`).
    Branching { inclusive: bool },
    /// Delay of a timer catch event or a timer start event of an event subprocess (`[events]`).
    Delay,
    /// Boundary event: a delay, and for an error or escalation on a task a probability
    /// (`[boundary]`).
    Boundary { probability: bool },
    /// Number of instances of a multi-instance activity (`[multi_instance]`).
    InstanceCount,
    /// Repeat probability of an activity with a standard loop (`[loops]`).
    Repeat,
    /// Distributions of the decision's inputs that no other decision sets (`[decisions]`).
    DecisionInputs {
        decision: String,
        inputs: Vec<String>,
    },
}

/// Variables that decisions set: table outputs and result variables.
fn decided(project: &Project) -> Vec<String> {
    project
        .dmn
        .values()
        .flat_map(|f| f.decisions.values())
        .filter_map(|d| d.table.as_ref())
        .flat_map(|t| t.outputs.iter().cloned())
        .chain(project.nodes().filter_map(|n| n.result_variable.clone()))
        .collect()
}

/// A variable that a business rule task's decision sets and a gateway can follow.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionOutput {
    /// Names the value is stored under: the preferred one first (the result variable, or
    /// `<resultVariable>.<output>` when the table has several outputs), then the output name.
    pub names: Vec<String>,
    pub decision: String,
    /// The business rule task that evaluates the decision.
    pub task: QId,
    /// The values the decision can give; `None` when an aggregation computes them. COLLECT
    /// without aggregation gives a list of these.
    pub values: Option<Vec<Value>>,
    /// COLLECT without aggregation: the variable holds a list.
    pub list: bool,
}

impl DecisionOutput {
    pub fn variable(&self) -> &str {
        &self.names[0]
    }
}

/// The outputs of the decisions that business rule tasks evaluate (loaded decision tables
/// only), in model order.
pub fn decision_outputs(project: &Project) -> Vec<DecisionOutput> {
    let mut out = Vec::new();
    for n in project.nodes() {
        let Some(id) = &n.called_decision else {
            continue;
        };
        let Some(table) = decision_table(project, id) else {
            continue;
        };
        let single = table.outputs.len() == 1;
        for o in &table.outputs {
            let mut names = Vec::new();
            if let Some(r) = &n.result_variable {
                names.push(if single {
                    r.clone()
                } else {
                    format!("{r}.{o}")
                });
            }
            if !o.is_empty() && !names.contains(o) {
                names.push(o.clone());
            }
            if names.is_empty() {
                continue;
            }
            out.push(DecisionOutput {
                names,
                decision: id.clone(),
                task: n.id.clone(),
                values: table.values(o),
                list: table.hit_policy == HitPolicy::Collect(None),
            });
        }
    }
    out
}

/// Every `DecisionOutput` stored under `variable`.
pub fn outputs_named<'a>(outputs: &'a [DecisionOutput], variable: &str) -> Vec<&'a DecisionOutput> {
    outputs
        .iter()
        .filter(|o| o.names.iter().any(|n| n == variable))
        .collect()
}

/// The decision outputs a split in `n`'s process can follow, those of decisions evaluated
/// closest before `n` first (a breadth-first walk back along incoming flows), then the rest
/// in model order.
pub fn outputs_for(project: &Project, n: &FlowNode) -> Vec<DecisionOutput> {
    let order = upstream(project, n);
    let mut out: Vec<DecisionOutput> = decision_outputs(project)
        .into_iter()
        .filter(|o| {
            project
                .node(&o.task)
                .is_some_and(|t| t.process == n.process)
        })
        .collect();
    out.sort_by_key(|o| {
        order
            .iter()
            .position(|t| *t == o.task)
            .unwrap_or(usize::MAX)
    });
    out
}

/// For a split without conditions: the first of `outputs_for` whose values match the names
/// of its outgoing flows (see `match_flows`), and the value(s) for each non-default flow.
pub fn suggest_output(
    project: &Project,
    n: &FlowNode,
) -> Option<(String, BTreeMap<String, OutputValues>)> {
    outputs_for(project, n)
        .iter()
        .find_map(|o| match_flows(project, n, o).map(|m| (o.variable().to_string(), m)))
}

/// The value(s) of `o` for each non-default outgoing flow of `n`, by the flows' names: a name
/// matches a value when both are equal, or else when one contains the other, ignoring case,
/// spaces and punctuation ("yes"/"no" also match true/false). Every non-default flow must
/// match, and no value may match two flows.
pub fn match_flows(
    project: &Project,
    n: &FlowNode,
    o: &DecisionOutput,
) -> Option<BTreeMap<String, OutputValues>> {
    let words: Vec<(&Value, String)> = o
        .values
        .iter()
        .flatten()
        .filter_map(|v| value_word(v).map(|w| (v, w)))
        .collect();
    let mut taken: Vec<&Value> = Vec::new();
    let mut map = BTreeMap::new();
    for f in n
        .outgoing
        .iter()
        .filter(|f| n.default_flow.as_ref() != Some(*f))
    {
        let name = normalize(
            project
                .flow(f)
                .and_then(|fl| fl.name.as_deref())
                .unwrap_or(""),
        );
        if name.is_empty() {
            return None;
        }
        let mut hits: Vec<&Value> = words
            .iter()
            .filter(|(v, w)| *w == name || bool_word(v, &name))
            .map(|(v, _)| *v)
            .collect();
        if hits.is_empty() {
            hits = words
                .iter()
                .filter(|(_, w)| {
                    !w.is_empty() && (name.contains(w.as_str()) || w.contains(name.as_str()))
                })
                .map(|(v, _)| *v)
                .collect();
        }
        if hits.is_empty() || hits.iter().any(|v| taken.contains(v)) {
            return None;
        }
        taken.extend(&hits);
        let literals = hits
            .iter()
            .filter_map(|v| OutputValue::from_value(v))
            .collect();
        map.insert(f.id.clone(), OutputValues::from_vec(literals));
    }
    Some(map)
}

/// Lower-case letters and digits only.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn value_word(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(normalize(s)),
        Value::Num(x) => Some(x.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null | Value::List(_) => None,
    }
}

fn bool_word(v: &Value, name: &str) -> bool {
    match v {
        Value::Bool(true) => matches!(name, "yes" | "y"),
        Value::Bool(false) => matches!(name, "no" | "n"),
        _ => false,
    }
}

/// Business rule tasks before `n`, nearest first: a breadth-first walk back along incoming
/// sequence flows.
fn upstream(project: &Project, n: &FlowNode) -> Vec<QId> {
    let mut seen = vec![n.id.clone()];
    let mut queue = std::collections::VecDeque::from([n.id.clone()]);
    let mut out = Vec::new();
    while let Some(q) = queue.pop_front() {
        let Some(node) = project.node(&q) else {
            continue;
        };
        for f in &node.incoming {
            let Some(src) = project.flow(f).map(|fl| fl.source.clone()) else {
                continue;
            };
            if seen.contains(&src) {
                continue;
            }
            seen.push(src.clone());
            if project
                .node(&src)
                .is_some_and(|s| s.called_decision.is_some())
            {
                out.push(src.clone());
            }
            queue.push_back(src);
        }
    }
    out
}

/// The loaded decision table `decision`, compiled.
pub fn decision_table(project: &Project, decision: &str) -> Option<Table> {
    project
        .decisions
        .get(decision)
        .and_then(|f| project.dmn.get(f))
        .and_then(|f| f.decisions.get(decision))
        .and_then(|d| d.table.as_ref())
        .and_then(|raw| Table::compile(decision, raw).ok())
}

/// Input `input` of decision `decision`, or of any loaded decision table that reads it (inputs
/// are process variables, so a scenario entry under one decision serves them all).
pub fn describe_input(project: &Project, decision: &str, input: &str) -> Option<InputDescription> {
    decision_table(project, decision)
        .and_then(|t| t.describe_input(input))
        .or_else(|| {
            project
                .decisions
                .keys()
                .filter_map(|d| decision_table(project, d))
                .find_map(|t| t.describe_input(input))
        })
}

/// A placeholder distribution for decision input `input`, from its type and what the rules
/// compare it with: equal shares of its values (plus "other" when a rule tests `not(...)`),
/// true half of the time, or numbers spread around the compared ones (not below 0 unless a
/// compared number is), so that every rule can match.
fn placeholder_input(project: &Project, decision: &str, input: &str) -> InputSpec {
    let Some(d) = describe_input(project, decision, input) else {
        return InputSpec::Number(NumberSpec::Uniform {
            min: 0.0,
            max: 100.0,
        });
    };
    match d.kind {
        InputKind::Text => {
            let mut values = d.values;
            if values.is_empty() {
                values.push("value".into());
            }
            if d.negated && !values.iter().any(|s| s == "other") {
                values.push("other".into());
            }
            let p = 1.0 / values.len() as f64;
            InputSpec::Choice(ChoiceSpec {
                choice: values.into_iter().map(|s| (s, p)).collect(),
            })
        }
        InputKind::Bool => InputSpec::Chance(ChanceSpec { p_true: 0.5 }),
        InputKind::Number | InputKind::Unknown => {
            let (min, max) = match (d.thresholds.first(), d.thresholds.last()) {
                (Some(&lo), Some(&hi)) => {
                    let margin = if hi > lo {
                        (hi - lo) / 2.0
                    } else {
                        lo.abs().max(1.0)
                    };
                    let floor = if lo >= 0.0 { 0.0 } else { f64::MIN };
                    ((lo - margin).max(floor), hi + margin)
                }
                _ => (0.0, 100.0),
            };
            InputSpec::Number(NumberSpec::Uniform { min, max })
        }
    }
}

/// What a decision gives for the scenario's input distributions, estimated from samples.
#[derive(Debug, Clone, PartialEq)]
pub struct DecisionPreview {
    pub samples: usize,
    /// Samples no rule matched (all outputs null).
    pub no_match: usize,
    /// UNIQUE with several matching rules, ANY with differing outputs.
    pub violations: usize,
    /// Output name -> (value as text, samples that gave it), most frequent first. A list
    /// (COLLECT) counts once for each value it holds; an empty list counts as "nothing".
    pub outputs: Vec<(String, Vec<(String, usize)>)>,
    /// Inputs drawn from nothing, taken as null: (input, why).
    pub unknown: Vec<(String, String)>,
}

/// Evaluate decision `decision` `samples` times with inputs drawn from the scenario's
/// `[decisions]` entries (fixed seed, so the preview stays put between frames). Inputs that
/// another decision sets, or whose entry is invalid, are null.
pub fn preview_decision(
    project: &Project,
    scenario: &Scenario,
    decision: &str,
    samples: usize,
) -> Option<DecisionPreview> {
    use rand::SeedableRng;
    let table = decision_table(project, decision)?;
    let mut samplers = BTreeMap::new();
    let mut unknown = Vec::new();
    for input in &table.inputs {
        let spec = scenario
            .decisions
            .values()
            .find_map(|d| d.inputs.get(input));
        match spec.map(InputSpec::sampler) {
            Some(Ok(s)) => {
                samplers.insert(input.clone(), s);
            }
            Some(Err(e)) => unknown.push((input.clone(), e)),
            None if decided(project).contains(input) => {
                unknown.push((input.clone(), "set by another decision".into()))
            }
            None => unknown.push((input.clone(), "no distribution".into())),
        }
    }
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(1);
    let mut counts: Vec<BTreeMap<String, usize>> = vec![BTreeMap::new(); table.outputs.len()];
    let (mut no_match, mut violations) = (0, 0);
    for _ in 0..samples {
        let values: BTreeMap<&String, Value> = samplers
            .iter()
            .map(|(k, s)| (k, s.sample(&mut rng)))
            .collect();
        let e = table.evaluate(|name| {
            values
                .iter()
                .find(|(k, _)| **k == name)
                .map_or(Value::Null, |(_, v)| v.clone())
        });
        no_match += usize::from(e.matched == 0);
        violations += usize::from(e.violation);
        for (i, o) in table.outputs.iter().enumerate() {
            let texts = match &e.outputs[o] {
                Value::List(items) if items.is_empty() => vec!["nothing".to_string()],
                Value::List(items) => items.iter().map(value_text).collect(),
                v => vec![value_text(v)],
            };
            for t in texts {
                *counts[i].entry(t).or_default() += 1;
            }
        }
    }
    let outputs = table
        .outputs
        .iter()
        .zip(counts)
        .map(|(o, c)| {
            let mut list: Vec<(String, usize)> = c.into_iter().collect();
            list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            (o.clone(), list)
        })
        .collect();
    Some(DecisionPreview {
        samples,
        no_match,
        violations,
        outputs,
        unknown,
    })
}

/// A value for people: strings without quotes, null as "nothing".
fn value_text(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Null => "nothing".into(),
        v => v.to_string(),
    }
}

/// The parameters `n` takes from the scenario, in the order they are usually edited.
pub fn needs(project: &Project, n: &FlowNode) -> Vec<Need> {
    let mut out = Vec::new();
    match &n.kind {
        NodeKind::Task(TaskKind::Receive) => {}
        NodeKind::Task(_) => out.push(Need::Duration),
        // A call activity whose process is not loaded runs as a task.
        NodeKind::CallActivity if !calls_loaded_process(project, n) => out.push(Need::Duration),
        NodeKind::ExclusiveGateway | NodeKind::InclusiveGateway if n.outgoing.len() > 1 => {
            // A split whose every (non-default) flow has a condition is decided by them.
            let conditioned = n
                .outgoing
                .iter()
                .filter(|f| n.default_flow.as_ref() != Some(*f))
                .all(|f| project.flow(f).is_some_and(|fl| fl.condition.is_some()));
            if !conditioned {
                out.push(Need::Branching {
                    inclusive: n.kind == NodeKind::InclusiveGateway,
                });
            }
        }
        NodeKind::IntermediateCatchEvent | NodeKind::StartEvent
            if n.event_defs.iter().any(|d| matches!(d, EventDef::Timer))
                && (n.kind == NodeKind::IntermediateCatchEvent
                    || project.is_event_sub_start(n)) =>
        {
            out.push(Need::Delay)
        }
        NodeKind::BoundaryEvent => {
            // Errors and escalations on subprocesses and call activities are thrown inside;
            // only those on tasks need a probability.
            let on_task = n
                .attached_to
                .as_ref()
                .and_then(|a| project.node(a))
                .is_some_and(|h| matches!(h.kind, NodeKind::Task(_)));
            match n.event_defs.first() {
                Some(EventDef::Timer) => out.push(Need::Boundary { probability: false }),
                Some(EventDef::Error(_) | EventDef::Escalation(_)) if on_task => {
                    out.push(Need::Boundary { probability: true })
                }
                _ => {}
            }
        }
        _ => {}
    }
    match &n.looping {
        Some(Looping::MultiInstance {
            cardinality,
            input_collection,
            ..
        }) => {
            let bare = |t: &String| t.trim().trim_start_matches('=').trim().to_string();
            let decided = decided(project);
            let known = cardinality.iter().any(|c| bare(c).parse::<u32>().is_ok())
                || input_collection
                    .iter()
                    .chain(cardinality)
                    .any(|c| decided.contains(&bare(c)));
            if !known {
                out.push(Need::InstanceCount);
            }
        }
        Some(Looping::Standard { .. }) => out.push(Need::Repeat),
        None => {}
    }
    if let Some(id) = &n.called_decision
        && let Some(table) = project
            .decisions
            .get(id)
            .and_then(|f| project.dmn.get(f))
            .and_then(|f| f.decisions.get(id))
            .and_then(|d| d.table.as_ref())
    {
        let decided = decided(project);
        let inputs: Vec<String> = table
            .inputs
            .iter()
            .filter(|i| !decided.contains(i))
            .cloned()
            .collect();
        if !inputs.is_empty() {
            out.push(Need::DecisionInputs {
                decision: id.clone(),
                inputs,
            });
        }
    }
    out
}

/// Whether `n` is a call activity whose called process is loaded.
pub fn calls_loaded_process(project: &Project, n: &FlowNode) -> bool {
    n.called_element
        .as_ref()
        .is_some_and(|c| project.processes.contains_key(c))
}

/// The resource pool a task uses unless the scenario overrides it: its lane, else its pool.
pub fn default_pool(project: &Project, n: &FlowNode) -> Option<String> {
    n.lane
        .clone()
        .or_else(|| project.participant_name(&n.process).map(str::to_string))
}

/// Whether arrivals can start cases at `n`: a none or timer start event at the top level of
/// a process.
pub fn is_arrival_start(n: &FlowNode) -> bool {
    n.kind == NodeKind::StartEvent
        && n.parent.is_none()
        && n.event_defs
            .iter()
            .all(|d| matches!(d, EventDef::None | EventDef::Timer))
}

/// The key under which `map` configures element `q`: its qualified id, or its bare id when
/// that is unique in the project.
pub fn key_of<T>(map: &BTreeMap<String, T>, project: &Project, q: &QId) -> Option<String> {
    let full = q.to_string();
    if map.contains_key(&full) {
        return Some(full);
    }
    (map.contains_key(&q.id) && project.nodes().filter(|n| n.id.id == q.id).count() == 1)
        .then(|| q.id.clone())
}

/// Whether element key `key` refers to an element that exists in the project.
fn resolves(project: &Project, key: &str) -> bool {
    match key.split_once('#') {
        Some((f, i)) => project.node(&QId::new(f, i)).is_some(),
        None => project.nodes().filter(|n| n.id.id == key).count() == 1,
    }
}

fn fixed(secs: f64) -> DistSpec {
    DistSpec::Fixed {
        value: Seconds(secs),
    }
}

/// What `Scenario::update_from_model` changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    /// Elements that got a placeholder entry.
    pub added: Vec<QId>,
    /// Resource pools that were added (with one resource).
    pub pools_added: Vec<String>,
    /// Whether a placeholder arrival generator was added.
    pub arrival_added: bool,
    /// Entries (as `section.key`) whose element no longer exists in the model.
    pub orphaned: Vec<String>,
}

impl Update {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.pools_added.is_empty()
            && !self.arrival_added
            && self.orphaned.is_empty()
    }
}

impl Scenario {
    /// The input distribution for decision input `input`, wherever it is defined (inputs are
    /// process variables: one definition serves every decision that reads them).
    pub fn decision_input_mut(&mut self, input: &str) -> Option<(&String, &mut InputSpec)> {
        self.decisions
            .iter_mut()
            .find_map(|(id, d)| d.inputs.get_mut(input).map(|s| (id, s)))
    }

    fn has_decision_input(&self, input: &str) -> bool {
        self.decisions
            .values()
            .any(|d| d.inputs.contains_key(input))
    }

    /// Indices of the arrival generators that fire start event `q`.
    pub fn arrivals_of(&self, project: &Project, q: &QId) -> Vec<usize> {
        let full = q.to_string();
        let unique = project.nodes().filter(|n| n.id.id == q.id).count() == 1;
        self.arrivals
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                a.triggers
                    .iter()
                    .any(|t| *t == full || (unique && *t == q.id))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// The needs of `n` that the scenario does not cover yet.
    pub fn missing(&self, project: &Project, n: &FlowNode) -> Vec<Need> {
        needs(project, n)
            .into_iter()
            .filter(|need| {
                let covered = match need {
                    Need::Duration => key_of(&self.activities, project, &n.id).is_some(),
                    Need::Branching { .. } => key_of(&self.gateways, project, &n.id).is_some(),
                    Need::Delay => key_of(&self.events, project, &n.id).is_some(),
                    Need::Boundary { probability } => key_of(&self.boundary, project, &n.id)
                        .and_then(|k| self.boundary.get(&k))
                        .is_some_and(|b| {
                            b.delay.is_some() && (!probability || b.probability.is_some())
                        }),
                    Need::InstanceCount => key_of(&self.multi_instance, project, &n.id).is_some(),
                    Need::Repeat => key_of(&self.loops, project, &n.id).is_some(),
                    Need::DecisionInputs { inputs, .. } => {
                        inputs.iter().all(|i| self.has_decision_input(i))
                    }
                };
                !covered
            })
            .collect()
    }

    /// Give every element whose parameters are missing a placeholder entry (with its
    /// qualified id), add the resource pools tasks use and, if there is none, an arrival
    /// generator for the none and timer start events of the main file. Existing entries are
    /// kept; entries of elements that no longer exist are reported, not removed.
    pub fn update_from_model(&mut self, project: &Project) -> Update {
        let mut u = Update::default();
        for n in project.nodes() {
            let (added, pool) = self.fill(project, n);
            if added {
                u.added.push(n.id.clone());
            }
            u.pools_added.extend(pool);
        }
        if self.arrivals.is_empty() {
            let main = self
                .model
                .as_ref()
                .and_then(|m| m.main.clone())
                .or_else(|| project.files.keys().next().cloned());
            let triggers: Vec<String> = project
                .nodes()
                .filter(|n| is_arrival_start(n) && Some(&n.id.file) == main.as_ref())
                .map(|n| n.id.to_string())
                .collect();
            self.arrivals.push(ArrivalSpec {
                label: Some("Arrivals".into()),
                inter_arrival: fixed(3600.0),
                triggers,
            });
            u.arrival_added = true;
        }
        u.orphaned = self.orphans(project);
        u
    }

    /// Give element `n` placeholder entries for the parameters it is missing and add the
    /// resource pool it uses if there is none. Returns whether entries were added and the
    /// pool that was added.
    pub fn fill(&mut self, project: &Project, n: &FlowNode) -> (bool, Option<String>) {
        let missing = self.missing(project, n);
        let added = !missing.is_empty();
        let mut added_pool = None;
        let key = n.id.to_string();
        for need in missing {
            match need {
                Need::Duration => {
                    self.activities.insert(
                        key.clone(),
                        ActivitySpec {
                            label: n.name.clone(),
                            duration: fixed(600.0),
                            fixed_cost: 0.0,
                            resource: None,
                        },
                    );
                }
                Need::Branching { inclusive } => {
                    let spec = match suggest_output(project, n) {
                        // The names of the flows say which decision output picks them.
                        Some((output, values)) => GatewaySpec {
                            label: n.name.clone(),
                            probabilities: BTreeMap::new(),
                            if_completed: None,
                            then: None,
                            output: Some(output),
                            values,
                        },
                        // Inclusive: each non-default flow taken half of the time; XOR: equal
                        // shares.
                        None => {
                            let flows: Vec<&QId> = n
                                .outgoing
                                .iter()
                                .filter(|f| !inclusive || n.default_flow.as_ref() != Some(*f))
                                .collect();
                            let p = if inclusive {
                                0.5
                            } else {
                                1.0 / flows.len() as f64
                            };
                            GatewaySpec {
                                label: n.name.clone(),
                                probabilities: flows.iter().map(|f| (f.id.clone(), p)).collect(),
                                if_completed: None,
                                then: None,
                                output: None,
                                values: BTreeMap::new(),
                            }
                        }
                    };
                    self.gateways.insert(key.clone(), spec);
                }
                Need::Delay => {
                    self.events.insert(
                        key.clone(),
                        EventSpec {
                            label: n.name.clone(),
                            delay: fixed(3600.0),
                        },
                    );
                }
                Need::Boundary { probability } => {
                    let entry = key_of(&self.boundary, project, &n.id).unwrap_or(key.clone());
                    let b = self.boundary.entry(entry).or_insert(BoundarySpec {
                        label: n.name.clone(),
                        probability: None,
                        delay: None,
                    });
                    if probability {
                        b.probability.get_or_insert(0.1);
                        b.delay.get_or_insert(fixed(600.0));
                    } else {
                        b.delay.get_or_insert(fixed(3600.0));
                    }
                }
                Need::InstanceCount => {
                    self.multi_instance.insert(
                        key.clone(),
                        MultiInstanceSpec {
                            label: n.name.clone(),
                            count: CountSpec::Fixed(3),
                        },
                    );
                }
                Need::Repeat => {
                    self.loops.insert(
                        key.clone(),
                        LoopSpec {
                            label: n.name.clone(),
                            repeat_probability: 0.2,
                        },
                    );
                }
                Need::DecisionInputs { decision, inputs } => {
                    for i in inputs {
                        if !self.has_decision_input(&i) {
                            self.decisions
                                .entry(decision.clone())
                                .or_default()
                                .inputs
                                .insert(i.clone(), placeholder_input(project, &decision, &i));
                        }
                    }
                }
            }
        }
        if needs(project, n).contains(&Need::Duration) {
            let spec =
                key_of(&self.activities, project, &n.id).and_then(|k| self.activities.get(&k));
            let pool = spec
                .and_then(|s| s.resource.clone())
                .or_else(|| default_pool(project, n));
            if let Some(p) = pool
                && !self.resources.contains_key(&p)
            {
                self.resources.insert(
                    p.clone(),
                    ResourceSpec {
                        count: 1,
                        cost_per_hour: 0.0,
                        calendar: None,
                    },
                );
                added_pool = Some(p);
            }
        }
        (added, added_pool)
    }

    /// Entries (as `section.key`) whose element does not exist in the project.
    pub fn orphans(&self, project: &Project) -> Vec<String> {
        let mut out = Vec::new();
        let mut check = |section: &str, keys: Vec<&String>| {
            for k in keys {
                if !resolves(project, k) {
                    out.push(format!("{section}.{k}"));
                }
            }
        };
        check("activities", self.activities.keys().collect());
        check("gateways", self.gateways.keys().collect());
        check("events", self.events.keys().collect());
        check("boundary", self.boundary.keys().collect());
        check("multi_instance", self.multi_instance.keys().collect());
        check("loops", self.loops.keys().collect());
        for k in self.decisions.keys() {
            if !project.decisions.contains_key(k) {
                out.push(format!("decisions.{k}"));
            }
        }
        out
    }

    /// Remove the entries `orphans` reports.
    pub fn remove_orphans(&mut self, project: &Project) {
        self.activities.retain(|k, _| resolves(project, k));
        self.gateways.retain(|k, _| resolves(project, k));
        self.events.retain(|k, _| resolves(project, k));
        self.boundary.retain(|k, _| resolves(project, k));
        self.multi_instance.retain(|k, _| resolves(project, k));
        self.loops.retain(|k, _| resolves(project, k));
        self.decisions
            .retain(|k, _| project.decisions.contains_key(k));
    }
}
