//! Validation tests V1–V6 and V12 (spec §8).

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

fn run(fixture: &str, scenario: &str) -> Vec<ReplicationResult> {
    let project = load(fixture);
    let scenario = Scenario::from_toml(scenario).expect("scenario parses");
    let bound = scenario.bind(&project).expect("scenario binds");
    run_replications(&bound)
}

fn bind_err(fixture: &str, scenario: &str) -> String {
    let project = load(fixture);
    Scenario::from_toml(scenario)
        .unwrap()
        .bind(&project)
        .unwrap_err()
        .to_string()
}

fn v1(interval: &str, cases: u32) -> String {
    format!(
        r#"
[simulation]
seed = 1
cases = {cases}
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "{interval}" }}
triggers = ["Start"]
[resources.Dev]
count = 1
cost_per_hour = 60.0
[activities.Task_Work]
duration = {{ dist = "fixed", value = "10m" }}
fixed_cost = 2.0
"#
    )
}

#[test]
fn v1_single_task_no_queue() {
    let r = &run("v1-single-task.bpmn", &v1("60m", 100))[0];
    assert_eq!(r.cases_started, 100);
    assert_eq!(r.cases_completed, 100);
    assert_eq!(r.case_cycle_time.min, 10.0 * MIN);
    assert_eq!(r.case_cycle_time.max, 10.0 * MIN);
    assert_eq!(r.case_cycle_time.mean, 10.0 * MIN);
    let task = r.activities.values().next().unwrap();
    assert_eq!(task.runs, 100);
    assert_eq!(task.waiting.max, 0.0);
    let dev = &r.resources["Dev"];
    assert!(
        (dev.utilisation - 1.0 / 6.0).abs() < 1e-12,
        "{}",
        dev.utilisation
    );
    // cost = 10 min at 60/h + fixed 2.0
    assert!((r.case_cost.mean - 12.0).abs() < 1e-9);
    assert!(r.stuck.is_empty());
    assert_eq!(r.end_events.values().sum::<u64>(), 100);
}

#[test]
fn v2_overload_waiting_grows() {
    let r = &run("v1-single-task.bpmn", &v1("5m", 100))[0];
    let waits: Vec<f64> = r.records.iter().map(|x| x.waiting()).collect();
    assert!(
        waits.windows(2).all(|w| w[1] > w[0]),
        "waiting grows steadily"
    );
    // k-th case waits k * (10 - 5) minutes
    assert_eq!(waits[99], 99.0 * 5.0 * MIN);
    assert!(r.resources["Dev"].utilisation > 0.99);
    assert_eq!(r.cases_completed, 100);
}

const V4: &str = r#"
[simulation]
seed = 7
cases = 20000
[[arrivals]]
inter_arrival = { dist = "fixed", value = "1m" }
triggers = ["Start"]
[activities.Task_A]
duration = { dist = "fixed", value = "1m" }
[activities.Task_B]
duration = { dist = "fixed", value = "1m" }
[gateways.Split]
probabilities = { Flow_a = 0.7, Flow_b = 0.3 }
"#;

#[test]
fn v4_xor_frequencies() {
    let r = &run("v4-xor.bpmn", V4)[0];
    let a = r.activities[&QId::new("v4-xor.bpmn", "Task_A")].runs as f64;
    let b = r.activities[&QId::new("v4-xor.bpmn", "Task_B")].runs as f64;
    assert_eq!(a + b, 20000.0);
    // 5 standard deviations of a binomial(20000, 0.7)
    let tol = 5.0 * (20000.0_f64 * 0.7 * 0.3).sqrt();
    assert!((a - 14000.0).abs() < tol, "A ran {a} times");
}

const V5: &str = r#"
[simulation]
cases = 50
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

#[test]
fn v5_and_split_join_takes_the_longer_branch() {
    let r = &run("v5-and.bpmn", V5)[0];
    assert_eq!(r.case_cycle_time.min, 20.0 * MIN);
    assert_eq!(r.case_cycle_time.max, 20.0 * MIN);
    assert_eq!(
        r.end_events.values().sum::<u64>(),
        50,
        "one token per case reaches the end"
    );
    assert!(r.stuck.is_empty());
}

