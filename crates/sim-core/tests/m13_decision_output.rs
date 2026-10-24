//! Gateways that follow a decision output: the `[gateways]` entry names the variable a
//! business rule task's decision sets and which value(s) take which flow.

use std::path::PathBuf;

use sim_core::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dmn-output")
        .join(name)
}

fn ticket() -> Project {
    Project::open_main(&fixture("ticket.bpmn")).expect("ticket model loads")
}

fn scenario() -> String {
    std::fs::read_to_string(fixture("scenario.toml"))
        .unwrap()
        .replace("\r\n", "\n")
}

fn bind(s: &str) -> BoundModel<'static> {
    let p = Box::leak(Box::new(ticket()));
    Scenario::from_toml(s)
        .unwrap()
        .bind(p)
        .unwrap_or_else(|e| panic!("{e}"))
}

fn run(s: &str) -> ReplicationResult {
    run_replications(&bind(s)).remove(0)
}

fn bind_err(s: &str) -> String {
    Scenario::from_toml(s)
        .unwrap()
        .bind(&ticket())
        .unwrap_err()
        .to_string()
}

fn runs(r: &ReplicationResult, task: &str) -> f64 {
    r.activities
        .get(&QId::new("ticket.bpmn", task))
        .map_or(0.0, |a| a.runs as f64)
}

/// Replace the `[gateways.<id>]` entry of the fixture scenario by `entry`.
fn with_gateway(id: &str, entry: &str) -> String {
    let s = scenario();
    let head = format!("[gateways.{id}]\n");
    let start = s.find(&head).expect("entry exists");
    let end = s[start + head.len()..]
        .find("\n[")
        .map_or(s.len(), |i| start + head.len() + i + 1);
    format!("{}{head}{entry}\n{}", &s[..start], &s[end..])
}

#[test]
fn both_decisions_load_and_the_scenario_binds_without_warnings() {
    let p = ticket();
    assert_eq!(p.dmn.len(), 2);
    let b = bind(&scenario());
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
}

#[test]
fn the_outputs_of_one_decision_drive_two_gateways_and_a_list_drives_an_inclusive_one() {
    let r = run(&scenario());
    assert_eq!(r.cases_completed, 4000);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    let n = 4000.0;
    let near = |task: &str, p: f64| {
        let c = runs(&r, task);
        assert!(
            (c - n * p).abs() < 5.0 * (n * p * (1.0 - p)).sqrt(),
            "{task}: {c} runs, expected about {}",
            n * p
        );
    };
    near("Task_Front", 0.5);
    near("Task_Back", 0.3);
    near("Task_Expert", 0.2);
    near("Task_Escalate", 0.32);
    near("Task_Legal", 0.8);
    near("Task_Tech", 0.5);

    // The paths agree with what the decision gave, case by case.
    let d = &r.decisions["ticket-triage"];
    let count = |queue: &str| -> f64 {
        d.outcomes
            .iter()
            .filter(|(k, _)| k.contains(&format!("queue=\"{queue}\"")))
            .map(|(_, c)| *c as f64)
            .sum()
    };
    assert_eq!(count("front"), runs(&r, "Task_Front"));
    assert_eq!(count("back"), runs(&r, "Task_Back"));
    assert_eq!(count("specialist"), runs(&r, "Task_Expert"));
    let urgent: f64 = d
        .outcomes
        .iter()
        .filter(|(k, _)| k.contains("priority=\"urgent\""))
        .map(|(_, c)| *c as f64)
        .sum();
    assert_eq!(urgent, runs(&r, "Task_Escalate"));
}

#[test]
fn fixed_inputs_give_fixed_paths() {
    let fixed = |category: &str, severity: f64| {
        scenario()
            .replace(
                "category = { choice = { billing = 0.5, account = 0.3, outage = 0.2 } }",
                &format!("category = {{ choice = {{ {category} = 1.0 }} }}"),
            )
            .replace(
                "severity = { dist = \"uniform\", min = 0.0, max = 5.0 }",
                &format!("severity = {{ dist = \"fixed\", value = {severity} }}"),
            )
            .replace("cases = 4000", "cases = 10")
    };
    let r = run(&fixed("account", 4.0));
    assert_eq!(
        [
            "Task_Front",
            "Task_Back",
            "Task_Expert",
            "Task_Escalate",
            "Task_Legal",
            "Task_Tech"
        ]
        .map(|t| runs(&r, t)),
        [0.0, 10.0, 0.0, 10.0, 10.0, 10.0]
    );
    let r = run(&fixed("account", 1.0));
    assert_eq!(runs(&r, "Task_Escalate"), 0.0, "normal priority");
    let r = run(&fixed("billing", 1.0));
    assert_eq!(
        (
            runs(&r, "Task_Front"),
            runs(&r, "Task_Legal"),
            runs(&r, "Task_Tech")
        ),
        (10.0, 10.0, 0.0)
    );
}

