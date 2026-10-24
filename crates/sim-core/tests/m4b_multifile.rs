//! Call activities, several files, discovery and the multi-file collaboration (M4b),
//! including validation tests V10, V10b and V10c (spec §8).

use std::path::PathBuf;

use sim_core::*;

const MIN: f64 = 60.0;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn multi() -> Project {
    Project::open_main(&fixture("multi-file/main.bpmn")).expect("multi-file loads")
}

fn scenario_text() -> String {
    std::fs::read_to_string(fixture("multi-file/scenario.toml")).unwrap()
}

fn run_multi(scenario: &str) -> ReplicationResult {
    let project = multi();
    let bound = Scenario::from_toml(scenario)
        .unwrap()
        .bind(&project)
        .unwrap_or_else(|e| panic!("{e}"));
    run_replications(&bound).remove(0)
}

fn q(file: &str, id: &str) -> QId {
    QId::new(file, id)
}

// ---------------------------------------------------------------- V10

const V10: &str = r#"
[simulation]
cases = 5
[[arrivals]]
inter_arrival = { dist = "fixed", value = "120m" }
triggers = ["main.bpmn#Start"]
[resources.Dev]
count = 1
cost_per_hour = 60.0
[activities.Task_Pre]
duration = { dist = "fixed", value = "5m" }
fixed_cost = 1.0
[activities.Task_Child]
duration = { dist = "fixed", value = "10m" }
fixed_cost = 2.0
[activities.Task_Post]
duration = { dist = "fixed", value = "15m" }
fixed_cost = 3.0
"#;

#[test]
fn v10_call_activity_runs_the_child_in_the_same_case_and_sums_costs() {
    let project = Project::open_main(&fixture("v10/main.bpmn")).unwrap();
    assert_eq!(
        project.files.len(),
        2,
        "child.bpmn was found next to the main file"
    );
    let bound = Scenario::from_toml(V10).unwrap().bind(&project).unwrap();
    let r = run_replications(&bound).remove(0);
    // The parent waits for the child: 5 + 10 + 15.
    assert_eq!(r.case_cycle_time.mean, 30.0 * MIN);
    assert_eq!(r.cases_completed, 5);
    // One case, one cost: 30 minutes at 60/h plus fixed costs 1 + 2 + 3.
    assert!(
        (r.case_cost.mean - 36.0).abs() < 1e-9,
        "{}",
        r.case_cost.mean
    );
    assert!(r.records.iter().all(|x| x.case < 5));
    let child = &r.processes[&q("child.bpmn", "Process_Child")];
    assert_eq!((child.instances, child.cycle_time.mean), (5, 10.0 * MIN));
    let parent = &r.processes[&q("main.bpmn", "Process_Parent")];
    assert_eq!(parent.cycle_time.mean, 30.0 * MIN);
    assert!(r.stuck.is_empty());
}

#[test]
fn call_activity_shares_resources_with_its_parent() {
    // The parent's task and the child's task use the same single "Dev": the second case's
    // first task has to wait for the first case's child to finish.
    let project = Project::open_main(&fixture("v10/main.bpmn")).unwrap();
    let s = V10.replace("value = \"120m\"", "value = \"10m\"");
    let r = run_replications(&Scenario::from_toml(&s).unwrap().bind(&project).unwrap()).remove(0);
    assert!(r.activities.values().any(|a| a.waiting.max > 0.0));
    assert_eq!(r.resources["Dev"].tasks, 15);
}

// ---------------------------------------------------------------- V10c, discovery

#[test]
fn v10c_duplicate_process_ids_fail_naming_both_files() {
    let folder = Project::load_dir(&fixture("dup-process"))
        .unwrap_err()
        .to_string();
    assert!(
        folder.contains("Process_SubA")
            && folder.contains("main.bpmn")
            && folder.contains("subprocess-a.bpmn"),
        "{folder}"
    );
    // Discovery from the main file notices the clash when it resolves the call activity.
    let found = Project::open_main(&fixture("dup-process/main.bpmn"))
        .unwrap_err()
        .to_string();
    assert!(
        found.contains("Process_SubA")
            && found.contains("main.bpmn")
            && found.contains("subprocess-a.bpmn"),
        "{found}"
    );
}

#[test]
fn discovery_loads_exactly_the_referenced_files() {
    let p = multi();
    let files: Vec<&str> = p.files.keys().map(String::as_str).collect();
    assert_eq!(
        files,
        ["main.bpmn", "subprocess-a.bpmn", "subprocess-b.bpmn"],
        "unrelated.bpmn stays out"
    );
    let dmn: Vec<&str> = p.dmn.keys().map(String::as_str).collect();
    assert_eq!(
        dmn,
        ["decision-limit.dmn", "decision-risk.dmn"],
        "decision-unused.dmn stays out"
    );
    // (main.bpmn does not show the suppliers the called processes talk to.)
    let issues = validate(&p);
    assert!(
        issues.iter().all(|i| i.severity == Severity::Warning
            && i.message
                .contains("show that pool and the message flow here too")),
        "{issues:?}"
    );
}

