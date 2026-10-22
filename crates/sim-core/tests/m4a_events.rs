//! Link/timer events, terminate, embedded subprocesses, boundary events, signal start (M4a),
//! including validation test V9 and V11 (spec §8).

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

fn base(cases: u32, interval: &str, triggers: &str, extra: &str) -> String {
    format!(
        r#"
[simulation]
cases = {cases}
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "{interval}" }}
triggers = [{triggers}]
{extra}
"#
    )
}

fn fixed(name: &str, minutes: u32) -> String {
    format!("[activities.{name}]\nduration = {{ dist = \"fixed\", value = \"{minutes}m\" }}\n")
}

fn dev(count: u32) -> String {
    format!("[resources.Dev]\ncount = {count}\n")
}

fn ids(r: &ReplicationResult, id: &str) -> u64 {
    r.end_events
        .iter()
        .filter(|(k, _)| k.id == id)
        .map(|(_, n)| *n)
        .sum()
}

#[test]
fn new_fixtures_have_no_validation_errors() {
    for f in [
        "v11-link.bpmn",
        "v11-direct.bpmn",
        "timer-catch.bpmn",
        "terminate.bpmn",
        "subprocess.bpmn",
        "subprocess-boundary.bpmn",
        "v9-boundary-timer.bpmn",
        "v9-boundary-timer-nonint.bpmn",
        "boundary-error.bpmn",
        "boundary-message.bpmn",
        "signal-start.bpmn",
    ] {
        let errors: Vec<_> = validate(&load(f))
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{f}: {errors:?}");
    }
}

