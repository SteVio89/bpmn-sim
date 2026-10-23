//! Statistics across replications: mean, standard deviation and a 95% confidence interval
//! (Student's t) for the most important figures (spec §6).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::model::QId;
use crate::stats::ReplicationResult;

/// Two-sided 97.5% quantile of Student's t distribution with `df` degrees of freedom.
pub fn t_975(df: usize) -> f64 {
    const TABLE: [f64; 30] = [
        12.7062, 4.3027, 3.1824, 2.7764, 2.5706, 2.4469, 2.3646, 2.3060, 2.2622, 2.2281, 2.2010,
        2.1788, 2.1604, 2.1448, 2.1314, 2.1199, 2.1098, 2.1009, 2.0930, 2.0860, 2.0796, 2.0739,
        2.0687, 2.0639, 2.0595, 2.0555, 2.0518, 2.0484, 2.0452, 2.0423,
    ];
    match df {
        0 => f64::INFINITY,
        1..=30 => TABLE[df - 1],
        _ => {
            // Cornish-Fisher expansion around the normal quantile (accurate to ~1e-4 for df > 30).
            let z = 1.959_964_f64;
            let v = df as f64;
            z + (z.powi(3) + z) / (4.0 * v)
                + (5.0 * z.powi(5) + 16.0 * z.powi(3) + 3.0 * z) / (96.0 * v * v)
        }
    }
}

/// Estimate of a quantity from independent replications.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Estimate {
    pub n: usize,
    pub mean: f64,
    pub std_dev: f64,
    /// Half width of the 95% confidence interval of the mean; `None` with fewer than 2 values.
    pub ci95: Option<f64>,
}

impl Estimate {
    pub fn of(values: &[f64]) -> Self {
        let n = values.len();
        if n == 0 {
            return Self::default();
        }
        let mean = values.iter().sum::<f64>() / n as f64;
        if n == 1 {
            return Self {
                n,
                mean,
                std_dev: 0.0,
                ci95: None,
            };
        }
        let var = values.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        let std_dev = var.sqrt();
        Self {
            n,
            mean,
            std_dev,
            ci95: Some(t_975(n - 1) * std_dev / (n as f64).sqrt()),
        }
    }

    pub fn low(&self) -> f64 {
        self.mean - self.ci95.unwrap_or(0.0)
    }

    pub fn high(&self) -> f64 {
        self.mean + self.ci95.unwrap_or(0.0)
    }

