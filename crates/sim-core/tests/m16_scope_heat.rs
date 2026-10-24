//! Subprocesses and call activities report what happens inside them per run, so the heat map
//! can colour them like tasks.

use std::path::PathBuf;

use sim_core::diagram::{HeatMetric, heat};
use sim_core::*;

const MIN: f64 = 60.0;

fn load() -> Project {
    let mut p = Project::new();
    for f in ["outer.bpmn", "inner.bpmn"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/scope-heat")
            .join(f);
        p.add_path(&path).expect("fixture parses");
    }
    p
}

// Cases arrive every 2m; one clerk checks for 5m (they queue: 0, 3, 6 minutes), closing takes
// 1m and needs nobody.
const SCENARIO: &str = r#"
[simulation]
cases = 3
[[arrivals]]
inter_arrival = { dist = "fixed", value = "2m" }
triggers = ["Start_Outer"]
[resources.Staff]
count = 1
[activities.Task_A]
duration = { dist = "fixed", value = "5m" }
resource = "Staff"
[activities.Task_B]
duration = { dist = "fixed", value = "1m" }
"#;

#[test]
fn subprocesses_and_call_activities_sum_up_what_happens_inside() {
    let p = load();
    let s = Scenario::from_toml(SCENARIO).unwrap();
    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    assert_eq!(r.cases_completed, 3);

    let sub = &r.scopes[&QId::new("inner.bpmn", "Sub")];
    assert_eq!(sub.runs, 3);
    assert_eq!(sub.waiting.mean, 3.0 * MIN);
    assert_eq!(sub.processing.mean, 5.0 * MIN);
    // Ends at 5, 10, 15 after starting at 0, 2, 4.
    assert_eq!(sub.duration.mean, 8.0 * MIN);
    assert_eq!(sub.pools.iter().collect::<Vec<_>>(), ["Staff"]);

    // The call activity also counts closing (1m), which needs no pool.
    let call = &r.scopes[&QId::new("outer.bpmn", "Call_Inner")];
    assert_eq!(call.runs, 3);
    assert_eq!(call.waiting.mean, 3.0 * MIN);
    assert_eq!(call.processing.mean, 6.0 * MIN);
    assert_eq!(call.duration.mean, 9.0 * MIN);

    let h = |file: &str, m| heat(&p, file, &r, m);
    assert_eq!(
        h("outer.bpmn", HeatMetric::Waiting)[&QId::new("outer.bpmn", "Call_Inner")],
        3.0 * MIN
    );
    assert_eq!(
        h("inner.bpmn", HeatMetric::Processing)[&QId::new("inner.bpmn", "Sub")],
        5.0 * MIN
    );
    assert_eq!(
        h("outer.bpmn", HeatMetric::Runs)[&QId::new("outer.bpmn", "Call_Inner")],
        3.0
    );
    let staff = r.resources["Staff"].utilisation;
    assert!(staff > 0.0);
    assert_eq!(
        h("outer.bpmn", HeatMetric::Utilisation)[&QId::new("outer.bpmn", "Call_Inner")],
        staff
    );
    // A task inside is still coloured by its own values.
    assert_eq!(
        h("inner.bpmn", HeatMetric::Waiting)[&QId::new("inner.bpmn", "Task_A")],
        3.0 * MIN
    );
}

#[test]
fn runs_cut_short_by_the_horizon_are_left_out() {
    let p = load();
    let s = Scenario::from_toml(&SCENARIO.replace("cases = 3", "cases = 3\nhorizon = \"630s\""))
        .unwrap();
    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    // By 10.5m two subprocesses have ended (at 5m and 10m) but only one call (at 6m; the next at 11m).
    assert_eq!(r.scopes[&QId::new("outer.bpmn", "Call_Inner")].runs, 1);
    assert_eq!(r.scopes[&QId::new("inner.bpmn", "Sub")].runs, 2);
}

#[test]
fn called_instances_know_their_caller() {
    let p = load();
    let s = Scenario::from_toml(SCENARIO).unwrap();
    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    for c in 0..3 {
        let of_case: Vec<&InstanceRecord> = r.instances.iter().filter(|i| i.case == c).collect();
        assert_eq!(of_case.len(), 2);
        let (outer, inner) = (of_case[0], of_case[1]);
        assert_eq!(outer.process.id, "Process_Outer");
        assert_eq!((outer.parent, &outer.call), (None, &None));
        assert_eq!(inner.parent, Some(outer.id));
        assert_eq!(inner.call, Some(QId::new("outer.bpmn", "Call_Inner")));
        assert!(inner.end.is_some() && inner.start >= outer.start);
        assert_eq!(
            r.instances[inner.id as usize].id, inner.id,
            "ids are positions"
        );
    }
}
