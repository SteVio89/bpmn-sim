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
/// name is a resource pool.
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
        }
        HeatMetric::Utilisation => {
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
