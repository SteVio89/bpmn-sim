//! What the numbers of one replication mean: where the cases spend their time, whether the
//! process keeps up with its arrivals, and which resource pool holds it up. The figures in
//! `ReplicationResult` answer "how much"; this module turns them into findings a modeller
//! can act on ("add a clerk", "the queue keeps growing"). See `docs/results.md`.

use std::collections::BTreeMap;

use crate::model::QId;
use crate::stats::{ReplicationResult, Summary};

/// Below this utilisation a pool is mostly idle.
pub const IDLE: f64 = 0.3;
/// From this utilisation on waiting times rise steeply (queueing theory: the wait of an
/// M/M/1 queue is ρ/(1-ρ) service times, 4 at 80%, 9 at 90%).
pub const HIGH: f64 = 0.8;
/// From this utilisation on a pool is practically always busy.
pub const SATURATED: f64 = 0.95;

/// Number of points in the time series (work in progress, queue lengths).
const SERIES_POINTS: usize = 200;

/// One case: where its time went.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseRow {
    pub id: u64,
    pub arrival: f64,
    pub completion: Option<f64>,
    pub failed: bool,
    pub warm_up: bool,
    pub cost: f64,
    /// Number of task executions (including cancelled ones).
    pub tasks: usize,
    /// Working time of its tasks, summed.
    pub work: f64,
    /// Time its tasks waited before they started (for a free resource, or for working hours
    /// to begin), summed.
    pub wait: f64,
    /// Time its tasks were paused outside working hours, summed.
    pub paused: f64,
    /// The task where the case waited longest, and how long.
    pub longest_wait: Option<(QId, f64)>,
}

impl CaseRow {
    pub fn cycle_time(&self) -> Option<f64> {
        self.completion.map(|c| c - self.arrival)
    }

    /// Completed and not part of the warm-up: what the statistics count.
    pub fn counted(&self) -> bool {
        self.completion.is_some() && !self.warm_up
    }
}

/// Where an average completed case spent its time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimeBreakdown {
    /// Completed cases (after the warm-up) these means are taken over.
    pub cases: usize,
    pub cycle: f64,
    pub work: f64,
    pub wait: f64,
    pub paused: f64,
    /// The rest of the cycle time: timers, messages, other pools, intermediate events,
    /// tasks without a duration. 0 when parallel branches overlap.
    pub other: f64,
    /// Tasks ran in parallel: work, wait and pauses add up to more than the cycle time.
    pub parallel: bool,
}

/// How the cycle time develops over the run: the mean of the earliest and of the latest
/// third of the completed cases, in order of arrival.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trend {
    pub early: f64,
    pub late: f64,
}

impl Trend {
    /// Later cases take much longer than earlier ones: the queues keep growing, the process
    /// does not keep up with the arrivals.
    pub fn growing(&self) -> bool {
        self.late > 1.5 * self.early
    }
}

/// How heavily a resource pool is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Load {
    Idle,
    Moderate,
    High,
    Saturated,
}

impl Load {
    pub fn of(utilisation: f64) -> Self {
        if utilisation >= SATURATED {
            Load::Saturated
        } else if utilisation >= HIGH {
            Load::High
        } else if utilisation >= IDLE {
            Load::Moderate
        } else {
            Load::Idle
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Load::Idle => "mostly idle",
            Load::Moderate => "moderate",
            Load::High => "high: waits rise steeply",
            Load::Saturated => "overloaded",
        }
    }
}

/// One resource pool: how busy it was and how much waiting it caused.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolInsight {
    pub name: String,
    pub count: u32,
    pub utilisation: f64,
    pub load: Load,
    /// Completed tasks.
    pub tasks: u64,
    /// Waiting time of the completed tasks of this pool (after the warm-up).
    pub wait: Summary,
    /// Total waiting time of those tasks.
    pub total_wait: f64,
    /// Average number of tasks waiting for the pool over the run (Little's law).
    pub mean_queue: f64,
    pub max_queue: usize,
    /// Tasks waiting for the pool over time: (time, most waiting at once in that stretch).
    pub queue: Vec<[f64; 2]>,
    /// Activities that use the pool, with their completed runs.
    pub activities: Vec<(QId, usize)>,
}

