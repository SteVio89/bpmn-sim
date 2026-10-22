//! Discrete-event engine (spec §5). One `Engine` is one replication.
//!
//! Determinism: events are ordered by `(time, sequence number)`, all maps are `BTreeMap`s and
//! all randomness comes from one seeded ChaCha generator.
//!
//! Tokens wait through *subscriptions*: a catch event, receive task or boundary event subscribes
//! to a message, a signal or a timer. Cancelling an activity deactivates its subscriptions.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::model::*;
use crate::scenario::{BoundModel, SignalScope};
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
}

/// An embedded subprocess activation; `parent` is the token waiting at the subprocess node.
struct Scope {
    node: QId,
    parent: u64,
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
    process: QId,
    tokens: BTreeMap<u64, Token>,
    /// AND join node -> incoming flow -> waiting tokens (FIFO).
    joins: BTreeMap<QId, BTreeMap<QId, VecDeque<u64>>>,
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
    link_catch: BTreeMap<(QId, String), QId>,
    signal_starts: BTreeMap<String, Vec<QId>>,
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
        let mut link_catch = BTreeMap::new();
        let mut signal_starts: BTreeMap<String, Vec<QId>> = BTreeMap::new();
        for n in m.project.nodes().filter(|n| m.active.contains(&n.process)) {
            if let Some(a) = &n.attached_to {
                boundaries_of
                    .entry(a.clone())
                    .or_default()
                    .push(n.id.clone());
            }
            if n.kind == NodeKind::StartEvent {
                if let Some(p) = &n.parent {
                    sub_starts.entry(p.clone()).or_default().push(n.id.clone());
                }
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
            link_catch,
            signal_starts,
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
        while let Some(Reverse((Time(t), _, ev))) = self.queue.pop() {
            self.now = t;
            match ev {
                Ev::Arrival(g) => self.on_arrival(g),
                Ev::TaskDone(run) => self.on_task_done(run),
                Ev::Reply { group, flow } => self.deliver(group, flow),
                Ev::Timer(sub) => self.fire_sub(sub),
            }
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
            process,
            tokens: BTreeMap::new(),
            joins: BTreeMap::new(),
            done: false,
        });
        self.cases[case].open_instances += 1;
        inst
    }

