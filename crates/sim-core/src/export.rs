//! CSV export of results and the simulated event log (spec §6). All times are seconds.

use std::collections::BTreeMap;

use crate::aggregate::AggregateResult;
use crate::calendar::SimClock;
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

/// One row per finished or cancelled task execution. Times are seconds since the simulation
/// start (`*_s`) and UTC timestamps.
pub fn event_log_csv(project: &Project, results: &[ReplicationResult]) -> String {
    let mut out = String::from(
        "replication_seed,case_id,instance_id,activity_id,activity_name,resource,queued_s,start_s,end_s,work_s,cost,cancelled,queued_time,start_time,end_time\n",
    );
    for r in results {
        let clock = SimClock {
            epoch: r.start_epoch,
        };
        for x in &r.records {
            out += &format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
                r.seed,
                x.case,
                x.instance,
                field(&x.activity.to_string()),
                field(&name_of(project, &x.activity)),
                field(x.resource.as_deref().unwrap_or("")),
                x.queued,
                x.start,
                x.end,
                x.work,
                x.cost,
                x.cancelled,
                clock.format(x.queued),
                clock.format(x.start),
                clock.format(x.end)
            );
        }
    }
    out
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The event log as XES (IEEE 1849): one trace per case and replication, a `start` and a
/// `complete` event per task execution (`ate_abort` instead of `complete` if it was cancelled;
/// a task cancelled before it started only has the abort event).
pub fn event_log_xes(project: &Project, results: &[ReplicationResult]) -> String {
    let mut out = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<log xes.version="1.0" xes.features="nested-attributes" xmlns="http://www.xes-standard.org/">
  <extension name="Concept" prefix="concept" uri="http://www.xes-standard.org/concept.xesext"/>
  <extension name="Time" prefix="time" uri="http://www.xes-standard.org/time.xesext"/>
  <extension name="Lifecycle" prefix="lifecycle" uri="http://www.xes-standard.org/lifecycle.xesext"/>
  <extension name="Organizational" prefix="org" uri="http://www.xes-standard.org/org.xesext"/>
  <global scope="trace"><string key="concept:name" value="__INVALID__"/></global>
  <global scope="event">
    <string key="concept:name" value="__INVALID__"/>
    <date key="time:timestamp" value="1970-01-01T00:00:00.000Z"/>
    <string key="lifecycle:transition" value="complete"/>
  </global>
  <classifier name="Activity" keys="concept:name"/>
"#,
    );
    for r in results {
        let clock = SimClock {
            epoch: r.start_epoch,
        };
        let mut by_case: BTreeMap<u64, Vec<(f64, usize, String)>> = BTreeMap::new();
        for (n, x) in r.records.iter().enumerate() {
            let name = xml(&name_of(project, &x.activity));
            let resource = x
                .resource
                .as_deref()
                .map(|r| format!("<string key=\"org:resource\" value=\"{}\"/>", xml(r)))
                .unwrap_or_default();
            let event = |transition: &str, t: f64| {
                format!(
                    "    <event><string key=\"concept:name\" value=\"{name}\"/><string key=\"lifecycle:transition\" value=\"{transition}\"/><date key=\"time:timestamp\" value=\"{}\"/>{resource}</event>\n",
                    clock.format(t)
                )
            };
            let events = by_case.entry(x.case).or_default();
            let began = !(x.cancelled && x.start >= x.end);
            if began {
                events.push((x.start, 2 * n, event("start", x.start)));
            }
            let end = if x.cancelled { "ate_abort" } else { "complete" };
            events.push((x.end, 2 * n + 1, event(end, x.end)));
        }
        for (case, mut events) in by_case {
            events.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            out += &format!(
                "  <trace>\n    <string key=\"concept:name\" value=\"{}-{case}\"/>\n",
                r.seed
            );
            for (_, _, e) in events {
                out += &e;
            }
            out += "  </trace>\n";
        }
    }
    out + "</log>\n"
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
        row("messages", "", "", "sent", r.messages.len() as f64);
        row("problems", "", "", "lost_messages", r.messages_lost as f64);
        row(
            "problems",
            "",
            "",
            "undelivered_messages",
            r.undelivered.iter().map(|u| u.count).sum::<usize>() as f64,
        );
        row("problems", "", "", "stuck_tokens", r.stuck.len() as f64);
    }
    out
}

/// Aggregated figures across replications: mean, standard deviation and the half width of the
/// 95% confidence interval.
pub fn aggregate_csv(project: &Project, agg: &AggregateResult) -> String {
    let mut out = String::from("scope,id,name,metric,n,mean,std_dev,ci95_half_width\n");
    let mut row =
        |scope: &str, id: &str, name: &str, metric: &str, e: &crate::aggregate::Estimate| {
            out += &format!(
                "{scope},{},{},{metric},{},{},{},{}\n",
                field(id),
                field(name),
                e.n,
                e.mean,
                e.std_dev,
                e.ci95.map(|c| c.to_string()).unwrap_or_default()
            );
        };
    row("cases", "", "", "started", &agg.cases_started);
    row("cases", "", "", "completed", &agg.cases_completed);
    row(
        "case_cycle_time",
        "",
        "",
        "mean_s",
        &agg.case_cycle_time_mean,
    );
    row(
        "case_cycle_time",
        "",
        "",
        "median_s",
        &agg.case_cycle_time_median,
    );
    row("case_cycle_time", "", "", "p90_s", &agg.case_cycle_time_p90);
    row("cases", "", "", "mean_cost", &agg.case_cost_mean);
    for (id, a) in &agg.activities {
        let (name, id) = (name_of(project, id), id.to_string());
        row("activity", &id, &name, "runs", &a.runs);
        row("activity", &id, &name, "cancelled", &a.cancelled);
        row("activity", &id, &name, "mean_wait_s", &a.waiting_mean);
        row(
            "activity",
            &id,
            &name,
            "mean_processing_s",
            &a.processing_mean,
        );
        row("activity", &id, &name, "total_cost", &a.total_cost);
    }
    for (name, p) in &agg.resources {
        row("resource", name, name, "utilisation", &p.utilisation);
        row("resource", name, name, "busy_time_s", &p.busy_time);
        row("resource", name, name, "cost", &p.cost);
    }
    for (id, e) in &agg.process_cycle_time_mean {
        row("process", &id.to_string(), "", "mean_cycle_time_s", e);
    }
    for (id, e) in &agg.end_events {
        row(
            "end_event",
            &id.to_string(),
            &name_of(project, id),
            "tokens",
            e,
        );
    }
    row("problems", "", "", "lost_messages", &agg.messages_lost);
    row("problems", "", "", "stuck_tokens", &agg.stuck_tokens);
    out
}
