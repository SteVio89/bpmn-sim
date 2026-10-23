//! DMN decision tables evaluated in the simulation, gateway conditions (M6).

use std::path::PathBuf;

use sim_core::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn claim() -> Project {
    Project::open_main(&fixture("dmn/claim.bpmn")).expect("claim model loads")
}

fn scenario() -> String {
    std::fs::read_to_string(fixture("dmn/scenario.toml"))
        .unwrap()
        .replace("\r\n", "\n")
}

fn run(s: &str) -> ReplicationResult {
    let p = claim();
    run_replications(
        &Scenario::from_toml(s)
            .unwrap()
            .bind(&p)
            .unwrap_or_else(|e| panic!("{e}")),
    )
    .remove(0)
}

fn bind_err(s: &str) -> String {
    Scenario::from_toml(s)
        .unwrap()
        .bind(&claim())
        .unwrap_err()
        .to_string()
}

fn runs(r: &ReplicationResult, task: &str) -> f64 {
    r.activities
        .get(&QId::new("claim.bpmn", task))
        .map_or(0.0, |a| a.runs as f64)
}

#[test]
fn the_decision_file_is_discovered_and_the_model_validates() {
    let p = claim();
    assert_eq!(p.dmn.len(), 1);
    assert!(validate(&p).is_empty(), "{:?}", validate(&p));
}

#[test]
fn decision_output_drives_the_gateway_conditions() {
    let r = run(&scenario());
    assert_eq!(r.cases_completed, 4000);
    assert!(r.stuck.is_empty());
    let n = 4000.0;
    let within = |count: f64, p: f64| (count - n * p).abs() < 5.0 * (n * p * (1.0 - p)).sqrt();
    assert!(
        within(runs(&r, "Task_Pay"), 0.4),
        "auto {}",
        runs(&r, "Task_Pay")
    );
    assert!(
        within(runs(&r, "Task_Manual"), 0.1),
        "manual {}",
        runs(&r, "Task_Manual")
    );
    assert!(
        within(runs(&r, "Task_Reject"), 0.5),
        "reject {}",
        runs(&r, "Task_Reject")
    );

    // The decision statistics agree with the paths taken.
    let d = &r.decisions["claim-route"];
    assert_eq!(d.evaluations, 4000);
    assert_eq!((d.no_match, d.violations), (0, 0));
    assert_eq!(d.outcomes["\"auto\""] as f64, runs(&r, "Task_Pay"));
    assert_eq!(d.outcomes["\"manual\""] as f64, runs(&r, "Task_Manual"));
    assert_eq!(d.outcomes["\"reject\""] as f64, runs(&r, "Task_Reject"));
}

#[test]
fn fixed_inputs_give_fixed_routes() {
    let fixed = |amount: f64, segment: &str| {
        scenario()
            .replace(
                "amount = { dist = \"uniform\", min = 0.0, max = 10000.0 }",
                &format!("amount = {{ dist = \"fixed\", value = {amount} }}"),
            )
            .replace(
                "segment = { choice = { gold = 0.25, silver = 0.75 } }",
                &format!("segment = {{ choice = {{ {segment} = 1.0 }} }}"),
            )
            .replace("cases = 4000", "cases = 20")
    };
    assert_eq!(runs(&run(&fixed(2500.0, "gold")), "Task_Manual"), 20.0);
    assert_eq!(runs(&run(&fixed(2500.0, "silver")), "Task_Pay"), 20.0);
    assert_eq!(
        runs(&run(&fixed(1000.0, "gold")), "Task_Manual"),
        20.0,
        "ranges include their bounds"
    );
    assert_eq!(runs(&run(&fixed(5000.5, "gold")), "Task_Reject"), 20.0);
}

#[test]
fn no_matching_rule_and_no_true_condition_leaves_the_token_stuck() {
    // A negative amount matches no rule (the number distribution is not cut off at 0): the route
    // is null, no condition holds, there is no default flow, so the token waits at the gateway.
    let s = scenario()
        .replace(
            "amount = { dist = \"uniform\", min = 0.0, max = 10000.0 }",
            "amount = { dist = \"fixed\", value = -1.0 }",
        )
        .replace("< 1000", "x")
        .replace("cases = 4000", "cases = 3");
    let r = run(&s);
    let d = &r.decisions["claim-route"];
    assert_eq!(d.evaluations, 3);
    assert_eq!(r.stuck.len(), 0, "-1 < 1000 matches rule R1");
    // Now with an input outside every rule.
    let p = claim();
    let text = std::fs::read_to_string(fixture("dmn/claim-route.dmn"))
        .unwrap()
        .replace("&lt; 1000", "[0..1000[");
    let mut q = Project::new();
    q.add_path(&fixture("dmn/claim.bpmn")).unwrap();
    q.add_dmn_source("claim-route.dmn", &text).unwrap();
    let r = run_replications(&Scenario::from_toml(&s).unwrap().bind(&q).unwrap()).remove(0);
    assert_eq!(r.decisions["claim-route"].no_match, 3);
    assert_eq!(r.stuck.len(), 3);
    assert!(
        r.stuck
            .iter()
            .all(|t| t.element.id == "Gateway_Route" && t.state.contains("no outgoing condition"))
    );
    drop(p);
}

