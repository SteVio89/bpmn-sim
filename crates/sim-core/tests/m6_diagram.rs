//! BPMNDI layout and the data behind the diagram view (M6).

use std::path::PathBuf;

use sim_core::diagram::{HeatMetric, heat, snapshot};
use sim_core::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn v13() -> Project {
    let mut p = Project::new();
    p.add_path(&fixture("v13-model.bpmn")).unwrap();
    p
}

fn q(id: &str) -> QId {
    QId::new("v13-model.bpmn", id)
}

#[test]
fn shapes_edges_and_lanes_are_read() {
    let p = v13();
    let f = &p.files["v13-model.bpmn"];
    let d = &f.diagram;
    // Pool, 2 lanes, 11 flow nodes; 12 sequence flows.
    assert_eq!(d.shapes.len(), 14);
    assert_eq!(d.edges.len(), 12);
    assert_eq!(
        d.shapes[&q("Task_Intake")],
        Bounds {
            x: 250.0,
            y: 110.0,
            width: 100.0,
            height: 80.0
        }
    );
    assert_eq!(d.edges[&q("F12")], vec![(1100.0, 340.0), (1180.0, 340.0)]);
    assert!(
        d.edges[&q("F11")].len() == 4,
        "elbow edge keeps all waypoints"
    );
    assert_eq!(f.lanes[&q("Lane_Clerks")], "Clerks");
    assert!(d.shapes.contains_key(&q("Part_Support")) && d.shapes.contains_key(&q("Lane_Experts")));
}

#[test]
fn files_without_a_diagram_have_an_empty_layout() {
    let mut p = Project::new();
    p.add_path(&fixture("v1-single-task.bpmn")).unwrap();
    assert_eq!(p.files["v1-single-task.bpmn"].diagram, Diagram::default());
    // Broken layout data is skipped, not fatal.
    let text = std::fs::read_to_string(fixture("v13-model.bpmn"))
        .unwrap()
        .replace(
            "width=\"100\" height=\"80\"/></bpmndi:BPMNShape>",
            "width=\"oops\" height=\"80\"/></bpmndi:BPMNShape>",
        );
    let mut p = Project::new();
    p.add_source("v13-model.bpmn", &text).unwrap();
    assert!(p.files["v13-model.bpmn"].diagram.shapes.len() < 14);
}

fn run() -> (Project, ReplicationResult) {
    let p = v13();
    let s = std::fs::read_to_string(fixture("v13-model.toml"))
        .unwrap()
        .replace("replications = 30", "replications = 1")
        .replace("cases = 5000", "cases = 300")
        .replace("warm_up_cases = 500", "warm_up_cases = 0");
    let r = run_replications(&Scenario::from_toml(&s).unwrap().bind(&p).unwrap()).remove(0);
    (p, r)
}

#[test]
fn heat_values_come_from_the_results() {
    let (p, r) = run();
    let file = "v13-model.bpmn";
    let waiting = heat(&p, file, &r, HeatMetric::Waiting);
    assert_eq!(
        waiting[&q("Task_Close")],
        r.activities[&q("Task_Close")].waiting.mean
    );
    assert_eq!(
        heat(&p, file, &r, HeatMetric::Runs)[&q("Task_Intake")],
        300.0
    );
    assert_eq!(
        heat(&p, file, &r, HeatMetric::Processing)[&q("Task_Close")],
        180.0
    );
    let util = heat(&p, file, &r, HeatMetric::Utilisation);
    let clerks = r.resources["Clerks"].utilisation;
    assert_eq!(util[&q("Task_Intake")], clerks);
    assert_eq!(
        util[&q("Lane_Clerks")],
        clerks,
        "lanes are coloured by their pool"
    );
    assert_eq!(util[&q("Task_Close")], r.resources["Experts"].utilisation);
    assert!(heat(&p, "other.bpmn", &r, HeatMetric::Waiting).is_empty());
}

#[test]
fn replay_snapshot_counts_waiting_and_working_tokens() {
    let (_, r) = run();
    // At every record's start time it is working (and nothing is counted twice).
    for x in r.records.iter().take(50).filter(|x| x.end > x.start) {
        let s = snapshot(&r, x.start);
        assert!(s[&x.activity].working >= 1);
    }
    // Totals at an instant equal the records overlapping it.
    let t = r.end_time / 2.0;
    let s = snapshot(&r, t);
    let working: usize = s.values().map(|o| o.working).sum();
    let waiting: usize = s.values().map(|o| o.waiting).sum();
    assert_eq!(
        working,
        r.records
            .iter()
            .filter(|x| x.start <= t && t < x.end)
            .count()
    );
    assert_eq!(
        waiting,
        r.records
            .iter()
            .filter(|x| x.queued <= t && t < x.start)
            .count()
    );
    // Clerks: never more than 2 working at once; Experts: never more than 1.
    for k in 0..200 {
        let s = snapshot(&r, r.end_time * f64::from(k) / 200.0);
        let at = |ids: &[&str]| {
            ids.iter()
                .map(|i| s.get(&q(i)).map_or(0, |o| o.working))
                .sum::<usize>()
        };
        assert!(at(&["Task_Intake", "Task_Simple", "Task_Review"]) <= 2);
        assert!(at(&["Task_Check", "Task_Close"]) <= 1);
    }
    assert!(snapshot(&r, -1.0).is_empty());
}
