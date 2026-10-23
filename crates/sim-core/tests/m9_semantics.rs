//! BPMN semantics completed after M8: conditional flows leaving activities, thrown errors and
//! escalations, multi-instance activities and loops.

use std::sync::Arc;

use sim_core::manual::{Action, Game, Pending};
use sim_core::*;

const NS: &str = r#"xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL""#;

fn q(id: &str) -> QId {
    QId::new("m.bpmn", id)
}

fn model(process: &str) -> Project {
    model_with(process, "")
}

/// One file `m.bpmn` with one process; `defs` goes next to the process (errors, escalations).
fn model_with(process: &str, defs: &str) -> Project {
    let xml = format!(
        r#"<bpmn:definitions {NS} id="D">{defs}<bpmn:process id="P">{process}</bpmn:process></bpmn:definitions>"#
    );
    let mut p = Project::new();
    p.add_source("m.bpmn", &xml).expect("model parses");
    p
}

fn run(p: &Project, scenario: &str) -> ReplicationResult {
    let bound = Scenario::from_toml(scenario)
        .unwrap()
        .bind(p)
        .unwrap_or_else(|e| panic!("{e}"));
    run_replications(&bound).remove(0)
}

fn bind_err(p: &Project, scenario: &str) -> String {
    match Scenario::from_toml(scenario).unwrap().bind(p) {
        Ok(_) => panic!("binding should fail"),
        Err(e) => e.to_string(),
    }
}

fn fixed(name: &str, minutes: u32) -> String {
    format!("[activities.{name}]\nduration = {{ dist = \"fixed\", value = \"{minutes}m\" }}\n")
}

fn base(cases: u32, extra: &str) -> String {
    format!(
        "[simulation]\ncases = {cases}\n[[arrivals]]\ninter_arrival = {{ dist = \"fixed\", value = \"1h\" }}\ntriggers = [\"Start\"]\n{extra}"
    )
}

fn runs(r: &ReplicationResult, task: &str) -> usize {
    r.activities.get(&q(task)).map_or(0, |a| a.runs)
}

fn ends(r: &ReplicationResult, id: &str) -> u64 {
    r.end_events.get(&q(id)).copied().unwrap_or(0)
}

// ---------------------------------------------------------------- conditional flows

/// `Score` sets `score` (uniform 0..100) through a decision table. Its outgoing flows: `F_high`
/// (condition `score > 50`), `F_audit` (no condition) and the default flow `F_low`.
fn scored() -> Project {
    let mut p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:businessRuleTask id="Score" default="F_low">
          <bpmn:extensionElements>
            <zeebe:calledDecision xmlns:zeebe="http://camunda.org/schema/zeebe/1.0" decisionId="band" resultVariable="band"/>
          </bpmn:extensionElements>
        </bpmn:businessRuleTask>
        <bpmn:task id="High"/><bpmn:task id="Low"/><bpmn:task id="Audit"/>
        <bpmn:endEvent id="E_high"/><bpmn:endEvent id="E_low"/><bpmn:endEvent id="E_audit"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Score"/>
        <bpmn:sequenceFlow id="F_high" sourceRef="Score" targetRef="High">
          <bpmn:conditionExpression>= score &gt; 50</bpmn:conditionExpression>
        </bpmn:sequenceFlow>
        <bpmn:sequenceFlow id="F_low" sourceRef="Score" targetRef="Low"/>
        <bpmn:sequenceFlow id="F_audit" sourceRef="Score" targetRef="Audit"/>
        <bpmn:sequenceFlow id="F1" sourceRef="High" targetRef="E_high"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Low" targetRef="E_low"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Audit" targetRef="E_audit"/>"#,
    );
    p.add_dmn_source(
        "band.dmn",
        r#"<definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/" id="DD" namespace="x">
          <decision id="band" name="Band">
            <decisionTable id="T">
              <input id="I"><inputExpression id="IE" typeRef="number"><text>score</text></inputExpression></input>
              <output id="O" name="high" typeRef="boolean"/>
              <rule id="R1"><inputEntry id="R1I"><text>&gt; 50</text></inputEntry><outputEntry id="R1O"><text>true</text></outputEntry></rule>
              <rule id="R2"><inputEntry id="R2I"><text>&lt;= 50</text></inputEntry><outputEntry id="R2O"><text>false</text></outputEntry></rule>
            </decisionTable>
          </decision>
        </definitions>"#,
    )
    .unwrap();
    p
}

