//! Token trace and the replay positions derived from it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use sim_core::diagram::{TokenPlace, messages_in_flight, token_positions};
use sim_core::*;

const MIN: f64 = 60.0;

fn run(fixture: &str, scenario: &str) -> ReplicationResult {
    let mut p = Project::new();
    p.add_path(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(fixture),
    )
    .unwrap();
    run_replications(
        &Scenario::from_toml(scenario)
            .unwrap()
            .bind(&p)
            .unwrap_or_else(|e| panic!("{e}")),
    )
    .remove(0)
}

const V5: &str = r#"
[simulation]
cases = 5
trace_cases = 2
[[arrivals]]
inter_arrival = { dist = "fixed", value = "60m" }
triggers = ["Start"]
[resources.TeamA]
count = 1
[resources.TeamB]
count = 1
[activities.Task_A]
duration = { dist = "fixed", value = "10m" }
[activities.Task_B]
duration = { dist = "fixed", value = "20m" }
"#;

fn ids(r: &ReplicationResult, case: u64) -> Vec<String> {
    r.trace
        .iter()
        .filter(|e| e.case == case)
        .map(|e| match &e.kind {
            TraceKind::Arrive { node, via } => format!(
                "{}@{}<{}",
                node.id,
                e.time / MIN,
                via.as_ref().map_or("-", |f| f.id.as_str())
            ),
            TraceKind::Gone => format!("gone@{}", e.time / MIN),
        })
        .collect()
}

#[test]
fn the_trace_records_every_step_of_the_first_cases() {
    let r = run("v5-and.bpmn", V5);
    assert_eq!(
        ids(&r, 0),
        [
            "Start@0<-",
            "Split@0<F0",
            "Task_A@0<F1",
            "Task_B@0<F2",
            "Join@10<F3",
            "Join@20<F4",
            "gone@20", // the token merged into the one that continues
            "End@20<F5",
            "gone@20",
        ]
    );
    assert_eq!(ids(&r, 1)[0], "Start@60<-");
    assert!(
        ids(&r, 2).is_empty(),
        "only trace_cases = 2 cases are traced"
    );
}

fn one(c: u64) -> BTreeSet<u64> {
    [c].into()
}

#[test]
fn positions_follow_the_trace() {
    let r = run("v5-and.bpmn", V5);
    let at = |t: f64, travel: f64| {
        let mut v: Vec<String> = token_positions(&r, &one(0), t * MIN, travel * MIN)
            .into_iter()
            .map(|x| match x.place {
                TokenPlace::OnFlow { flow, fraction } => format!("{}:{:.2}", flow.id, fraction),
                TokenPlace::Waiting { node } => format!("waiting {}", node.id),
                TokenPlace::Working { node } => format!("working {}", node.id),
                TokenPlace::At { node } => format!("at {}", node.id),
            })
            .collect();
        v.sort();
        v
    };
    // Right after the start both branch tokens are on their way to the tasks.
    assert_eq!(at(0.5, 1.0), ["F1:0.50", "F2:0.50"]);
    assert_eq!(at(5.0, 1.0), ["working Task_A", "working Task_B"]);
    // A waits at the join until B arrives.
    assert_eq!(at(15.0, 1.0), ["at Join", "working Task_B"]);
    // B is drawn running into the join while the merged token already leaves for the end;
    // after that only nothing is left.
    assert_eq!(at(20.5, 1.0), ["F4:0.50", "F5:0.50"]);
    assert!(at(22.0, 1.0).is_empty());
    assert!(at(-1.0, 1.0).is_empty());
    // Untraced cases have no positions.
    assert!(token_positions(&r, &one(3), 190.0 * MIN, MIN).is_empty());
}

#[test]
fn queued_tokens_are_shown_waiting() {
    // Cases every 5 minutes, one TeamA, 10 minute tasks: case 1 waits for Task_A.
    let r = run(
        "v5-and.bpmn",
        &V5.replace("value = \"60m\"", "value = \"5m\""),
    );
    let v = token_positions(&r, &one(1), 7.0 * MIN, 0.5 * MIN);
    assert!(
        v.iter()
            .any(|x| matches!(&x.place, TokenPlace::Waiting { node } if node.id == "Task_A")),
        "{v:?}"
    );
}

#[test]
fn messages_are_shown_in_flight() {
    let s = r#"
[simulation]
cases = 2
[[arrivals]]
inter_arrival = { dist = "fixed", value = "60m" }
triggers = ["Start_A", "Start_B"]
[resources.A]
count = 1
[resources.B]
count = 1
[activities.Task_Send]
duration = { dist = "fixed", value = "10m" }
[activities.Task_B]
duration = { dist = "fixed", value = "5m" }
"#;
    let r = run("v7-message.bpmn", s);
    let m = messages_in_flight(&r, &one(0), 10.5 * MIN, MIN);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].0.id, "MF1");
    assert!((m[0].1 - 0.5).abs() < 1e-9);
    assert!(messages_in_flight(&r, &one(0), 12.0 * MIN, MIN).is_empty());
}

#[test]
fn tracing_does_not_change_the_results() {
    let traced = run("v5-and.bpmn", V5);
    let untraced = run(
        "v5-and.bpmn",
        &V5.replace("trace_cases = 2", "trace_cases = 0"),
    );
    assert!(untraced.trace.is_empty());
    assert_eq!(traced.case_cycle_time, untraced.case_cycle_time);
    assert_eq!(traced.records, untraced.records);
}