#[test]
fn discovery_is_transitive() {
    // v10: main -> child. Starting from a file that nobody calls only loads that file.
    assert_eq!(
        Project::open_main(&fixture("v10/child.bpmn"))
            .unwrap()
            .files
            .len(),
        1
    );
    assert_eq!(discover_files(&fixture("v10/main.bpmn")).unwrap().len(), 2);
}

#[test]
fn unresolved_call_activities_are_reported_not_fatal() {
    let p = Project::open_main(&fixture("bad-model.bpmn")).unwrap();
    assert!(
        validate(&p)
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("Process_Nope"))
    );
}

#[test]
fn duplicate_decision_ids_are_an_error() {
    let mut p = Project::new();
    let xml = std::fs::read_to_string(fixture("multi-file/decision-risk.dmn")).unwrap();
    p.add_dmn_source("a.dmn", &xml).unwrap();
    let e = p.add_dmn_source("b.dmn", &xml).unwrap_err().to_string();
    assert!(
        e.contains("decision-risk") && e.contains("a.dmn") && e.contains("b.dmn"),
        "{e}"
    );
}

// ---------------------------------------------------------------- V10b

#[test]
fn v10b_ambiguous_bare_ids_fail_and_qualified_ids_work() {
    let project = multi();
    let bare = scenario_text().replace("\"subprocess-a.bpmn#Task_Analyse\"", "Task_Analyse");
    let e = Scenario::from_toml(&bare)
        .unwrap()
        .bind(&project)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Task_Analyse")
            && e.contains("ambiguous")
            && e.contains("subprocess-a.bpmn")
            && e.contains("subprocess-b.bpmn"),
        "{e}"
    );
    assert!(
        Scenario::from_toml(&scenario_text())
            .unwrap()
            .bind(&project)
            .is_ok()
    );
}

#[test]
fn v10b_multi_file_collaboration_runs_end_to_end() {
    let r = run_multi(&scenario_text());
    assert_eq!(r.cases_completed, 10);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!(r.messages_lost, 0);
    assert!(r.undelivered.is_empty());

    // Customer 0..5 order, 5..10 rush; Ops: prepare 5..15; SubA 15..55 (10 + 30 || 5);
    // signal at 55; Billing: SubB 55..115 (20 + 40 || 6), invoice 115..120; Customer ends at 120.
    assert_eq!(r.case_cycle_time.min, 120.0 * MIN);
    assert_eq!(r.case_cycle_time.max, 120.0 * MIN);

    // The same element ids in the two files are kept apart.
    for (file, analyse) in [("subprocess-a.bpmn", 10.0), ("subprocess-b.bpmn", 20.0)] {
        let a = &r.activities[&q(file, "Task_Analyse")];
        assert_eq!(a.runs, 10, "{file}");
        assert_eq!(a.processing.mean, analyse * MIN, "{file}");
    }
    // Lanes with the same name in different files (and a pool of the main file) share one pool:
    // Ops = Prepare + (Analyse + Check in A) + (Analyse + Check in B) = 5 completed tasks per case.
    assert_eq!(r.resources["Ops"].tasks, 50);
    assert_eq!(r.resources["Reviewers"].tasks, 20);
    assert_eq!(r.resources["Customer"].tasks, 20);

    // Two instances of the Ops pool in each case: the first signals, the second is interrupted
    // by it (signal boundary event on its running task).
    assert_eq!(r.boundary_fired[&q("main.bpmn", "Sig_Boundary")], 10);
    let rush = &r.activities[&q("main.bpmn", "Task_Rush2")];
    assert_eq!((rush.runs, rush.cancelled), (0, 10));
    assert_eq!(r.end_events[&q("main.bpmn", "End_Rush_Short")], 10);
    assert!(!r.end_events.contains_key(&q("main.bpmn", "End_Rush")));
    assert_eq!(r.signals.len(), 10);
    assert!(
        r.signals
            .iter()
            .all(|s| s.name == "Prepared" && s.receivers == 1)
    );

    // Messages: four between the pools plus one to each black box, per case.
    let by = |o: MessageOutcome| r.messages.iter().filter(|m| m.outcome == o).count();
    assert_eq!(by(MessageOutcome::BlackBox), 20);
    assert_eq!(by(MessageOutcome::StartedInstance), 30);
    assert_eq!(by(MessageOutcome::Delivered), 10);

    // Per-process instance cycle times.
    assert_eq!(
        r.processes[&q("subprocess-a.bpmn", "Process_SubA")]
            .cycle_time
            .mean,
        40.0 * MIN
    );
    assert_eq!(
        r.processes[&q("subprocess-b.bpmn", "Process_SubB")]
            .cycle_time
            .mean,
        60.0 * MIN
    );
    assert_eq!(r.processes[&q("main.bpmn", "Process_Ops")].instances, 20);
}

