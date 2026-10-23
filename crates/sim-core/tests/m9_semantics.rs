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

// ---------------------------------------------------------------- errors and escalations

const ERRORS: &str = r#"<bpmn:error id="Err_A" errorCode="A"/><bpmn:error id="Err_B" errorCode="B"/>
    <bpmn:escalation id="Esc_X" escalationCode="X"/>"#;

/// `Sub` runs `Work` (5 min) and then throws error `thrown` from `ErrEnd`, while `Long`
/// (60 min) runs beside it. `boundaries` are attached to `Sub`; `Sub` itself leads to `E_ok`.
fn erroring(thrown: &str, boundaries: &str) -> Project {
    model_with(
        &format!(
            r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:startEvent id="S_start"/>
          <bpmn:parallelGateway id="S_split"/>
          <bpmn:task id="Work"/><bpmn:task id="Long"/>
          <bpmn:endEvent id="ErrEnd"><bpmn:errorEventDefinition {thrown}/></bpmn:endEvent>
          <bpmn:endEvent id="S_end"/>
          <bpmn:sequenceFlow id="S0" sourceRef="S_start" targetRef="S_split"/>
          <bpmn:sequenceFlow id="S1" sourceRef="S_split" targetRef="Work"/>
          <bpmn:sequenceFlow id="S2" sourceRef="S_split" targetRef="Long"/>
          <bpmn:sequenceFlow id="S3" sourceRef="Work" targetRef="ErrEnd"/>
          <bpmn:sequenceFlow id="S4" sourceRef="Long" targetRef="S_end"/>
        </bpmn:subProcess>
        {boundaries}
        <bpmn:task id="Handle"/><bpmn:task id="Other"/>
        <bpmn:endEvent id="E_ok"/><bpmn:endEvent id="E_handled"/><bpmn:endEvent id="E_other"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="E_ok"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Handle" targetRef="E_handled"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Other" targetRef="E_other"/>"#
        ),
        ERRORS,
    )
}

const ON_A: &str = r#"<bpmn:boundaryEvent id="B_A" attachedToRef="Sub"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:boundaryEvent>
    <bpmn:sequenceFlow id="FB_A" sourceRef="B_A" targetRef="Handle"/>"#;
const ON_ANY: &str = r#"<bpmn:boundaryEvent id="B_any" attachedToRef="Sub"><bpmn:errorEventDefinition/></bpmn:boundaryEvent>
    <bpmn:sequenceFlow id="FB_any" sourceRef="B_any" targetRef="Other"/>"#;

fn error_scenario() -> String {
    base(
        10,
        &format!(
            "{}{}{}{}",
            fixed("Work", 5),
            fixed("Long", 60),
            fixed("Handle", 5),
            fixed("Other", 5)
        ),
    )
}