impl PoolInsight {
    /// Utilisation with `count` people instead, for the same work.
    pub fn utilisation_with(&self, count: u32) -> f64 {
        if count == 0 {
            f64::INFINITY
        } else {
            self.utilisation * f64::from(self.count) / f64::from(count)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warning,
    Problem,
}

/// What a finding is about, so each result tab can show its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Cases,
    Resources,
    Problems,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub level: Level,
    pub topic: Topic,
    pub text: String,
}

/// Everything derived from one replication.
#[derive(Debug, Clone, PartialEq)]
pub struct Insights {
    pub cases: Vec<CaseRow>,
    pub time: TimeBreakdown,
    /// `None` with fewer than 30 completed cases.
    pub trend: Option<Trend>,
    /// Cases in progress over time: (time, most open at once in that stretch).
    pub wip: Vec<[f64; 2]>,
    /// Pools in name order.
    pub pools: Vec<PoolInsight>,
    /// The pool the cases waited for longest in total, if they waited noticeably.
    pub bottleneck: Option<String>,
}

impl Insights {
    pub fn of(r: &ReplicationResult) -> Self {
        let cases = case_rows(r);
        let time = breakdown(&cases);
        let trend = trend(&cases);
        let mut wip_events: Vec<(f64, i32)> = Vec::with_capacity(cases.len() * 2);
        let failed_at: BTreeMap<u64, f64> = r.failed.iter().map(|f| (f.case, f.time)).collect();
        for c in &cases {
            wip_events.push((c.arrival, 1));
            if let Some(end) = c.completion.or_else(|| failed_at.get(&c.id).copied()) {
                wip_events.push((end, -1));
            }
        }
        let wip = level_over_time(wip_events, r.end_time).0;

        let mut pools: Vec<PoolInsight> = r
            .resources
            .iter()
            .map(|(name, p)| PoolInsight {
                name: name.clone(),
                count: p.count,
                utilisation: p.utilisation,
                load: Load::of(p.utilisation),
                tasks: p.tasks,
                wait: Summary::default(),
                total_wait: 0.0,
                mean_queue: 0.0,
                max_queue: 0,
                queue: Vec::new(),
                activities: Vec::new(),
            })
            .collect();
        for pool in &mut pools {
            let mut waits = Vec::new();
            let mut events = Vec::new();
            let mut activities: BTreeMap<&QId, usize> = BTreeMap::new();
            let mut all_wait = 0.0;
            for rec in r
                .records
                .iter()
                .filter(|x| x.resource.as_deref() == Some(pool.name.as_str()))
            {
                // The queue counts every task, warm-up included, like the utilisation.
                if rec.start > rec.queued {
                    events.push((rec.queued, 1));
                    events.push((rec.start, -1));
                    all_wait += rec.waiting();
                }
                let warm = r.cases.get(rec.case as usize).is_some_and(|c| c.warm_up);
                if !rec.cancelled && !warm {
                    waits.push(rec.waiting());
                    *activities.entry(&rec.activity).or_default() += 1;
                }
            }
            pool.total_wait = waits.iter().sum();
            pool.wait = Summary::of(&waits);
            pool.mean_queue = if r.end_time > 0.0 {
                all_wait / r.end_time
            } else {
                0.0
            };
            (pool.queue, pool.max_queue) = level_over_time(events, r.end_time);
            pool.activities = activities
                .into_iter()
                .map(|(a, n)| (a.clone(), n))
                .collect();
        }

        // The bottleneck must account for a real share of the case time, not a few seconds.
        let total_cycle = time.cycle * time.cases as f64;
        let bottleneck = pools
            .iter()
            .filter(|p| p.total_wait > 0.05 * total_cycle && p.total_wait > 0.0)
            .max_by(|a, b| a.total_wait.total_cmp(&b.total_wait))
            .map(|p| p.name.clone());

        Self {
            cases,
            time,
            trend,
            wip,
            pools,
            bottleneck,
        }
    }