#[test]
fn v6_and_join_never_mixes_instances() {
    // Random durations and heavily overlapping cases: branches finish interleaved.
    let scenario = r#"
[simulation]
seed = 3
cases = 500
[[arrivals]]
inter_arrival = { dist = "exponential", mean = "10m" }
triggers = ["Start"]
[resources.TeamA]
count = 3
[resources.TeamB]
count = 3
[activities.Task_A]
duration = { dist = "uniform", min = "1m", max = "40m" }
[activities.Task_B]
duration = { dist = "uniform", min = "1m", max = "40m" }
"#;
    let r = &run("v5-and.bpmn", scenario)[0];
    assert_eq!(r.cases_completed, 500);
    for case in &r.cases {
        let ends: Vec<f64> = r
            .records
            .iter()
            .filter(|x| x.case == case.id)
            .map(|x| x.end)
            .collect();
        assert_eq!(ends.len(), 2);
        let join_time = ends.iter().cloned().fold(0.0, f64::max);
        assert_eq!(
            case.completion,
            Some(join_time),
            "case {} joined with a foreign token",
            case.id
        );
    }
}

#[test]
fn v12_same_seed_is_bit_identical() {
    let a = run(
        "v5-and.bpmn",
        &V5.replace(
            "fixed\", value = \"10m",
            "uniform\", min = \"5m\", max = \"15m",
        ),
    );
    let b = run(
        "v5-and.bpmn",
        &V5.replace(
            "fixed\", value = \"10m",
            "uniform\", min = \"5m\", max = \"15m",
        ),
    );
    assert_eq!(a, b);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn different_seeds_differ_and_replications_use_consecutive_seeds() {
    let s = V4.replace("cases = 20000", "cases = 200\nreplications = 2");
    let r = run("v4-xor.bpmn", &s);
    assert_eq!(r.len(), 2);
    assert_eq!((r[0].seed, r[1].seed), (7, 8));
    assert_ne!(r[0].activities, r[1].activities);
}

#[test]
fn warm_up_cases_are_left_out_of_statistics() {
    let s = v1("5m", 20).replace("cases = 20", "cases = 20\nwarm_up_cases = 5");
    let r = &run("v1-single-task.bpmn", &s)[0];
    assert_eq!(r.case_cycle_time.count, 15);
    assert_eq!(r.cases.iter().filter(|c| c.warm_up).count(), 5);
    assert_eq!(r.activities.values().next().unwrap().runs, 15);
}

#[test]
fn scenario_errors_are_reported_with_element_ids() {
    let e = bind_err("v4-xor.bpmn", &V4.replace("Flow_b = 0.3", "Flow_b = 0.2"));
    assert!(e.contains("sum to"), "{e}");
    let e = bind_err(
        "v4-xor.bpmn",
        &V4.replace("[gateways.Split]", "[gateways.Nope]"),
    );
    assert!(e.contains("Nope") && e.contains("Split"), "{e}");
    let e = bind_err(
        "v1-single-task.bpmn",
        &v1("1m", 5).replace("[resources.Dev]", "[resources.Ops]"),
    );
    assert!(e.contains("'Dev' is not defined"), "{e}");
    let e = bind_err(
        "v1-single-task.bpmn",
        &v1("1m", 5).replace("Start", "Task_Work"),
    );
    assert!(e.contains("not a start event"), "{e}");
}

#[test]
fn unsupported_elements_fail_binding() {
    let project = load("v11-link.bpmn");
    let s = Scenario::from_toml(
        "[simulation]\ncases=1\n[[arrivals]]\ninter_arrival={dist=\"fixed\",value=\"1m\"}\ntriggers=[\"Start\"]\n[activities.Task_A]\nduration={dist=\"fixed\",value=\"1m\"}\n",
    )
    .unwrap();
    let e = s.bind(&project).unwrap_err().to_string();
    assert!(e.contains("intermediate catch event"), "{e}");
}

#[test]
fn bare_ambiguous_ids_need_qualification() {
    let mut project = Project::new();
    project
        .add_path(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/multi-file/subprocess-a.bpmn"),
        )
        .unwrap();
    project
        .add_path(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/multi-file/subprocess-b.bpmn"),
        )
        .unwrap();
    let s = Scenario::from_toml("[simulation]\ncases=1\n[[arrivals]]\ninter_arrival={dist=\"fixed\",value=\"1m\"}\ntriggers=[\"Start_1\"]\n").unwrap();
    let e = s.bind(&project).unwrap_err().to_string();
    assert!(
        e.contains("ambiguous")
            && e.contains("subprocess-a.bpmn")
            && e.contains("subprocess-b.bpmn"),
        "{e}"
    );
}

#[test]
fn duration_parsing() {
    use sim_core::dist::parse_duration;
    assert_eq!(parse_duration("90s"), Ok(90.0));
    assert_eq!(parse_duration("1.5h"), Ok(5400.0));
    assert_eq!(parse_duration("2w"), Ok(1_209_600.0));
    assert!(parse_duration("10").is_err());
    assert!(parse_duration("5x").is_err());
}

#[test]
fn stuck_instances_are_reported_with_location() {
    // An XOR split feeding an AND join: the join waits forever for the other branch.
    let s = V4.replace("20000", "10");
    let r = &run("stuck-join.bpmn", &s)[0];
    assert_eq!(r.cases_started, 10);
    assert_eq!(r.cases_completed, 0);
    assert_eq!(r.stuck.len(), 10);
    assert!(
        r.stuck
            .iter()
            .all(|t| t.element.id == "Join" && t.state.contains("AND join"))
    );
}

#[test]
fn mm1_queue_matches_theory() {
    // lambda = 1/10 min, mu = 1/8 min, rho = 0.8: Wq = 32 min, W = 40 min.
    // (Proper confidence-interval check arrives with M5.)
    let s = r#"
[simulation]
seed = 11
replications = 5
cases = 60000
warm_up_cases = 5000
[[arrivals]]
inter_arrival = { dist = "exponential", mean = "10m" }
triggers = ["Start"]
[resources.Dev]
count = 1
[activities.Task_Work]
duration = { dist = "exponential", mean = "8m" }
"#;
    let rs = run("v1-single-task.bpmn", s);
    let n = rs.len() as f64;
    let wq = rs
        .iter()
        .map(|r| r.activities.values().next().unwrap().waiting.mean)
        .sum::<f64>()
        / n
        / MIN;
    let w = rs.iter().map(|r| r.case_cycle_time.mean).sum::<f64>() / n / MIN;
    assert!((wq - 32.0).abs() < 3.0, "Wq = {wq}");
    assert!((w - 40.0).abs() < 3.0, "W = {w}");
}

#[test]
fn skeleton_round_trips_through_toml_and_runs() {
    for fixture in ["v1-single-task.bpmn", "v4-xor.bpmn", "v5-and.bpmn"] {
        let project = load(fixture);
        let sk = Scenario::skeleton(&project, None);
        let text = sk.to_toml().unwrap();
        let back = Scenario::from_toml(&text).unwrap_or_else(|e| panic!("{fixture}: {e}\n{text}"));
        let bound = back
            .bind(&project)
            .unwrap_or_else(|e| panic!("{fixture}: {e}\n{text}"));
        let r = run_replications(&bound);
        assert_eq!(r[0].cases_completed, 100, "{fixture}");
    }
}

#[test]
fn exports_have_one_row_per_record() {
    let project = load("v5-and.bpmn");
    let r = run("v5-and.bpmn", V5);
    let log = sim_core::export::event_log_csv(&project, &r);
    assert_eq!(log.lines().count(), 1 + 100);
    assert!(log.lines().nth(1).unwrap().contains("v5-and.bpmn#Task_"));
    let sum = sim_core::export::summary_csv(&project, &r);
    assert!(sum.contains("resource,TeamA,TeamA,utilisation"));
}

#[test]
fn progress_is_reported_per_replication() {
    let project = load("v1-single-task.bpmn");
    let s = Scenario::from_toml(&v1("60m", 5).replace("seed = 1", "seed = 1\nreplications = 3"))
        .unwrap();
    let bound = s.bind(&project).unwrap();
    let mut seen = Vec::new();
    run_replications_with(&bound, |d, t| seen.push((d, t)));
    assert_eq!(seen, vec![(1, 3), (2, 3), (3, 3)]);
}