    /// Is `x` inside the confidence interval?
    pub fn contains(&self, x: f64) -> bool {
        self.ci95.is_some() && (self.low()..=self.high()).contains(&x)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityEstimate {
    pub runs: Estimate,
    pub cancelled: Estimate,
    pub waiting_mean: Estimate,
    pub processing_mean: Estimate,
    pub total_cost: Estimate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResourceEstimate {
    pub utilisation: Estimate,
    pub busy_time: Estimate,
    pub cost: Estimate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AggregateResult {
    pub replications: usize,
    pub cases_started: Estimate,
    pub cases_completed: Estimate,
    pub case_cycle_time_mean: Estimate,
    pub case_cycle_time_median: Estimate,
    pub case_cycle_time_p90: Estimate,
    pub case_cost_mean: Estimate,
    pub activities: BTreeMap<QId, ActivityEstimate>,
    pub resources: BTreeMap<String, ResourceEstimate>,
    pub process_cycle_time_mean: BTreeMap<QId, Estimate>,
    pub end_events: BTreeMap<QId, Estimate>,
    pub messages_lost: Estimate,
    pub stuck_tokens: Estimate,
}

pub fn aggregate(rs: &[ReplicationResult]) -> AggregateResult {
    let est =
        |f: &dyn Fn(&ReplicationResult) -> f64| Estimate::of(&rs.iter().map(f).collect::<Vec<_>>());
    // Quantities of elements that did not occur in a replication count as 0 (runs, costs, ...);
    // averages (waiting, cycle times) only use replications where the element occurred.
    let zero_filled = |f: &dyn Fn(&ReplicationResult) -> Option<f64>| {
        Estimate::of(&rs.iter().map(|r| f(r).unwrap_or(0.0)).collect::<Vec<_>>())
    };
    let present = |f: &dyn Fn(&ReplicationResult) -> Option<f64>| {
        Estimate::of(&rs.iter().filter_map(f).collect::<Vec<_>>())
    };

    let activity_ids: BTreeSet<&QId> = rs.iter().flat_map(|r| r.activities.keys()).collect();
    let activities = activity_ids
        .into_iter()
        .map(|id| {
            (
                id.clone(),
                ActivityEstimate {
                    runs: zero_filled(&|r| r.activities.get(id).map(|a| a.runs as f64)),
                    cancelled: zero_filled(&|r| r.activities.get(id).map(|a| a.cancelled as f64)),
                    waiting_mean: present(&|r| {
                        r.activities
                            .get(id)
                            .filter(|a| a.waiting.count > 0)
                            .map(|a| a.waiting.mean)
                    }),
                    processing_mean: present(&|r| {
                        r.activities
                            .get(id)
                            .filter(|a| a.processing.count > 0)
                            .map(|a| a.processing.mean)
                    }),
                    total_cost: zero_filled(&|r| r.activities.get(id).map(|a| a.total_cost)),
                },
            )
        })
        .collect();
    let pool_names: BTreeSet<&String> = rs.iter().flat_map(|r| r.resources.keys()).collect();
    let resources = pool_names
        .into_iter()
        .map(|n| {
            (
                n.clone(),
                ResourceEstimate {
                    utilisation: zero_filled(&|r| r.resources.get(n).map(|p| p.utilisation)),
                    busy_time: zero_filled(&|r| r.resources.get(n).map(|p| p.busy_time)),
                    cost: zero_filled(&|r| r.resources.get(n).map(|p| p.cost)),
                },
            )
        })
        .collect();
    let process_ids: BTreeSet<&QId> = rs.iter().flat_map(|r| r.processes.keys()).collect();
    let process_cycle_time_mean = process_ids
        .into_iter()
        .map(|id| {
            (
                id.clone(),
                present(&|r| {
                    r.processes
                        .get(id)
                        .filter(|p| p.instances > 0)
                        .map(|p| p.cycle_time.mean)
                }),
            )
        })
        .collect();
    let end_ids: BTreeSet<&QId> = rs.iter().flat_map(|r| r.end_events.keys()).collect();
    let end_events = end_ids
        .into_iter()
        .map(|id| {
            (
                id.clone(),
                zero_filled(&|r| r.end_events.get(id).map(|n| *n as f64)),
            )
        })
        .collect();

    AggregateResult {
        replications: rs.len(),
        cases_started: est(&|r| r.cases_started as f64),
        cases_completed: est(&|r| r.cases_completed as f64),
        case_cycle_time_mean: present(&|r| {
            (r.case_cycle_time.count > 0).then_some(r.case_cycle_time.mean)
        }),
        case_cycle_time_median: present(&|r| {
            (r.case_cycle_time.count > 0).then_some(r.case_cycle_time.median)
        }),
        case_cycle_time_p90: present(&|r| {
            (r.case_cycle_time.count > 0).then_some(r.case_cycle_time.p90)
        }),
        case_cost_mean: present(&|r| (r.case_cost.count > 0).then_some(r.case_cost.mean)),
        activities,
        resources,
        process_cycle_time_mean,
        end_events,
        messages_lost: est(&|r| r.messages_lost as f64),
        stuck_tokens: est(&|r| r.stuck.len() as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_quantiles_match_tables() {
        assert!((t_975(1) - 12.7062).abs() < 1e-4);
        assert!((t_975(9) - 2.2622).abs() < 1e-4);
        assert!((t_975(30) - 2.0423).abs() < 1e-4);
        // Beyond the table: published values 2.0211 (40), 2.0003 (60), 1.9799 (120).
        assert!((t_975(40) - 2.0211).abs() < 5e-4, "{}", t_975(40));
        assert!((t_975(60) - 2.0003).abs() < 5e-4, "{}", t_975(60));
        assert!((t_975(120) - 1.9799).abs() < 5e-4, "{}", t_975(120));
    }

    #[test]
    fn estimate_of_known_values() {
        let e = Estimate::of(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!((e.n, e.mean), (5, 3.0));
        assert!((e.std_dev - 2.5_f64.sqrt()).abs() < 1e-12);
        // t(4) = 2.7764: half width = 2.7764 * 1.5811 / sqrt(5)
        assert!((e.ci95.unwrap() - 2.7764 * 2.5_f64.sqrt() / 5.0_f64.sqrt()).abs() < 1e-9);
        assert!(e.contains(3.0) && !e.contains(10.0));
        assert_eq!(Estimate::of(&[7.0]).ci95, None);
        assert_eq!(Estimate::of(&[]).n, 0);
    }
}