    pub fn pool(&self, name: &str) -> Option<&PoolInsight> {
        self.pools.iter().find(|p| p.name == name)
    }

    /// The findings in plain language, problems first.
    pub fn findings(&self, r: &ReplicationResult) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut add = |level, topic, text: String| out.push(Finding { level, topic, text });
        let started = r.cases_started;
        let failed = r.failed.len() as u64;
        let open = started.saturating_sub(r.cases_completed + failed);

        if failed > 0 {
            add(
                Level::Problem,
                Topic::Problems,
                format!(
                    "{failed} of {started} cases failed through an incident (a gateway with no \
                     way out or an uncaught error). See Problems for where."
                ),
            );
        }
        if !r.stuck.is_empty() {
            add(
                Level::Problem,
                Topic::Problems,
                format!(
                    "{} token(s) got stuck and never finished: the model has a deadlock or \
                     waits for something that never comes. See Problems for where.",
                    r.stuck.len()
                ),
            );
        }
        if let Some(t) = self.trend.filter(Trend::growing) {
            let fix = match &self.bottleneck {
                Some(b) => format!("Add people to {b} or lower the arrival rate."),
                None => "Add capacity or lower the arrival rate.".into(),
            };
            add(
                Level::Problem,
                Topic::Cases,
                format!(
                    "The process does not keep up with the arrivals: the latest cases took \
                     {} on average, the earliest {}. Queues keep growing, so the cycle times \
                     depend on how long you simulate and do not settle. {fix}",
                    duration(t.late),
                    duration(t.early)
                ),
            );
        }
        if r.horizon_reached && open > 0 {
            add(
                Level::Warning,
                Topic::Cases,
                format!(
                    "{open} of {started} cases ({:.0}%) were still open when the run stopped \
                     at the horizon. Their (long) cycle times are not in the statistics.",
                    100.0 * open as f64 / started.max(1) as f64
                ),
            );
        }

        let s = &r.case_cycle_time;
        if s.count > 0 {
            add(
                Level::Info,
                Topic::Cases,
                format!(
                    "Half of the {} completed cases took up to {}, 9 in 10 up to {}, the \
                     slowest {}.",
                    s.count,
                    duration(s.median),
                    duration(s.p90),
                    duration(s.max)
                ),
            );
        }
        let t = &self.time;
        if t.cases > 0 && t.cycle > 0.0 {
            let pct = |x: f64| 100.0 * x / t.cycle;
            let mut parts = vec![format!(
                "worked on for {} ({:.0}%)",
                duration(t.work),
                pct(t.work)
            )];
            if t.wait > 0.005 * t.cycle {
                parts.push(format!(
                    "waited {} ({:.0}%) for a free resource or for working hours",
                    duration(t.wait),
                    pct(t.wait)
                ));
            }
            if t.paused > 0.005 * t.cycle {
                parts.push(format!(
                    "was paused {} ({:.0}%) outside working hours",
                    duration(t.paused),
                    pct(t.paused)
                ));
            }
            if t.other > 0.01 * t.cycle {
                parts.push(format!(
                    "spent {} ({:.0}%) on timers, messages, other pools or tasks without \
                     a resource",
                    duration(t.other),
                    pct(t.other)
                ));
            }
            let mut text = format!(
                "An average case took {}: it was {}.",
                duration(t.cycle),
                join(&parts)
            );
            if t.parallel {
                text.push_str(
                    " (Tasks ran in parallel, so the parts add up to more than the cycle time.)",
                );
            }
            add(Level::Info, Topic::Cases, text);
            if t.wait > 0.5 * t.cycle {
                let at = match &self.bottleneck {
                    Some(b) => format!(", most of it for {b}"),
                    None => String::new(),
                };
                add(
                    Level::Warning,
                    Topic::Cases,
                    format!(
                        "Cases spend most of their time waiting{at}. More capacity there \
                         shortens the cycle time more than faster work does."
                    ),
                );
            }
        }

