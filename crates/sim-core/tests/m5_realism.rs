//! Calendars, horizon, replications with confidence intervals, V3 and exports (M5).

use std::path::PathBuf;

use sim_core::*;

const MIN: f64 = 60.0;
const H: f64 = 3600.0;

fn load(fixture: &str) -> Project {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(fixture);
    let mut p = Project::new();
    p.add_path(&path).expect("fixture parses");
    p
}

fn run_all(fixture: &str, scenario: &str) -> Vec<ReplicationResult> {
    let project = load(fixture);
    let bound = Scenario::from_toml(scenario)
        .unwrap()
        .bind(&project)
        .unwrap_or_else(|e| panic!("{e}"));
    run_replications(&bound)
}

fn run(fixture: &str, scenario: &str) -> ReplicationResult {
    run_all(fixture, scenario).remove(0)
}

fn bind_err(fixture: &str, scenario: &str) -> String {
    let project = load(fixture);
    Scenario::from_toml(scenario)
        .unwrap()
        .bind(&project)
        .unwrap_err()
        .to_string()
}

const OFFICE: &str = "[calendars.office]\ndays = [\"mon\", \"tue\", \"wed\", \"thu\", \"fri\"]\nhours = \"09:00-17:00\"\n";

/// One resource `Dev` (optionally with the office calendar) and one task `Task_Work`.
fn single(sim: &str, interval: &str, task: &str, calendar: bool) -> String {
    format!(
        r#"
[simulation]
{sim}
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "{interval}" }}
triggers = ["Start"]
[resources.Dev]
count = 1
cost_per_hour = 60.0
{}
[activities.Task_Work]
duration = {{ dist = "fixed", value = "{task}" }}
{}
"#,
        if calendar {
            "calendar = \"office\""
        } else {
            ""
        },
        if calendar { OFFICE } else { "" },
    )
}

// ---------------------------------------------------------------- calendars

#[test]
fn work_outside_opening_hours_waits_for_the_next_opening() {
    // Case arrives Monday 00:00 (the default start); the office opens at 09:00.
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 1", "1d", "10m", true),
    );
    assert_eq!(r.case_cycle_time.mean, 9.0 * H + 10.0 * MIN);
    let rec = &r.records[0];
    assert_eq!(
        (rec.queued, rec.start, rec.end),
        (0.0, 9.0 * H, 9.0 * H + 10.0 * MIN)
    );
    assert_eq!(rec.waiting(), 9.0 * H);
    assert_eq!(
        rec.processing(),
        10.0 * MIN,
        "processing time is working time"
    );
}

#[test]
fn a_running_task_pauses_at_closing_time_and_continues_next_morning() {
    // Arrival Monday 15:00, ten hours of work: 2h until 17:00, 8h on Tuesday.
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 1\nstart = \"2024-01-01T15:00\"", "1d", "10h", true),
    );
    let rec = &r.records[0];
    assert_eq!(rec.start, 0.0);
    assert_eq!(rec.end, 26.0 * H, "Tuesday 17:00");
    assert_eq!(rec.work, 10.0 * H);
    assert_eq!(rec.waiting(), 0.0);
    assert_eq!(r.case_cycle_time.mean, 26.0 * H);
    assert!(
        (r.resources["Dev"].cost - 10.0 * 60.0).abs() < 1e-9,
        "paid for working time only"
    );
}

#[test]
fn weekends_are_skipped_and_the_start_date_decides_the_weekday() {
    // Friday 16:00 + 2h of work: 1h on Friday, 1h on Monday.
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 1\nstart = \"2024-01-05T16:00\"", "1d", "2h", true),
    );
    // Monday 10:00 is 66 hours after Friday 16:00.
    assert_eq!(r.records[0].end, 66.0 * H);
}

#[test]
fn utilisation_is_busy_time_over_opening_time() {
    // Five days, one 4h task per day at 09:00: 20h busy of 40h open (the window covers five days).
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 5\nstart = \"2024-01-01T09:00\"", "1d", "4h", true),
    );
    assert_eq!(r.resources["Dev"].busy_time, 20.0 * H);
    assert_eq!(r.resources["Dev"].utilisation, 0.5);
    assert!(r.case_cycle_time.max == 4.0 * H);
}

#[test]
fn a_calendar_can_have_other_hours_on_other_days() {
    // Weekday evenings and long weekends; the case arrives Friday 23:00 with two hours of work.
    let shifts = r#"[calendars.office]
periods = [
  { days = ["mon", "tue", "wed", "thu", "fri"], hours = "20:00-24:00" },
  { days = ["sat", "sun"], hours = "08:00-24:00" },
]
"#;
    let text =
        single("cases = 1\nstart = \"2024-01-05T23:00\"", "1d", "2h", true).replace(OFFICE, shifts);
    let s = Scenario::from_toml(&text).unwrap();
    assert_eq!(s.calendars["office"].periods.len(), 2);
    let saved = s.to_toml().unwrap();
    assert_eq!(Scenario::from_toml(&saved).unwrap(), s, "{saved}");
    let r = run("v1-single-task.bpmn", &text);
    let rec = &r.records[0];
    assert_eq!(
        (rec.start, rec.end),
        (0.0, 10.0 * H),
        "1h on Friday, 1h from Saturday 08:00"
    );
    assert_eq!(rec.processing(), 2.0 * H);
}