    fn start_instance(&mut self, inst: usize, starts: Vec<QId>) {
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
            },
        );
        id
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
        while let Some((tok, node_id, via)) = work.pop() {
            // The token may have been cancelled (terminate, interrupting boundary) meanwhile.
            if !self.alive(inst, tok) {
                continue;
            }
            let m = self.m;
            let Some(node) = m.project.node(&node_id) else {
                continue;
            };
            let first_def = node.event_defs.first();
            match &node.kind {
                NodeKind::EndEvent => {
                    self.throw(inst, node);
                    if !self.alive(inst, tok) {
                        continue;
                    }
                    if matches!(first_def, Some(EventDef::Terminate)) {
                        self.terminate(inst, tok);
                    }
                    *self.end_events.entry(node_id.clone()).or_default() += 1;
                    self.consume(inst, tok, &mut work);
                }
                NodeKind::Task(TaskKind::Receive) => {
                    self.arm_boundaries(inst, tok, node);
                    self.wait_message(inst, tok, node, &mut work);
                }
                NodeKind::Task(_) => self.request_task(inst, tok, node),
                NodeKind::SubProcess => self.enter_scope(inst, tok, node, &mut work),
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
                        self.throw(inst, node);
                        if self.alive(inst, tok) {
                            self.route(inst, tok, node, &mut work);
                        }
                    }
                }
                NodeKind::ParallelGateway if node.incoming.len() > 1 => {
                    self.and_join(inst, tok, node, via, &mut work)
                }
                _ => {
                    self.set_token(inst, tok, &node_id, "passing");
                    self.route(inst, tok, node, &mut work);
                }
            }
        }
        self.check_completion(inst);
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
                self.instances[inst].tokens.remove(t);
            }
            self.set_token(inst, keep, &node.id, "passing");
            self.route(inst, keep, node, work);
        }
    }

    /// Send a token out of `node`: XOR picks one flow, everything else takes all flows.
    fn route(&mut self, inst: usize, tok: u64, node: &FlowNode, work: &mut Vec<Work>) {
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
            _ => node.outgoing.clone(),
        };
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
        let Some(t) = self.instances[inst].tokens.remove(&tok) else {
            return;
        };
        for s in t.subs {
            self.deactivate_sub(s);
        }
        if let Some(sc) = t.scope
            && !self.instances[inst]
                .tokens
                .values()
                .any(|x| x.scope == Some(sc))
        {
            let (parent, node_id) = (self.scopes[sc].parent, self.scopes[sc].node.clone());
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

    fn enter_scope(&mut self, inst: usize, tok: u64, node: &FlowNode, work: &mut Vec<Work>) {
        let sc = self.scopes.len();
        self.scopes.push(Scope {
            node: node.id.clone(),
            parent: tok,
        });
        self.set_token(inst, tok, &node.id, "inside embedded subprocess");
        if let Some(t) = self.instances[inst].tokens.get_mut(&tok) {
            t.child_scope = Some(sc);
        }
        self.arm_boundaries(inst, tok, node);
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
        let starts: Vec<QId> = m
            .project
            .nodes()
            .filter(|n| {
                n.process == *process
                    && n.parent.is_none()
                    && n.kind == NodeKind::StartEvent
                    && n.event_defs.iter().all(|d| matches!(d, EventDef::None))
            })
            .map(|n| n.id.clone())
            .collect();
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
        self.instances[inst].tokens.remove(&tok);
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

    fn timer_sub(&mut self, inst: usize, owner: u64, node: &QId, boundary: bool, delay: f64) {
        let s = self.add_sub(inst, owner, node, boundary, SubKind::Timer);
        self.schedule(self.now + delay, Ev::Timer(s));
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
                Some(EventDef::Timer | EventDef::Error | EventDef::Escalation) => {
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

    /// A subscription's trigger happened.
    fn fire_sub(&mut self, s: usize) {
        if !self.subs[s].active {
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
        let r = &mut self.runs[id];
        r.start = self.now;
        r.duration = duration;
        r.state = RunState::Running;
        let (inst, tok) = (r.instance, r.token);
        self.set_token(inst, tok, &node, "working");
        self.schedule(self.now + duration, Ev::TaskDone(id));
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
            activity: node_id.clone(),
            resource: cfg.pool.clone(),
            queued,
            start,
            end: self.now,
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
        if node.kind == NodeKind::Task(TaskKind::Send) {
            self.throw(inst, node);
        }
        let mut work = Vec::new();
        if self.alive(inst, tok) {
            self.route(inst, tok, node, &mut work);
        }
        self.process(inst, work);
    }

    /// Cut a task short (interrupting boundary event, terminate end event).
    fn cancel_run(&mut self, id: usize) {
        let state = self.runs[id].state;
        if !matches!(state, RunState::Queued | RunState::Running) {
            return;
        }
        self.runs[id].state = RunState::Cancelled;
        let m = self.m;
        let (inst, node_id, queued, start) = {
            let r = &self.runs[id];
            (r.instance, r.node.clone(), r.queued, r.start)
        };
        let Some(cfg) = m.activities.get(&node_id) else {
            return;
        };
        let (worked, begun) = match state {
            RunState::Running => (self.now - start, start),
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
            activity: node_id,
            resource: cfg.pool.clone(),
            queued,
            start: begun,
            end: self.now,
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
            } else if target.kind == NodeKind::StartEvent {
                if self.cases[case].completion.is_none() {
                    self.log_message(group, f, MessageOutcome::StartedInstance);
                    let inst = self.create_instance(case, group, target.process.clone(), None);
                    self.start_instance(inst, vec![target.id.clone()]);
                    return;
                }
            } else if self.m.buffer_unconsumed && target.kind != NodeKind::BoundaryEvent {
                // (A boundary event only listens while its activity runs: never buffered.)
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

    fn check_completion(&mut self, inst: usize) {
        let i = &mut self.instances[inst];
        if i.done || !i.tokens.is_empty() {
            return;
        }
        i.done = true;
        i.finished = Some(self.now);
        let (case, parent) = (i.case, i.parent);
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
        let resources = self
            .pools
            .iter()
            .map(|(name, p)| {
                let def = &self.m.pools[name];
                let available = f64::from(def.count) * end_time;
                (
                    name.clone(),
                    ResourceStats {
                        count: def.count,
                        tasks: p.tasks,
                        busy_time: p.busy_time,
                        utilisation: if available > 0.0 {
                            p.busy_time / available
                        } else {
                            0.0
                        },
                        cost: p.busy_time / 3600.0 * def.cost_per_hour,
                    },
                )
            })
            .collect();

        // A token sitting at an embedded subprocess is not reported itself: its inner tokens are.
        let stuck = self
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
            processes,
            cases,
            records: self.records,
        }
    }
}