#[test]
fn several_values_can_take_one_flow() {
    let s = with_gateway(
        "Gateway_Queue",
        "output = \"triage.queue\"\nvalues = { Flow_front = [\"front\", \"back\"], Flow_expert = \"specialist\", Flow_back = \"nobody\" }",
    );
    let b = bind(&s);
    assert!(
        b.warnings
            .iter()
            .any(|w| w.contains("Flow_back") && w.contains("never gives \"nobody\"")),
        "{:?}",
        b.warnings
    );
    let r = run_replications(&b).remove(0);
    assert_eq!(runs(&r, "Task_Back"), 0.0);
    assert_eq!(runs(&r, "Task_Front") + runs(&r, "Task_Expert"), 4000.0);
}

#[test]
fn the_output_name_alone_works_as_well() {
    // The engine stores each output under its own name too.
    let s = with_gateway(
        "Gateway_Queue",
        "output = \"queue\"\nvalues = { Flow_front = \"front\", Flow_back = \"back\", Flow_expert = \"specialist\" }",
    );
    let r = run(&s.replace("cases = 4000", "cases = 200"));
    assert_eq!(
        runs(&r, "Task_Front") + runs(&r, "Task_Back") + runs(&r, "Task_Expert"),
        200.0
    );
}

#[test]
fn an_unmapped_value_without_default_flow_is_reported_and_leaves_the_token_stuck() {
    let s = with_gateway(
        "Gateway_Queue",
        "output = \"triage.queue\"\nvalues = { Flow_front = \"front\", Flow_back = \"back\", Flow_expert = \"elsewhere\" }",
    )
    .replace("cases = 4000", "cases = 50");
    let b = bind(&s);
    assert!(
        b.warnings
            .iter()
            .any(|w| w.contains("can give \"specialist\"") && w.contains("no flow takes")),
        "{:?}",
        b.warnings
    );
    let r = run_replications(&b).remove(0);
    assert!(!r.stuck.is_empty());
    assert!(
        r.stuck.iter().all(|t| t.element.id == "Gateway_Queue"
            && t.state.contains("triage.queue = \"specialist\"")),
        "{:?}",
        r.stuck
    );
}

#[test]
fn the_default_flow_takes_what_no_value_matches() {
    // Only "urgent" is mapped; "normal" goes along the default flow without a warning.
    let b = bind(&scenario());
    assert!(!b.warnings.iter().any(|w| w.contains("Gateway_Priority")));
    let s = with_gateway(
        "Gateway_Priority",
        "output = \"triage.priority\"\nvalues = { Flow_urgent = \"urgent\", Flow_normal = \"normal\" }",
    );
    assert!(bind_err(&s).contains("default flow"), "{}", bind_err(&s));
}

#[test]
fn entries_are_checked() {
    let err = |entry: &str| bind_err(&with_gateway("Gateway_Queue", entry));
    let all =
        "values = { Flow_front = \"front\", Flow_back = \"back\", Flow_expert = \"specialist\" }";

    // The result variable of a table with several outputs holds all of them.
    let e = err(&format!("output = \"triage\"\n{all}"));
    assert!(
        e.contains("several decision outputs")
            && e.contains("triage.queue")
            && e.contains("triage.priority"),
        "{e}"
    );
    let e = err(&format!("output = \"nothing\"\n{all}"));
    assert!(
        e.contains("not set by any decision") && e.contains("checks"),
        "{e}"
    );
    let e = err(all);
    assert!(e.contains("values need output"), "{e}");
    let e =
        err("output = \"triage.queue\"\nvalues = { Flow_front = \"front\", Flow_back = \"back\" }");
    assert!(
        e.contains("no value for outgoing flow 'Flow_expert'"),
        "{e}"
    );
    let e = err(&format!(
        "output = \"triage.queue\"\n{all}\nprobabilities = {{ Flow_front = 1.0 }}"
    ));
    assert!(e.contains("not several"), "{e}");
    let e = err(
        "output = \"triage.queue\"\nvalues = { Flow_front = \"front\", Flow_back = \"back\", Flow_expert = \"specialist\", F1 = \"x\" }",
    );
    assert!(e.contains("'F1' is not an outgoing flow"), "{e}");
    let e = err(
        "output = \"triage.queue\"\nvalues = { Flow_front = [], Flow_back = \"back\", Flow_expert = \"specialist\" }",
    );
    assert!(e.contains("no value for flow 'Flow_front'"), "{e}");
}