#[test]
fn tasks_without_a_calendar_are_unaffected_and_unknown_calendars_fail() {
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 1", "1d", "10m", false),
    );
    assert_eq!(r.case_cycle_time.mean, 10.0 * MIN);
    let e = bind_err(
        "v1-single-task.bpmn",
        &single("cases = 1", "1d", "10m", true)
            .replace("calendar = \"office\"", "calendar = \"night\""),
    );
    assert!(e.contains("night") && e.contains("[calendars]"), "{e}");
    let e = bind_err(
        "v1-single-task.bpmn",
        &single("cases = 1", "1d", "10m", true).replace("09:00-17:00", "17:00-09:00"),
    );
    assert!(e.contains("office") && e.contains("overnight"), "{e}");
}

#[test]
fn cancelling_a_paused_task_counts_only_the_hours_worked() {
    // Task of 10h starts Monday 09:00; the boundary timer fires after 12 wall-clock hours (21:00),
    // by which the task has worked 8h and is paused for the night.
    let s = single("cases = 1\nstart = \"2024-01-01T09:00\"", "1d", "10h", true)
        .replace("Task_Work", "Task_Long")
        + "[activities.Task_Handle]\nduration = { dist = \"fixed\", value = \"1m\" }\n[boundary.B]\ndelay = { dist = \"fixed\", value = \"12h\" }\n";
    let r = run("v9-boundary-timer.bpmn", &s);
    let long = r
        .records
        .iter()
        .find(|x| x.activity.id == "Task_Long")
        .unwrap();
    assert!(long.cancelled);
    assert_eq!((long.start, long.end, long.work), (0.0, 12.0 * H, 8.0 * H));
    assert_eq!(r.resources["Dev"].busy_time, 8.0 * H);
}

// ---------------------------------------------------------------- horizon

#[test]
fn horizon_cuts_the_run_and_reports_unfinished_work_not_a_deadlock() {
    let r = run(
        "v1-single-task.bpmn",
        &single("horizon = \"65m\"", "10m", "10m", false),
    );
    assert!(r.horizon_reached);
    assert_eq!(r.end_time, 65.0 * MIN);
    // Arrivals at 0, 10, ..., 60 start 7 cases; cases 0..=5 are done by minute 60, case 6 is running.
    assert_eq!((r.cases_started, r.cases_completed), (7, 6));
    assert!(r.stuck.is_empty());
    assert_eq!(r.unfinished.len(), 1);
    assert_eq!(r.unfinished[0].state, "working");
    // Busy time includes the 5 minutes the running task has worked.
    assert_eq!(r.resources["Dev"].busy_time, 65.0 * MIN);
    assert_eq!(r.resources["Dev"].utilisation, 1.0);
}

#[test]
fn cases_and_horizon_stop_at_whichever_comes_first() {
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 3\nhorizon = \"10d\"", "10m", "1m", false),
    );
    assert!(!r.horizon_reached);
    assert_eq!(r.cases_completed, 3);
    let r = run(
        "v1-single-task.bpmn",
        &single("cases = 1000\nhorizon = \"1h\"", "10m", "1m", false),
    );
    assert!(r.horizon_reached);
    assert_eq!(r.cases_started, 7);
    let e = bind_err("v1-single-task.bpmn", &single("", "10m", "1m", false));
    assert!(
        e.contains("simulation.cases") && e.contains("horizon"),
        "{e}"
    );
}

// ---------------------------------------------------------------- replications

const MM1: &str = r#"
[simulation]
seed = 2024
replications = 20
cases = 40000
warm_up_cases = 4000
[[arrivals]]
inter_arrival = { dist = "exponential", mean = "10m" }
triggers = ["Start"]
[resources.Dev]
count = 1
[activities.Task_Work]
duration = { dist = "exponential", mean = "8m" }
"#;

