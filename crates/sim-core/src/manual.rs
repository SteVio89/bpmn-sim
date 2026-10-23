//! Manual token game: one case is played by hand, without time or resources. Everything that
//! needs no decision happens immediately; the player starts cases at start events, chooses the
//! outgoing flows of XOR / inclusive splits, decides which event of an event-based gateway
//! happens, fires catch events that nothing in the model triggers (timers, unsent messages,
//! signals) and boundary events, and, optionally, completes tasks.
//!
//! The semantics follow the timed engine (scopes, call activities, message groups, joins).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use crate::model::*;
use crate::project::Project;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenState {
    /// At a task, waiting to be completed by the player (only with `stop_at_tasks`).
    AtTask,
    /// At an XOR (`multiple == false`) or inclusive split, waiting for a choice.
    Choose {
        multiple: bool,
    },
    /// At a catch event or receive task.
    WaitMessage,
    WaitSignal(String),
    WaitTimer,
    /// At an event-based gateway.
    WaitEventGateway,
    /// At an AND or inclusive join.
    AtJoin,
    /// At an embedded subprocess / call activity while its content runs.
    InSubprocess,
    InCall,
    /// Stuck: an inclusive split or condition-less element without a way on.
    Passing,
}

#[derive(Debug, Clone)]
struct Token {
    inst: usize,
    node: QId,
    state: TokenState,
    scope: Option<usize>,
    child_scope: Option<usize>,
    child_inst: Option<usize>,
}

#[derive(Debug, Clone)]
struct Inst {
    process: QId,
    group: usize,
    parent: Option<(usize, u64)>,
    done: bool,
}

/// A token as shown to the player.
#[derive(Debug, Clone, PartialEq)]
pub struct GameToken {
    pub id: u64,
    pub instance: usize,
    pub process: QId,
    pub node: QId,
    pub state: TokenState,
}

/// Something the player can (or must) do.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    /// Choose the outgoing flow(s) of a split.
    Choose {
        token: u64,
        gateway: QId,
        flows: Vec<QId>,
        multiple: bool,
    },
    /// Complete the task the token is at.
    Complete { token: u64, task: QId },
    /// Let one of these events happen (the catch event itself, an event after an event-based
    /// gateway, or a boundary event of the activity the token is at).
    Fire {
        token: u64,
        at: QId,
        events: Vec<QId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Start { event: QId },
    Choose { token: u64, flows: Vec<QId> },
    Complete { token: u64 },
    Fire { token: u64, event: QId },
}

type Work = (u64, QId, Option<QId>);

#[derive(Debug, Clone)]
pub struct Game {
    project: Arc<Project>,
    /// Stop at every task until the player completes it.
    pub stop_at_tasks: bool,
    insts: Vec<Inst>,
    tokens: BTreeMap<u64, Token>,
    next_token: u64,
    /// Subprocess activations: (instance, waiting parent token, subprocess node).
    scopes: Vec<(usize, u64, QId)>,
    groups: usize,
    buffer: BTreeMap<(usize, QId), usize>,
    joins: BTreeMap<(usize, QId), BTreeMap<QId, VecDeque<u64>>>,
    or_reach: BTreeMap<QId, BTreeSet<QId>>,
    /// What happened, in words.
    pub log: Vec<String>,
}

impl Game {
    pub fn new(project: Arc<Project>) -> Self {
        let or_reach = project
            .nodes()
            .filter(|n| n.kind == NodeKind::InclusiveGateway && n.incoming.len() > 1)
            .map(|j| (j.id.clone(), crate::engine::reaching(&project, j)))
            .collect();
        Self {
            project,
            stop_at_tasks: false,
            insts: Vec::new(),
            tokens: BTreeMap::new(),
            next_token: 0,
            scopes: Vec::new(),
            groups: 1,
            buffer: BTreeMap::new(),
            joins: BTreeMap::new(),
            or_reach,
            log: Vec::new(),
        }
    }

