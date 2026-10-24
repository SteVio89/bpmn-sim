//! Pools that coordinate within a case: gateways that ask whether something has already
//! happened in the case.

use std::path::PathBuf;

use sim_core::*;

const MIN: f64 = 60.0;

fn load(fixture: &str) -> Project {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(fixture);
    let mut p = Project::new();
    p.add_path(&path).expect("fixture parses");
    p
}

fn run(fixture: &str, scenario: &str) -> ReplicationResult {
    let project = load(fixture);
    let scenario = Scenario::from_toml(scenario).expect("scenario parses");
    let bound = scenario.bind(&project).unwrap_or_else(|e| panic!("{e}"));
    run_replications(&bound).remove(0)
}

fn bind_err(fixture: &str, scenario: &str) -> String {
    let project = load(fixture);
    Scenario::from_toml(scenario)
        .unwrap()
        .bind(&project)
        .unwrap_err()
        .to_string()
}

/// history-gateway.bpmn with `cases` cases an hour apart, durations in minutes and `ready`
/// as the `[gateways.Gateway_Ready]` entry.
fn history(cases: u32, make: u32, look: u32, ready: &str) -> String {
    format!(
        r#"
[simulation]
cases = {cases}
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "60m" }}
triggers = ["Start_A", "Start_B"]
[resources.A]
count = 5
[resources.B]
count = 5
[activities.Task_Make]
duration = {{ dist = "fixed", value = "{make}m" }}
[activities.Task_Look]
duration = {{ dist = "fixed", value = "{look}m" }}
[activities.Task_Use]
duration = {{ dist = "fixed", value = "5m" }}
[gateways.Gateway_Ready]
{ready}
"#
    )
}

const IF_MADE: &str = "if_completed = \"End_Made\"\nthen = \"Flow_yes\"";

#[test]
fn not_completed_yet_takes_the_other_flow_and_waits_for_the_signal() {
    let r = run("history-gateway.bpmn", &history(1, 30, 10, IF_MADE));
    // B asks at 10 m, A is still making: B waits for the signal at 30 m, then uses for 5 m.
    assert_eq!(r.case_cycle_time.max, 35.0 * MIN);
    assert_eq!(r.signals[0].receivers, 1);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
}

#[test]
fn completed_already_takes_the_then_flow() {
    let r = run("history-gateway.bpmn", &history(1, 30, 60, IF_MADE));
    // The signal at 30 m reached nobody; B asks at 60 m and goes straight on.
    assert_eq!(r.signals[0].receivers, 0);
    assert_eq!(r.case_cycle_time.max, 65.0 * MIN);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
}

#[test]
fn a_random_choice_after_the_signal_gets_stuck_where_the_history_does_not() {
    // With shares the gateway can say "not yet" after the signal has gone: B waits forever.
    let r = run(
        "history-gateway.bpmn",
        &history(
            20,
            30,
            60,
            "probabilities = { Flow_yes = 0.5, Flow_no = 0.5 }",
        ),
    );
    assert!(!r.stuck.is_empty());
    let r = run("history-gateway.bpmn", &history(20, 30, 60, IF_MADE));
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!(r.cases_completed, 20);
}

#[test]
fn the_history_belongs_to_the_case() {
    // Case 1 starts at 60 m and asks at 120 m. Case 0's "Made" (at 90 m) must not count: case
    // 1 waits for its own (at 150 m), so both cases take 95 m.
    let r = run("history-gateway.bpmn", &history(2, 90, 60, IF_MADE));
    assert_eq!(r.case_cycle_time.min, 95.0 * MIN);
    assert_eq!(r.case_cycle_time.max, 95.0 * MIN);
}

#[test]
fn a_task_counts_as_completed_once_it_is_done() {
    let ready = "if_completed = \"Task_Make\"\nthen = \"Flow_yes\"";
    assert_eq!(
        run("history-gateway.bpmn", &history(1, 30, 60, ready))
            .case_cycle_time
            .max,
        65.0 * MIN
    );
    assert_eq!(
        run("history-gateway.bpmn", &history(1, 30, 10, ready))
            .case_cycle_time
            .max,
        35.0 * MIN
    );
}

