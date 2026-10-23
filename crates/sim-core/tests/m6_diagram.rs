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

#[test]
fn label_positions_are_read_when_the_modeller_gave_them() {
    let text = std::fs::read_to_string(fixture("v13-model.bpmn")).unwrap().replace(
        r#"<bpmndi:BPMNEdge id="Flow_simple_di" bpmnElement="Flow_simple">"#,
        r#"<bpmndi:BPMNEdge id="Flow_simple_di" bpmnElement="Flow_simple"><bpmndi:BPMNLabel><dc:Bounds x="470" y="230" width="40" height="14"/></bpmndi:BPMNLabel>"#,
    );
    let mut p = Project::new();
    p.add_source("v13-model.bpmn", &text).unwrap();
    let d = &p.files["v13-model.bpmn"].diagram;
    assert_eq!(
        d.labels[&q("Flow_simple")],
        Bounds {
            x: 470.0,
            y: 230.0,
            width: 40.0,
            height: 14.0
        }
    );
    assert_eq!(
        d.edges[&q("Flow_simple")].len(),
        4,
        "the label does not disturb the waypoints"
    );
    assert!(
        !d.labels.contains_key(&q("Flow_complex")),
        "no label bounds: drawn at the middle of the flow"
    );
}

#[test]
fn data_stores_objects_annotations_and_associations_are_kept_for_drawing() {
    let mut p = Project::new();
    p.add_path(&fixture("task-types.bpmn")).unwrap();
    let f = &p.files["task-types.bpmn"];
    let t = |id: &str| QId::new("task-types.bpmn", id);
    assert_eq!(
        f.artifacts[&t("DS1")],
        Artifact {
            kind: ArtifactKind::DataStore,
            text: Some("Customer DB".into())
        }
    );
    assert_eq!(f.artifacts[&t("DO1")].kind, ArtifactKind::DataObject);
    assert_eq!(
        f.artifacts[&t("TA1")].text.as_deref(),
        Some("Checked by the team lead")
    );
    // Data associations have an arrow, the plain association to the annotation has none.
    assert!(f.associations[&t("DOA_1")]);
    assert!(f.associations[&t("DIA_1")]);
    assert!(!f.associations[&t("Assoc_1")]);
    assert!(f.diagram.shapes.contains_key(&t("DS1")));
    // They are not flow nodes: simulation and validation are unaffected.
    assert!(p.node(&t("DS1")).is_none());
    assert!(validate(&p).iter().all(|i| i.element.id != "DS1"));
}

#[test]
fn default_and_conditional_flows_are_marked() {
    let p = Project::open_main(&fixture("multi-file/main.bpmn")).unwrap();
    let (defaults, conditional) = sim_core::diagram::flow_markers(&p, "subprocess-a.bpmn");
    assert!(
        defaults.contains(&QId::new("subprocess-a.bpmn", "Flow_ok")),
        "{defaults:?}"
    );
    assert!(conditional.is_empty());
    // Conditions after a gateway get no marker; a condition on a flow leaving a task does.
    let p = Project::open_main(&fixture("dmn/claim.bpmn")).unwrap();
    let (defaults, conditional) = sim_core::diagram::flow_markers(&p, "claim.bpmn");
    assert!(defaults.is_empty() && conditional.is_empty());
    let text = std::fs::read_to_string(fixture("v13-model.bpmn"))
        .unwrap()
        .replace(r#"<bpmn:sequenceFlow id="F12" sourceRef="Task_Close" targetRef="End"/>"#, r#"<bpmn:sequenceFlow id="F12" sourceRef="Task_Close" targetRef="End"><bpmn:conditionExpression>=done</bpmn:conditionExpression></bpmn:sequenceFlow>"#);
    let mut p = Project::new();
    p.add_source("v13-model.bpmn", &text).unwrap();
    let (_, conditional) = sim_core::diagram::flow_markers(&p, "v13-model.bpmn");
    assert_eq!(
        conditional.into_iter().map(|q| q.id).collect::<Vec<_>>(),
        ["F12"]
    );
}