#[test]
fn a_default_flow_catches_unmatched_conditions() {
    let text = std::fs::read_to_string(fixture("dmn/claim.bpmn"))
        .unwrap()
        .replace(
            "<bpmn:exclusiveGateway id=\"Gateway_Route\" name=\"Route?\"/>",
            "<bpmn:exclusiveGateway id=\"Gateway_Route\" name=\"Route?\" default=\"Flow_reject\"/>",
        )
        .replace(
            "<bpmn:conditionExpression>=route = \"reject\"</bpmn:conditionExpression>",
            "",
        );
    let mut p = Project::new();
    p.add_source("claim.bpmn", &text).unwrap();
    p.add_path(&fixture("dmn/claim-route.dmn")).unwrap();
    let r =
        run_replications(&Scenario::from_toml(&scenario()).unwrap().bind(&p).unwrap()).remove(0);
    assert!(r.stuck.is_empty());
    assert!((runs(&r, "Task_Reject") - 2000.0).abs() < 5.0 * (1000.0_f64).sqrt());
}

#[test]
fn probabilities_override_conditions_with_a_warning() {
    let s = format!(
        "{}\n[gateways.Gateway_Route]\nprobabilities = {{ Flow_auto = 1.0, Flow_manual = 0.0, Flow_reject = 0.0 }}\n",
        scenario()
    );
    let p = claim();
    let b = Scenario::from_toml(&s).unwrap().bind(&p).unwrap();
    assert!(
        b.warnings
            .iter()
            .any(|w| w.contains("Gateway_Route") && w.contains("instead of the flow conditions"))
    );
    let r = run_replications(&b).remove(0);
    assert_eq!(runs(&r, "Task_Pay"), 4000.0);
}

#[test]
fn decision_configuration_errors() {
    let missing = scenario().replace(
        "segment = { choice = { gold = 0.25, silver = 0.75 } }\n",
        "",
    );
    let e = bind_err(&missing);
    assert!(e.contains("segment") && e.contains("claim-route"), "{e}");
    let bad_choice = scenario().replace("silver = 0.75", "silver = 0.5");
    assert!(bind_err(&bad_choice).contains("sum to 1"));
    let unknown = format!(
        "{}\n[decisions.nope.inputs]\nx = {{ p_true = 0.5 }}\n",
        scenario()
    );
    assert!(bind_err(&unknown).contains("nope"));
    // Unsupported FEEL in a condition.
    let text = std::fs::read_to_string(fixture("dmn/claim.bpmn"))
        .unwrap()
        .replace("=route = \"auto\"", "${route == 'auto'}");
    let mut p = Project::new();
    p.add_source("claim.bpmn", &text).unwrap();
    p.add_path(&fixture("dmn/claim-route.dmn")).unwrap();
    let e = Scenario::from_toml(&scenario())
        .unwrap()
        .bind(&p)
        .unwrap_err()
        .to_string();
    assert!(e.contains("Flow_auto") && e.contains("JUEL"), "{e}");
}

#[test]
fn skeleton_contains_decision_inputs_and_runs() {
    let p = claim();
    let sk = Scenario::skeleton(&p, None);
    assert!(sk.decisions["claim-route"].inputs.contains_key("amount"));
    assert!(
        sk.gateways.is_empty(),
        "the gateway is decided by its conditions"
    );
    // The placeholder string input `segment` is sampled as a number, so only rule R1 (< 1000,
    // any segment) can match: all claims go the automatic way.
    let back = Scenario::from_toml(&sk.to_toml().unwrap()).unwrap();
    let r = run_replications(&back.bind(&p).unwrap()).remove(0);
    assert_eq!(r.cases_completed, 100);
}

#[test]
fn multi_file_business_rule_tasks_are_evaluated() {
    let p = Project::open_main(&fixture("multi-file/main.bpmn")).unwrap();
    let s = std::fs::read_to_string(fixture("multi-file/scenario.toml")).unwrap();
    let r = run_replications(&Scenario::from_toml(&s).unwrap().bind(&p).unwrap()).remove(0);
    // Both decisions read the same `amount` (sampled once per instance, then reused).
    assert_eq!(r.decisions["decision-risk"].evaluations, 10);
    assert_eq!(r.decisions["decision-limit"].evaluations, 10);
    assert_eq!(
        r.decisions["decision-risk"].outcomes,
        r.decisions["decision-limit"].outcomes
    );
}