#[test]
fn history_entries_are_checked() {
    let err = |ready: &str| bind_err("history-gateway.bpmn", &history(1, 30, 10, ready));
    assert!(
        err("if_completed = \"End_Made\"\nthen = \"Flow_b3\"").contains("not an outgoing flow")
    );
    assert!(err("if_completed = \"End_Made\"").contains("go together"));
    assert!(err("then = \"Flow_yes\"").contains("go together"));
    assert!(err("if_completed = \"Nowhere\"\nthen = \"Flow_yes\"").contains("does not exist"));
    assert!(
        err("if_completed = \"End_Made\"\nthen = \"Flow_yes\"\nprobabilities = { Flow_yes = 1.0, Flow_no = 0.0 }")
            .contains("not both")
    );
}

#[test]
fn history_entries_survive_a_round_trip() {
    let s = Scenario::from_toml(&history(1, 30, 10, IF_MADE)).unwrap();
    let again = Scenario::from_toml(&s.to_toml().unwrap()).unwrap();
    assert_eq!(s, again);
    let g = &again.gateways["Gateway_Ready"];
    assert_eq!(g.if_completed.as_deref(), Some("End_Made"));
    assert!(g.probabilities.is_empty());
}

// ------------------------------------------------- call activities without their process

const V10_ALONE: &str = r#"
[simulation]
cases = 5
[[arrivals]]
inter_arrival = { dist = "fixed", value = "120m" }
triggers = ["main.bpmn#Start"]
[resources.Dev]
count = 1
[activities.Task_Pre]
duration = { dist = "fixed", value = "5m" }
[activities.Task_Post]
duration = { dist = "fixed", value = "15m" }
"#;

#[test]
fn a_call_activity_without_its_process_runs_as_a_task_with_a_duration() {
    // v10/main.bpmn on its own: the called process is not loaded.
    let p = load("v10/main.bpmn");
    let issues = validate(&p);
    assert!(
        issues
            .iter()
            .all(|i| i.severity == Severity::Warning && i.message.contains("Process_Child")),
        "{issues:?}"
    );
    let s = format!(
        "{V10_ALONE}[activities.Call]\nduration = {{ dist = \"fixed\", value = \"20m\" }}\nfixed_cost = 4.0\n"
    );
    let r = run("v10/main.bpmn", &s);
    assert_eq!(r.case_cycle_time.max, 40.0 * MIN);
    let call = &r.activities[&QId::new("main.bpmn", "Call")];
    assert_eq!(call.runs, 5);
    assert_eq!(call.total_cost, 20.0);
}

#[test]
fn a_call_activity_without_its_process_or_a_duration_is_an_error() {
    let e = bind_err("v10/main.bpmn", V10_ALONE);
    assert!(
        e.contains("Process_Child") && e.contains("[activities] entry with a duration"),
        "{e}"
    );
}

#[test]
fn the_skeleton_gives_a_call_activity_without_its_process_a_duration() {
    let p = load("v10/main.bpmn");
    let s = Scenario::skeleton(&p, None);
    assert!(s.activities.contains_key("main.bpmn#Call"));
    s.bind(&p).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn a_duration_for_a_call_activity_whose_process_is_loaded_is_ignored() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/v10/main.bpmn");
    let p = Project::open_main(&path).unwrap();
    let s = Scenario::from_toml(&format!(
        "{V10_ALONE}[activities.Task_Child]\nduration = {{ dist = \"fixed\", value = \"10m\" }}\n[activities.Call]\nduration = {{ dist = \"fixed\", value = \"20m\" }}\n"
    ))
    .unwrap();
    let b = s.bind(&p).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        b.warnings.iter().any(|w| w.contains("ignored")),
        "{:?}",
        b.warnings
    );
    let r = run_replications(&b).remove(0);
    // Pre 5 + child task 10 + Post 15: the called process runs, not the 20 minutes.
    assert_eq!(r.case_cycle_time.max, 30.0 * MIN);
}