    fn label(&self, id: &QId) -> String {
        self.project
            .node(id)
            .map_or_else(|| id.id.clone(), |n| n.label().to_string())
    }

    /// Start events the player can start a case (or another pool of it) with. Processes that
    /// a call activity calls are started by the call, not offered here.
    pub fn start_events(&self) -> Vec<QId> {
        let called: BTreeSet<&QId> = self
            .project
            .nodes()
            .filter_map(|n| n.called_element.as_ref())
            .filter_map(|c| self.project.processes.get(c))
            .collect();
        self.project
            .nodes()
            .filter(|n| {
                n.kind == NodeKind::StartEvent
                    && n.parent.is_none()
                    && !called.contains(&n.process)
                    && n.event_defs
                        .iter()
                        .all(|d| matches!(d, EventDef::None | EventDef::Timer))
            })
            .map(|n| n.id.clone())
            .collect()
    }

    pub fn tokens(&self) -> Vec<GameToken> {
        self.tokens
            .iter()
            .map(|(id, t)| GameToken {
                id: *id,
                instance: t.inst,
                process: self.insts[t.inst].process.clone(),
                node: t.node.clone(),
                state: t.state.clone(),
            })
            .collect()
    }

    /// Instances that have completed: (process, instance).
    pub fn finished(&self) -> Vec<(QId, usize)> {
        self.insts
            .iter()
            .enumerate()
            .filter(|(_, i)| i.done)
            .map(|(n, i)| (i.process.clone(), n))
            .collect()
    }

    pub fn pending(&self) -> Vec<Pending> {
        let mut out = Vec::new();
        for (id, t) in &self.tokens {
            let Some(n) = self.project.node(&t.node) else {
                continue;
            };
            let boundaries: Vec<QId> = self
                .project
                .nodes()
                .filter(|b| b.attached_to.as_ref() == Some(&t.node) && self.fireable(&b.id))
                .map(|b| b.id.clone())
                .collect();
            match &t.state {
                TokenState::Choose { multiple } => out.push(Pending::Choose {
                    token: *id,
                    gateway: t.node.clone(),
                    flows: n.outgoing.clone(),
                    multiple: *multiple,
                }),
                TokenState::AtTask => {
                    out.push(Pending::Complete {
                        token: *id,
                        task: t.node.clone(),
                    });
                    if !boundaries.is_empty() {
                        out.push(Pending::Fire {
                            token: *id,
                            at: t.node.clone(),
                            events: boundaries,
                        });
                    }
                }
                TokenState::WaitMessage | TokenState::WaitSignal(_) | TokenState::WaitTimer => {
                    let mut events: Vec<QId> = Some(t.node.clone())
                        .filter(|e| self.fireable(e))
                        .into_iter()
                        .collect();
                    events.extend(boundaries);
                    if !events.is_empty() {
                        out.push(Pending::Fire {
                            token: *id,
                            at: t.node.clone(),
                            events,
                        });
                    }
                }
                TokenState::WaitEventGateway => {
                    let events: Vec<QId> = self
                        .targets(n)
                        .into_iter()
                        .filter(|e| self.fireable(e))
                        .collect();
                    if !events.is_empty() {
                        out.push(Pending::Fire {
                            token: *id,
                            at: t.node.clone(),
                            events,
                        });
                    }
                }
                TokenState::InSubprocess | TokenState::InCall if !boundaries.is_empty() => out
                    .push(Pending::Fire {
                        token: *id,
                        at: t.node.clone(),
                        events: boundaries,
                    }),
                _ => {}
            }
        }
        out
    }

    /// Can the player make this event happen? Not for a message that the model itself sends
    /// (a message flow from an element of a modelled process); timers, signals, messages from
    /// black boxes or from nowhere, errors and escalations can be fired by hand.
    fn fireable(&self, event: &QId) -> bool {
        let Some(n) = self.project.node(event) else {
            return false;
        };
        let is_message = n.kind == NodeKind::Task(TaskKind::Receive)
            || n.event_defs
                .iter()
                .any(|d| matches!(d, EventDef::Message(_)));
        !is_message
            || !self
                .project
                .files
                .values()
                .flat_map(|f| f.message_flows.values())
                .any(|f| f.target == *event && self.project.node(&f.source).is_some())
    }