#[test]
fn a_thrown_error_is_caught_by_the_boundary_event_of_its_subprocess() {
    let r = run(&erroring(r#"errorRef="Err_A""#, ON_A), &error_scenario());
    assert_eq!(r.cases_completed, 10);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    // The subprocess is cancelled with everything in it: `Long` never finishes.
    assert_eq!(runs(&r, "Handle"), 10);
    assert_eq!(runs(&r, "Long"), 0);
    assert_eq!(r.activities[&q("Long")].cancelled, 10);
    assert_eq!(ends(&r, "E_ok"), 0);
    assert_eq!(ends(&r, "E_handled"), 10);
    assert_eq!(r.boundary_fired[&q("B_A")], 10);
    assert!((r.case_cycle_time.mean - 600.0).abs() < 1e-9);
}

#[test]
fn an_exact_error_code_wins_over_a_catch_all_boundary_event() {
    let both = format!("{ON_A}{ON_ANY}");
    let r = run(&erroring(r#"errorRef="Err_A""#, &both), &error_scenario());
    assert_eq!((runs(&r, "Handle"), runs(&r, "Other")), (10, 0));
    // Another code, and an error without a code, only match the catch-all.
    let r = run(&erroring(r#"errorRef="Err_B""#, &both), &error_scenario());
    assert_eq!((runs(&r, "Handle"), runs(&r, "Other")), (0, 10));
    let r = run(&erroring("", &both), &error_scenario());
    assert_eq!((runs(&r, "Handle"), runs(&r, "Other")), (0, 10));
}

#[test]
fn an_uncaught_error_leaves_the_token_stuck_at_the_end_event() {
    let r = run(&erroring(r#"errorRef="Err_B""#, ON_A), &error_scenario());
    assert_eq!(r.cases_completed, 0);
    let errs: Vec<_> = r
        .stuck
        .iter()
        .filter(|s| s.element == q("ErrEnd"))
        .collect();
    assert_eq!(errs.len(), 10);
    assert_eq!(errs[0].state, "uncaught error 'B'");
}

#[test]
fn an_error_travels_up_through_nested_subprocesses() {
    let p = model_with(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Outer">
          <bpmn:startEvent id="O_start"/>
          <bpmn:subProcess id="Inner">
            <bpmn:startEvent id="I_start"/>
            <bpmn:endEvent id="ErrEnd"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:endEvent>
            <bpmn:sequenceFlow id="I0" sourceRef="I_start" targetRef="ErrEnd"/>
          </bpmn:subProcess>
          <bpmn:boundaryEvent id="B_inner" attachedToRef="Inner"><bpmn:errorEventDefinition errorRef="Err_B"/></bpmn:boundaryEvent>
          <bpmn:endEvent id="O_end"/><bpmn:endEvent id="O_b"/>
          <bpmn:sequenceFlow id="O0" sourceRef="O_start" targetRef="Inner"/>
          <bpmn:sequenceFlow id="O1" sourceRef="Inner" targetRef="O_end"/>
          <bpmn:sequenceFlow id="O2" sourceRef="B_inner" targetRef="O_b"/>
        </bpmn:subProcess>
        <bpmn:boundaryEvent id="B_outer" attachedToRef="Outer"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:boundaryEvent>
        <bpmn:endEvent id="E_ok"/><bpmn:endEvent id="E_caught"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Outer"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Outer" targetRef="E_ok"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B_outer" targetRef="E_caught"/>"#,
        ERRORS,
    );
    let r = run(&p, &base(4, ""));
    assert_eq!(r.cases_completed, 4);
    assert_eq!(ends(&r, "E_caught"), 4);
    assert_eq!(ends(&r, "O_b") + ends(&r, "O_end") + ends(&r, "E_ok"), 0);
}

#[test]
fn an_error_in_a_called_process_is_caught_at_the_call_activity() {
    let mut p = model_with(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:callActivity id="Call" calledElement="Child"/>
        <bpmn:boundaryEvent id="B_call" attachedToRef="Call"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:boundaryEvent>
        <bpmn:endEvent id="E_ok"/><bpmn:endEvent id="E_caught"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Call"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Call" targetRef="E_ok"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B_call" targetRef="E_caught"/>"#,
        ERRORS,
    );
    // The child file declares its own error element with the same code.
    p.add_source(
        "child.bpmn",
        &format!(
            r#"<bpmn:definitions {NS} id="C"><bpmn:error id="Child_Err" errorCode="A"/>
            <bpmn:process id="Child">
              <bpmn:startEvent id="C_start"/><bpmn:task id="C_work"/>
              <bpmn:endEvent id="C_err"><bpmn:errorEventDefinition errorRef="Child_Err"/></bpmn:endEvent>
              <bpmn:sequenceFlow id="C0" sourceRef="C_start" targetRef="C_work"/>
              <bpmn:sequenceFlow id="C1" sourceRef="C_work" targetRef="C_err"/>
            </bpmn:process></bpmn:definitions>"#
        ),
    )
    .unwrap();
    let r = run(&p, &base(5, &fixed("C_work", 10)));
    assert_eq!(r.cases_completed, 5);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!(ends(&r, "E_caught"), 5);
    assert_eq!(ends(&r, "E_ok"), 0);
    assert!((r.case_cycle_time.mean - 600.0).abs() < 1e-9);
}

/// `Sub` throws escalation X from an intermediate event, then runs `Work` (10 min).
fn escalating(interrupting: bool) -> Project {
    model_with(
        &format!(
            r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:startEvent id="S_start"/>
          <bpmn:intermediateThrowEvent id="Esc"><bpmn:escalationEventDefinition escalationRef="Esc_X"/></bpmn:intermediateThrowEvent>
          <bpmn:task id="Work"/><bpmn:endEvent id="S_end"/>
          <bpmn:sequenceFlow id="S0" sourceRef="S_start" targetRef="Esc"/>
          <bpmn:sequenceFlow id="S1" sourceRef="Esc" targetRef="Work"/>
          <bpmn:sequenceFlow id="S2" sourceRef="Work" targetRef="S_end"/>
        </bpmn:subProcess>
        <bpmn:boundaryEvent id="B_esc" attachedToRef="Sub" cancelActivity="{interrupting}"><bpmn:escalationEventDefinition escalationRef="Esc_X"/></bpmn:boundaryEvent>
        <bpmn:task id="Notify"/>
        <bpmn:endEvent id="E_ok"/><bpmn:endEvent id="E_notified"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="E_ok"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B_esc" targetRef="Notify"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Notify" targetRef="E_notified"/>"#
        ),
        ERRORS,
    )
}

#[test]
fn a_non_interrupting_escalation_starts_its_boundary_path_and_the_thrower_goes_on() {
    let r = run(
        &escalating(false),
        &base(5, &format!("{}{}", fixed("Work", 10), fixed("Notify", 5))),
    );
    assert_eq!(r.cases_completed, 5);
    assert_eq!((runs(&r, "Work"), runs(&r, "Notify")), (5, 5));
    assert_eq!((ends(&r, "E_ok"), ends(&r, "E_notified")), (5, 5));
}

#[test]
fn an_interrupting_escalation_cancels_the_subprocess() {
    let r = run(
        &escalating(true),
        &base(5, &format!("{}{}", fixed("Work", 10), fixed("Notify", 5))),
    );
    assert_eq!(r.cases_completed, 5);
    assert_eq!((runs(&r, "Work"), runs(&r, "Notify")), (0, 5));
    assert_eq!((ends(&r, "E_ok"), ends(&r, "E_notified")), (0, 5));
}

#[test]
fn an_uncaught_escalation_has_no_effect() {
    // Nothing catches it at the top level of the process: the token just goes on.
    let p = model_with(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:intermediateThrowEvent id="Esc"><bpmn:escalationEventDefinition escalationRef="Esc_X"/></bpmn:intermediateThrowEvent>
        <bpmn:endEvent id="E_ok"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Esc"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Esc" targetRef="E_ok"/>"#,
        ERRORS,
    );
    assert_eq!(
        p.node(&q("Esc")).unwrap().event_defs,
        [EventDef::Escalation(Some("X".into()))]
    );
    let r = run(&p, &base(3, ""));
    assert_eq!(ends(&r, "E_ok"), 3);
}

#[test]
fn error_boundaries_on_subprocesses_need_no_probability_but_on_tasks_they_do() {
    // Bound without a [boundary] entry (the scenarios above); a task's error boundary still
    // needs one, nothing in the model can throw it.
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="T"/><bpmn:endEvent id="E"/>
        <bpmn:boundaryEvent id="B" attachedToRef="T"><bpmn:errorEventDefinition/></bpmn:boundaryEvent>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="T"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T" targetRef="E"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B" targetRef="E"/>"#,
    );
    let e = bind_err(&p, &base(1, &fixed("T", 5)));
    assert!(e.contains("needs a [boundary] entry"), "{e}");
    // A probability on a subprocess's error boundary adds failures on top of thrown errors.
    let s = format!(
        "{}[boundary.B_any]\nprobability = 1.0\ndelay = {{ dist = \"fixed\", value = \"1m\" }}\n",
        error_scenario()
    );
    let r = run(
        &erroring(r#"errorRef="Err_A""#, &format!("{ON_A}{ON_ANY}")),
        &s,
    );
    assert_eq!((runs(&r, "Handle"), runs(&r, "Other")), (0, 10));
}

#[test]
fn the_token_game_catches_thrown_errors_too() {
    let mut g = Game::new(Arc::new(erroring(r#"errorRef="Err_A""#, ON_A)));
    g.stop_at_events = false;
    g.apply(Action::Start { event: q("Start") }).unwrap();
    let work = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::Complete { token, task } if task == q("Work") => Some(token),
            _ => None,
        })
        .expect("Work waits");
    g.apply(Action::Complete { token: work }).unwrap();
    let at: Vec<String> = g.tokens().iter().map(|t| t.node.id.clone()).collect();
    assert_eq!(at, ["Handle"]);
    assert!(
        g.log.iter().any(|l| l == "error 'A' caught by 'B_A'"),
        "{:?}",
        g.log
    );
}

// ---------------------------------------------------------------- multi-instance and loops

/// `T` (10 min, one resource `Clerk` with `clerks` people) carries `looping`.
fn looped(looping: &str) -> Project {
    model(&format!(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:task id="T">{looping}</bpmn:task>
        <bpmn:endEvent id="E"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="T"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T" targetRef="E"/>"#
    ))
}

fn clerks(n: u32, extra: &str) -> String {
    base(
        1,
        &format!(
            "[resources.Clerk]\ncount = {n}\n[activities.T]\nduration = {{ dist = \"fixed\", value = \"10m\" }}\nresource = \"Clerk\"\n{extra}"
        ),
    )
}

const PARALLEL: &str = r#"<bpmn:multiInstanceLoopCharacteristics/>"#;
const SEQUENTIAL: &str = r#"<bpmn:multiInstanceLoopCharacteristics isSequential="true"/>"#;

#[test]
fn parallel_instances_run_at_once_and_compete_for_resources() {
    let p = looped(PARALLEL);
    // Four instances, two clerks: two rounds of 10 minutes.
    let r = run(&p, &clerks(2, "[multi_instance.T]\ncount = 4\n"));
    assert_eq!(r.cases_completed, 1);
    assert_eq!(runs(&r, "T"), 4);
    assert_eq!(ends(&r, "E"), 1);
    assert!((r.case_cycle_time.mean - 1200.0).abs() < 1e-9);
    // Enough clerks: all at once.
    let r = run(&p, &clerks(4, "[multi_instance.T]\ncount = 4\n"));
    assert!((r.case_cycle_time.mean - 600.0).abs() < 1e-9);
}

#[test]
fn sequential_instances_run_one_after_the_other() {
    let r = run(
        &looped(SEQUENTIAL),
        &clerks(4, "[multi_instance.T]\ncount = 3\n"),
    );
    assert_eq!(runs(&r, "T"), 3);
    assert!((r.case_cycle_time.mean - 1800.0).abs() < 1e-9);
}

#[test]
fn zero_instances_pass_straight_through() {
    let r = run(
        &looped(PARALLEL),
        &clerks(1, "[multi_instance.T]\ncount = 0\n"),
    );
    assert_eq!((r.cases_completed, runs(&r, "T")), (1, 0));
    assert_eq!(r.case_cycle_time.mean, 0.0);
}

#[test]
fn the_count_comes_from_the_model_or_from_a_distribution() {
    let p = looped(
        r#"<bpmn:multiInstanceLoopCharacteristics><bpmn:loopCardinality>= 5</bpmn:loopCardinality></bpmn:multiInstanceLoopCharacteristics>"#,
    );
    assert_eq!(runs(&run(&p, &clerks(5, "")), "T"), 5);
    // The scenario overrides the model.
    assert_eq!(
        runs(&run(&p, &clerks(5, "[multi_instance.T]\ncount = 2\n")), "T"),
        2
    );
    // A uniform count draws whole numbers with equal chances.
    let s = base(
        3000,
        "[activities.T]\nduration = { dist = \"fixed\", value = \"1m\" }\n[multi_instance.T]\ncount = { dist = \"uniform\", min = 1, max = 3 }\n",
    );
    let r = run(&looped(PARALLEL), &s);
    let mean = runs(&r, "T") as f64 / 3000.0;
    assert!((mean - 2.0).abs() < 0.06, "mean count {mean}");
}

#[test]
fn a_multi_instance_activity_without_a_count_is_a_binding_error() {
    let e = bind_err(&looped(PARALLEL), &clerks(1, ""));
    assert!(e.contains("needs a [multi_instance] entry"), "{e}");
    let e = bind_err(&looped(""), &clerks(1, "[multi_instance.T]\ncount = 2\n"));
    assert!(e.contains("is not a multi-instance activity"), "{e}");
}

#[test]
fn the_input_collection_can_come_from_a_decision() {
    // `Items` (business rule, COLLECT) produces the list `parts`; `T` runs once per element.
    let mut p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:businessRuleTask id="Items">
          <bpmn:extensionElements>
            <zeebe:calledDecision xmlns:zeebe="http://camunda.org/schema/zeebe/1.0" decisionId="parts" resultVariable="parts"/>
          </bpmn:extensionElements>
        </bpmn:businessRuleTask>
        <bpmn:task id="T">
          <bpmn:multiInstanceLoopCharacteristics>
            <bpmn:extensionElements>
              <zeebe:loopCharacteristics xmlns:zeebe="http://camunda.org/schema/zeebe/1.0" inputCollection="= parts" inputElement="part"/>
            </bpmn:extensionElements>
          </bpmn:multiInstanceLoopCharacteristics>
        </bpmn:task>
        <bpmn:endEvent id="E"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Items"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Items" targetRef="T"/>
        <bpmn:sequenceFlow id="F2" sourceRef="T" targetRef="E"/>"#,
    );
    p.add_dmn_source(
        "parts.dmn",
        r#"<definitions xmlns="https://www.omg.org/spec/DMN/20191111/MODEL/" id="DD" namespace="x">
          <decision id="parts" name="Parts">
            <decisionTable id="T" hitPolicy="COLLECT">
              <input id="I"><inputExpression id="IE" typeRef="number"><text>size</text></inputExpression></input>
              <output id="O" name="part" typeRef="string"/>
              <rule id="R1"><inputEntry id="R1I"><text>-</text></inputEntry><outputEntry id="R1O"><text>"frame"</text></outputEntry></rule>
              <rule id="R2"><inputEntry id="R2I"><text>&gt; 50</text></inputEntry><outputEntry id="R2O"><text>"motor"</text></outputEntry></rule>
              <rule id="R3"><inputEntry id="R3I"><text>&gt; 80</text></inputEntry><outputEntry id="R3O"><text>"sensor"</text></outputEntry></rule>
            </decisionTable>
          </decision>
        </definitions>"#,
    )
    .unwrap();
    let s = base(
        1,
        &format!(
            "{}{}[decisions.parts.inputs]\nsize = {{ dist = \"fixed\", value = 90.0 }}\n",
            fixed("Items", 1),
            fixed("T", 1)
        ),
    );
    assert_eq!(runs(&run(&p, &s), "T"), 3);
    // The skeleton leaves the count to the decision.
    assert!(Scenario::skeleton(&p, None).multi_instance.is_empty());
}

#[test]
fn a_boundary_timer_cancels_every_instance() {
    let p = model(&format!(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:task id="T">{PARALLEL}</bpmn:task>
        <bpmn:boundaryEvent id="B" attachedToRef="T"><bpmn:timerEventDefinition/></bpmn:boundaryEvent>
        <bpmn:endEvent id="E"/><bpmn:endEvent id="E_late"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="T"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T" targetRef="E"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B" targetRef="E_late"/>"#
    ));
    let r = run(
        &p,
        &clerks(
            1,
            "[multi_instance.T]\ncount = 3\n[boundary.B]\ndelay = { dist = \"fixed\", value = \"15m\" }\n",
        ),
    );
    // One clerk: the first instance finishes at 10 min, the second is cut off at 15 min.
    assert_eq!(runs(&r, "T"), 1);
    assert_eq!(r.activities[&q("T")].cancelled, 2);
    assert_eq!((ends(&r, "E"), ends(&r, "E_late")), (0, 1));
    assert_eq!(r.boundary_fired[&q("B")], 1);
}

#[test]
fn an_error_in_one_instance_of_a_subprocess_cancels_them_all() {
    let p = model_with(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:multiInstanceLoopCharacteristics/>
          <bpmn:startEvent id="S_start"/><bpmn:task id="Work"/>
          <bpmn:exclusiveGateway id="X"/>
          <bpmn:endEvent id="S_end"/>
          <bpmn:endEvent id="ErrEnd"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:endEvent>
          <bpmn:sequenceFlow id="S0" sourceRef="S_start" targetRef="Work"/>
          <bpmn:sequenceFlow id="S1" sourceRef="Work" targetRef="X"/>
          <bpmn:sequenceFlow id="S_ok" sourceRef="X" targetRef="S_end"/>
          <bpmn:sequenceFlow id="S_err" sourceRef="X" targetRef="ErrEnd"/>
        </bpmn:subProcess>
        <bpmn:boundaryEvent id="B_A" attachedToRef="Sub"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:boundaryEvent>
        <bpmn:endEvent id="E_ok"/><bpmn:endEvent id="E_err"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="E_ok"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B_A" targetRef="E_err"/>"#,
        ERRORS,
    );
    let s = base(
        400,
        &format!(
            "{}[multi_instance.Sub]\ncount = 3\n[gateways.X]\nprobabilities = {{ S_ok = 0.8, S_err = 0.2 }}\n",
            fixed("Work", 10)
        ),
    );
    let r = run(&p, &s);
    assert_eq!(r.cases_completed, 400);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    // The subprocess fails when any of its three instances fails: 1 - 0.8^3 = 0.488.
    let failed = ends(&r, "E_err") as f64 / 400.0;
    assert!((failed - 0.488).abs() < 0.07, "failed share {failed}");
    assert_eq!(ends(&r, "E_err") + ends(&r, "E_ok"), 400);
}

#[test]
fn a_loop_repeats_with_its_probability_up_to_the_maximum() {
    let p = looped(r#"<bpmn:standardLoopCharacteristics loopMaximum="3"/>"#);
    let s = |prob: f64| {
        base(
            2000,
            &format!("{}[loops.T]\nrepeat_probability = {prob}\n", fixed("T", 1)),
        )
    };
    // Always repeat: capped at three runs per case.
    assert_eq!(runs(&run(&p, &s(1.0)), "T"), 6000);
    // Repeat with 0.5: 1 + 0.5 + 0.25 = 1.75 runs on average.
    let mean = runs(&run(&p, &s(0.5)), "T") as f64 / 2000.0;
    assert!((mean - 1.75).abs() < 0.06, "mean runs {mean}");
    // Test before: the first run is decided too (0.5 + 0.25 + 0.125).
    let p = looped(r#"<bpmn:standardLoopCharacteristics testBefore="true" loopMaximum="3"/>"#);
    let mean = runs(&run(&p, &s(0.5)), "T") as f64 / 2000.0;
    assert!((mean - 0.875).abs() < 0.06, "mean runs {mean}");
}

#[test]
fn an_endless_loop_is_rejected() {
    let e = bind_err(
        &looped("<bpmn:standardLoopCharacteristics/>"),
        &base(
            1,
            &format!("{}[loops.T]\nrepeat_probability = 1.0\n", fixed("T", 1)),
        ),
    );
    assert!(e.contains("never ends"), "{e}");
}

#[test]
fn the_skeleton_gives_multi_instance_and_loop_activities_an_entry() {
    let s = Scenario::skeleton(&looped(PARALLEL), None);
    assert!(s.multi_instance.contains_key("m.bpmn#T"));
    let s = Scenario::skeleton(&looped("<bpmn:standardLoopCharacteristics/>"), None);
    assert!(s.loops.contains_key("m.bpmn#T"));
    // And it binds as written.
    let toml = s.to_toml().unwrap();
    let p = looped("<bpmn:standardLoopCharacteristics/>");
    Scenario::from_toml(&toml).unwrap().bind(&p).unwrap();
}
