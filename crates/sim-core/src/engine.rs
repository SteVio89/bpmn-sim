//! Discrete-event engine (spec §5). One `Engine` is one replication.
//!
//! Determinism: events are ordered by `(time, sequence number)`, all maps are `BTreeMap`s and
//! all randomness comes from one seeded ChaCha generator.
//!
//! Tokens wait through *subscriptions*: a catch event, receive task or boundary event subscribes
//! to a message, a signal or a timer. Cancelling an activity deactivates its subscriptions.
//! The start events of event subprocesses subscribe while their enclosing scope runs; their
//! subscriptions belong to the token at the enclosing subprocess, or to no token
//! (`NO_TOKEN`) at the top level of an instance.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::feel::Value;
use crate::model::*;
use crate::scenario::Sampler;
use crate::scenario::{BoundModel, SignalScope, SplitRule};
use crate::stats::*;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Time(f64);
impl Eq for Time {}
impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Time {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Ev {
    Arrival(usize),
    TaskDone(usize),
    /// A black box replies: deliver message flow `flow` (index into `Engine::msg_flows`) in `group`.
    Reply {
        group: usize,
        flow: usize,
    },
    /// A timer subscription fires.
    Timer(usize),
}

struct Case {
    arrival: f64,
    open_instances: usize,
    cost: f64,
    completion: Option<f64>,
    /// Elements that history gateways ask about and that have completed in this case.
    completed: BTreeSet<QId>,
}

struct Token {
    node: QId,
    state: String,
    /// Embedded subprocess scope this token lives in.
    scope: Option<usize>,
    /// Scope started by this token while it sits at an embedded subprocess.
    child_scope: Option<usize>,
    run: Option<usize>,
    /// Instance started by this token while it sits at a call activity.
    child_instance: Option<usize>,
    subs: Vec<usize>,
    /// One iteration of a multi-instance or looping activity: it runs the activity once and
    /// then ends in its scope instead of leaving along the outgoing flows.
    iteration: bool,
}

/// An embedded subprocess activation, or the iterations of a multi-instance or looping
/// activity (`repeat`); `parent` is the token waiting at the activity.
struct Scope {
    node: QId,
    parent: u64,
    repeat: Option<Repeat>,
}

/// Iterations of a multi-instance (`total`) or standard loop (`chance`, `maximum`) activity.
struct Repeat {
    sequential: bool,
    total: Option<u32>,
    chance: f64,
    maximum: Option<u32>,
    started: u32,
}

enum SubKind {
    Message,
    Signal(String),
    Timer,
}

/// A token waiting for something: a catch event / receive task (`boundary == false`), or a
/// boundary event watching the activity the token sits at.
struct Sub {
    inst: usize,
    owner: u64,
    node: QId,
    boundary: bool,
    kind: SubKind,
    active: bool,
}

struct Instance {
    case: usize,
    /// Messages are matched within a group: the top-level instances of a case form one, every
    /// call activity starts a new one for the called process and what it starts by messages.
    group: usize,
    /// Call activity token (instance, token) waiting for this instance to complete.
    parent: Option<(usize, u64)>,
    started: f64,
    finished: Option<f64>,
    /// Process variables (decision inputs and outputs).
    variables: BTreeMap<String, Value>,
    process: QId,
    tokens: BTreeMap<u64, Token>,
    /// AND join node -> incoming flow -> waiting tokens (FIFO).
    joins: BTreeMap<QId, BTreeMap<QId, VecDeque<u64>>>,
    /// Subscriptions of the event subprocesses at the top level.
    event_subs: Vec<usize>,
    done: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunState {
    Queued,
    Running,
    Done,
    Cancelled,
}

struct TaskRun {
    instance: usize,
    token: u64,
    node: QId,
    queued: f64,
    start: f64,
    duration: f64,
    state: RunState,
}

#[derive(Default)]
struct Pool {
    busy: u32,
    busy_time: f64,
    tasks: u64,
    queue: VecDeque<usize>,
}

/// Work item: a token arrives at `node`, via `via` (None for start events).
type Work = (u64, QId, Option<QId>);

/// Owner of the event subprocess subscriptions at the top level of an instance.
const NO_TOKEN: u64 = u64::MAX;

/// Who catches a thrown error or escalation.
enum Catcher {
    /// A boundary event of the activity token `owner` sits at.
    Boundary { inst: usize, owner: u64, event: QId },
    /// An event subprocess (its start event `start`) of `scope` (`None`: the top level).
    EventSub {
        inst: usize,
        scope: Option<usize>,
        owner: u64,
        start: QId,
    },
}

pub struct Engine<'a> {
    m: &'a BoundModel<'a>,
    seed: u64,
    rng: ChaCha8Rng,
    now: f64,
    seq: u64,
    queue: BinaryHeap<Reverse<(Time, u64, Ev)>>,
    started: u64,
    cases: Vec<Case>,
    completion_order: Vec<usize>,
    instances: Vec<Instance>,
    scopes: Vec<Scope>,
    subs: Vec<Sub>,
    next_token: u64,
    runs: Vec<TaskRun>,
    pools: BTreeMap<String, Pool>,
    records: Vec<ActivityRecord>,
    end_events: BTreeMap<QId, u64>,
    boundary_fired: BTreeMap<QId, u64>,
    decision_stats: BTreeMap<String, DecisionStats>,
    trace: Vec<TraceEvent>,
    horizon_reached: bool,
    msg_flows: Vec<&'a MessageFlow>,
    /// group -> case
    groups: Vec<usize>,
    /// (group, receiving element) -> subscriptions, FIFO.
    waiting_msg: BTreeMap<(usize, QId), VecDeque<usize>>,
    /// signal name -> (case, subscription).
    waiting_sig: BTreeMap<String, BTreeSet<(usize, usize)>>,
    /// (group, receiving element) -> buffered message count.
    buffer: BTreeMap<(usize, QId), usize>,
    messages: Vec<MessageRecord>,
    signals: Vec<SignalRecord>,
    // Static lookups derived from the model.
    boundaries_of: BTreeMap<QId, Vec<QId>>,
    sub_starts: BTreeMap<QId, Vec<QId>>,
    /// (process, enclosing subprocess or `None` for the top level) -> start events of its
    /// event subprocesses.
    event_subs_of: BTreeMap<(QId, Option<QId>), Vec<QId>>,
    /// Event subprocess subscription -> scope it belongs to (`None`: the top level).
    es_subs: BTreeMap<usize, Option<usize>>,
    link_catch: BTreeMap<(QId, String), QId>,
    signal_starts: BTreeMap<String, Vec<QId>>,
    /// Inclusive join -> every element from which a token can still reach it.
    or_reach: BTreeMap<QId, BTreeSet<QId>>,
    /// Elements whose completion a history gateway asks about.
    watched: BTreeSet<QId>,
}

/// Run all replications; replication `i` uses seed `seed + i`.
pub fn run_replications(m: &BoundModel) -> Vec<ReplicationResult> {
    run_replications_with(m, |_, _| {})
}

/// Like `run_replications`; `progress(done, total)` is called after each replication.
pub fn run_replications_with(
    m: &BoundModel,
    mut progress: impl FnMut(u32, u32),
) -> Vec<ReplicationResult> {
    let total = m.sim.replications;
    (0..total)
        .map(|i| {
            let r = Engine::new(m, m.sim.seed.wrapping_add(u64::from(i))).run();
            progress(i + 1, total);
            r
        })
        .collect()
}

impl<'a> Engine<'a> {
    pub fn new(m: &'a BoundModel<'a>, seed: u64) -> Self {
        let mut boundaries_of: BTreeMap<QId, Vec<QId>> = BTreeMap::new();
        let mut sub_starts: BTreeMap<QId, Vec<QId>> = BTreeMap::new();
        let mut event_subs_of: BTreeMap<(QId, Option<QId>), Vec<QId>> = BTreeMap::new();
        let mut link_catch = BTreeMap::new();
        let mut signal_starts: BTreeMap<String, Vec<QId>> = BTreeMap::new();
        for n in m.project.nodes().filter(|n| m.active.contains(&n.process)) {
            if let Some(a) = &n.attached_to {
                boundaries_of
                    .entry(a.clone())
                    .or_default()
                    .push(n.id.clone());
            }
            if n.kind == NodeKind::SubProcess {
                sub_starts.insert(n.id.clone(), m.project.start_nodes(&n.process, Some(&n.id)));
            }
            if m.project.is_event_sub_start(n)
                && let Some(es) = n.parent.as_ref().and_then(|p| m.project.node(p))
            {
                event_subs_of
                    .entry((n.process.clone(), es.parent.clone()))
                    .or_default()
                    .push(n.id.clone());
            }
            if n.kind == NodeKind::StartEvent && n.parent.is_none() {
                for d in &n.event_defs {
                    if let EventDef::Signal(name) = d {
                        signal_starts
                            .entry(name.clone().unwrap_or_default())
                            .or_default()
                            .push(n.id.clone());
                    }
                }
            }
            if n.kind == NodeKind::IntermediateCatchEvent {
                for d in &n.event_defs {
                    if let EventDef::Link(name) = d {
                        link_catch.insert((n.process.clone(), name.clone()), n.id.clone());
                    }
                }
            }
        }
        let or_reach = m
            .project
            .nodes()
            .filter(|n| {
                m.active.contains(&n.process)
                    && n.kind == NodeKind::InclusiveGateway
                    && n.incoming.len() > 1
            })
            .map(|j| (j.id.clone(), reaching(m.project, j)))
            .collect();
        Self {
            m,
            seed,
            rng: ChaCha8Rng::seed_from_u64(seed),
            now: 0.0,
            seq: 0,
            queue: BinaryHeap::new(),
            started: 0,
            cases: Vec::new(),
            completion_order: Vec::new(),
            instances: Vec::new(),
            scopes: Vec::new(),
            subs: Vec::new(),
            next_token: 0,
            runs: Vec::new(),
            pools: m
                .pools
                .keys()
                .map(|k| (k.clone(), Pool::default()))
                .collect(),
            records: Vec::new(),
            end_events: BTreeMap::new(),
            boundary_fired: BTreeMap::new(),
            decision_stats: BTreeMap::new(),
            trace: Vec::new(),
            horizon_reached: false,
            msg_flows: m
                .project
                .files
                .values()
                .flat_map(|f| f.message_flows.values())
                .collect(),
            groups: Vec::new(),
            waiting_msg: BTreeMap::new(),
            waiting_sig: BTreeMap::new(),
            buffer: BTreeMap::new(),
            messages: Vec::new(),
            signals: Vec::new(),
            boundaries_of,
            sub_starts,
            event_subs_of,
            es_subs: BTreeMap::new(),
            link_catch,
            signal_starts,
            or_reach,
            watched: m.history.values().map(|h| h.element.clone()).collect(),
        }
    }

    fn schedule(&mut self, at: f64, ev: Ev) {
        self.seq += 1;
        self.queue.push(Reverse((Time(at), self.seq, ev)));
    }

    pub fn run(mut self) -> ReplicationResult {
        // The first case of every generator arrives at t = 0.
        for g in 0..self.m.arrivals.len() {
            self.schedule(0.0, Ev::Arrival(g));
        }
        let horizon = self.m.sim.horizon;
        while let Some(Reverse((Time(t), _, ev))) = self.queue.pop() {
            if horizon.is_some_and(|h| t > h) {
                // Cut off at the horizon: later events never happen.
                self.horizon_reached = true;
                break;
            }
            self.now = t;
            match ev {
                Ev::Arrival(g) => self.on_arrival(g),
                Ev::TaskDone(run) => self.on_task_done(run),
                Ev::Reply { group, flow } => self.deliver(group, flow),
                Ev::Timer(sub) => self.fire_sub(sub),
            }
        }
        if self.horizon_reached {
            self.now = horizon.unwrap_or(self.now);
        }
        self.finish()
    }

    fn on_arrival(&mut self, g: usize) {
        // Once enough cases are started, the next generator firing only marks the end of the
        // observation window (it advances the clock but starts nothing).
        if self.started >= self.m.sim.cases {
            return;
        }
        let case = self.cases.len();
        let group = self.groups.len();
        self.groups.push(case);
        self.cases.push(Case {
            arrival: self.now,
            open_instances: 0,
            cost: 0.0,
            completion: None,
            completed: BTreeSet::new(),
        });
        self.started += 1;

        // One instance per process; all triggers of that process get a token.
        let mut per_process: Vec<(QId, Vec<QId>)> = Vec::new();
        for t in &self.m.arrivals[g].triggers {
            let Some(node) = self.m.project.node(t) else {
                continue;
            };
            match per_process.iter_mut().find(|(p, _)| *p == node.process) {
                Some((_, starts)) => starts.push(t.clone()),
                None => per_process.push((node.process.clone(), vec![t.clone()])),
            }
        }
        // Create every instance before running any of them: a case must not look complete
        // while a later pool's instance has not been created yet. Instances then run in trigger order.
        let created: Vec<(usize, Vec<QId>)> = per_process
            .into_iter()
            .map(|(process, starts)| (self.create_instance(case, group, process, None), starts))
            .collect();
        for (inst, starts) in created {
            self.start_instance(inst, starts);
        }

        let gap = self.m.arrivals[g].inter_arrival.sample(&mut self.rng);
        self.schedule(self.now + gap, Ev::Arrival(g));
    }

    fn create_instance(
        &mut self,
        case: usize,
        group: usize,
        process: QId,
        parent: Option<(usize, u64)>,
    ) -> usize {
        let inst = self.instances.len();
        self.instances.push(Instance {
            case,
            group,
            parent,
            started: self.now,
            finished: None,
            variables: BTreeMap::new(),
            process,
            tokens: BTreeMap::new(),
            joins: BTreeMap::new(),
            event_subs: Vec::new(),
            done: false,
        });
        self.cases[case].open_instances += 1;
        inst
    }

    fn start_instance(&mut self, inst: usize, starts: Vec<QId>) {
        let process = self.instances[inst].process.clone();
        self.arm_event_subs(inst, NO_TOKEN, None, &process, None);
        let mut work = Vec::new();
        for s in starts.into_iter().rev() {
            let tok = self.new_token(inst, &s, "starting", None);
            work.push((tok, s, None));
        }
        self.process(inst, work);
    }

    fn new_token(&mut self, inst: usize, node: &QId, state: &str, scope: Option<usize>) -> u64 {
        let id = self.next_token;
        self.next_token += 1;
        self.instances[inst].tokens.insert(
            id,
            Token {
                node: node.clone(),
                state: state.into(),
                scope,
                child_scope: None,
                run: None,
                child_instance: None,
                subs: Vec::new(),
                iteration: false,
            },
        );
        id
    }

    fn is_iteration(&self, inst: usize, tok: u64) -> bool {
        self.instances[inst]
            .tokens
            .get(&tok)
            .is_some_and(|t| t.iteration)
    }

    fn trace(&mut self, inst: usize, tok: u64, kind: TraceKind) {
        let case = self.instances[inst].case;
        if (case as u64) < self.m.sim.trace_cases {
            self.trace.push(TraceEvent {
                time: self.now,
                case: case as u64,
                instance: inst as u64,
                token: tok,
                kind,
            });
        }
    }

    /// Remove a token from its instance (traced as gone).
    fn remove_token(&mut self, inst: usize, tok: u64) -> Option<Token> {
        let t = self.instances[inst].tokens.remove(&tok);
        if t.is_some() {
            self.trace(inst, tok, TraceKind::Gone);
        }
        t
    }

    fn set_token(&mut self, inst: usize, tok: u64, node: &QId, state: &str) {
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.node = node.clone();
            t.state = state.into();
        }
    }

    fn alive(&self, inst: usize, tok: u64) -> bool {
        self.instances[inst].tokens.contains_key(&tok)
    }

    /// Move tokens through zero-time elements until they wait or end.
    fn process(&mut self, inst: usize, mut work: Vec<Work>) {
        loop {
            self.run_work(inst, &mut work);
            // Inclusive joins can only be judged once every token has settled.
            if !self.fire_or_joins(inst, &mut work) {
                break;
            }
        }
        self.check_completion(inst);
    }

    fn run_work(&mut self, inst: usize, work: &mut Vec<Work>) {
        let mut work = std::mem::take(work);
        while let Some((tok, node_id, via)) = work.pop() {
            // The token may have been cancelled (terminate, interrupting boundary) meanwhile.
            if !self.alive(inst, tok) {
                continue;
            }
            let m = self.m;
            let Some(node) = m.project.node(&node_id) else {
                continue;
            };
            self.trace(
                inst,
                tok,
                TraceKind::Arrive {
                    node: node_id.clone(),
                    via: via.clone(),
                },
            );
            let first_def = node.event_defs.first();
            if let Some(cfg) = m.loops.get(&node_id)
                && !self.is_iteration(inst, tok)
            {
                self.start_iterations(inst, tok, node, cfg, &mut work);
                continue;
            }
            match &node.kind {
                NodeKind::EndEvent => {
                    self.mark_completed(inst, &node_id);
                    self.throw(inst, node);
                    if !self.alive(inst, tok) {
                        continue;
                    }
                    *self.end_events.entry(node_id.clone()).or_default() += 1;
                    match first_def {
                        Some(EventDef::Terminate) => self.terminate(inst, tok),
                        Some(EventDef::Error(code)) => {
                            if !self.raise(inst, tok, true, code.as_deref(), &mut work) {
                                // Camunda raises an incident: the token stays and is reported.
                                self.set_token(
                                    inst,
                                    tok,
                                    &node_id,
                                    &format!(
                                        "uncaught error '{}'",
                                        code.as_deref().unwrap_or_default()
                                    ),
                                );
                                continue;
                            }
                        }
                        Some(EventDef::Escalation(code)) => {
                            self.raise(inst, tok, false, code.as_deref(), &mut work);
                        }
                        _ => {}
                    }
                    if self.alive(inst, tok) {
                        self.consume(inst, tok, &mut work);
                    }
                }
                NodeKind::Task(TaskKind::Receive) => {
                    self.arm_boundaries(inst, tok, node);
                    self.wait_message(inst, tok, node, &mut work);
                }
                NodeKind::Task(_) => self.request_task(inst, tok, node),
                NodeKind::SubProcess => self.enter_scope(inst, tok, node, &mut work),
                // A call activity whose process is not loaded runs as a task.
                NodeKind::CallActivity if m.activities.contains_key(&node_id) => {
                    self.request_task(inst, tok, node)
                }
                NodeKind::CallActivity => self.call(inst, tok, node),
                NodeKind::IntermediateCatchEvent => match first_def {
                    Some(EventDef::Signal(name)) => {
                        self.wait_signal(inst, tok, node, name.clone().unwrap_or_default())
                    }
                    Some(EventDef::Timer) => self.wait_timer(inst, tok, node),
                    Some(EventDef::Link(_)) => {
                        self.set_token(inst, tok, &node_id, "passing");
                        self.route(inst, tok, node, &mut work);
                    }
                    _ => self.wait_message(inst, tok, node, &mut work),
                },
                NodeKind::IntermediateThrowEvent => {
                    self.set_token(inst, tok, &node_id, "passing");
                    if let Some(EventDef::Link(name)) = first_def {
                        // Immediate jump to the link catch event with the same name (V11).
                        match self.link_catch.get(&(node.process.clone(), name.clone())) {
                            Some(c) => work.push((tok, c.clone(), None)),
                            None => self.consume(inst, tok, &mut work),
                        }
                    } else {
                        self.mark_completed(inst, &node_id);
                        self.throw(inst, node);
                        if let Some(EventDef::Escalation(code)) = first_def
                            && self.alive(inst, tok)
                        {
                            self.raise(inst, tok, false, code.as_deref(), &mut work);
                        }
                        if self.alive(inst, tok) {
                            self.route(inst, tok, node, &mut work);
                        }
                    }
                }
                NodeKind::ParallelGateway if node.incoming.len() > 1 => {
                    self.and_join(inst, tok, node, via, &mut work)
                }
                NodeKind::InclusiveGateway if node.incoming.len() > 1 => {
                    self.set_token(inst, tok, &node_id, "waiting at inclusive join");
                    if let Some(flow) = via {
                        self.instances[inst]
                            .joins
                            .entry(node_id.clone())
                            .or_default()
                            .entry(flow)
                            .or_default()
                            .push_back(tok);
                    }
                }
                NodeKind::EventBasedGateway => self.event_gateway(inst, tok, node, &mut work),
                _ => {
                    self.set_token(inst, tok, &node_id, "passing");
                    self.route(inst, tok, node, &mut work);
                }
            }
        }
    }

    /// Fire every inclusive join of the instance that no other token (of the same subprocess
    /// scope) can still reach. Returns whether one fired (then there is new work).
    fn fire_or_joins(&mut self, inst: usize, work: &mut Vec<Work>) -> bool {
        let mut fired = false;
        let joins: Vec<QId> = self.instances[inst]
            .joins
            .iter()
            .filter(|(j, flows)| {
                self.or_reach.contains_key(*j) && flows.values().any(|q| !q.is_empty())
            })
            .map(|(j, _)| j.clone())
            .collect();
        for j in joins {
            let Some(reach) = self.or_reach.get(&j).cloned() else {
                continue;
            };
            let i = &self.instances[inst];
            let waiting: BTreeSet<u64> = i.joins[&j].values().flatten().copied().collect();
            let scopes: BTreeSet<Option<usize>> = waiting
                .iter()
                .filter_map(|t| i.tokens.get(t).map(|x| x.scope))
                .collect();
            for scope in scopes {
                let i = &self.instances[inst];
                let blocked = i.tokens.iter().any(|(id, t)| {
                    !waiting.contains(id) && t.scope == scope && reach.contains(&t.node)
                });
                if blocked {
                    continue;
                }
                // One token per incoming flow (of this scope) is consumed; the first continues.
                let mut firing = Vec::new();
                let tokens = &self.instances[inst].tokens;
                let scope_of = |t: &u64| tokens.get(t).map(|x| x.scope);
                let mut picks = Vec::new();
                if let Some(flows) = self.instances[inst].joins.get(&j) {
                    for (f, q) in flows {
                        if let Some(pos) = q.iter().position(|t| scope_of(t) == Some(scope)) {
                            picks.push((f.clone(), pos));
                        }
                    }
                }
                if let Some(flows) = self.instances[inst].joins.get_mut(&j) {
                    for (f, pos) in picks {
                        if let Some(t) = flows.get_mut(&f).and_then(|q| q.remove(pos)) {
                            firing.push(t);
                        }
                    }
                }
                let Some((&keep, rest)) = firing.split_first() else {
                    continue;
                };
                for t in rest {
                    self.remove_token(inst, *t);
                }
                let m = self.m;
                if let Some(node) = m.project.node(&j) {
                    self.set_token(inst, keep, &j, "passing");
                    self.route(inst, keep, node, work);
                    fired = true;
                }
            }
        }
        fired
    }

    /// Event-based gateway: wait for whichever following catch event fires first; the others
    /// are cancelled with it (all subscriptions belong to the waiting token).
    fn event_gateway(&mut self, inst: usize, tok: u64, node: &FlowNode, work: &mut Vec<Work>) {
        let m = self.m;
        let targets: Vec<&FlowNode> = node
            .outgoing
            .iter()
            .filter_map(|f| m.project.flow(f))
            .filter_map(|f| m.project.node(&f.target))
            .collect();
        // A message already buffered for one of the events decides at once.
        let group = self.instances[inst].group;
        for t in &targets {
            let is_message = matches!(t.kind, NodeKind::Task(TaskKind::Receive))
                || matches!(t.event_defs.first(), Some(EventDef::Message(_)));
            if is_message
                && let Some(n) = self.buffer.get_mut(&(group, t.id.clone()))
                && *n > 0
            {
                *n -= 1;
                self.set_token(inst, tok, &t.id, "passing");
                self.route(inst, tok, t, work);
                return;
            }
        }
        self.set_token(inst, tok, &node.id, "waiting at event-based gateway");
        for t in targets {
            match t.event_defs.first() {
                Some(EventDef::Signal(name)) => {
                    self.add_sub(
                        inst,
                        tok,
                        &t.id,
                        false,
                        SubKind::Signal(name.clone().unwrap_or_default()),
                    );
                }
                Some(EventDef::Timer) => {}
                _ => {
                    self.add_sub(inst, tok, &t.id, false, SubKind::Message);
                }
            }
            // Timer events (and externally triggered events) fire after their `[events]` delay.
            self.external_timer(inst, tok, t);
        }
    }

    fn and_join(
        &mut self,
        inst: usize,
        tok: u64,
        node: &FlowNode,
        via: Option<QId>,
        work: &mut Vec<Work>,
    ) {
        self.set_token(inst, tok, &node.id, "waiting at AND join");
        let waiting = self.instances[inst]
            .joins
            .entry(node.id.clone())
            .or_default();
        if let Some(flow) = via {
            waiting.entry(flow).or_default().push_back(tok);
        }
        // Only tokens of this instance are ever in `waiting`, so instances cannot mix (V6).
        if node
            .incoming
            .iter()
            .all(|f| waiting.get(f).is_some_and(|q| !q.is_empty()))
        {
            let mut firing = Vec::new();
            for f in &node.incoming {
                if let Some(t) = waiting.get_mut(f).and_then(VecDeque::pop_front) {
                    firing.push(t);
                }
            }
            let keep = firing[0];
            for t in &firing[1..] {
                self.remove_token(inst, *t);
            }
            self.set_token(inst, keep, &node.id, "passing");
            self.route(inst, keep, node, work);
        }
    }

    /// Send a token out of `node`: XOR picks one flow, everything else takes all flows.
    fn route(&mut self, inst: usize, tok: u64, node: &FlowNode, work: &mut Vec<Work>) {
        if self.is_iteration(inst, tok) {
            // One iteration is done; the activity itself moves on when all are.
            self.consume(inst, tok, work);
            return;
        }
        if let Some(cfg) = self.m.activity_splits.get(&node.id) {
            let vars = &self.instances[inst].variables;
            let mut taken: Vec<&QId> = match &cfg.rule {
                SplitRule::Conditions(flows) => flows
                    .iter()
                    .filter(|(_, e)| e.eval(vars))
                    .map(|(f, _)| f)
                    .collect(),
                SplitRule::Probabilities(flows) => flows
                    .iter()
                    .filter(|(_, p)| self.rng.random::<f64>() < *p)
                    .map(|(f, _)| f)
                    .collect(),
            };
            if taken.is_empty() {
                taken.extend(cfg.default.as_ref());
            }
            if taken.is_empty() && cfg.always.is_empty() {
                // BPMN would raise an error; the token stays here and is reported as stuck.
                self.set_token(
                    inst,
                    tok,
                    &node.id,
                    "no outgoing condition is true and there is no default flow",
                );
                return;
            }
            let flows: Vec<QId> = node
                .outgoing
                .iter()
                .filter(|f| cfg.always.contains(f) || taken.contains(f))
                .cloned()
                .collect();
            self.send_along(inst, tok, node, flows, work);
            return;
        }
        if let Some(h) = self.m.history.get(&node.id) {
            let case = self.instances[inst].case;
            let flow = if self.cases[case].completed.contains(&h.element) {
                h.then.clone()
            } else {
                h.otherwise.clone()
            };
            self.send_along(inst, tok, node, vec![flow], work);
            return;
        }
        if let Some(cfg) = self.m.conditions.get(&node.id) {
            let vars = &self.instances[inst].variables;
            let mut chosen: Vec<QId> = cfg
                .flows
                .iter()
                .filter(|(_, e)| e.eval(vars))
                .map(|(f, _)| f.clone())
                .collect();
            if node.kind == NodeKind::ExclusiveGateway {
                chosen.truncate(1); // the first true condition in model order
            }
            if chosen.is_empty() {
                match &cfg.default {
                    Some(d) => chosen.push(d.clone()),
                    None => {
                        // BPMN would raise an error; the token stays here and is reported as stuck.
                        self.set_token(
                            inst,
                            tok,
                            &node.id,
                            "no outgoing condition is true and there is no default flow",
                        );
                        return;
                    }
                }
            }
            self.send_along(inst, tok, node, chosen, work);
            return;
        }
        let flows: Vec<QId> = match (&node.kind, self.m.gateways.get(&node.id)) {
            (NodeKind::ExclusiveGateway, Some(probs)) => {
                let r: f64 = self.rng.random();
                let mut acc = 0.0;
                let mut pick = &probs[probs.len() - 1].0;
                for (f, p) in probs {
                    acc += p;
                    if r < acc {
                        pick = f;
                        break;
                    }
                }
                vec![pick.clone()]
            }
            _ => match self.m.inclusive.get(&node.id) {
                Some(cfg) if node.kind == NodeKind::InclusiveGateway => {
                    let mut chosen: Vec<QId> = Vec::new();
                    for (f, p) in &cfg.flows {
                        if self.rng.random::<f64>() < *p {
                            chosen.push(f.clone());
                        }
                    }
                    if chosen.is_empty() {
                        // None chosen: the default flow, else the most likely flow (first on ties).
                        let fallback = cfg.default.clone().or_else(|| {
                            cfg.flows
                                .iter()
                                .fold(None::<&(QId, f64)>, |best, x| match best {
                                    Some(b) if b.1 >= x.1 => Some(b),
                                    _ => Some(x),
                                })
                                .map(|(f, _)| f.clone())
                        });
                        chosen.extend(fallback);
                    }
                    chosen
                }
                _ => node.outgoing.clone(),
            },
        };
        self.send_along(inst, tok, node, flows, work);
    }

    fn send_along(
        &mut self,
        inst: usize,
        tok: u64,
        node: &FlowNode,
        flows: Vec<QId>,
        work: &mut Vec<Work>,
    ) {
        self.mark_completed(inst, &node.id);
        if flows.is_empty() {
            // Implicit end (validation warns about it).
            self.consume(inst, tok, work);
            return;
        }
        let scope = self.instances[inst].tokens.get(&tok).and_then(|t| t.scope);
        let mut next = Vec::new();
        for (i, f) in flows.iter().enumerate() {
            let Some(target) = self.m.project.flow(f).map(|fl| fl.target.clone()) else {
                continue;
            };
            let t = if i == 0 {
                tok
            } else {
                self.new_token(inst, &node.id, "passing", scope)
            };
            next.push((t, target, Some(f.clone())));
        }
        // Reverse so the first flow is processed first (work is a stack).
        work.extend(next.into_iter().rev());
    }

    // ------------------------------------------------------------ scopes

    /// Remove a token that reached an end; the enclosing subprocess completes with its last token.
    fn consume(&mut self, inst: usize, tok: u64, work: &mut Vec<Work>) {
        let Some(t) = self.remove_token(inst, tok) else {
            return;
        };
        for s in t.subs {
            self.deactivate_sub(s);
        }
        // Only event subprocesses still run in the scope: it is completing and starts no new
        // ones (BPMN §13.5.4).
        let m = self.m;
        let rest: Vec<&Token> = self.instances[inst]
            .tokens
            .values()
            .filter(|x| x.scope == t.scope)
            .collect();
        if !rest.is_empty()
            && rest.iter().all(|x| {
                m.project
                    .node(&x.node)
                    .is_some_and(|n| n.triggered_by_event)
            })
        {
            let owner = t.scope.map_or(NO_TOKEN, |sc| self.scopes[sc].parent);
            self.deactivate_event_subs(inst, t.scope, owner);
        }
        if let Some(sc) = t.scope
            && !self.instances[inst]
                .tokens
                .values()
                .any(|x| x.scope == Some(sc))
        {
            let (parent, node_id) = (self.scopes[sc].parent, self.scopes[sc].node.clone());
            if self.alive(inst, parent) && self.next_iteration(sc) {
                self.spawn_iteration(inst, sc, 1, work);
                return;
            }
            if self.alive(inst, parent)
                && let Some(node) = self.m.project.node(&node_id)
            {
                self.cancel_subs(inst, parent);
                if let Some(p) = self.instances[inst].tokens.get_mut(&parent) {
                    p.child_scope = None;
                }
                self.set_token(inst, parent, &node_id, "passing");
                self.route(inst, parent, node, work);
            }
        }
    }

    /// A token arrives at a multi-instance or looping activity: it waits there while the
    /// iterations run in a scope of their own (all at once, or one after the other).
    fn start_iterations(
        &mut self,
        inst: usize,
        tok: u64,
        node: &FlowNode,
        cfg: &crate::scenario::LoopCfg,
        work: &mut Vec<Work>,
    ) {
        use crate::scenario::{Count, LoopCfg};
        let repeat = match cfg {
            LoopCfg::Multi { sequential, count } => {
                let n = match count {
                    Count::Sample(d) => d.sample(&mut self.rng),
                    Count::Variable(v) => match self.instances[inst].variables.get(v) {
                        Some(Value::List(items)) => items.len() as u32,
                        Some(Value::Num(x)) => x.round().max(0.0) as u32,
                        _ => 0,
                    },
                };
                Repeat {
                    sequential: *sequential,
                    total: Some(n),
                    chance: 0.0,
                    maximum: None,
                    started: 0,
                }
            }
            LoopCfg::Standard {
                repeat, maximum, ..
            } => Repeat {
                sequential: true,
                total: None,
                chance: *repeat,
                maximum: *maximum,
                started: 0,
            },
        };
        let sc = self.scopes.len();
        self.scopes.push(Scope {
            node: node.id.clone(),
            parent: tok,
            repeat: Some(repeat),
        });
        // A loop that tests before the first iteration may not run at all.
        let first = match cfg {
            LoopCfg::Standard {
                test_before: true, ..
            } => self.next_iteration(sc),
            _ => self.scopes[sc]
                .repeat
                .as_ref()
                .is_some_and(|r| r.total != Some(0) && r.maximum != Some(0)),
        };
        if !first {
            self.set_token(inst, tok, &node.id, "passing");
            self.route(inst, tok, node, work);
            return;
        }
        let state = match cfg {
            LoopCfg::Multi { .. } => "running multi-instance activity",
            LoopCfg::Standard { .. } => "running loop activity",
        };
        self.set_token(inst, tok, &node.id, state);
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.child_scope = Some(sc);
        }
        self.arm_boundaries(inst, tok, node);
        let count = match &self.scopes[sc].repeat {
            Some(r) if !r.sequential => r.total.unwrap_or(1),
            _ => 1,
        };
        self.spawn_iteration(inst, sc, count, work);
    }

    /// Whether a sequential multi-instance or loop activity runs another iteration.
    fn next_iteration(&mut self, sc: usize) -> bool {
        let Some(r) = &self.scopes[sc].repeat else {
            return false;
        };
        if r.maximum.is_some_and(|m| r.started >= m) {
            return false;
        }
        match r.total {
            Some(total) => r.sequential && r.started < total,
            None => {
                let chance = r.chance;
                self.rng.random::<f64>() < chance
            }
        }
    }

    fn spawn_iteration(&mut self, inst: usize, sc: usize, count: u32, work: &mut Vec<Work>) {
        let node = self.scopes[sc].node.clone();
        let mut items = Vec::new();
        for _ in 0..count {
            if let Some(r) = self.scopes[sc].repeat.as_mut() {
                r.started += 1;
            }
            let t = self.new_token(inst, &node, "starting", Some(sc));
            if let Some(x) = self.instances[inst].tokens.get_mut(&t) {
                x.iteration = true;
            }
            items.push((t, node.clone(), None));
        }
        work.extend(items.into_iter().rev());
    }

    fn enter_scope(&mut self, inst: usize, tok: u64, node: &FlowNode, work: &mut Vec<Work>) {
        let sc = self.scopes.len();
        self.scopes.push(Scope {
            node: node.id.clone(),
            parent: tok,
            repeat: None,
        });
        self.set_token(inst, tok, &node.id, "inside embedded subprocess");
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.child_scope = Some(sc);
        }
        self.arm_boundaries(inst, tok, node);
        self.arm_event_subs(inst, tok, Some(sc), &node.process, Some(&node.id));
        let starts = self.sub_starts.get(&node.id).cloned().unwrap_or_default();
        let mut items = Vec::new();
        for s in starts {
            let t = self.new_token(inst, &s, "starting", Some(sc));
            items.push((t, s, None));
        }
        work.extend(items.into_iter().rev());
    }

    /// Terminate end event: cancel everything else in the enclosing subprocess, or in the whole
    /// instance at top level.
    fn terminate(&mut self, inst: usize, tok: u64) {
        let scope = self.instances[inst].tokens.get(&tok).and_then(|t| t.scope);
        let victims: Vec<u64> = self.instances[inst]
            .tokens
            .iter()
            .filter(|(id, t)| **id != tok && (scope.is_none() || t.scope == scope))
            .map(|(id, _)| *id)
            .collect();
        for v in victims {
            if self.alive(inst, v) {
                self.cancel_token(inst, v);
            }
        }
    }

    /// Cancel what a token is doing at its activity (task run, subprocess scope, subscriptions).
    fn cancel_activity(&mut self, inst: usize, tok: u64) {
        self.cancel_subs(inst, tok);
        let (run, child, callee) = match self.instances[inst].tokens.get_mut(&tok) {
            Some(t) => (t.run.take(), t.child_scope.take(), t.child_instance.take()),
            None => return,
        };
        if let Some(c) = callee {
            self.cancel_instance(c);
        }
        if let Some(r) = run {
            self.cancel_run(r);
        }
        if let Some(sc) = child {
            let inner: Vec<u64> = self.instances[inst]
                .tokens
                .iter()
                .filter(|(_, t)| t.scope == Some(sc))
                .map(|(id, _)| *id)
                .collect();
            for t in inner {
                self.cancel_token(inst, t);
            }
        }
    }

    /// Cancel a called instance (its call activity was interrupted or terminated).
    fn cancel_instance(&mut self, inst: usize) {
        if self.instances[inst].done {
            return;
        }
        let toks: Vec<u64> = self.instances[inst].tokens.keys().copied().collect();
        for t in toks {
            if self.alive(inst, t) {
                self.cancel_token(inst, t);
            }
        }
        self.instances[inst].done = true;
        self.deactivate_event_subs(inst, None, NO_TOKEN);
        let case = self.instances[inst].case;
        self.cases[case].open_instances -= 1;
    }

    /// A token arrives at a call activity: start the called process as a new instance of the
    /// same case; the token continues when that instance completes.
    fn call(&mut self, inst: usize, tok: u64, node: &FlowNode) {
        let m = self.m;
        let Some(process) = node
            .called_element
            .as_ref()
            .and_then(|c| m.project.processes.get(c))
        else {
            return;
        };
        let starts = m.project.start_nodes(process, None);
        let case = self.instances[inst].case;
        let group = self.groups.len();
        self.groups.push(case);
        self.set_token(
            inst,
            tok,
            &node.id,
            &format!("waiting for called process '{}'", process.id),
        );
        self.arm_boundaries(inst, tok, node);
        let child = self.create_instance(case, group, process.clone(), Some((inst, tok)));
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.child_instance = Some(child);
        }
        self.start_instance(child, starts);
    }

    fn cancel_token(&mut self, inst: usize, tok: u64) {
        self.cancel_activity(inst, tok);
        for flows in self.instances[inst].joins.values_mut() {
            for q in flows.values_mut() {
                q.retain(|t| *t != tok);
            }
        }
        self.remove_token(inst, tok);
    }

    // ------------------------------------------------------------ subscriptions

    fn add_sub(
        &mut self,
        inst: usize,
        owner: u64,
        node: &QId,
        boundary: bool,
        kind: SubKind,
    ) -> usize {
        let id = self.subs.len();
        let case = self.instances[inst].case;
        let group = self.instances[inst].group;
        match &kind {
            SubKind::Message => self
                .waiting_msg
                .entry((group, node.clone()))
                .or_default()
                .push_back(id),
            SubKind::Signal(name) => {
                self.waiting_sig
                    .entry(name.clone())
                    .or_default()
                    .insert((case, id));
            }
            SubKind::Timer => {}
        }
        self.subs.push(Sub {
            inst,
            owner,
            node: node.clone(),
            boundary,
            kind,
            active: true,
        });
        if let Some(t) = self.instances[inst].tokens.get_mut(&owner) {
            t.subs.push(id);
        }
        id
    }

    fn timer_sub(
        &mut self,
        inst: usize,
        owner: u64,
        node: &QId,
        boundary: bool,
        delay: f64,
    ) -> usize {
        let s = self.add_sub(inst, owner, node, boundary, SubKind::Timer);
        self.schedule(self.now + delay, Ev::Timer(s));
        s
    }

    fn deactivate_sub(&mut self, s: usize) {
        let sub = &mut self.subs[s];
        if !sub.active {
            return;
        }
        sub.active = false;
        if let SubKind::Signal(name) = &sub.kind {
            let case = self.instances[sub.inst].case;
            if let Some(set) = self.waiting_sig.get_mut(name) {
                set.remove(&(case, s));
            }
        }
    }

    fn cancel_subs(&mut self, inst: usize, tok: u64) {
        let subs = match self.instances[inst].tokens.get_mut(&tok) {
            Some(t) => std::mem::take(&mut t.subs),
            None => return,
        };
        for s in subs {
            self.deactivate_sub(s);
        }
    }

    /// Start the boundary events attached to an activity the token has just entered.
    fn arm_boundaries(&mut self, inst: usize, tok: u64, activity: &FlowNode) {
        // The boundary events of a multi-instance or looping activity watch all iterations
        // together: they are armed on the token waiting at the activity, not per iteration.
        if self.is_iteration(inst, tok) {
            return;
        }
        let boundaries = self
            .boundaries_of
            .get(&activity.id)
            .cloned()
            .unwrap_or_default();
        for b in boundaries {
            let m = self.m;
            let Some(node) = m.project.node(&b) else {
                continue;
            };
            match node.event_defs.first() {
                Some(EventDef::Timer | EventDef::Error(_) | EventDef::Escalation(_)) => {
                    let Some(cfg) = m.boundaries.get(&b) else {
                        continue;
                    };
                    if cfg.probability < 1.0 && self.rng.random::<f64>() >= cfg.probability {
                        continue;
                    }
                    let Some(delay) = cfg.delay.as_ref().map(|d| d.sample(&mut self.rng)) else {
                        continue;
                    };
                    self.timer_sub(inst, tok, &b, true, delay);
                }
                Some(EventDef::Message(_)) => {
                    self.add_sub(inst, tok, &b, true, SubKind::Message);
                }
                Some(EventDef::Signal(name)) => {
                    self.add_sub(
                        inst,
                        tok,
                        &b,
                        true,
                        SubKind::Signal(name.clone().unwrap_or_default()),
                    );
                }
                _ => {}
            }
        }
    }

    /// Who catches an error (`error`) or escalation thrown by token `tok`, innermost first: an
    /// event subprocess of the scope the token is in, then a boundary event of the embedded
    /// subprocess or call activity around it, and so on outwards (from a called process's top
    /// level the search continues at the call activity in the calling instance). At one level a
    /// handler for exactly this code wins over a catch-all one (without a code). What an event
    /// subprocess throws is not caught by the event subprocesses next to it.
    fn find_catch(
        &self,
        inst: usize,
        tok: u64,
        error: bool,
        code: Option<&str>,
    ) -> Option<Catcher> {
        let (mut i, mut t) = (inst, tok);
        let mut skip_event_subs = false;
        loop {
            let scope = self.instances[i].tokens.get(&t)?.scope;
            if !skip_event_subs && let Some(start) = self.event_sub_catch(i, scope, error, code) {
                let owner = scope.map_or(NO_TOKEN, |sc| self.scopes[sc].parent);
                return Some(Catcher::EventSub {
                    inst: i,
                    scope,
                    owner,
                    start,
                });
            }
            let (owner_inst, mut owner) = match scope {
                Some(sc) => (i, self.scopes[sc].parent),
                None => self.instances[i].parent?,
            };
            // An iteration's boundary events belong to the whole activity.
            let o = self.instances[owner_inst].tokens.get(&owner)?;
            if o.iteration
                && let Some(sc) = o.scope
            {
                owner = self.scopes[sc].parent;
            }
            let activity = &self.instances[owner_inst].tokens.get(&owner)?.node;
            let codes: Vec<(&QId, Option<&str>)> = self
                .boundaries_of
                .get(activity)
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    let n = self.m.project.node(b)?;
                    match (n.event_defs.first()?, error) {
                        (EventDef::Error(c), true) | (EventDef::Escalation(c), false) => {
                            Some((b, c.as_deref()))
                        }
                        _ => None,
                    }
                })
                .collect();
            if let Some(b) = best_match(&codes, code) {
                return Some(Catcher::Boundary {
                    inst: owner_inst,
                    owner,
                    event: b.clone(),
                });
            }
            skip_event_subs = self
                .m
                .project
                .node(activity)
                .is_some_and(|n| n.triggered_by_event);
            (i, t) = (owner_inst, owner);
        }
    }

    /// The start event of an event subprocess of `scope` (`None`: the top level of the
    /// instance) that catches this error or escalation.
    fn event_sub_catch(
        &self,
        inst: usize,
        scope: Option<usize>,
        error: bool,
        code: Option<&str>,
    ) -> Option<QId> {
        let container = match scope {
            // The iterations of a multi-instance or loop activity are no process level.
            Some(sc) if self.scopes[sc].repeat.is_some() => return None,
            Some(sc) => Some(self.scopes[sc].node.clone()),
            None => None,
        };
        let key = (self.instances[inst].process.clone(), container);
        let codes: Vec<(&QId, Option<&str>)> = self
            .event_subs_of
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let n = self.m.project.node(s)?;
                match (n.event_defs.first()?, error) {
                    (EventDef::Error(c), true) | (EventDef::Escalation(c), false) => {
                        Some((s, c.as_deref()))
                    }
                    _ => None,
                }
            })
            .collect();
        best_match(&codes, code).cloned()
    }

    /// Throw an error or escalation from token `tok`; returns whether a boundary event or an
    /// event subprocess caught it. An error, and an escalation caught by an interrupting
    /// boundary event, cancel the catching activity with everything inside (including the
    /// thrower); a non-interrupting escalation boundary starts an extra token and the thrower
    /// goes on. An event subprocess cancels the rest of its scope if it is interrupting.
    fn raise(
        &mut self,
        inst: usize,
        tok: u64,
        error: bool,
        code: Option<&str>,
        work: &mut Vec<Work>,
    ) -> bool {
        let (owner_inst, owner, b) = match self.find_catch(inst, tok, error, code) {
            None => return false,
            Some(Catcher::Boundary { inst, owner, event }) => (inst, owner, event),
            Some(Catcher::EventSub {
                inst: owner_inst,
                scope,
                owner,
                start,
            }) => {
                let interrupting = error
                    || self
                        .m
                        .project
                        .node(&start)
                        .is_some_and(|n| n.cancel_activity);
                *self.boundary_fired.entry(start.clone()).or_default() += 1;
                let item = self.start_event_sub(owner_inst, scope, owner, &start, interrupting);
                if owner_inst == inst {
                    work.push(item);
                } else {
                    self.process(owner_inst, vec![item]);
                }
                return true;
            }
        };
        let m = self.m;
        let interrupting = error || m.project.node(&b).is_some_and(|n| n.cancel_activity);
        *self.boundary_fired.entry(b.clone()).or_default() += 1;
        let next = if interrupting {
            self.cancel_activity(owner_inst, owner);
            self.set_token(owner_inst, owner, &b, "passing");
            owner
        } else {
            let scope = self.instances[owner_inst]
                .tokens
                .get(&owner)
                .and_then(|t| t.scope);
            self.new_token(owner_inst, &b, "passing", scope)
        };
        if owner_inst == inst {
            work.push((next, b, None));
        } else {
            self.process(owner_inst, vec![(next, b, None)]);
        }
        true
    }

    /// A subscription's trigger happened.
    fn fire_sub(&mut self, s: usize) {
        if !self.subs[s].active {
            return;
        }
        if let Some(&scope) = self.es_subs.get(&s) {
            self.fire_event_sub(s, scope);
            return;
        }
        let (inst, owner, node_id, boundary) = {
            let sub = &self.subs[s];
            (sub.inst, sub.owner, sub.node.clone(), sub.boundary)
        };
        let m = self.m;
        let Some(node) = m.project.node(&node_id) else {
            return;
        };
        if !self.alive(inst, owner) {
            self.deactivate_sub(s);
            return;
        }
        if !boundary {
            self.cancel_subs(inst, owner);
            self.resume(inst, owner, node);
            return;
        }
        *self.boundary_fired.entry(node_id.clone()).or_default() += 1;
        let mut work = Vec::new();
        if node.cancel_activity {
            // Interrupting: the activity is cancelled and its token continues on the boundary flow.
            self.cancel_activity(inst, owner);
            self.set_token(inst, owner, &node_id, "passing");
            work.push((owner, node_id, None));
        } else {
            // Non-interrupting: a timer fires once; message/signal boundaries stay armed.
            if matches!(self.subs[s].kind, SubKind::Timer) {
                self.deactivate_sub(s);
            }
            let scope = self.instances[inst]
                .tokens
                .get(&owner)
                .and_then(|t| t.scope);
            let t = self.new_token(inst, &node_id, "passing", scope);
            work.push((t, node_id, None));
        }
        self.process(inst, work);
    }

    /// Continue a token that was waiting at `node`.
    fn resume(&mut self, inst: usize, tok: u64, node: &FlowNode) {
        let mut work = Vec::new();
        self.set_token(inst, tok, &node.id, "passing");
        self.route(inst, tok, node, &mut work);
        self.process(inst, work);
    }

    // ------------------------------------------------------------ event subprocesses

    /// A process level starts (`container`: the embedded subprocess, `None` for the top level
    /// of the instance's process): the start events of its event subprocesses subscribe to
    /// their message or signal, and to their `[events]` delay (timers, external triggers).
    /// Errors and escalations need no subscription (`find_catch`).
    fn arm_event_subs(
        &mut self,
        inst: usize,
        owner: u64,
        scope: Option<usize>,
        process: &QId,
        container: Option<&QId>,
    ) {
        let starts = self
            .event_subs_of
            .get(&(process.clone(), container.cloned()))
            .cloned()
            .unwrap_or_default();
        let m = self.m;
        for s in starts {
            let Some(node) = m.project.node(&s) else {
                continue;
            };
            let mut ids = Vec::new();
            match node.event_defs.first() {
                Some(EventDef::Message(_)) => {
                    ids.push(self.add_sub(inst, owner, &s, false, SubKind::Message));
                }
                Some(EventDef::Signal(name)) => ids.push(self.add_sub(
                    inst,
                    owner,
                    &s,
                    false,
                    SubKind::Signal(name.clone().unwrap_or_default()),
                )),
                _ => {}
            }
            if let Some(d) = m.events.get(&s) {
                let delay = d.sample(&mut self.rng);
                ids.push(self.timer_sub(inst, owner, &s, false, delay));
            }
            for id in ids {
                self.es_subs.insert(id, scope);
                if owner == NO_TOKEN {
                    self.instances[inst].event_subs.push(id);
                }
            }
        }
    }

    /// The trigger of an event subprocess's start event happened.
    fn fire_event_sub(&mut self, s: usize, scope: Option<usize>) {
        let (inst, owner, start) = {
            let sub = &self.subs[s];
            (sub.inst, sub.owner, sub.node.clone())
        };
        let gone = if owner == NO_TOKEN {
            self.instances[inst].done
        } else {
            !self.alive(inst, owner)
        };
        if gone {
            self.deactivate_sub(s);
            return;
        }
        let interrupting = self
            .m
            .project
            .node(&start)
            .is_some_and(|n| n.cancel_activity);
        if !interrupting && matches!(self.subs[s].kind, SubKind::Timer) {
            // A non-interrupting timer fires once; messages and signals stay armed.
            self.deactivate_sub(s);
        }
        *self.boundary_fired.entry(start.clone()).or_default() += 1;
        let item = self.start_event_sub(inst, scope, owner, &start, interrupting);
        self.process(inst, vec![item]);
    }

    /// Start the event subprocess of `start` in `scope`: a token waits at the event subprocess
    /// (so the scope completes only after it) while its own scope runs from `start`. If it is
    /// interrupting, everything else in the scope is cancelled first and no other event
    /// subprocess of the scope can start any more. Returns the work item for `start`.
    fn start_event_sub(
        &mut self,
        inst: usize,
        scope: Option<usize>,
        owner: u64,
        start: &QId,
        interrupting: bool,
    ) -> Work {
        if interrupting {
            self.deactivate_event_subs(inst, scope, owner);
            let victims: Vec<u64> = self.instances[inst]
                .tokens
                .iter()
                .filter(|(_, t)| scope.is_none() || t.scope == scope)
                .map(|(id, _)| *id)
                .collect();
            for v in victims {
                if self.alive(inst, v) {
                    self.cancel_token(inst, v);
                }
            }
        }
        let m = self.m;
        let es = m
            .project
            .node(start)
            .and_then(|n| n.parent.clone())
            .unwrap_or_else(|| start.clone());
        let tok = self.new_token(inst, &es, "inside event subprocess", scope);
        self.trace(
            inst,
            tok,
            TraceKind::Arrive {
                node: es.clone(),
                via: None,
            },
        );
        let sc = self.scopes.len();
        self.scopes.push(Scope {
            node: es.clone(),
            parent: tok,
            repeat: None,
        });
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.child_scope = Some(sc);
        }
        let process = self.instances[inst].process.clone();
        self.arm_event_subs(inst, tok, Some(sc), &process, Some(&es));
        let first = self.new_token(inst, start, "starting", Some(sc));
        (first, start.clone(), None)
    }

    /// End the event subprocess subscriptions of `scope` (owned by `owner`).
    fn deactivate_event_subs(&mut self, inst: usize, scope: Option<usize>, owner: u64) {
        let subs: Vec<usize> = if owner == NO_TOKEN {
            std::mem::take(&mut self.instances[inst].event_subs)
        } else {
            let Some(t) = self.instances[inst].tokens.get(&owner) else {
                return;
            };
            t.subs
                .iter()
                .copied()
                .filter(|s| self.es_subs.get(s) == Some(&scope))
                .collect()
        };
        if let Some(t) = self.instances[inst].tokens.get_mut(&owner) {
            t.subs.retain(|s| !subs.contains(s));
        }
        for s in subs {
            self.deactivate_sub(s);
        }
    }

    // ------------------------------------------------------------ tasks

    fn request_task(&mut self, inst: usize, tok: u64, node: &FlowNode) {
        let Some(cfg) = self.m.activities.get(&node.id) else {
            return;
        };
        let id = self.runs.len();
        self.runs.push(TaskRun {
            instance: inst,
            token: tok,
            node: node.id.clone(),
            queued: self.now,
            start: self.now,
            duration: 0.0,
            state: RunState::Queued,
        });
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.run = Some(id);
        }
        self.arm_boundaries(inst, tok, node);
        let free = match &cfg.pool {
            None => true,
            Some(p) => {
                let cap = self.m.pools.get(p).map_or(0, |d| d.count);
                self.pools.get(p).is_some_and(|pool| pool.busy < cap)
            }
        };
        if free {
            self.start_task(id);
        } else if let Some(p) = &cfg.pool {
            self.set_token(inst, tok, &node.id, &format!("queued for resource '{p}'"));
            if let Some(pool) = self.pools.get_mut(p) {
                pool.queue.push_back(id);
            }
        }
    }

    fn start_task(&mut self, id: usize) {
        let node = self.runs[id].node.clone();
        let Some(cfg) = self.m.activities.get(&node) else {
            return;
        };
        let duration = cfg.duration.sample(&mut self.rng);
        if let Some(pool) = cfg.pool.as_ref().and_then(|p| self.pools.get_mut(p)) {
            pool.busy += 1;
        }
        // With a calendar the work starts when the resource is next open and pauses outside
        // opening hours; `duration` is working time, `end` is wall-clock time.
        let calendar = self.calendar_of(&node);
        let (begin, end) = match calendar {
            Some(c) => {
                let begin = c.next_open(self.now);
                (begin, c.advance(begin, duration))
            }
            None => (self.now, self.now + duration),
        };
        let r = &mut self.runs[id];
        r.start = begin;
        r.duration = duration;
        r.state = RunState::Running;
        let (inst, tok) = (r.instance, r.token);
        self.set_token(inst, tok, &node, "working");
        self.schedule(end, Ev::TaskDone(id));
    }

    fn calendar_of(&self, activity: &QId) -> Option<&'a crate::calendar::Calendar> {
        let m = self.m;
        m.activities
            .get(activity)?
            .pool
            .as_ref()
            .and_then(|p| m.pools.get(p))
            .and_then(|d| d.calendar.as_ref())
    }

    /// Give a freed resource to the next task still waiting for it (FIFO).
    fn release(&mut self, pool: &str, worked: f64, completed: bool) {
        if let Some(p) = self.pools.get_mut(pool) {
            p.busy -= 1;
            p.busy_time += worked;
            if completed {
                p.tasks += 1;
            }
        }
        while let Some(n) = self.pools.get_mut(pool).and_then(|p| p.queue.pop_front()) {
            if self.runs[n].state == RunState::Queued {
                self.start_task(n);
                break;
            }
        }
    }

    fn on_task_done(&mut self, id: usize) {
        if self.runs[id].state != RunState::Running {
            return; // cancelled while running
        }
        self.runs[id].state = RunState::Done;
        let (inst, tok, node_id, queued, start, duration) = {
            let r = &self.runs[id];
            (
                r.instance,
                r.token,
                r.node.clone(),
                r.queued,
                r.start,
                r.duration,
            )
        };
        let m = self.m;
        let Some(cfg) = m.activities.get(&node_id) else {
            return;
        };
        let rate = cfg
            .pool
            .as_ref()
            .and_then(|p| m.pools.get(p))
            .map_or(0.0, |d| d.cost_per_hour);
        let cost = duration / 3600.0 * rate + cfg.fixed_cost;
        let case = self.instances[inst].case;
        self.cases[case].cost += cost;
        self.records.push(ActivityRecord {
            case: case as u64,
            instance: inst as u64,
            token: tok,
            activity: node_id.clone(),
            resource: cfg.pool.clone(),
            queued,
            start,
            end: self.now,
            work: duration,
            cost,
            cancelled: false,
        });
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.run = None;
        }
        self.cancel_subs(inst, tok);

        // Release the resource and hand it to the next waiting task (FIFO) before moving on.
        if let Some(p) = &cfg.pool {
            self.release(p, duration, true);
        }

        let Some(node) = m.project.node(&node_id) else {
            return;
        };
        // Before its messages go out, so a receiver that asks already sees it done.
        if !self.is_iteration(inst, tok) {
            self.mark_completed(inst, &node_id);
        }
        if node.kind == NodeKind::Task(TaskKind::Send) {
            self.throw(inst, node);
        }
        if let Some(d) = m.decisions.get(&node_id) {
            self.decide(inst, d);
        }
        let mut work = Vec::new();
        if self.alive(inst, tok) {
            self.route(inst, tok, node, &mut work);
        }
        self.process(inst, work);
    }

    /// Evaluate a business rule task's decision table: inputs come from the instance's
    /// variables or are sampled (once per instance) from the scenario; outputs become variables.
    fn decide(&mut self, inst: usize, d: &crate::scenario::BoundDecision) {
        let rng = &mut self.rng;
        let vars = &mut self.instances[inst].variables;
        let result = d.table.evaluate(|name| {
            if let Some(v) = vars.get(name) {
                return v.clone();
            }
            let v = match d.samplers.get(name) {
                Some(Sampler::Number(dist)) => Value::Num(dist.sample_raw(rng)),
                Some(Sampler::Chance(p)) => Value::Bool(rng.random::<f64>() < *p),
                Some(Sampler::Choice(options)) => {
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
                None => Value::Null,
            };
            vars.insert(name.to_string(), v.clone());
            v
        });
        let single = result.outputs.len() == 1;
        for (name, v) in &result.outputs {
            if !name.is_empty() {
                vars.insert(name.clone(), v.clone());
            }
            if let Some(r) = &d.result_variable {
                let key = if single {
                    r.clone()
                } else {
                    format!("{r}.{name}")
                };
                vars.insert(key, v.clone());
            }
        }
        let stats = self.decision_stats.entry(d.decision.clone()).or_default();
        stats.evaluations += 1;
        if result.matched == 0 {
            stats.no_match += 1;
        }
        if result.violation {
            stats.violations += 1;
        }
        let outcome = result
            .outputs
            .iter()
            .map(|(k, v)| {
                if single {
                    v.to_string()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        *stats.outcomes.entry(outcome).or_default() += 1;
    }

    /// Cut a task short (interrupting boundary event, terminate end event).
    fn cancel_run(&mut self, id: usize) {
        let state = self.runs[id].state;
        if !matches!(state, RunState::Queued | RunState::Running) {
            return;
        }
        self.runs[id].state = RunState::Cancelled;
        let m = self.m;
        let (inst, tok, node_id, queued, start) = {
            let r = &self.runs[id];
            (r.instance, r.token, r.node.clone(), r.queued, r.start)
        };
        let Some(cfg) = m.activities.get(&node_id) else {
            return;
        };
        let (worked, begun) = match state {
            RunState::Running => (
                match self.calendar_of(&node_id) {
                    Some(c) => c.open_between(start, self.now),
                    None => (self.now - start).max(0.0),
                },
                start.min(self.now),
            ),
            _ => (0.0, self.now),
        };
        let rate = cfg
            .pool
            .as_ref()
            .and_then(|p| m.pools.get(p))
            .map_or(0.0, |d| d.cost_per_hour);
        let cost = worked / 3600.0 * rate;
        let case = self.instances[inst].case;
        self.cases[case].cost += cost;
        self.records.push(ActivityRecord {
            case: case as u64,
            instance: inst as u64,
            token: tok,
            activity: node_id,
            resource: cfg.pool.clone(),
            queued,
            start: begun,
            end: self.now,
            work: worked,
            cost,
            cancelled: true,
        });
        if state == RunState::Running
            && let Some(p) = &cfg.pool
        {
            self.release(p, worked, false);
        }
    }

    // ------------------------------------------------------------ messages and signals

    /// A throwing element sends its messages and signals (take no time, spec §5.3).
    fn throw(&mut self, inst: usize, node: &FlowNode) {
        let case = self.instances[inst].case;
        let group = self.instances[inst].group;
        let is_message = node.kind == NodeKind::Task(TaskKind::Send)
            || node
                .event_defs
                .iter()
                .any(|d| matches!(d, EventDef::Message(_)));
        if is_message {
            let flows: Vec<usize> = self
                .msg_flows
                .iter()
                .enumerate()
                .filter(|(_, f)| f.source == node.id)
                .map(|(i, _)| i)
                .collect();
            for f in flows {
                self.deliver(group, f);
            }
        }
        for d in &node.event_defs {
            if let EventDef::Signal(name) = d {
                self.broadcast(case, group, name.clone().unwrap_or_default());
            }
        }
    }

    /// Deliver message flow `flow` within `group` (spec §5.3).
    fn deliver(&mut self, group: usize, flow: usize) {
        let case = self.groups[group];
        let f = self.msg_flows[flow];
        let key = (group, f.target.clone());
        let mut outcome = MessageOutcome::Lost;
        if let Some(target) = self.m.project.node(&f.target) {
            let mut waiting = None;
            if let Some(q) = self.waiting_msg.get_mut(&key) {
                while let Some(s) = q.pop_front() {
                    if self.subs[s].active {
                        waiting = Some(s);
                        break;
                    }
                }
            }
            if let Some(s) = waiting {
                self.log_message(group, f, MessageOutcome::Delivered);
                self.fire_sub(s);
                if self.subs[s].active {
                    // Non-interrupting message boundary event: stays armed.
                    self.waiting_msg.entry(key).or_default().push_back(s);
                }
                return;
            } else if target.kind == NodeKind::StartEvent && target.parent.is_none() {
                if self.cases[case].completion.is_none() {
                    self.log_message(group, f, MessageOutcome::StartedInstance);
                    let inst = self.create_instance(case, group, target.process.clone(), None);
                    self.start_instance(inst, vec![target.id.clone()]);
                    return;
                }
            } else if self.m.buffer_unconsumed
                && !matches!(target.kind, NodeKind::BoundaryEvent | NodeKind::StartEvent)
            {
                // (A boundary event only listens while its activity runs, an event subprocess
                // while its scope runs: never buffered.)
                outcome = MessageOutcome::Buffered;
                *self.buffer.entry(key).or_default() += 1;
            }
        } else {
            // Black-box pool: only logged, optionally answered after a delay.
            outcome = MessageOutcome::BlackBox;
            if let Some(delay) = self.m.black_box_reply.get(&f.target) {
                let delay = delay.sample(&mut self.rng);
                let replies: Vec<usize> = self
                    .msg_flows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.source == f.target)
                    .map(|(i, _)| i)
                    .collect();
                for r in replies {
                    self.schedule(self.now + delay, Ev::Reply { group, flow: r });
                }
            }
        }
        self.log_message(group, f, outcome);
    }

    fn log_message(&mut self, group: usize, f: &MessageFlow, outcome: MessageOutcome) {
        self.messages.push(MessageRecord {
            time: self.now,
            case: self.groups[group] as u64,
            flow: f.id.clone(),
            source: f.source.clone(),
            target: f.target.clone(),
            outcome,
        });
    }

    /// Externally triggered catch events (`[events]`) can also fire after a delay.
    fn external_timer(&mut self, inst: usize, tok: u64, node: &FlowNode) {
        if let Some(d) = self.m.events.get(&node.id) {
            let delay = d.sample(&mut self.rng);
            self.timer_sub(inst, tok, &node.id, false, delay);
        }
    }

    fn wait_timer(&mut self, inst: usize, tok: u64, node: &FlowNode) {
        self.set_token(inst, tok, &node.id, "waiting for timer");
        self.external_timer(inst, tok, node);
    }

    /// A token arrives at a receive task / message catch event: take a buffered message or wait.
    fn wait_message(&mut self, inst: usize, tok: u64, node: &FlowNode, work: &mut Vec<Work>) {
        let key = (self.instances[inst].group, node.id.clone());
        match self.buffer.get_mut(&key) {
            Some(n) if *n > 0 => {
                *n -= 1;
                self.cancel_subs(inst, tok);
                self.set_token(inst, tok, &node.id, "passing");
                self.route(inst, tok, node, work);
            }
            _ => {
                self.set_token(inst, tok, &node.id, "waiting for message");
                self.add_sub(inst, tok, &node.id, false, SubKind::Message);
                self.external_timer(inst, tok, node);
            }
        }
    }

    fn wait_signal(&mut self, inst: usize, tok: u64, node: &FlowNode, name: String) {
        self.set_token(inst, tok, &node.id, &format!("waiting for signal '{name}'"));
        self.add_sub(inst, tok, &node.id, false, SubKind::Signal(name));
        self.external_timer(inst, tok, node);
    }

    /// Signals are not stored: only subscriptions waiting right now receive them (spec §5.4).
    fn broadcast(&mut self, case: usize, group: usize, name: String) {
        let global = self.m.signal_scope == SignalScope::Global;
        let woken: Vec<(usize, usize)> = self
            .waiting_sig
            .get(&name)
            .map(|set| {
                set.iter()
                    .filter(|(c, _)| global || *c == case)
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(set) = self.waiting_sig.get_mut(&name) {
            for w in &woken {
                set.remove(w);
            }
        }
        let mut receivers = 0;
        for (c, s) in woken {
            if !self.subs[s].active {
                continue;
            }
            receivers += 1;
            self.fire_sub(s);
            if self.subs[s].active {
                // Non-interrupting signal boundary event: stays armed.
                self.waiting_sig
                    .entry(name.clone())
                    .or_default()
                    .insert((c, s));
            }
        }
        // Signal start events create instances in the sender's case.
        let mut started = 0;
        for start in self.signal_starts.get(&name).cloned().unwrap_or_default() {
            if self.cases[case].completion.is_some() {
                break;
            }
            let Some(node) = self.m.project.node(&start) else {
                continue;
            };
            let inst = self.create_instance(case, group, node.process.clone(), None);
            self.start_instance(inst, vec![start]);
            started += 1;
        }
        self.signals.push(SignalRecord {
            time: self.now,
            case: case as u64,
            name,
            receivers,
            started,
        });
    }

    /// Remember that `node` completed in the instance's case, if a history gateway asks.
    fn mark_completed(&mut self, inst: usize, node: &QId) {
        if self.watched.contains(node) {
            let case = self.instances[inst].case;
            self.cases[case].completed.insert(node.clone());
        }
    }

    fn check_completion(&mut self, inst: usize) {
        let i = &mut self.instances[inst];
        if i.done || !i.tokens.is_empty() {
            return;
        }
        i.done = true;
        i.finished = Some(self.now);
        let (case, parent) = (i.case, i.parent);
        self.deactivate_event_subs(inst, None, NO_TOKEN);
        let c = &mut self.cases[case];
        c.open_instances -= 1;
        if c.open_instances == 0 {
            c.completion = Some(self.now);
            self.completion_order.push(case);
        }
        // A called instance completing releases the call activity token that waits for it.
        if let Some((pi, pt)) = parent
            && self.alive(pi, pt)
        {
            self.cancel_subs(pi, pt);
            if let Some(t) = self.instances[pi].tokens.get_mut(&pt) {
                t.child_instance = None;
                let node = t.node.clone();
                if let Some(n) = self.m.project.node(&node) {
                    self.resume(pi, pt, n);
                }
            }
        }
    }

    fn finish(self) -> ReplicationResult {
        let warm: BTreeSet<usize> = self
            .completion_order
            .iter()
            .take(self.m.sim.warm_up_cases as usize)
            .copied()
            .collect();
        let is_warm = |c: usize| warm.contains(&c);

        let cases: Vec<CaseRecord> = self
            .cases
            .iter()
            .enumerate()
            .map(|(i, c)| CaseRecord {
                id: i as u64,
                arrival: c.arrival,
                completion: c.completion,
                cost: c.cost,
                warm_up: is_warm(i),
            })
            .collect();
        let counted = || {
            cases
                .iter()
                .filter(|c| !c.warm_up && c.completion.is_some())
        };
        let cycle: Vec<f64> = counted()
            .filter_map(|c| c.completion.map(|e| e - c.arrival))
            .collect();
        let cost: Vec<f64> = counted().map(|c| c.cost).collect();

        let mut by_activity: BTreeMap<&QId, Vec<&ActivityRecord>> = BTreeMap::new();
        for r in self
            .records
            .iter()
            .filter(|r| !cases[r.case as usize].warm_up)
        {
            by_activity.entry(&r.activity).or_default().push(r);
        }
        let activities = by_activity
            .into_iter()
            .map(|(id, rs)| {
                let done: Vec<&&ActivityRecord> = rs.iter().filter(|r| !r.cancelled).collect();
                let waits: Vec<f64> = done.iter().map(|r| r.waiting()).collect();
                let procs: Vec<f64> = done.iter().map(|r| r.processing()).collect();
                (
                    id.clone(),
                    ActivityStats {
                        runs: done.len(),
                        cancelled: rs.len() - done.len(),
                        waiting: Summary::of(&waits),
                        processing: Summary::of(&procs),
                        total_cost: rs.iter().map(|r| r.cost).sum(),
                    },
                )
            })
            .collect();

        let mut by_process: BTreeMap<&QId, Vec<f64>> = BTreeMap::new();
        for i in &self.instances {
            if let (Some(end), false) = (i.finished, cases[i.case].warm_up) {
                by_process
                    .entry(&i.process)
                    .or_default()
                    .push(end - i.started);
            }
        }
        let processes = by_process
            .into_iter()
            .map(|(p, v)| {
                (
                    p.clone(),
                    ProcessStats {
                        instances: v.len(),
                        cycle_time: Summary::of(&v),
                    },
                )
            })
            .collect();

        // The observation window is the whole run; utilisation includes warm-up cases.
        let end_time = self.now;
        // Tasks still running at the horizon have worked until then.
        let mut busy: BTreeMap<&String, f64> =
            self.pools.iter().map(|(n, p)| (n, p.busy_time)).collect();
        for r in self.runs.iter().filter(|r| r.state == RunState::Running) {
            if let Some(pool) = self.m.activities.get(&r.node).and_then(|a| a.pool.as_ref()) {
                let worked = match self.calendar_of(&r.node) {
                    Some(c) => c.open_between(r.start, end_time),
                    None => (end_time - r.start).max(0.0),
                };
                if let Some(b) = busy.get_mut(pool) {
                    *b += worked.min(r.duration);
                }
            }
        }
        let resources = self
            .pools
            .iter()
            .map(|(name, p)| {
                let def = &self.m.pools[name];
                let open = match &def.calendar {
                    Some(c) => c.open_between(0.0, end_time),
                    None => end_time,
                };
                let busy_time = busy[name];
                let available = f64::from(def.count) * open;
                (
                    name.clone(),
                    ResourceStats {
                        count: def.count,
                        tasks: p.tasks,
                        busy_time,
                        utilisation: if available > 0.0 {
                            busy_time / available
                        } else {
                            0.0
                        },
                        cost: busy_time / 3600.0 * def.cost_per_hour,
                    },
                )
            })
            .collect();

        // A token sitting at an embedded subprocess is not reported itself: its inner tokens are.
        let alive_tokens: Vec<StuckToken> = self
            .instances
            .iter()
            .flat_map(|inst| {
                inst.tokens
                    .values()
                    .filter(|t| t.child_scope.is_none() && t.child_instance.is_none())
                    .map(move |t| StuckToken {
                        case: inst.case as u64,
                        process: inst.process.clone(),
                        element: t.node.clone(),
                        name: self.m.project.node(&t.node).and_then(|n| n.name.clone()),
                        state: t.state.clone(),
                    })
            })
            .collect();
        // Cut off by the horizon: unfinished work is not a deadlock.
        let (stuck, unfinished) = if self.horizon_reached {
            (Vec::new(), alive_tokens)
        } else {
            (alive_tokens, Vec::new())
        };

        ReplicationResult {
            seed: self.seed,
            end_time,
            cases_started: self.started,
            cases_completed: self.completion_order.len() as u64,
            case_cycle_time: Summary::of(&cycle),
            case_cost: Summary::of(&cost),
            activities,
            resources,
            end_events: self.end_events,
            stuck,
            unfinished,
            horizon_reached: self.horizon_reached,
            start_epoch: self.m.sim.clock.epoch,
            messages_lost: self
                .messages
                .iter()
                .filter(|x| x.outcome == MessageOutcome::Lost)
                .count() as u64,
            undelivered: self
                .buffer
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|((group, target), n)| UndeliveredMessage {
                    case: self.groups[*group] as u64,
                    target: target.clone(),
                    count: *n,
                })
                .collect(),
            messages: self.messages,
            signals: self.signals,
            boundary_fired: self.boundary_fired,
            decisions: self.decision_stats,
            trace: self.trace,
            processes,
            cases,
            records: self.records,
        }
    }
}

/// Every element of the same process (and subprocess level) from which `join` is reachable:
/// backwards over sequence flows, from link catch events to their link throws, and from
/// boundary events to the activity they are attached to.
/// The handler for exactly `code`, else a catch-all one (without a code).
pub(crate) fn best_match<'q>(
    handlers: &[(&'q QId, Option<&str>)],
    code: Option<&str>,
) -> Option<&'q QId> {
    let exact = handlers.iter().find(|(_, c)| c.is_some() && *c == code);
    exact
        .or_else(|| handlers.iter().find(|(_, c)| c.is_none()))
        .map(|(h, _)| *h)
}

pub(crate) fn reaching(project: &crate::project::Project, join: &FlowNode) -> BTreeSet<QId> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![join.id.clone()];
    while let Some(id) = stack.pop() {
        let Some(n) = project.node(&id) else { continue };
        let mut preds: Vec<QId> = n
            .incoming
            .iter()
            .filter_map(|f| project.flow(f))
            .map(|f| f.source.clone())
            .collect();
        if n.kind == NodeKind::IntermediateCatchEvent {
            for d in &n.event_defs {
                if let EventDef::Link(name) = d {
                    preds.extend(
                        project
                            .nodes()
                            .filter(|t| {
                                t.process == n.process
                                    && t.kind == NodeKind::IntermediateThrowEvent
                                    && t.event_defs
                                        .iter()
                                        .any(|e| matches!(e, EventDef::Link(l) if l == name))
                            })
                            .map(|t| t.id.clone()),
                    );
                }
            }
        }
        if let Some(a) = &n.attached_to {
            preds.push(a.clone());
        }
        for p in preds {
            if seen.insert(p.clone()) {
                stack.push(p);
            }
        }
    }
    seen
}