#[test]
fn values_are_written_back_as_toml() {
    let s = Scenario::from_toml(&scenario()).unwrap();
    let back = Scenario::from_toml(&s.to_toml().unwrap()).unwrap();
    assert_eq!(s, back);
    let g = &back.gateways["Gateway_Checks"];
    assert_eq!(g.output.as_deref(), Some("checks"));
    assert!(g.probabilities.is_empty());
}

#[test]
fn the_skeleton_maps_flow_names_to_decision_values() {
    let p = ticket();
    let sk = Scenario::skeleton(&p, None);
    let g = |id: &str| &sk.gateways[&format!("ticket.bpmn#{id}")];
    assert_eq!(g("Gateway_Queue").output.as_deref(), Some("triage.queue"));
    let one = |v: &str| scenario::OutputValues::One(scenario::OutputValue::Str(v.into()));
    assert_eq!(g("Gateway_Queue").values["Flow_front"], one("front"));
    assert_eq!(g("Gateway_Queue").values["Flow_back"], one("back"));
    assert_eq!(g("Gateway_Queue").values["Flow_expert"], one("specialist"));
    // After the join the nearest decision is still the triage; its other output matches.
    assert_eq!(
        g("Gateway_Priority").output.as_deref(),
        Some("triage.priority")
    );
    assert_eq!(
        g("Gateway_Priority").values.len(),
        1,
        "the default flow takes no value"
    );
    assert_eq!(g("Gateway_Checks").output.as_deref(), Some("checks"));

    // The skeleton binds without warnings and runs.
    let back = Scenario::from_toml(&sk.to_toml().unwrap()).unwrap();
    let b = back.bind(&p).unwrap_or_else(|e| panic!("{e}"));
    assert!(b.warnings.is_empty(), "{:?}", b.warnings);
    let r = run_replications(&b).remove(0);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
}

#[test]
fn flows_whose_names_match_no_decision_value_get_shares() {
    let text = std::fs::read_to_string(fixture("ticket.bpmn"))
        .unwrap()
        .replace("name=\"Back office\"", "name=\"Elsewhere\"");
    let mut p = Project::new();
    p.add_source("ticket.bpmn", &text).unwrap();
    for f in ["ticket-triage.dmn", "ticket-checks.dmn"] {
        p.add_path(&fixture(f)).unwrap();
    }
    let sk = Scenario::skeleton(&p, None);
    let g = &sk.gateways["ticket.bpmn#Gateway_Queue"];
    assert!(g.output.is_none());
    assert_eq!(g.probabilities.len(), 3);
}

#[test]
fn the_preview_shows_what_the_decision_gives_for_the_inputs() {
    let p = ticket();
    let s = Scenario::from_toml(&scenario()).unwrap();
    let pr = needs::preview_decision(&p, &s, "ticket-triage", 4000).unwrap();
    assert_eq!((pr.samples, pr.no_match, pr.violations), (4000, 0, 0));
    assert!(pr.unknown.is_empty(), "{:?}", pr.unknown);
    let (name, queue) = &pr.outputs[0];
    assert_eq!(name, "queue");
    assert_eq!(queue[0].0, "front", "most frequent first");
    let share = |v: &str| queue.iter().find(|(k, _)| k == v).unwrap().1 as f64 / 4000.0;
    assert!((share("front") - 0.5).abs() < 0.04);
    assert!((share("specialist") - 0.2).abs() < 0.04);
    // A COLLECT list counts once per value it holds.
    let pr = needs::preview_decision(&p, &s, "ticket-checks", 4000).unwrap();
    let legal = pr.outputs[0]
        .1
        .iter()
        .find(|(k, _)| k == "legal")
        .unwrap()
        .1;
    assert!((legal as f64 / 4000.0 - 0.8).abs() < 0.04);

    // Inputs without a distribution are null: here nothing matches.
    let none = Scenario::from_toml("").unwrap();
    let pr = needs::preview_decision(&p, &none, "ticket-triage", 100).unwrap();
    assert_eq!(pr.no_match, 100);
    assert_eq!(pr.unknown.len(), 2);
    assert_eq!(pr.outputs[0].1, [("nothing".to_string(), 100)]);
}

#[test]
fn inputs_are_described_with_their_labels_and_types() {
    let p = ticket();
    let d = needs::describe_input(&p, "ticket-triage", "category").unwrap();
    assert_eq!(d.title(), "Category (category)");
    assert_eq!(d.kind, dmn::InputKind::Text);
    assert_eq!(d.values, ["billing", "account", "outage"]);
    let d = needs::describe_input(&p, "ticket-triage", "severity").unwrap();
    assert_eq!((d.kind, d.thresholds), (dmn::InputKind::Number, vec![3.0]));
    // Found under another decision that reads the same variable.
    assert!(needs::describe_input(&p, "ticket-checks", "severity").is_some());
}