#[test]
fn v3_mm1_matches_theory_within_the_confidence_interval() {
    // lambda = 1/10 min, mu = 1/8 min, rho = 0.8: Wq = rho / (mu - lambda) = 32 min, W = 40 min.
    let rs = run_all("v1-single-task.bpmn", MM1);
    assert_eq!(rs.len(), 20);
    let agg = aggregate(&rs);
    let task = &agg.activities[&QId::new("v1-single-task.bpmn", "Task_Work")];
    let wq = &task.waiting_mean;
    let w = &agg.case_cycle_time_mean;
    assert!(
        wq.contains(32.0 * MIN),
        "Wq = {} +- {:?} min",
        wq.mean / MIN,
        wq.ci95.map(|c| c / MIN)
    );
    assert!(
        w.contains(40.0 * MIN),
        "W = {} +- {:?} min",
        w.mean / MIN,
        w.ci95.map(|c| c / MIN)
    );
    assert!(
        agg.resources["Dev"].utilisation.contains(0.8),
        "{:?}",
        agg.resources["Dev"].utilisation
    );
    // The interval is informative, not huge.
    assert!(wq.ci95.unwrap() < 0.1 * wq.mean);
    assert!(task.processing_mean.contains(8.0 * MIN));
}

#[test]
fn aggregate_handles_missing_elements_and_single_replications() {
    let rs = run_all(
        "v1-single-task.bpmn",
        &single("cases = 10\nreplications = 3", "20m", "10m", false),
    );
    let agg = aggregate(&rs);
    assert_eq!(agg.replications, 3);
    assert_eq!(agg.cases_completed.mean, 10.0);
    assert_eq!(agg.cases_completed.ci95, Some(0.0));
    assert_eq!(agg.case_cycle_time_mean.mean, 10.0 * MIN);
    let one = aggregate(&rs[..1]);
    assert_eq!(one.case_cycle_time_mean.ci95, None);
    assert!(
        !one.case_cycle_time_mean.contains(10.0 * MIN),
        "no interval from a single run"
    );
}

// ---------------------------------------------------------------- exports

#[test]
fn csv_and_xes_event_logs_are_well_formed() {
    let project = load("v9-boundary-timer.bpmn");
    let s = "[simulation]\ncases = 3\nstart = \"2026-03-02T08:30\"\n[[arrivals]]\ninter_arrival = { dist = \"fixed\", value = \"25m\" }\ntriggers = [\"Start\"]\n[resources.Dev]\ncount = 1\n[activities.Task_Long]\nduration = { dist = \"fixed\", value = \"60m\" }\n[activities.Task_Handle]\nduration = { dist = \"fixed\", value = \"5m\" }\n[boundary.B]\ndelay = { dist = \"fixed\", value = \"20m\" }\n";
    let bound = Scenario::from_toml(s).unwrap().bind(&project).unwrap();
    let rs = run_replications(&bound);
    let r = &rs[0];
    assert_eq!(
        r.records.len(),
        6,
        "a cancelled Task_Long and a Task_Handle per case"
    );

    let csv = export::event_log_csv(&project, &rs);
    let rows: Vec<Vec<&str>> = csv.lines().map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), 7);
    assert!(rows.iter().all(|r| r.len() == rows[0].len()));
    assert!(csv.contains("2026-03-02T08:30:00Z"), "{csv}");
    assert!(
        csv.lines().nth(1).unwrap().contains(",true,"),
        "the first record is the cancelled Task_Long"
    );

    let xes = export::event_log_xes(&project, &rs);
    let doc = roxmltree::Document::parse(&xes).expect("XES is well-formed XML");
    let count = |n: &str| doc.descendants().filter(|e| e.has_tag_name(n)).count();
    assert_eq!(count("trace"), 3);
    // Task_Long: start + abort; Task_Handle: start + complete.
    assert_eq!(count("event"), 12);
    let transitions: Vec<&str> = doc
        .descendants()
        .filter(|e| {
            e.has_tag_name("string")
                && e.attribute("key") == Some("lifecycle:transition")
                && e.parent().is_some_and(|p| p.has_tag_name("event"))
        })
        .filter_map(|e| e.attribute("value"))
        .collect();
    assert_eq!(transitions.iter().filter(|t| **t == "ate_abort").count(), 3);
    assert_eq!(transitions.iter().filter(|t| **t == "complete").count(), 3);
    assert!(xes.contains("org:resource"));
}

#[test]
fn aggregate_csv_has_a_row_per_metric() {
    let project = load("v1-single-task.bpmn");
    let rs = run_all(
        "v1-single-task.bpmn",
        &single("cases = 10\nreplications = 4", "20m", "10m", false),
    );
    let csv = export::aggregate_csv(&project, &aggregate(&rs));
    assert!(csv.starts_with("scope,id,name,metric,n,mean,std_dev,ci95_half_width"));
    assert!(csv.contains("case_cycle_time,,,mean_s,4,600,"));
    assert!(csv.contains("resource,Dev,Dev,utilisation,4,"));
    assert!(csv.lines().all(|l| l.split(',').count() == 8));
}

#[test]
fn bad_start_dates_are_scenario_errors() {
    let e = bind_err(
        "v1-single-task.bpmn",
        &single("cases = 1\nstart = \"tomorrow\"", "1d", "1m", false),
    );
    assert!(e.contains("start") && e.contains("YYYY-MM-DD"), "{e}");
}
