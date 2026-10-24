//! Data for the diagram view: heat-map values per element and token positions for replay
//! (spec §7.5). Drawing itself lives in the GUI.

use std::collections::BTreeMap;

use crate::model::QId;
use crate::project::Project;
use crate::stats::ReplicationResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeatMetric {
    /// Mean waiting time per activity (seconds).
    Waiting,
    /// Mean processing time per activity (seconds).
    Processing,
    /// Completed runs per activity.
    Runs,
    /// Utilisation of the resource pool an activity (or lane) uses.
    Utilisation,
}

/// Heat value per element of `file`: activities, and for utilisation also lanes and pools whose
/// name is a resource pool. Subprocesses and call activities show what happens inside them per
/// run: the waiting and working time of their tasks summed, how often they ran, and the busiest
/// resource pool their tasks used.
pub fn heat(
    project: &Project,
    file: &str,
    r: &ReplicationResult,
    metric: HeatMetric,
) -> BTreeMap<QId, f64> {
    let mut out = BTreeMap::new();
    let in_file = |q: &QId| q.file == file;
    match metric {
        HeatMetric::Waiting | HeatMetric::Processing | HeatMetric::Runs => {
            for (id, a) in r.activities.iter().filter(|(id, _)| in_file(id)) {
                let v = match metric {
                    HeatMetric::Waiting => a.waiting.mean,
                    HeatMetric::Processing => a.processing.mean,
                    _ => a.runs as f64,
                };
                out.insert(id.clone(), v);
            }
            for (id, s) in r.scopes.iter().filter(|(id, _)| in_file(id)) {
                let v = match metric {
                    HeatMetric::Waiting => s.waiting.mean,
                    HeatMetric::Processing => s.processing.mean,
                    _ => s.runs as f64,
                };
                out.insert(id.clone(), v);
            }
        }
        HeatMetric::Utilisation => {
            for (id, s) in r.scopes.iter().filter(|(id, _)| in_file(id)) {
                let busiest = s
                    .pools
                    .iter()
                    .filter_map(|p| r.resources.get(p))
                    .map(|res| res.utilisation)
                    .fold(None, |m: Option<f64>, u| Some(m.map_or(u, |m| m.max(u))));
                if let Some(u) = busiest {
                    out.insert(id.clone(), u);
                }
            }
            // Which pool each activity used, from the event log.
            let mut pool_of: BTreeMap<&QId, &str> = BTreeMap::new();
            for x in r.records.iter().filter(|x| in_file(&x.activity)) {
                if let Some(p) = &x.resource {
                    pool_of.entry(&x.activity).or_insert(p);
                }
            }
            for (id, pool) in pool_of {
                if let Some(res) = r.resources.get(pool) {
                    out.insert(id.clone(), res.utilisation);
                }
            }
            if let Some(f) = project.files.get(file) {
                for (lane, name) in &f.lanes {
                    if let Some(res) = r.resources.get(name) {
                        out.insert(lane.clone(), res.utilisation);
                    }
                }
                for p in f.participants.values() {
                    if let Some(res) = p.name.as_ref().and_then(|n| r.resources.get(n)) {
                        out.insert(p.id.clone(), res.utilisation);
                    }
                }
            }
        }
    }
    out
}

/// Tokens at activities at time `t`, from the event log: (waiting for a resource, working).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Occupancy {
    pub waiting: usize,
    pub working: usize,
}

pub fn snapshot(r: &ReplicationResult, t: f64) -> BTreeMap<QId, Occupancy> {
    let mut out: BTreeMap<QId, Occupancy> = BTreeMap::new();
    for x in &r.records {
        if x.queued <= t && t < x.start {
            out.entry(x.activity.clone()).or_default().waiting += 1;
        } else if x.start <= t && t < x.end {
            out.entry(x.activity.clone()).or_default().working += 1;
        }
    }
    out
}

/// Where a traced token is at a moment of the replay.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenPlace {
    /// Travelling along a sequence flow (`fraction` 0..1 of the way).
    OnFlow { flow: QId, fraction: f64 },
    /// At a task, waiting for its resource.
    Waiting { node: QId },
    /// At a task, being worked on.
    Working { node: QId },
    /// At any other element (event, gateway, call activity, subprocess).
    At { node: QId },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenView {
    pub case: u64,
    pub instance: u64,
    pub token: u64,
    pub place: TokenPlace,
    /// When the token last moved (reached the element it is at or travelling to).
    pub since: f64,
}

