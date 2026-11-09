//! Findings derived from a replication (bpmn-sim-3): where the time goes, whether the process
//! keeps up, which pool holds it up.

use std::path::PathBuf;

use sim_core::insights::{Level, Load, Topic};
use sim_core::*;

fn run(fixture: &str, scenario: &str) -> ReplicationResult {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(fixture);
    let mut p = Project::new();
    p.add_path(&path).expect("fixture parses");
    let bound = Scenario::from_toml(scenario)
        .expect("scenario parses")
        .bind(&p)
        .expect("scenario binds");
    run_replications(&bound).remove(0)
}

fn single_task(interval: &str, count: u32, cases: u32) -> String {
    format!(
        r#"
[simulation]
seed = 1
cases = {cases}
[[arrivals]]
inter_arrival = {{ dist = "exponential", mean = "{interval}" }}
triggers = ["Start"]
[resources.Dev]
count = {count}
[activities.Task_Work]
duration = {{ dist = "exponential", mean = "10m" }}
"#
    )
}

#[test]
fn overload_is_reported_with_its_bottleneck() {
    // A task of 10 minutes every 7 minutes on one person: utilisation would be 143%.
    let r = run("v1-single-task.bpmn", &single_task("7m", 1, 600));
    let i = Insights::of(&r);
    assert!(i.trend.unwrap().growing(), "{:?}", i.trend);
    assert_eq!(i.bottleneck.as_deref(), Some("Dev"));
    let dev = i.pool("Dev").unwrap();
    assert_eq!(dev.load, Load::Saturated);
    assert!(dev.max_queue > 10, "{}", dev.max_queue);
    assert!(dev.mean_queue > 1.0);
    assert_eq!(dev.activities.len(), 1);
    assert!((dev.utilisation_with(2) - dev.utilisation / 2.0).abs() < 1e-12);
    // Waiting dominates the cycle time.
    assert!(i.time.wait > i.time.work);
    let f = i.findings(&r);
    assert_eq!(f[0].level, Level::Problem);
    assert!(f[0].text.contains("does not keep up"), "{}", f[0].text);
    assert!(f[0].text.contains("Dev"));
    assert!(
        f.iter()
            .any(|f| f.topic == Topic::Resources && f.text.starts_with("Dev is"))
    );
    // Work in progress keeps rising while cases arrive.
    let peak = i.wip.iter().map(|p| p[1]).fold(0.0, f64::max);
    assert!(peak > 20.0, "{peak}");
}

#[test]
fn light_load_has_no_bottleneck_and_flags_idle_staff() {
    let r = run("v1-single-task.bpmn", &single_task("60m", 3, 300));
    let i = Insights::of(&r);
    assert!(!i.trend.unwrap().growing());
    assert_eq!(i.bottleneck, None);
    let dev = i.pool("Dev").unwrap();
    assert_eq!(dev.load, Load::Idle);
    let f = i.findings(&r);
    assert!(f.iter().all(|f| f.level == Level::Info), "{f:?}");
    assert!(f.iter().any(|f| f.text.contains("busy only")), "{f:?}");
    // One task per case, nothing else: the cycle time is work plus waiting.
    let t = &i.time;
    assert!(!t.parallel);
    assert!((t.work + t.wait + t.paused + t.other - t.cycle).abs() < 1e-6 * t.cycle);
    assert!(t.other < 1e-6 * t.cycle, "{t:?}");
}

#[test]
fn case_rows_add_up_the_tasks_of_each_case() {
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/v13-model.toml"),
    )
    .unwrap()
    .replace("cases = 5000", "cases = 800")
    .replace("warm_up_cases = 500", "warm_up_cases = 50");
    let r = run("v13-model.bpmn", &text);
    let i = Insights::of(&r);
    assert_eq!(i.cases.len(), r.cases.len());
    let tasks: usize = i.cases.iter().map(|c| c.tasks).sum();
    assert_eq!(tasks, r.records.len());
    let work: f64 = i.cases.iter().map(|c| c.work).sum();
    let records: f64 = r.records.iter().map(|x| x.work).sum();
    assert!((work - records).abs() < 1e-6 * records);
    // Check and Review run in parallel.
    assert!(i.time.parallel, "{:?}", i.time);
    // Statistics count the same cases as the cycle time summary.
    assert_eq!(i.time.cases, r.case_cycle_time.count);
    assert!((i.time.cycle - r.case_cycle_time.mean).abs() < 1e-6);
    assert_eq!(
        i.pools.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Clerks", "Experts"]
    );
    let f = i.findings(&r);
    assert!(
        f.iter().any(|f| f.text.starts_with("An average case took")),
        "{f:?}"
    );
    for f in &f {
        println!("{:?} {:?}: {}", f.level, f.topic, f.text);
    }
}
