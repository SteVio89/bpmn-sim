//! CSV export of results and the simulated event log (spec §6). All times are seconds.

use crate::project::Project;
use crate::stats::ReplicationResult;

fn field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn name_of(project: &Project, id: &crate::model::QId) -> String {
    project
        .node(id)
        .map(|n| n.label().to_string())
        .unwrap_or_default()
}

/// One row per finished task execution.
pub fn event_log_csv(project: &Project, results: &[ReplicationResult]) -> String {
    let mut out = String::from(
        "replication_seed,case_id,instance_id,activity_id,activity_name,resource,queued_s,start_s,end_s,cost\n",
    );
    for r in results {
        for x in &r.records {
            out += &format!(
                "{},{},{},{},{},{},{},{},{},{}\n",
                r.seed,
                x.case,
                x.instance,
                field(&x.activity.to_string()),
                field(&name_of(project, &x.activity)),
                field(x.resource.as_deref().unwrap_or("")),
                x.queued,
                x.start,
                x.end,
                x.cost
            );
        }
    }
    out
}

/// Tidy table: one row per (replication, scope, id, metric).
pub fn summary_csv(project: &Project, results: &[ReplicationResult]) -> String {
    let mut out = String::from("replication_seed,scope,id,name,metric,value\n");
    for r in results {
        let mut row = |scope: &str, id: &str, name: &str, metric: &str, value: f64| {
            out += &format!(
                "{},{scope},{},{},{metric},{value}\n",
                r.seed,
                field(id),
                field(name)
            );
        };
        row("run", "", "", "end_time_s", r.end_time);
        row("cases", "", "", "started", r.cases_started as f64);
        row("cases", "", "", "completed", r.cases_completed as f64);
        let c = &r.case_cycle_time;
        for (m, v) in [
            ("count", c.count as f64),
            ("mean_s", c.mean),
            ("std_dev_s", c.std_dev),
            ("min_s", c.min),
            ("median_s", c.median),
            ("p90_s", c.p90),
            ("max_s", c.max),
        ] {
            row("case_cycle_time", "", "", m, v);
        }
        row("cases", "", "", "mean_cost", r.case_cost.mean);
        for (id, a) in &r.activities {
            let name = name_of(project, id);
            let id = id.to_string();
            row("activity", &id, &name, "runs", a.runs as f64);
            row("activity", &id, &name, "mean_wait_s", a.waiting.mean);
            row(
                "activity",
                &id,
                &name,
                "mean_processing_s",
                a.processing.mean,
            );
            row("activity", &id, &name, "total_cost", a.total_cost);
        }
        for (name, p) in &r.resources {
            row("resource", name, name, "utilisation", p.utilisation);
            row("resource", name, name, "busy_time_s", p.busy_time);
            row("resource", name, name, "cost", p.cost);
        }
        for (id, n) in &r.end_events {
            row(
                "end_event",
                &id.to_string(),
                &name_of(project, id),
                "tokens",
                *n as f64,
            );
        }
        row("problems", "", "", "stuck_tokens", r.stuck.len() as f64);
    }
    out
}