/// Positions of the tokens of `cases` at time `t`, from the token trace. Moving from one element
/// to the next takes no simulated time; for display a token is shown travelling along the flow
/// during `travel` seconds after it arrived. A token leaving a join is held there until every
/// token it merged has been drawn arriving, so it never runs ahead of its own inputs.
pub fn token_positions(
    r: &ReplicationResult,
    cases: &std::collections::BTreeSet<u64>,
    t: f64,
    travel: f64,
) -> Vec<TokenView> {
    use crate::stats::{TraceEvent, TraceKind};
    struct Seen<'a> {
        arrive: &'a TraceEvent,
        /// The arrival before `arrive`.
        prev: Option<&'a TraceEvent>,
        gone: Option<f64>,
    }
    // Last arrival and whether the token is gone, per (instance, token).
    let mut last: BTreeMap<(u64, u64), Seen> = BTreeMap::new();
    for e in r.trace.iter().take_while(|e| e.time <= t) {
        if !cases.contains(&e.case) {
            continue;
        }
        let key = (e.instance, e.token);
        match &e.kind {
            TraceKind::Arrive { .. } => {
                let prev = last.get(&key).map(|s| s.arrive);
                last.insert(
                    key,
                    Seen {
                        arrive: e,
                        prev,
                        gone: None,
                    },
                );
            }
            TraceKind::Gone => {
                if let Some(entry) = last.get_mut(&key) {
                    entry.gone = Some(e.time);
                }
            }
        }
    }
    fn node_of(e: &TraceEvent) -> Option<&QId> {
        match &e.kind {
            TraceKind::Arrive { node, .. } => Some(node),
            TraceKind::Gone => None,
        }
    }
    // When the token is shown leaving the element it arrived from: at a join that merged other
    // tokens just then, not before the last of them (and itself) was drawn reaching it.
    let departs = |(instance, token): (u64, u64), s: &Seen| {
        let Some(prev) = s.prev else {
            return s.arrive.time;
        };
        let merged = last
            .iter()
            .filter(|((i, k), o)| {
                *i == instance
                    && *k != token
                    && o.gone == Some(s.arrive.time)
                    && node_of(o.arrive) == node_of(prev)
            })
            .map(|(_, o)| o.arrive.time + travel)
            .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));
        merged.map_or(s.arrive.time, |m| {
            m.max(prev.time + travel).max(s.arrive.time)
        })
    };
    let mut out = Vec::new();
    for (&(instance, token), s) in &last {
        let leaves = departs((instance, token), s);
        // Still held at the join: shown as it was after its previous arrival.
        let (e, since, gone) = match s.prev {
            Some(prev) if t < leaves => (prev, prev.time, None),
            _ => (s.arrive, leaves, s.gone),
        };
        let TraceKind::Arrive { node, via } = &e.kind else {
            continue;
        };
        let travelling = via.as_ref().filter(|_| t - since < travel);
        let place = match travelling {
            Some(flow) => TokenPlace::OnFlow {
                flow: flow.clone(),
                fraction: if travel > 0.0 {
                    (t - since) / travel
                } else {
                    1.0
                },
            },
            None if gone.is_some() => continue,
            None => {
                let rec = r.records.iter().find(|x| {
                    x.instance == instance
                        && x.token == token
                        && x.activity == *node
                        && x.queued == e.time
                });
                match rec {
                    Some(x) if t < x.start => TokenPlace::Waiting { node: node.clone() },
                    Some(x) if t < x.end => TokenPlace::Working { node: node.clone() },
                    _ => TokenPlace::At { node: node.clone() },
                }
            }
        };
        out.push(TokenView {
            case: e.case,
            instance,
            token,
            place,
            since,
        });
    }
    out
}

/// Messages of `cases` shown travelling along their message flow at time `t`: (flow, fraction).
pub fn messages_in_flight(
    r: &ReplicationResult,
    cases: &std::collections::BTreeSet<u64>,
    t: f64,
    travel: f64,
) -> Vec<(QId, f64)> {
    r.messages
        .iter()
        .filter(|m| cases.contains(&m.case) && m.time <= t && t < m.time + travel)
        .map(|m| {
            (
                m.flow.clone(),
                if travel > 0.0 {
                    (t - m.time) / travel
                } else {
                    1.0
                },
            )
        })
        .collect()
}

/// Sequence flows of `file` drawn with a marker at their start: default flows (a slash) and
/// conditional flows leaving an activity or event (a small diamond; conditions after gateways
/// get no marker in BPMN).
pub fn flow_markers(
    project: &Project,
    file: &str,
) -> (
    std::collections::BTreeSet<QId>,
    std::collections::BTreeSet<QId>,
) {
    let mut defaults = std::collections::BTreeSet::new();
    let mut conditional = std::collections::BTreeSet::new();
    let Some(f) = project.files.get(file) else {
        return (defaults, conditional);
    };
    for n in f.nodes.values() {
        if let Some(d) = &n.default_flow {
            defaults.insert(d.clone());
        }
    }
    for flow in f.sequence_flows.values() {
        let from_gateway = f
            .nodes
            .get(&flow.source)
            .is_some_and(|n| n.kind.is_gateway());
        if flow.condition.is_some() && !from_gateway && !defaults.contains(&flow.id) {
            conditional.insert(flow.id.clone());
        }
    }
    (defaults, conditional)
}