        for p in &self.pools {
            let share = |x: f64| {
                let all: f64 = self.pools.iter().map(|p| p.total_wait).sum();
                if all > 0.0 { 100.0 * x / all } else { 0.0 }
            };
            let is_bottleneck = self.bottleneck.as_deref() == Some(p.name.as_str());
            let more = p.count + 1;
            // An overloaded pool did less work than it was given: the estimate is a floor.
            let plus_one = format!(
                "With {more} instead of {} it would be {}{:.0}% busy.",
                p.count,
                if p.load == Load::Saturated {
                    "at least "
                } else {
                    "about "
                },
                100.0 * p.utilisation_with(more)
            );
            match p.load {
                Load::Saturated | Load::High => add(
                    if p.load == Load::Saturated || is_bottleneck {
                        Level::Warning
                    } else {
                        Level::Info
                    },
                    Topic::Resources,
                    format!(
                        "{} is {:.0}% busy ({}){}: tasks waited {} on average for it, \
                         at most {} at once. {plus_one}",
                        p.name,
                        100.0 * p.utilisation,
                        p.load.label(),
                        if is_bottleneck {
                            format!(
                                ", the bottleneck with {:.0}% of all waiting",
                                share(p.total_wait)
                            )
                        } else {
                            String::new()
                        },
                        duration(p.wait.mean),
                        p.max_queue
                    ),
                ),
                _ if is_bottleneck => add(
                    Level::Info,
                    Topic::Resources,
                    format!(
                        "Cases wait longest for {} ({:.0}% of all waiting, {} per task on \
                         average); it is {:.0}% busy. {plus_one}",
                        p.name,
                        share(p.total_wait),
                        duration(p.wait.mean),
                        100.0 * p.utilisation
                    ),
                ),
                Load::Idle if p.count > 1 => {
                    let fewer = p.count - 1;
                    let u = p.utilisation_with(fewer);
                    if u < HIGH {
                        add(
                            Level::Info,
                            Topic::Resources,
                            format!(
                                "{} is busy only {:.0}% of its working time. {fewer} instead \
                                 of {} would be about {:.0}% busy.",
                                p.name,
                                100.0 * p.utilisation,
                                p.count,
                                100.0 * u
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
        out.sort_by_key(|f| std::cmp::Reverse(f.level));
        out
    }
}

/// One row per case with its tasks summed up.
pub fn case_rows(r: &ReplicationResult) -> Vec<CaseRow> {
    let mut rows: Vec<CaseRow> = r
        .cases
        .iter()
        .map(|c| CaseRow {
            id: c.id,
            arrival: c.arrival,
            completion: c.completion,
            failed: c.failed,
            warm_up: c.warm_up,
            cost: c.cost,
            tasks: 0,
            work: 0.0,
            wait: 0.0,
            paused: 0.0,
            longest_wait: None,
        })
        .collect();
    for rec in &r.records {
        let Some(row) = rows.get_mut(rec.case as usize) else {
            continue;
        };
        let wait = rec.waiting();
        row.tasks += 1;
        row.work += rec.work;
        row.wait += wait;
        row.paused += (rec.end - rec.start - rec.work).max(0.0);
        if wait > 0.0 && row.longest_wait.as_ref().is_none_or(|(_, w)| wait > *w) {
            row.longest_wait = Some((rec.activity.clone(), wait));
        }
    }
    rows
}

fn breakdown(cases: &[CaseRow]) -> TimeBreakdown {
    let counted: Vec<&CaseRow> = cases.iter().filter(|c| c.counted()).collect();
    if counted.is_empty() {
        return TimeBreakdown::default();
    }
    let n = counted.len() as f64;
    let mean = |f: fn(&CaseRow) -> f64| counted.iter().map(|c| f(c)).sum::<f64>() / n;
    let cycle = mean(|c| c.cycle_time().unwrap_or(0.0));
    let work = mean(|c| c.work);
    let wait = mean(|c| c.wait);
    let paused = mean(|c| c.paused);
    let rest = cycle - work - wait - paused;
    // Rounding leaves tiny negative rests in purely sequential models.
    let parallel = rest < -1e-6 * cycle.max(1.0);
    TimeBreakdown {
        cases: counted.len(),
        cycle,
        work,
        wait,
        paused,
        other: rest.max(0.0),
        parallel,
    }
}

fn trend(cases: &[CaseRow]) -> Option<Trend> {
    // Cases are numbered in order of arrival.
    let cycles: Vec<f64> = cases
        .iter()
        .filter(|c| c.counted())
        .filter_map(CaseRow::cycle_time)
        .collect();
    if cycles.len() < 30 {
        return None;
    }
    let third = cycles.len() / 3;
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    Some(Trend {
        early: mean(&cycles[..third]),
        late: mean(&cycles[cycles.len() - third..]),
    })
}

/// A count that changes by `(time, +1/-1)` events, as the highest value in each of
/// `SERIES_POINTS` equal stretches of `[0, end]`, plus the highest value overall.
fn level_over_time(mut events: Vec<(f64, i32)>, end: f64) -> (Vec<[f64; 2]>, usize) {
    events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let end = events.last().map_or(end, |e| e.0.max(end));
    if end <= 0.0 {
        return (Vec::new(), 0);
    }
    let width = end / SERIES_POINTS as f64;
    let mut peaks = vec![0usize; SERIES_POINTS];
    let (mut level, mut max) = (0i64, 0usize);
    let mut bucket = 0;
    for (t, d) in events {
        let b = ((t / width) as usize).min(SERIES_POINTS - 1);
        // The level holds over the stretches between two events.
        while bucket < b {
            bucket += 1;
            peaks[bucket] = peaks[bucket].max(level.max(0) as usize);
        }
        level += i64::from(d);
        let l = level.max(0) as usize;
        peaks[b] = peaks[b].max(l);
        max = max.max(l);
    }
    // Cases or tasks still open at the end stay open until the end.
    for p in &mut peaks[bucket + 1..] {
        *p = level.max(0) as usize;
    }
    let series = peaks
        .iter()
        .enumerate()
        .map(|(i, &p)| [(i as f64 + 0.5) * width, p as f64])
        .collect();
    (series, max)
}

/// A duration in seconds in a readable unit ("45 s", "12.5 min", "3.2 h", "4.1 days").
pub fn duration(seconds: f64) -> String {
    let s = seconds.abs();
    let (v, u) = if s < 60.0 {
        (seconds, "s")
    } else if s < 3600.0 {
        (seconds / 60.0, "min")
    } else if s < 2.0 * 86_400.0 {
        (seconds / 3600.0, "h")
    } else {
        (seconds / 86_400.0, "days")
    };
    if v.abs() >= 100.0 {
        format!("{v:.0} {u}")
    } else {
        format!("{v:.1} {u}")
    }
}

fn join(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(30.0), "30.0 s");
        assert_eq!(duration(90.0), "1.5 min");
        assert_eq!(duration(5400.0), "1.5 h");
        assert_eq!(duration(86_400.0 * 3.0), "3.0 days");
        assert_eq!(duration(86_400.0 * 365.0), "365 days");
    }

    #[test]
    fn level_follows_the_events() {
        let (s, max) = level_over_time(vec![(0.0, 1), (10.0, 1), (20.0, -1), (30.0, -1)], 100.0);
        assert_eq!(max, 2);
        assert_eq!(s.len(), SERIES_POINTS);
        // Between 10 and 20 two are open; after 30 none.
        let at = |t: f64| s.iter().find(|p| p[0] >= t).unwrap()[1];
        assert_eq!(at(5.0), 1.0);
        assert_eq!(at(15.0), 2.0);
        assert_eq!(at(25.0), 1.0);
        assert_eq!(at(50.0), 0.0);
    }

    #[test]
    fn load_thresholds() {
        assert_eq!(Load::of(0.1), Load::Idle);
        assert_eq!(Load::of(0.5), Load::Moderate);
        assert_eq!(Load::of(0.85), Load::High);
        assert_eq!(Load::of(0.99), Load::Saturated);
    }

    #[test]
    fn lists_are_joined() {
        let s = |v: &[&str]| join(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(s(&["a"]), "a");
        assert_eq!(s(&["a", "b"]), "a and b");
        assert_eq!(s(&["a", "b", "c"]), "a, b, and c");
    }
}
