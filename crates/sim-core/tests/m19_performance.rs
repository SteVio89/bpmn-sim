//! Running replications in parallel and interned ids (bpmn-sim-2).

use std::path::PathBuf;

use sim_core::*;

fn load(fixture: &str) -> Project {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(fixture);
    let mut p = Project::new();
    p.add_path(&path).expect("fixture parses");
    p
}

#[test]
fn parallel_replications_match_sequential_ones() {
    let project = load("v13-model.bpmn");
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/v13-model.toml"),
    )
    .unwrap();
    let mut scenario = Scenario::from_toml(&text).unwrap();
    scenario.simulation.replications = 12;
    scenario.simulation.cases = Some(300);
    scenario.simulation.warm_up_cases = 20;
    let bound = scenario.bind(&project).unwrap();

    let mut progress = Vec::new();
    let sequential = run_replications_on(&bound, 1, |_, _| {});
    let parallel = run_replications_on(&bound, 4, |d, t| progress.push((d, t)));
    assert_eq!(parallel, sequential);
    // Results come back in replication order, each with its own seed.
    let seeds: Vec<u64> = parallel.iter().map(|r| r.seed).collect();
    assert_eq!(seeds, (13..25).collect::<Vec<_>>());
    // Progress counts every replication once, on the calling thread.
    assert_eq!(progress, (1..=12).map(|d| (d, 12)).collect::<Vec<_>>());
    // More threads than replications is fine.
    assert_eq!(run_replications_on(&bound, 64, |_, _| {}), sequential);
}

#[test]
fn names_are_interned_and_behave_like_strings() {
    let a = Name::from("Task_A");
    let b = Name::from(String::from("Task_A"));
    assert_eq!(a, b);
    assert!(std::ptr::eq(a.as_str(), b.as_str()));
    assert_eq!(a, "Task_A");
    assert_eq!(String::from("Task_A"), a);
    assert_ne!(a, Name::from("Task_B"));
    // Ordered like the strings, so maps keyed by ids iterate in the same order as before.
    let mut names: Vec<Name> = ["b", "a", "ab", "", "B"].map(Name::from).to_vec();
    names.sort();
    let strs: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
    assert_eq!(strs, ["", "B", "a", "ab", "b"]);
    let q = QId::new("model.bpmn", "Task_A");
    assert_eq!(q.to_string(), "model.bpmn#Task_A");
    assert_eq!(serde_json::to_string(&q).unwrap(), "\"model.bpmn#Task_A\"");
}
