//! What elements need from a scenario, updating a scenario from the model, problems tied to
//! elements and the pilot run (configuration in the desktop app).

use std::path::PathBuf;

use sim_core::needs::{Need, needs};
use sim_core::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn multi() -> Project {
    Project::open_main(&fixture("multi-file/main.bpmn")).expect("multi-file loads")
}

fn load(name: &str) -> Project {
    let mut p = Project::new();
    p.add_path(&fixture(name)).unwrap();
    p
}

#[test]
fn a_fresh_skeleton_binds_without_warnings() {
    let p = multi();
    let sk = Scenario::skeleton(&p, Some("main.bpmn"));
    let b = sk.bind(&p).unwrap_or_else(|e| panic!("{e}"));
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    // `amount` is read by two decisions but defined once.
    let defined: usize = sk
        .decisions
        .values()
        .filter(|d| d.inputs.contains_key("amount"))
        .count();
    assert_eq!(defined, 1);
}

#[test]
fn needs_name_the_parameters_of_each_element() {
    let p = load("v13-model.bpmn");
    let of = |id: &str| needs(&p, p.node(&QId::new("v13-model.bpmn", id)).unwrap());
    assert_eq!(of("Task_Intake"), [Need::Duration]);
    assert_eq!(of("Split"), [Need::Branching { inclusive: false }]);
    assert!(of("Start").is_empty());
}

#[test]
fn update_from_model_keeps_edits_and_fills_gaps() {
    let p = load("v13-model.bpmn");
    let mut s =
        Scenario::from_toml(&std::fs::read_to_string(fixture("v13-model.toml")).unwrap()).unwrap();
    s.activities.remove("Task_Close");
    s.activities
        .insert("Task_Gone".into(), s.activities["Task_Intake"].clone());
    let before = s.activities["Task_Intake"].duration.clone();
    let u = s.update_from_model(&p);
    assert_eq!(u.added, [QId::new("v13-model.bpmn", "Task_Close")]);
    assert!(u.pools_added.is_empty() && !u.arrival_added);
    assert_eq!(u.orphaned, ["activities.Task_Gone"]);
    assert_eq!(s.activities["Task_Intake"].duration, before);
    assert!(s.activities.contains_key("v13-model.bpmn#Task_Close"));
    s.remove_orphans(&p);
    assert!(s.orphans(&p).is_empty());
    s.bind(&p).unwrap_or_else(|e| panic!("{e}"));
    // A second update has nothing to do.
    assert!(s.update_from_model(&p).is_empty());
}

#[test]
fn missing_reports_unconfigured_needs() {
    let p = load("v13-model.bpmn");
    let mut s = Scenario::skeleton(&p, None);
    let task = p.node(&QId::new("v13-model.bpmn", "Task_Check")).unwrap();
    assert!(s.missing(&p, task).is_empty());
    s.activities.remove("v13-model.bpmn#Task_Check");
    assert_eq!(s.missing(&p, task), [Need::Duration]);
}

#[test]
fn check_ties_problems_to_elements() {
    let p = load("v13-model.bpmn");
    let mut s = Scenario::skeleton(&p, None);
    s.activities.remove("v13-model.bpmn#Task_Check");
    s.resources.get_mut("Clerks").unwrap().count = 0;
    let c = s.check(&p);
    assert!(c.bound.is_none());
    assert!(c.errors.iter().any(
        |e| e.element == Some(QId::new("v13-model.bpmn", "Task_Check"))
            && e.message.contains("no [activities] entry")
    ));
    assert!(c.errors.iter().any(|e| e.pool.as_deref() == Some("Clerks")));
    // `bind` reports the same messages.
    let text = s.bind(&p).unwrap_err().to_string();
    assert!(
        text.contains("Task_Check") && text.contains("Clerks"),
        "{text}"
    );
}

#[test]
fn pilot_estimates_load_and_counts_flows() {
    let p = load("v13-model.bpmn");
    let s =
        Scenario::from_toml(&std::fs::read_to_string(fixture("v13-model.toml")).unwrap()).unwrap();
    let pilot = sim_core::pilot::pilot(&p, &s).unwrap();
    assert_eq!(pilot.cases, sim_core::pilot::PILOT_CASES);
    // V13 is a stable system: both pools have a load well below 1.
    let clerks = pilot.pools["Clerks"].load;
    let experts = pilot.pools["Experts"].load;
    assert!((0.55..0.85).contains(&clerks), "{clerks}");
    assert!((0.2..0.45).contains(&experts), "{experts}");
    assert_eq!(pilot.overloaded().count(), 0);
    let simple = pilot.flows[&QId::new("v13-model.bpmn", "Flow_simple")] as f64;
    assert!((simple / 300.0 - 0.6).abs() < 0.1, "{simple}");

    // One expert for 40 % of the cases at 10 min each, but a case every 2 minutes: overloaded.
    let mut busy = s.clone();
    busy.arrivals[0].inter_arrival = sim_core::dist::DistSpec::Exponential {
        mean: sim_core::dist::Seconds(120.0),
    };
    let pilot = sim_core::pilot::pilot(&p, &busy).unwrap();
    assert!(pilot.overloaded().any(|(name, _)| name == "Experts"));
}