fn scored_scenario(extra: &str) -> String {
    base(
        2000,
        &format!(
            "{}{}{}{}[decisions.band.inputs]\nscore = {{ dist = \"uniform\", min = 0.0, max = 100.0 }}\n{extra}",
            fixed("Score", 5),
            fixed("High", 5),
            fixed("Low", 5),
            fixed("Audit", 5)
        ),
    )
}

#[test]
fn conditional_flows_of_a_task_follow_their_conditions_and_the_default() {
    let r = run(&scored(), &scored_scenario(""));
    assert_eq!(r.cases_completed, 2000);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    // The flow without a condition is always taken, high and low exclude each other.
    assert_eq!(runs(&r, "Audit"), 2000);
    assert_eq!(runs(&r, "High") + runs(&r, "Low"), 2000);
    assert_eq!(ends(&r, "E_audit"), 2000);
    let high = runs(&r, "High") as f64 / 2000.0;
    assert!((high - 0.5).abs() < 0.05, "high share {high}");
}

#[test]
fn gateway_probabilities_override_the_conditions_of_a_task() {
    let r = run(
        &scored(),
        &scored_scenario("[gateways.Score]\nprobabilities = { F_high = 0.2 }\n"),
    );
    assert_eq!(runs(&r, "Audit"), 2000);
    assert_eq!(runs(&r, "High") + runs(&r, "Low"), 2000);
    let high = runs(&r, "High") as f64 / 2000.0;
    assert!((high - 0.2).abs() < 0.04, "high share {high}");
}

#[test]
fn probabilities_only_apply_to_conditional_flows() {
    let e = bind_err(
        &scored(),
        &scored_scenario("[gateways.Score]\nprobabilities = { F_high = 0.2, F_audit = 1.0 }\n"),
    );
    assert!(
        e.contains("'F_audit' is not a conditional outgoing flow"),
        "{e}"
    );
    let e = bind_err(
        &scored(),
        &scored_scenario("[gateways.Score]\nprobabilities = { }\n"),
    );
    assert!(
        e.contains("no probability for conditional flow 'F_high'"),
        "{e}"
    );
}

#[test]
fn a_task_without_a_true_condition_and_default_flow_gets_stuck() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="T"/><bpmn:endEvent id="E"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="T"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T" targetRef="E">
          <bpmn:conditionExpression>= missing &gt; 1</bpmn:conditionExpression>
        </bpmn:sequenceFlow>"#,
    );
    let r = run(&p, &base(3, &fixed("T", 5)));
    assert_eq!(r.cases_completed, 0);
    assert_eq!(r.stuck.len(), 3);
    assert!(r.stuck[0].state.contains("no outgoing condition is true"));
}

#[test]
fn the_player_chooses_which_conditions_of_a_task_hold() {
    let mut g = Game::new(Arc::new(scored()));
    g.stop_at_tasks = false;
    g.apply(Action::Start { event: q("Start") }).unwrap();
    let (token, flows) = match g.pending().as_slice() {
        [Pending::Choose { token, flows, .. }] => (*token, flows.clone()),
        other => panic!("{other:?}"),
    };
    // Offered: the conditional flow and the default flow, not the unconditional one.
    assert_eq!(flows, [q("F_high"), q("F_low")]);
    assert!(
        g.apply(Action::Choose {
            token,
            flows: vec![q("F_audit")]
        })
        .is_err()
    );
    g.apply(Action::Choose {
        token,
        flows: vec![q("F_high")],
    })
    .unwrap();
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
    let finished: Vec<String> = g
        .log
        .iter()
        .filter(|l| l.contains("take"))
        .cloned()
        .collect();
    assert_eq!(finished, ["'Score': take to 'High'"]);
}
