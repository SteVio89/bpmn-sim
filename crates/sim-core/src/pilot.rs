//! Pilot run: a short single replication of a scenario that gives quick feedback while it is
//! being configured (load per resource pool, cycle time, traffic per flow).

use std::collections::BTreeMap;

use crate::engine::run_replications;
use crate::model::QId;
use crate::project::Project;
use crate::scenario::Scenario;
use crate::stats::{Summary, TraceKind};

/// Cases simulated by a pilot run.
pub const PILOT_CASES: u64 = 300;

#[derive(Debug, Clone, PartialEq)]
pub struct PoolLoad {
    pub count: u32,
    /// Work arriving per unit of capacity while cases arrive (open hours only, with a
    /// calendar); above 1 the queue keeps growing.
    pub load: f64,
    /// Share of time busy over the whole run.
    pub utilisation: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pilot {
    pub cases: u64,
    pub completed: u64,
    pub cycle_time: Summary,
    pub pools: BTreeMap<String, PoolLoad>,
    /// Sequence flow -> how many tokens passed it.
    pub flows: BTreeMap<QId, u64>,
    /// Tokens stuck at the end.
    pub stuck: usize,
}

impl Pilot {
    /// Pools whose load is above 1 (they cannot keep up with the arrivals).
    pub fn overloaded(&self) -> impl Iterator<Item = (&String, &PoolLoad)> {
        self.pools.iter().filter(|(_, p)| p.load > 1.0)
    }
}

/// The scenario cut down to a pilot run: one replication of at most `PILOT_CASES` cases, no
/// warm-up and no horizon, every case traced (for the flow counts).
pub fn pilot_scenario(s: &Scenario) -> Scenario {
    let mut p = s.clone();
    let sim = &mut p.simulation;
    sim.replications = 1;
    sim.cases = Some(sim.cases.map_or(PILOT_CASES, |c| c.min(PILOT_CASES)));
    sim.horizon = None;
    sim.warm_up_cases = 0;
    sim.trace_cases = PILOT_CASES;
    p
}

/// Run the pilot of `scenario`; `Err` if the scenario does not bind.
pub fn pilot(project: &Project, scenario: &Scenario) -> Result<Pilot, String> {
    let bound = pilot_scenario(scenario)
        .bind(project)
        .map_err(|e| e.to_string())?;
    let r = run_replications(&bound).remove(0);
    let (first, last) = r
        .cases
        .iter()
        .map(|c| c.arrival)
        .fold((f64::MAX, f64::MIN), |(a, b), t| (a.min(t), b.max(t)));
    let mut work: BTreeMap<&str, f64> = BTreeMap::new();
    for rec in &r.records {
        if let Some(p) = &rec.resource {
            *work.entry(p.as_str()).or_default() += rec.work;
        }
    }
    let pools = r
        .resources
        .iter()
        .map(|(name, s)| {
            let def = &bound.pools[name];
            let open = match &def.calendar {
                Some(c) => c.open_between(first, last),
                None => last - first,
            };
            let capacity = open * f64::from(s.count);
            let demand = work.get(name.as_str()).copied().unwrap_or(0.0);
            let load = if capacity > 0.0 {
                demand / capacity
            } else {
                0.0
            };
            (
                name.clone(),
                PoolLoad {
                    count: s.count,
                    load,
                    utilisation: s.utilisation,
                },
            )
        })
        .collect();
    let mut flows = BTreeMap::new();
    for e in &r.trace {
        if let TraceKind::Arrive { via: Some(f), .. } = &e.kind {
            *flows.entry(f.clone()).or_default() += 1;
        }
    }
    Ok(Pilot {
        cases: r.cases_started,
        completed: r.cases_completed,
        cycle_time: r.case_cycle_time.clone(),
        pools,
        flows,
        stuck: r.stuck.len(),
    })
}