#[test]
fn v11_link_events_behave_like_a_sequence_flow() {
    let s = base(20, "30m", "\"Start\"", &fixed("Task_A", 10));
    let link = run("v11-link.bpmn", &s);
    let direct = run("v11-direct.bpmn", &s);
    assert_eq!(link.case_cycle_time, direct.case_cycle_time);
    assert_eq!(link.case_cycle_time.mean, 10.0 * MIN);
    let times = |r: &ReplicationResult| {
        r.records
            .iter()
            .map(|x| (x.queued, x.start, x.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(times(&link), times(&direct));
    assert_eq!(link.end_time, direct.end_time);
}

#[test]
fn timer_catch_event_waits_for_its_configured_delay() {
    let s = base(
        5,
        "120m",
        "\"Start\"",
        &format!(
            "{}[events.Wait]\ndelay = {{ dist = \"fixed\", value = \"30m\" }}\n",
            fixed("Task_A", 10)
        ),
    );
    let r = run("timer-catch.bpmn", &s);
    assert_eq!(r.case_cycle_time.mean, 40.0 * MIN);
    let e = bind_err(
        "timer-catch.bpmn",
        &base(5, "1m", "\"Start\"", &fixed("Task_A", 10)),
    );
    assert!(e.contains("Wait") && e.contains("[events]"), "{e}");
}

#[test]
fn terminate_end_event_cancels_the_rest_of_the_instance() {
    let s = base(
        5,
        "60m",
        "\"Start\"",
        &format!(
            "{}{}[resources.Fast]\ncount=1\n[resources.Slow]\ncount=1\n",
            fixed("Task_A", 10),
            fixed("Task_B", 30)
        ),
    );
    let r = run("terminate.bpmn", &s);
    assert_eq!(r.cases_completed, 5);
    assert_eq!(
        r.case_cycle_time.max,
        10.0 * MIN,
        "the case ends when the terminate end event is reached"
    );
    assert_eq!(ids(&r, "End_Term"), 5);
    assert_eq!(ids(&r, "End_B"), 0);
    let b = &r.activities[&QId::new("terminate.bpmn", "Task_B")];
    assert_eq!((b.runs, b.cancelled), (0, 5));
    // The slow resource only worked until the cancellation (10 of its 30 minutes).
    assert_eq!(r.resources["Slow"].busy_time, 5.0 * 10.0 * MIN);
    assert!(r.stuck.is_empty());
}

#[test]
fn embedded_subprocess_runs_inline() {
    let s = base(
        5,
        "60m",
        "\"Start\"",
        &format!(
            "{}{}{}",
            fixed("Task_In", 5),
            fixed("Task_In2", 5),
            fixed("Task_After", 5)
        ),
    );
    let r = run("subprocess.bpmn", &s);
    assert_eq!(r.case_cycle_time.mean, 15.0 * MIN);
    assert_eq!(r.cases_completed, 5);
    assert!(r.stuck.is_empty());
}

fn sub_boundary(delay: &str) -> String {
    base(
        4,
        "120m",
        "\"Start\"",
        &format!(
            "{}{}{}[boundary.Timeout]\ndelay = {{ dist = \"fixed\", value = \"{delay}\" }}\n",
            dev(1),
            fixed("Task_In", 30),
            fixed("Task_In2", 30)
        ),
    )
}

#[test]
fn interrupting_boundary_event_cancels_a_whole_subprocess() {
    let r = run("subprocess-boundary.bpmn", &sub_boundary("40m"));
    assert_eq!(r.case_cycle_time.mean, 40.0 * MIN);
    assert_eq!(ids(&r, "End_Timeout"), 4);
    assert_eq!(ids(&r, "End_Ok"), 0);
    let t2 = &r.activities[&QId::new("subprocess-boundary.bpmn", "Task_In2")];
    assert_eq!((t2.runs, t2.cancelled), (0, 4));
    assert_eq!(r.resources["Dev"].busy_time, 4.0 * 40.0 * MIN);
    assert!(r.stuck.is_empty());
    // A slower timer never fires.
    let r = run("subprocess-boundary.bpmn", &sub_boundary("100m"));
    assert_eq!(r.case_cycle_time.mean, 60.0 * MIN);
    assert_eq!(ids(&r, "End_Ok"), 4);
    assert!(r.boundary_fired.is_empty());
}

fn v9(extra: &str, interval: &str, cases: u32) -> String {
    base(
        cases,
        interval,
        "\"Start\"",
        &format!(
            "{}{}{}{extra}",
            dev(1),
            fixed("Task_Long", 60),
            fixed("Task_Handle", 5)
        ),
    )
}

const TIMER_20: &str = "[boundary.B]\ndelay = { dist = \"fixed\", value = \"20m\" }\n";

#[test]
fn v9_interrupting_timer_cancels_the_task_and_releases_the_resource() {
    // One resource, a new case every 25 minutes: the second case only starts on time if the
    // first case's cancelled task released the resource at minute 20.
    let r = run("v9-boundary-timer.bpmn", &v9(TIMER_20, "25m", 10));
    let long = &r.activities[&QId::new("v9-boundary-timer.bpmn", "Task_Long")];
    assert_eq!((long.runs, long.cancelled), (0, 10));
    assert!(
        r.records
            .iter()
            .filter(|x| x.activity.id == "Task_Long")
            .all(|x| x.cancelled && x.waiting() == 0.0 && x.processing() == 20.0 * MIN)
    );
    assert_eq!(r.resources["Dev"].busy_time, 10.0 * 20.0 * MIN);
    assert_eq!(
        r.case_cycle_time.mean,
        25.0 * MIN,
        "20 min until the timeout, 5 min on the boundary path"
    );
    assert_eq!((ids(&r, "End_Alt"), ids(&r, "End_Ok")), (10, 0));
    assert_eq!(r.boundary_fired.values().sum::<u64>(), 10);
    assert!(r.stuck.is_empty());
}

#[test]
fn timer_slower_than_the_task_never_fires() {
    let r = run(
        "v9-boundary-timer.bpmn",
        &v9(
            "[boundary.B]\ndelay = { dist = \"fixed\", value = \"90m\" }\n",
            "120m",
            5,
        ),
    );
    assert_eq!(r.case_cycle_time.mean, 60.0 * MIN);
    assert_eq!((ids(&r, "End_Ok"), ids(&r, "End_Alt")), (5, 0));
    assert!(r.boundary_fired.is_empty());
}

#[test]
fn non_interrupting_boundary_adds_a_second_token_and_keeps_the_task() {
    let r = run("v9-boundary-timer-nonint.bpmn", &v9(TIMER_20, "120m", 5));
    assert_eq!((ids(&r, "End_Ok"), ids(&r, "End_Alt")), (5, 5));
    let long = &r.activities[&QId::new("v9-boundary-timer-nonint.bpmn", "Task_Long")];
    assert_eq!((long.runs, long.cancelled), (5, 0));
    assert_eq!(
        r.case_cycle_time.mean,
        60.0 * MIN,
        "the case waits for both paths"
    );
}

#[test]
fn error_boundary_fires_with_its_probability() {
    let spec = |p: f64| {
        format!("[boundary.B]\nprobability = {p}\ndelay = {{ dist = \"fixed\", value = \"5m\" }}\n")
    };
    let n = 4000;
    let r = run("boundary-error.bpmn", &v9(&spec(0.3), "60m", n));
    let fired = r.boundary_fired.values().sum::<u64>() as f64;
    let tol = 5.0 * (f64::from(n) * 0.3 * 0.7).sqrt();
    assert!((fired - 1200.0).abs() < tol, "fired {fired}");
    assert_eq!(ids(&r, "End_Alt") as f64, fired);
    let r = run("boundary-error.bpmn", &v9(&spec(0.0), "60m", 50));
    assert!(r.boundary_fired.is_empty());
    let e = bind_err(
        "boundary-error.bpmn",
        &v9(
            "[boundary.B]\ndelay = { dist = \"fixed\", value = \"5m\" }\n",
            "60m",
            5,
        ),
    );
    assert!(e.contains("probability"), "{e}");
}

#[test]
fn message_boundary_event_interrupts_while_the_activity_runs() {
    let extra = format!(
        "[resources.A]\ncount=1\n[resources.B]\ncount=1\n{}{}",
        fixed("Task_Long", 60),
        fixed("Task_Handle", 5)
    );
    let s = |b: u32| {
        base(
            5,
            "200m",
            "\"Start_A\", \"Start_B\"",
            &format!("{extra}{}", fixed("Task_B", b)),
        )
    };
    // The message arrives at minute 10, while Task_Long runs: it is cancelled.
    let r = run("boundary-message.bpmn", &s(10));
    assert_eq!(r.case_cycle_time.mean, 15.0 * MIN);
    assert_eq!(
        r.activities[&QId::new("boundary-message.bpmn", "Task_Long")].cancelled,
        5
    );
    assert_eq!(
        r.messages
            .iter()
            .filter(|m| m.outcome == MessageOutcome::Delivered)
            .count(),
        5
    );
    // At minute 90 the activity is long finished: boundary events do not buffer, the message is lost.
    let r = run("boundary-message.bpmn", &s(90));
    assert_eq!(r.messages_lost, 5);
    assert!(r.boundary_fired.is_empty());
    assert_eq!(r.case_cycle_time.mean, 90.0 * MIN);
}

#[test]
fn signal_start_event_creates_an_instance_in_the_senders_case() {
    let s = base(
        5,
        "60m",
        "\"Start_A\"",
        &format!(
            "[resources.A]\ncount=1\n[resources.B]\ncount=1\n{}{}",
            fixed("Task_A", 10),
            fixed("Task_B", 5)
        ),
    );
    let r = run("signal-start.bpmn", &s);
    assert_eq!(r.case_cycle_time.mean, 15.0 * MIN);
    assert!(r.signals.iter().all(|x| x.started == 1 && x.receivers == 0));
    assert_eq!(r.resources["B"].tasks, 5);
    assert_eq!(ids(&r, "End_B"), 5);
}

#[test]
fn runs_with_all_new_elements_are_deterministic() {
    let s = v9(
        "[boundary.B]\nprobability = 0.5\ndelay = { dist = \"uniform\", min = \"1m\", max = \"30m\" }\n",
        "15m",
        200,
    );
    let a = serde_json::to_string(&run("boundary-error.bpmn", &s)).unwrap();
    let b = serde_json::to_string(&run("boundary-error.bpmn", &s)).unwrap();
    assert_eq!(a, b);
}
