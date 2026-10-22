//! Validation tests V7 and V8 (spec §8) plus message/signal edge cases.

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

/// `triggers`: start events fired together; `extra`: further scenario text.
fn scenario(triggers: &str, extra: &str) -> String {
    format!(
        r#"
[simulation]
cases = 10
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "60m" }}
triggers = [{triggers}]
[resources.A]
count = 1
[resources.B]
count = 1
{extra}
"#
    )
}

const BOTH: &str = r#""Start_A", "Start_B""#;
const TASK_SEND_10: &str =
    "[activities.Task_Send]\nduration = { dist = \"fixed\", value = \"10m\" }\n";
const TASK_B_5: &str = "[activities.Task_B]\nduration = { dist = \"fixed\", value = \"5m\" }\n";

#[test]
fn fixtures_validate_cleanly() {
    for f in [
        "v7-message.bpmn",
        "v7-early.bpmn",
        "v7-start.bpmn",
        "v7-blackbox.bpmn",
        "v8-signal-early.bpmn",
        "v8-signal-ok.bpmn",
    ] {
        let issues = validate(&load(f));
        assert!(issues.is_empty(), "{f}: {issues:?}");
    }
}

#[test]
fn v7_receiver_continues_only_after_the_message() {
    let r = run(
        "v7-message.bpmn",
        &scenario(BOTH, &format!("{TASK_SEND_10}{TASK_B_5}")),
    );
    // B waits at its catch event from t=0; the message leaves A at t=10; B then needs 5 more.
    // (If the case were considered complete when A ends, this would be 10.)
    assert_eq!(r.case_cycle_time.min, 15.0 * MIN);
    assert_eq!(r.case_cycle_time.max, 15.0 * MIN);
    assert_eq!(r.cases_completed, 10);
    assert_eq!(r.messages.len(), 10);
    assert!(
        r.messages
            .iter()
            .all(|m| m.outcome == MessageOutcome::Delivered && m.time % (60.0 * MIN) == 10.0 * MIN)
    );
    assert!(r.stuck.is_empty() && r.undelivered.is_empty());
}

const EARLY_PRE: &str = "[activities.Task_Pre]\nduration = { dist = \"fixed\", value = \"5m\" }\n";

#[test]
fn v7_early_message_is_buffered_by_default() {
    let r = run("v7-early.bpmn", &scenario(BOTH, EARLY_PRE));
    assert!(
        r.messages
            .iter()
            .all(|m| m.outcome == MessageOutcome::Buffered)
    );
    assert_eq!(r.messages_lost, 0);
    assert_eq!(r.cases_completed, 10);
    assert_eq!(
        r.case_cycle_time.max,
        5.0 * MIN,
        "B takes the buffered message after its 5 minutes of work"
    );
    assert!(r.undelivered.is_empty());
}

#[test]
fn v7_early_message_is_lost_without_buffering() {
    let r = run(
        "v7-early.bpmn",
        &scenario(
            BOTH,
            &format!("{EARLY_PRE}[messages]\nbuffer_unconsumed = false\n"),
        ),
    );
    assert_eq!(r.messages_lost, 10);
    assert!(r.messages.iter().all(|m| m.outcome == MessageOutcome::Lost));
    assert_eq!(r.cases_completed, 0);
    assert_eq!(r.stuck.len(), 10);
    assert!(
        r.stuck
            .iter()
            .all(|s| s.element.id == "Catch" && s.state == "waiting for message")
    );
}