#[test]
fn xor_loop_with_link_events_repeats_the_task() {
    let looping = scenario_text()
        .replace(
            "probabilities = { Flow_ok = 1.0, Flow_retry = 0.0 }",
            "probabilities = { Flow_ok = 0.5, Flow_retry = 0.5 }",
        )
        .replace("cases = 10", "cases = 200");
    let r = run_multi(&looping);
    assert_eq!(r.cases_completed, 200);
    let runs_a = r.activities[&q("subprocess-a.bpmn", "Task_Analyse")].runs;
    assert!(
        runs_a > 300 && runs_a < 500,
        "geometric number of attempts, expected ~400, got {runs_a}"
    );
    assert!(r.stuck.is_empty());
}

#[test]
fn call_activity_boundary_event_cancels_the_called_instance() {
    // A timer boundary on the call activity in a small model made on the fly.
    let main = std::fs::read_to_string(fixture("v10/main.bpmn")).unwrap().replace(
        "<bpmn:task id=\"Task_Post\"/>",
        "<bpmn:boundaryEvent id=\"Timeout\" attachedToRef=\"Call\"><bpmn:timerEventDefinition/></bpmn:boundaryEvent><bpmn:task id=\"Task_Post\"/><bpmn:endEvent id=\"End_T\"/><bpmn:sequenceFlow id=\"FT\" sourceRef=\"Timeout\" targetRef=\"End_T\"/>",
    );
    let mut project = Project::new();
    project.add_source("main.bpmn", &main).unwrap();
    project.add_path(&fixture("v10/child.bpmn")).unwrap();
    let s = format!("{V10}[boundary.Timeout]\ndelay = {{ dist = \"fixed\", value = \"8m\" }}\n");
    let r = run_replications(&Scenario::from_toml(&s).unwrap().bind(&project).unwrap()).remove(0);
    // Pre 0..5, call from 5, timeout at 13 while the child's 10 minute task runs.
    assert_eq!(r.case_cycle_time.mean, 13.0 * MIN);
    let child = &r.activities[&q("child.bpmn", "Task_Child")];
    assert_eq!((child.runs, child.cancelled), (0, 5));
    assert_eq!(r.cases_completed, 5);
    assert!(r.stuck.is_empty());
}

#[test]
fn recursive_calls_and_missing_children_fail_binding() {
    let own = std::fs::read_to_string(fixture("v10/main.bpmn"))
        .unwrap()
        .replace("Process_Child", "Process_Parent");
    let mut p = Project::new();
    p.add_source("main.bpmn", &own).unwrap();
    let e = Scenario::from_toml(V10)
        .unwrap()
        .bind(&p)
        .unwrap_err()
        .to_string();
    assert!(e.contains("recursive"), "{e}");

    let mut p = Project::new();
    p.add_path(&fixture("v10/main.bpmn")).unwrap();
    let e = Scenario::from_toml(V10)
        .unwrap()
        .bind(&p)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Process_Child") && e.contains("not defined"),
        "{e}"
    );
}

#[test]
fn model_main_and_files_are_checked() {
    let project = multi();
    let wrong = scenario_text().replace(
        "main = \"main.bpmn\"",
        "main = \"nope.bpmn\"\nfiles = [\"decision-risk.dmn\", \"missing.bpmn\"]",
    );
    let e = Scenario::from_toml(&wrong)
        .unwrap()
        .bind(&project)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("nope.bpmn") && e.contains("missing.bpmn") && !e.contains("decision-risk.dmn"),
        "{e}"
    );
}

#[test]
fn multi_file_runs_are_deterministic() {
    let s = scenario_text().replace(
        "Flow_ok = 1.0, Flow_retry = 0.0",
        "Flow_ok = 0.6, Flow_retry = 0.4",
    );
    let a = serde_json::to_string(&run_multi(&s)).unwrap();
    let b = serde_json::to_string(&run_multi(&s)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn skeleton_for_the_multi_file_model_binds_and_runs() {
    let project = multi();
    let sk = Scenario::skeleton(&project, Some("main.bpmn"));
    assert!(sk.activities.contains_key("subprocess-a.bpmn#Task_Analyse"));
    assert!(sk.activities.contains_key("subprocess-b.bpmn#Task_Analyse"));
    assert_eq!(sk.arrivals[0].triggers, ["main.bpmn#TimerStart"]);
    let back = Scenario::from_toml(&sk.to_toml().unwrap()).unwrap();
    let r = run_replications(&back.bind(&project).unwrap_or_else(|e| panic!("{e}"))).remove(0);
    assert_eq!(r.cases_completed, 100);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
}