    fn targets(&self, n: &FlowNode) -> Vec<QId> {
        n.outgoing
            .iter()
            .filter_map(|f| self.project.flow(f).map(|f| f.target.clone()))
            .collect()
    }

    pub fn apply(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::Start { event } => {
                if !self.start_events().contains(&event) {
                    return Err(format!(
                        "{} is not a start event that can be started by hand",
                        event
                    ));
                }
                let process = self
                    .project
                    .node(&event)
                    .map(|n| n.process.clone())
                    .ok_or("unknown start event")?;
                self.log.push(format!("start '{}'", self.label(&event)));
                let inst = self.new_instance(process, 0, None);
                self.start_instance(inst, vec![event]);
                Ok(())
            }
            Action::Choose { token, flows } => {
                let t = self.tokens.get(&token).ok_or("no such token")?.clone();
                let TokenState::Choose { multiple } = t.state else {
                    return Err("this token is not waiting for a choice".into());
                };
                let n = self
                    .project
                    .node(&t.node)
                    .cloned()
                    .ok_or("unknown gateway")?;
                if flows.is_empty()
                    || (!multiple && flows.len() != 1)
                    || flows.iter().any(|f| !n.outgoing.contains(f))
                {
                    return Err(if multiple {
                        "choose one or more outgoing flows"
                    } else {
                        "choose exactly one outgoing flow"
                    }
                    .into());
                }
                let names: Vec<String> = flows.iter().map(|f| self.flow_label(f)).collect();
                self.log
                    .push(format!("'{}': take {}", n.label(), names.join(", ")));
                let mut work = Vec::new();
                self.route(token, &n, Some(flows), &mut work);
                self.process(t.inst, work);
                Ok(())
            }
            Action::Complete { token } => {
                let t = self.tokens.get(&token).ok_or("no such token")?.clone();
                if t.state != TokenState::AtTask {
                    return Err("this token is not at a task".into());
                }
                let n = self.project.node(&t.node).cloned().ok_or("unknown task")?;
                self.log.push(format!("complete '{}'", n.label()));
                self.leave_task(token, &n, t.inst);
                Ok(())
            }
            Action::Fire { token, event } => {
                let t = self.tokens.get(&token).ok_or("no such token")?.clone();
                let offered = self.pending().into_iter().any(|p| {
                    matches!(p, Pending::Fire { token: pt, events, .. } if pt == token && events.contains(&event))
                });
                if !offered {
                    return Err(format!(
                        "'{}' cannot be made to happen by hand now",
                        self.label(&event)
                    ));
                }
                let at = self
                    .project
                    .node(&t.node)
                    .cloned()
                    .ok_or("unknown element")?;
                let ev = self.project.node(&event).cloned().ok_or("unknown event")?;
                self.log.push(format!("'{}' happens", ev.label()));
                let mut work = Vec::new();
                if ev.attached_to.as_ref() == Some(&t.node) {
                    // Boundary event of the activity the token is at.
                    if ev.cancel_activity {
                        self.cancel_activity(token);
                        self.set(token, &event, TokenState::Passing);
                        work.push((token, event, None));
                    } else {
                        let nt = self.new_token(t.inst, &event, t.scope);
                        work.push((nt, event, None));
                    }
                } else if event == t.node
                    && matches!(
                        t.state,
                        TokenState::WaitMessage | TokenState::WaitSignal(_) | TokenState::WaitTimer
                    )
                {
                    self.route(token, &at, None, &mut work);
                } else if t.state == TokenState::WaitEventGateway
                    && self.targets(&at).contains(&event)
                {
                    self.set(token, &event, TokenState::Passing);
                    self.route(token, &ev, None, &mut work);
                } else {
                    return Err(format!("'{}' cannot happen for this token now", ev.label()));
                }
                self.process(t.inst, work);
                Ok(())
            }
        }
    }

    fn flow_label(&self, f: &QId) -> String {
        match self.project.flow(f) {
            Some(fl) => fl
                .name
                .clone()
                .unwrap_or_else(|| format!("to '{}'", self.label(&fl.target))),
            None => f.id.clone(),
        }
    }

    fn new_instance(&mut self, process: QId, group: usize, parent: Option<(usize, u64)>) -> usize {
        self.insts.push(Inst {
            process,
            group,
            parent,
            done: false,
        });
        self.insts.len() - 1
    }

    fn start_instance(&mut self, inst: usize, starts: Vec<QId>) {
        let mut work = Vec::new();
        for s in starts.into_iter().rev() {
            let t = self.new_token(inst, &s, None);
            work.push((t, s, None));
        }
        self.process(inst, work);
    }

    fn new_token(&mut self, inst: usize, node: &QId, scope: Option<usize>) -> u64 {
        let id = self.next_token;
        self.next_token += 1;
        self.tokens.insert(
            id,
            Token {
                inst,
                node: node.clone(),
                state: TokenState::Passing,
                scope,
                child_scope: None,
                child_inst: None,
            },
        );
        id
    }

    fn set(&mut self, tok: u64, node: &QId, state: TokenState) {
        if let Some(t) = self.tokens.get_mut(&tok) {
            t.node = node.clone();
            t.state = state;
        }
    }

    fn process(&mut self, inst: usize, mut work: Vec<Work>) {
        loop {
            while let Some((tok, node_id, via)) = work.pop() {
                if !self.tokens.contains_key(&tok) {
                    continue;
                }
                let Some(n) = self.project.node(&node_id).cloned() else {
                    continue;
                };
                let inst = self.tokens[&tok].inst;
                self.set(tok, &node_id, TokenState::Passing);
                let def = n.event_defs.first().cloned();
                match &n.kind {
                    NodeKind::EndEvent => {
                        self.throw(inst, &n);
                        if matches!(def, Some(EventDef::Terminate)) {
                            self.terminate(tok);
                            self.log.push(format!("terminate at '{}'", n.label()));
                        }
                        self.consume(tok, &mut work);
                    }
                    NodeKind::Task(TaskKind::Receive) => self.wait_message(tok, &n, &mut work),
                    NodeKind::Task(_) => {
                        if self.stop_at_tasks {
                            self.set(tok, &node_id, TokenState::AtTask);
                        } else {
                            if n.kind == NodeKind::Task(TaskKind::Send) {
                                self.throw(inst, &n);
                            }
                            self.route(tok, &n, None, &mut work);
                        }
                    }
                    NodeKind::ExclusiveGateway | NodeKind::InclusiveGateway
                        if n.outgoing.len() > 1 && n.incoming.len() <= 1 =>
                    {
                        self.set(
                            tok,
                            &node_id,
                            TokenState::Choose {
                                multiple: n.kind == NodeKind::InclusiveGateway,
                            },
                        );
                    }
                    NodeKind::ParallelGateway | NodeKind::InclusiveGateway
                        if n.incoming.len() > 1 =>
                    {
                        self.set(tok, &node_id, TokenState::AtJoin);
                        if let Some(f) = via {
                            self.joins
                                .entry((inst, node_id.clone()))
                                .or_default()
                                .entry(f)
                                .or_default()
                                .push_back(tok);
                        }
                        if n.kind == NodeKind::ParallelGateway {
                            self.and_join(inst, &n, &mut work);
                        }
                    }
                    NodeKind::ExclusiveGateway if n.outgoing.len() > 1 => {
                        // An XOR gateway that joins and splits: still a choice.
                        self.set(tok, &node_id, TokenState::Choose { multiple: false });
                    }
                    NodeKind::EventBasedGateway => {
                        // A message already waiting for one of the events decides at once.
                        let group = self.insts[inst].group;
                        let targets = self.targets(&n);
                        let buffered = targets
                            .iter()
                            .find(|t| {
                                self.buffer
                                    .get(&(group, (*t).clone()))
                                    .is_some_and(|c| *c > 0)
                            })
                            .cloned();
                        match buffered {
                            Some(t) => {
                                if let Some(c) = self.buffer.get_mut(&(group, t.clone())) {
                                    *c -= 1;
                                }
                                if let Some(tn) = self.project.node(&t).cloned() {
                                    self.set(tok, &t, TokenState::Passing);
                                    self.route(tok, &tn, None, &mut work);
                                }
                            }
                            None => self.set(tok, &node_id, TokenState::WaitEventGateway),
                        }
                    }
                    NodeKind::IntermediateCatchEvent => match def {
                        Some(EventDef::Link(_)) => self.route(tok, &n, None, &mut work),
                        Some(EventDef::Signal(name)) => self.set(
                            tok,
                            &node_id,
                            TokenState::WaitSignal(name.unwrap_or_default()),
                        ),
                        Some(EventDef::Message(_)) => self.wait_message(tok, &n, &mut work),
                        _ => self.set(tok, &node_id, TokenState::WaitTimer),
                    },
                    NodeKind::IntermediateThrowEvent => {
                        if let Some(EventDef::Link(name)) = &def {
                            let target = self.project.nodes().find(|c| {
                                c.process == n.process
                                    && c.kind == NodeKind::IntermediateCatchEvent
                                    && c.event_defs
                                        .iter()
                                        .any(|d| matches!(d, EventDef::Link(l) if l == name))
                            });
                            match target {
                                Some(c) => work.push((tok, c.id.clone(), None)),
                                None => self.consume(tok, &mut work),
                            }
                        } else {
                            self.throw(inst, &n);
                            if self.tokens.contains_key(&tok) {
                                self.route(tok, &n, None, &mut work);
                            }
                        }
                    }
                    NodeKind::SubProcess => {
                        let sc = self.scopes.len();
                        self.scopes.push((inst, tok, node_id.clone()));
                        self.set(tok, &node_id, TokenState::InSubprocess);
                        if let Some(t) = self.tokens.get_mut(&tok) {
                            t.child_scope = Some(sc);
                        }
                        let starts: Vec<QId> = self
                            .project
                            .nodes()
                            .filter(|c| {
                                c.parent.as_ref() == Some(&node_id)
                                    && c.kind == NodeKind::StartEvent
                            })
                            .map(|c| c.id.clone())
                            .collect();
                        for s in starts.into_iter().rev() {
                            let nt = self.new_token(inst, &s, Some(sc));
                            work.push((nt, s, None));
                        }
                    }
                    NodeKind::CallActivity => {
                        let target = n
                            .called_element
                            .as_ref()
                            .and_then(|c| self.project.processes.get(c))
                            .cloned();
                        match target {
                            None => {
                                self.log.push(format!(
                                    "'{}' calls a process that is not loaded; passing on",
                                    n.label()
                                ));
                                self.route(tok, &n, None, &mut work);
                            }
                            Some(p) => {
                                let starts: Vec<QId> = self
                                    .project
                                    .nodes()
                                    .filter(|c| {
                                        c.process == p
                                            && c.parent.is_none()
                                            && c.kind == NodeKind::StartEvent
                                            && c.event_defs
                                                .iter()
                                                .all(|d| matches!(d, EventDef::None))
                                    })
                                    .map(|c| c.id.clone())
                                    .collect();
                                let group = self.groups;
                                self.groups += 1;
                                self.set(tok, &node_id, TokenState::InCall);
                                let child = self.new_instance(p, group, Some((inst, tok)));
                                if let Some(t) = self.tokens.get_mut(&tok) {
                                    t.child_inst = Some(child);
                                }
                                self.start_instance(child, starts);
                            }
                        }
                    }
                    _ => self.route(tok, &n, None, &mut work),
                }
            }
            if !self.fire_or_joins(inst, &mut work) {
                break;
            }
        }
        self.check_instances();
    }

    fn wait_message(&mut self, tok: u64, n: &FlowNode, work: &mut Vec<Work>) {
        let group = self
            .tokens
            .get(&tok)
            .map_or(0, |t| self.insts[t.inst].group);
        match self.buffer.get_mut(&(group, n.id.clone())) {
            Some(c) if *c > 0 => {
                *c -= 1;
                self.route(tok, n, None, work);
            }
            _ => self.set(tok, &n.id, TokenState::WaitMessage),
        }
    }

    fn leave_task(&mut self, tok: u64, n: &FlowNode, inst: usize) {
        if n.kind == NodeKind::Task(TaskKind::Send) {
            self.throw(inst, n);
        }
        let mut work = Vec::new();
        if self.tokens.contains_key(&tok) {
            self.route(tok, n, None, &mut work);
        }
        self.process(inst, work);
    }

    /// Send a token along `flows` (or all outgoing flows).
    fn route(&mut self, tok: u64, n: &FlowNode, flows: Option<Vec<QId>>, work: &mut Vec<Work>) {
        let flows = flows.unwrap_or_else(|| n.outgoing.clone());
        if flows.is_empty() {
            self.consume(tok, work);
            return;
        }
        let Some(t) = self.tokens.get(&tok).cloned() else {
            return;
        };
        let mut next = Vec::new();
        for (i, f) in flows.iter().enumerate() {
            let Some(target) = self.project.flow(f).map(|fl| fl.target.clone()) else {
                continue;
            };
            let id = if i == 0 {
                tok
            } else {
                self.new_token(t.inst, &n.id, t.scope)
            };
            next.push((id, target, Some(f.clone())));
        }
        work.extend(next.into_iter().rev());
    }

    fn and_join(&mut self, inst: usize, n: &FlowNode, work: &mut Vec<Work>) {
        let key = (inst, n.id.clone());
        let ready = self.joins.get(&key).is_some_and(|w| {
            n.incoming
                .iter()
                .all(|f| w.get(f).is_some_and(|q| !q.is_empty()))
        });
        if ready {
            self.fire_join(&key, n, None, work);
        }
    }

    /// Consume one waiting token per incoming flow (of `scope`, if given); the first continues.
    fn fire_join(
        &mut self,
        key: &(usize, QId),
        n: &FlowNode,
        scope: Option<Option<usize>>,
        work: &mut Vec<Work>,
    ) {
        let mut firing = Vec::new();
        let tokens = &self.tokens;
        if let Some(w) = self.joins.get_mut(key) {
            for q in w.values_mut() {
                let pos = q
                    .iter()
                    .position(|t| scope.is_none_or(|s| tokens.get(t).map(|x| x.scope) == Some(s)));
                if let Some(t) = pos.and_then(|p| q.remove(p)) {
                    firing.push(t);
                }
            }
        }
        let Some((&keep, rest)) = firing.split_first() else {
            return;
        };
        for t in rest {
            self.tokens.remove(t);
        }
        if n.outgoing.len() > 1
            && matches!(
                n.kind,
                NodeKind::InclusiveGateway | NodeKind::ExclusiveGateway
            )
        {
            // A join that also splits: the player chooses.
            let multiple = n.kind == NodeKind::InclusiveGateway;
            self.set(keep, &n.id, TokenState::Choose { multiple });
        } else {
            self.route(keep, n, None, work);
        }
    }

    fn fire_or_joins(&mut self, inst: usize, work: &mut Vec<Work>) -> bool {
        let keys: Vec<(usize, QId)> = self
            .joins
            .iter()
            .filter(|((i, j), w)| {
                *i == inst && self.or_reach.contains_key(j) && w.values().any(|q| !q.is_empty())
            })
            .map(|(k, _)| k.clone())
            .collect();
        let mut fired = false;
        for key in keys {
            let Some(n) = self.project.node(&key.1).cloned() else {
                continue;
            };
            let Some(reach) = self.or_reach.get(&key.1).cloned() else {
                continue;
            };
            let waiting: BTreeSet<u64> = self.joins[&key].values().flatten().copied().collect();
            let scopes: BTreeSet<Option<usize>> = waiting
                .iter()
                .filter_map(|t| self.tokens.get(t).map(|x| x.scope))
                .collect();
            for scope in scopes {
                let blocked = self.tokens.iter().any(|(id, t)| {
                    t.inst == inst
                        && !waiting.contains(id)
                        && t.scope == scope
                        && reach.contains(&t.node)
                });
                if !blocked {
                    self.fire_join(&key, &n, Some(scope), work);
                    fired = true;
                }
            }
        }
        fired
    }

    /// Remove a token that reached its end; a subprocess completes with its last token.
    fn consume(&mut self, tok: u64, work: &mut Vec<Work>) {
        let Some(t) = self.tokens.remove(&tok) else {
            return;
        };
        if let Some(sc) = t.scope
            && !self.tokens.values().any(|x| x.scope == Some(sc))
        {
            let (_, parent, node) = self.scopes[sc].clone();
            if let Some(n) = self.project.node(&node).cloned()
                && self.tokens.contains_key(&parent)
            {
                if let Some(p) = self.tokens.get_mut(&parent) {
                    p.child_scope = None;
                }
                self.set(parent, &node, TokenState::Passing);
                self.route(parent, &n, None, work);
            }
        }
    }

    fn terminate(&mut self, tok: u64) {
        let Some(t) = self.tokens.get(&tok).cloned() else {
            return;
        };
        let victims: Vec<u64> = self
            .tokens
            .iter()
            .filter(|(id, x)| {
                **id != tok && x.inst == t.inst && (t.scope.is_none() || x.scope == t.scope)
            })
            .map(|(id, _)| *id)
            .collect();
        for v in victims {
            self.cancel_token(v);
        }
    }

    fn cancel_activity(&mut self, tok: u64) {
        let (child_scope, child_inst) = match self.tokens.get_mut(&tok) {
            Some(t) => (t.child_scope.take(), t.child_inst.take()),
            None => return,
        };
        if let Some(sc) = child_scope {
            let inner: Vec<u64> = self
                .tokens
                .iter()
                .filter(|(_, t)| t.scope == Some(sc))
                .map(|(id, _)| *id)
                .collect();
            for t in inner {
                self.cancel_token(t);
            }
        }
        if let Some(ci) = child_inst {
            let inner: Vec<u64> = self
                .tokens
                .iter()
                .filter(|(_, t)| t.inst == ci)
                .map(|(id, _)| *id)
                .collect();
            for t in inner {
                self.cancel_token(t);
            }
            self.insts[ci].done = true;
            self.insts[ci].parent = None;
        }
    }

    fn cancel_token(&mut self, tok: u64) {
        self.cancel_activity(tok);
        for w in self.joins.values_mut() {
            for q in w.values_mut() {
                q.retain(|t| *t != tok);
            }
        }
        self.tokens.remove(&tok);
    }

    /// Instances without tokens are complete; a called one releases its call activity.
    fn check_instances(&mut self) {
        loop {
            let finished: Vec<usize> = (0..self.insts.len())
                .filter(|i| !self.insts[*i].done && !self.tokens.values().any(|t| t.inst == *i))
                .collect();
            if finished.is_empty() {
                return;
            }
            for i in finished {
                self.insts[i].done = true;
                self.log.push(format!(
                    "instance of '{}' completed",
                    self.insts[i].process.id
                ));
                if let Some((pi, pt)) = self.insts[i].parent
                    && let Some(t) = self.tokens.get(&pt).cloned()
                    && let Some(n) = self.project.node(&t.node).cloned()
                {
                    if let Some(x) = self.tokens.get_mut(&pt) {
                        x.child_inst = None;
                    }
                    let mut work = Vec::new();
                    self.route(pt, &n, None, &mut work);
                    self.process(pi, work);
                }
            }
        }
    }

    /// Messages and signals of a throwing element (send task, throw / end event).
    fn throw(&mut self, inst: usize, n: &FlowNode) {
        let group = self.insts[inst].group;
        let is_message = n.kind == NodeKind::Task(TaskKind::Send)
            || n.event_defs
                .iter()
                .any(|d| matches!(d, EventDef::Message(_)));
        if is_message {
            let flows: Vec<MessageFlow> = self
                .project
                .files
                .values()
                .flat_map(|f| f.message_flows.values())
                .filter(|f| f.source == n.id)
                .cloned()
                .collect();
            for f in flows {
                self.deliver(group, &f);
            }
        }
        for d in &n.event_defs {
            if let EventDef::Signal(name) = d {
                self.signal(group, name.clone().unwrap_or_default());
            }
        }
    }

    fn deliver(&mut self, group: usize, f: &MessageFlow) {
        let Some(target) = self.project.node(&f.target).cloned() else {
            self.log
                .push(format!("message to black box '{}'", f.target.id));
            return;
        };
        let waiting = self.tokens.iter().find(|(_, t)| {
            self.insts[t.inst].group == group
                && ((t.node == f.target && t.state == TokenState::WaitMessage)
                    || (t.state == TokenState::WaitEventGateway
                        && self
                            .project
                            .node(&t.node)
                            .is_some_and(|g| self.targets(g).contains(&f.target))))
        });
        if let Some((&tok, t)) = waiting {
            let inst = t.inst;
            self.log
                .push(format!("message reaches '{}'", target.label()));
            let mut work = Vec::new();
            self.set(tok, &target.id, TokenState::Passing);
            self.route(tok, &target, None, &mut work);
            self.process(inst, work);
        } else if target.kind == NodeKind::StartEvent {
            self.log
                .push(format!("message starts '{}'", target.label()));
            let inst = self.new_instance(target.process.clone(), group, None);
            self.start_instance(inst, vec![target.id.clone()]);
        } else {
            self.log.push(format!(
                "message for '{}' is kept until it is waited for",
                target.label()
            ));
            *self.buffer.entry((group, target.id.clone())).or_default() += 1;
        }
    }

    fn signal(&mut self, group: usize, name: String) {
        self.log.push(format!("signal '{name}'"));
        let woken: Vec<(u64, QId)> = self
            .tokens
            .iter()
            .filter_map(|(id, t)| match &t.state {
                TokenState::WaitSignal(s) if *s == name => Some((*id, t.node.clone())),
                TokenState::WaitEventGateway => self.project.node(&t.node).and_then(|g| {
                    self.targets(g)
                        .into_iter()
                        .find(|x| self.project.node(x).is_some_and(|e| matches!(e.event_defs.first(), Some(EventDef::Signal(Some(s))) if *s == name)))
                        .map(|x| (*id, x))
                }),
                _ => None,
            })
            .collect();
        for (tok, node) in woken {
            let (Some(t), Some(n)) = (
                self.tokens.get(&tok).cloned(),
                self.project.node(&node).cloned(),
            ) else {
                continue;
            };
            let mut work = Vec::new();
            self.set(tok, &node, TokenState::Passing);
            self.route(tok, &n, None, &mut work);
            self.process(t.inst, work);
        }
        let starts: Vec<FlowNode> = self
            .project
            .nodes()
            .filter(|n| {
                n.kind == NodeKind::StartEvent
                    && n.parent.is_none()
                    && n.event_defs
                        .iter()
                        .any(|d| matches!(d, EventDef::Signal(Some(s)) if *s == name))
            })
            .cloned()
            .collect();
        for s in starts {
            let inst = self.new_instance(s.process.clone(), group, None);
            self.start_instance(inst, vec![s.id.clone()]);
        }
    }
}
