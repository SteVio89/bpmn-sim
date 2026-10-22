//! Result types and statistics of one replication (spec §6).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::QId;

/// Descriptive statistics. Percentiles use linear interpolation between closest ranks
/// (R type 7). Everything is 0 when `count` is 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Summary {
    pub count: usize,
    pub mean: f64,
    pub std_dev: f64,
    pub min: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
}

impl Summary {
    pub fn of(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut v = values.to_vec();
        v.sort_by(f64::total_cmp);
        let n = v.len() as f64;
        let mean = v.iter().sum::<f64>() / n;
        let std_dev = if v.len() > 1 {
            (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
        } else {
            0.0
        };
        Self {
            count: v.len(),
            mean,
            std_dev,
            min: v[0],
            median: percentile(&v, 0.5),
            p90: percentile(&v, 0.9),
            max: v[v.len() - 1],
        }
    }
}

/// `sorted` must be non-empty and ascending.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// One finished task execution; the basis of the event log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityRecord {
    pub case: u64,
    pub instance: u64,
    pub activity: QId,
    pub resource: Option<String>,
    /// Time the token arrived at the task.
    pub queued: f64,
    pub start: f64,
    pub end: f64,
    pub cost: f64,
}

impl ActivityRecord {
    pub fn waiting(&self) -> f64 {
        self.start - self.queued
    }
    pub fn processing(&self) -> f64 {
        self.end - self.start
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseRecord {
    pub id: u64,
    pub arrival: f64,
    pub completion: Option<f64>,
    pub cost: f64,
    /// Among the first `warm_up_cases` completed cases; left out of the statistics.
    pub warm_up: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityStats {
    pub runs: usize,
    pub waiting: Summary,
    pub processing: Summary,
    pub total_cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceStats {
    pub count: u32,
    pub tasks: u64,
    pub busy_time: f64,
    /// busy time / (count * observation window)
    pub utilisation: f64,
    pub cost: f64,
}

/// A token still alive when the run ended (spec §5.8).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StuckToken {
    pub case: u64,
    pub process: QId,
    pub element: QId,
    pub name: Option<String>,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplicationResult {
    pub seed: u64,
    /// Observation window: time of the last event processed.
    pub end_time: f64,
    pub cases_started: u64,
    pub cases_completed: u64,
    pub case_cycle_time: Summary,
    pub case_cost: Summary,
    pub activities: BTreeMap<QId, ActivityStats>,
    pub resources: BTreeMap<String, ResourceStats>,
    /// Tokens consumed per end event.
    pub end_events: BTreeMap<QId, u64>,
    pub stuck: Vec<StuckToken>,
    pub cases: Vec<CaseRecord>,
    pub records: Vec<ActivityRecord>,
}
