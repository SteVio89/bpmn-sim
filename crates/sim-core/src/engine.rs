//! Discrete-event engine (spec §5). One `Engine` is one replication.
//!
//! Determinism: events are ordered by `(time, sequence number)`, all maps are `BTreeMap`s and
//! all randomness comes from one seeded ChaCha generator.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::model::*;
use crate::scenario::BoundModel;
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
}

struct Instance {
    case: usize,
    process: QId,
    tokens: BTreeMap<u64, Token>,
    /// AND join node -> incoming flow -> waiting tokens (FIFO).
    joins: BTreeMap<QId, BTreeMap<QId, VecDeque<u64>>>,
    done: bool,
}

struct TaskRun {
    instance: usize,
    token: u64,
    node: QId,
    queued: f64,
    start: f64,
    duration: f64,
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
    next_token: u64,
    runs: Vec<TaskRun>,
    pools: BTreeMap<String, Pool>,
    records: Vec<ActivityRecord>,
    end_events: BTreeMap<QId, u64>,
}

/// Run all replications; replication `i` uses seed `seed + i`.
pub fn run_replications(m: &BoundModel) -> Vec<ReplicationResult> {
    (0..u64::from(m.sim.replications))
        .map(|i| Engine::new(m, m.sim.seed.wrapping_add(i)).run())
        .collect()
}

impl<'a> Engine<'a> {
    pub fn new(m: &'a BoundModel<'a>, seed: u64) -> Self {
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
            next_token: 0,
            runs: Vec::new(),
            pools: m
                .pools
                .keys()
                .map(|k| (k.clone(), Pool::default()))
                .collect(),
            records: Vec::new(),
            end_events: BTreeMap::new(),
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
        for (process, starts) in per_process {
            let inst = self.instances.len();
            self.instances.push(Instance {
                case,
                process,
                tokens: BTreeMap::new(),
                joins: BTreeMap::new(),
                done: false,
            });
            self.cases[case].open_instances += 1;
            let mut work = Vec::new();
            for s in starts.into_iter().rev() {
                let tok = self.new_token(inst, &s, "starting");
                work.push((tok, s, None));
            }
            self.process(inst, work);
        }

        let gap = self.m.arrivals[g].inter_arrival.sample(&mut self.rng);
        self.schedule(self.now + gap, Ev::Arrival(g));
    }

    fn new_token(&mut self, inst: usize, node: &QId, state: &str) -> u64 {
        let id = self.next_token;
        self.next_token += 1;
        self.instances[inst].tokens.insert(
            id,
            Token {
                node: node.clone(),
                state: state.into(),
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

    /// Move tokens through zero-time elements until they wait or end.
    fn process(&mut self, inst: usize, mut work: Vec<Work>) {
        while let Some((tok, node_id, via)) = work.pop() {
            let m = self.m;
            let Some(node) = m.project.node(&node_id) else {
                continue;
            };
            match &node.kind {
                NodeKind::EndEvent => {
                    self.instances[inst].tokens.remove(&tok);
                    *self.end_events.entry(node_id.clone()).or_default() += 1;
                }
                NodeKind::Task(_) => self.request_task(inst, tok, node),
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
            self.instances[inst].tokens.remove(&tok);
            return;
        }
        let mut next = Vec::new();
        for (i, f) in flows.iter().enumerate() {
            let Some(target) = self.m.project.flow(f).map(|fl| fl.target.clone()) else {
                continue;
            };
            let t = if i == 0 {
                tok
            } else {
                self.new_token(inst, &node.id, "passing")
            };
            next.push((t, target, Some(f.clone())));
        }
        // Reverse so the first flow is processed first (work is a stack).
        work.extend(next.into_iter().rev());
    }

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
        });
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
        self.runs[id].start = self.now;
        self.runs[id].duration = duration;
        let (inst, tok) = (self.runs[id].instance, self.runs[id].token);
        self.set_token(inst, tok, &node, "working");
        self.schedule(self.now + duration, Ev::TaskDone(id));
    }

    fn on_task_done(&mut self, id: usize) {
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
        });

        // Release the resource and hand it to the next waiting task (FIFO) before moving on.
        if let Some(p) = &cfg.pool {
            let next = self.pools.get_mut(p).and_then(|pool| {
                pool.busy -= 1;
                pool.busy_time += duration;
                pool.tasks += 1;
                pool.queue.pop_front()
            });
            if let Some(n) = next {
                self.start_task(n);
            }
        }

        let Some(node) = m.project.node(&node_id) else {
            return;
        };
        let mut work = Vec::new();
        self.route(inst, tok, node, &mut work);
        self.process(inst, work);
    }

    fn check_completion(&mut self, inst: usize) {
        let i = &mut self.instances[inst];
        if i.done || !i.tokens.is_empty() {
            return;
        }
        i.done = true;
        let case = i.case;
        let c = &mut self.cases[case];
        c.open_instances -= 1;
        if c.open_instances == 0 {
            c.completion = Some(self.now);
            self.completion_order.push(case);
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
                let waits: Vec<f64> = rs.iter().map(|r| r.waiting()).collect();
                let procs: Vec<f64> = rs.iter().map(|r| r.processing()).collect();
                (
                    id.clone(),
                    ActivityStats {
                        runs: rs.len(),
                        waiting: Summary::of(&waits),
                        processing: Summary::of(&procs),
                        total_cost: rs.iter().map(|r| r.cost).sum(),
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

        let stuck = self
            .instances
            .iter()
            .flat_map(|inst| {
                inst.tokens.values().map(move |t| StuckToken {
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
            cases,
            records: self.records,
        }
    }
}