#[test]
fn message_start_event_creates_an_instance_in_the_same_case() {
    let r = run(
        "v7-start.bpmn",
        &scenario(r#""Start_A""#, &format!("{TASK_SEND_10}{TASK_B_5}")),
    );
    assert!(
        r.messages
            .iter()
            .all(|m| m.outcome == MessageOutcome::StartedInstance)
    );
    assert_eq!(r.case_cycle_time.mean, 15.0 * MIN);
    assert_eq!(r.resources["B"].tasks, 10);
    assert_eq!(r.end_events.len(), 2, "both pools reached their end event");
    assert!(r.stuck.is_empty());
}

#[test]
fn processes_reached_by_messages_need_their_own_configuration() {
    let e = bind_err("v7-start.bpmn", &scenario(r#""Start_A""#, TASK_SEND_10));
    assert!(e.contains("Task_B") && e.contains("no [activities]"), "{e}");
}

#[test]
fn message_start_events_cannot_be_arrival_triggers() {
    let e = bind_err("v7-start.bpmn", &scenario(r#""MStart""#, ""));
    assert!(e.contains("MStart") && e.contains("message"), "{e}");
}

#[test]
fn black_box_messages_are_logged_and_can_be_answered() {
    let base = scenario(r#""Start_A""#, TASK_SEND_10);
    // Without a configured reply the requester waits forever and is reported as stuck.
    let r = run("v7-blackbox.bpmn", &base);
    assert!(
        r.messages
            .iter()
            .all(|m| m.outcome == MessageOutcome::BlackBox)
    );
    assert_eq!(r.cases_completed, 0);
    assert!(r.stuck.iter().all(|s| s.element.id == "Wait"));

    let with_reply = format!(
        "{base}[messages.black_box_reply]\nPart_X = {{ dist = \"fixed\", value = \"30m\" }}\n"
    );
    let r = run("v7-blackbox.bpmn", &with_reply);
    assert_eq!(r.cases_completed, 10);
    assert_eq!(r.case_cycle_time.mean, 40.0 * MIN);
    assert!(r.stuck.is_empty());
    // The reply is a message too: it is delivered to the waiting receive task.
    assert_eq!(
        r.messages
            .iter()
            .filter(|m| m.outcome == MessageOutcome::Delivered)
            .count(),
        10
    );
}

#[test]
fn unknown_black_box_participant_is_a_scenario_error() {
    let s = scenario(
        r#""Start_A""#,
        &format!(
            "{TASK_SEND_10}[messages.black_box_reply]\nNope = {{ dist = \"fixed\", value = \"1m\" }}\n"
        ),
    );
    assert!(bind_err("v7-blackbox.bpmn", &s).contains("Nope"));
}

#[test]
fn v8_signal_thrown_before_the_catch_is_active_leaves_the_receiver_stuck() {
    let r = run(
        "v8-signal-early.bpmn",
        &scenario(
            BOTH,
            "[activities.Task_B]\nduration = { dist = \"fixed\", value = \"5m\" }\n",
        ),
    );
    assert!(
        r.signals.iter().all(|s| s.name == "Go" && s.receivers == 0),
        "signals are not stored"
    );
    assert_eq!(r.cases_completed, 0);
    assert_eq!(r.stuck.len(), 10);
    assert!(
        r.stuck
            .iter()
            .all(|s| s.element.id == "Catch" && s.state == "waiting for signal 'Go'")
    );
}

#[test]
fn signal_reaches_a_waiting_catch_event() {
    let r = run(
        "v8-signal-ok.bpmn",
        &scenario(
            BOTH,
            "[activities.Task_A]\nduration = { dist = \"fixed\", value = \"10m\" }\n",
        ),
    );
    assert_eq!(r.cases_completed, 10);
    assert_eq!(r.case_cycle_time.mean, 10.0 * MIN);
    assert!(r.signals.iter().all(|s| s.receivers == 1));
    assert!(r.stuck.is_empty());
}

#[test]
fn signals_stay_within_the_case_unless_global() {
    // Arrivals every 5 minutes, signal sent at t = 10 + 5k. Case 0's signal reaches case 1's
    // waiting catch event only with global scope.
    let sc = |scope: &str| {
        scenario(
            BOTH,
            "[activities.Task_A]\nduration = { dist = \"fixed\", value = \"10m\" }\n",
        )
        .replace("value = \"60m\"", "value = \"5m\"")
        .replace("cases = 10", &format!("cases = 2\n{scope}"))
    };
    let local = run("v8-signal-ok.bpmn", &sc(""));
    assert_eq!(local.signals[0].receivers, 1);
    let global = run("v8-signal-ok.bpmn", &sc("signal_scope = \"global\""));
    assert_eq!(global.signals[0].receivers, 2);
    assert_eq!(global.signals[1].receivers, 0);
    assert_eq!(global.cases_completed, 2);
}

#[test]
fn message_runs_are_deterministic() {
    let s = scenario(
        BOTH,
        &format!(
            "{TASK_B_5}[activities.Task_Send]\nduration = {{ dist = \"exponential\", mean = \"10m\" }}\n"
        ),
    );
    let a = serde_json::to_string(&run("v7-message.bpmn", &s)).unwrap();
    let b = serde_json::to_string(&run("v7-message.bpmn", &s)).unwrap();
    assert_eq!(a, b);
}
