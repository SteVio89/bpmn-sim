//! Gaps to BPMN 2.0.2 closed after M9: process levels without a start event, and clear
//! reports for elements the simulator does not run (event subprocesses, instantiating
//! receive tasks and event-based gateways).

use std::sync::Arc;

use sim_core::manual::Game;
use sim_core::*;

const NS: &str = r#"xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL""#;

fn q(id: &str) -> QId {
    QId::new("m.bpmn", id)
}

fn model(process: &str) -> Project {
    let xml = format!(
        r#"<bpmn:definitions {NS} id="D"><bpmn:message id="Msg" name="M"/><bpmn:process id="P">{process}</bpmn:process></bpmn:definitions>"#
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

fn issues_of(p: &Project, id: &str) -> Vec<String> {
    validate(p)
        .into_iter()
        .filter(|i| i.element == q(id))
        .map(|i| i.message)
        .collect()
}

// ---------------------------------------------------------------- no start event

/// `Sub` has neither start nor end events: `A` (10 min) and `B` (20 min) have no incoming
/// flows, `B` is followed by `C` (5 min).
fn startless_sub() -> Project {
    model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:task id="A"/><bpmn:task id="B"/><bpmn:task id="C"/>
          <bpmn:sequenceFlow id="S0" sourceRef="B" targetRef="C"/>
        </bpmn:subProcess>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="End"/>"#,
    )
}

#[test]
fn a_subprocess_without_start_event_starts_every_activity_without_incoming_flow() {
    let p = startless_sub();
    let scenario = base(3, &(fixed("A", 10) + &fixed("B", 20) + &fixed("C", 5)));
    let r = run(&p, &scenario);
    assert_eq!(r.cases_completed, 3);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    for t in ["A", "B", "C"] {
        assert_eq!(r.activities[&q(t)].runs, 3, "{t}");
    }
    assert!((r.case_cycle_time.mean - 25.0 * 60.0).abs() < 1e-9);
}

#[test]
fn activities_of_a_subprocess_without_start_event_need_no_flows() {
    let p = startless_sub();
    for t in ["A", "B", "C"] {
        assert_eq!(issues_of(&p, t), Vec::<String>::new(), "{t}");
    }
}

#[test]
fn a_subprocess_with_neither_start_event_nor_free_activity_is_a_binding_error() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:task id="A"/><bpmn:sequenceFlow id="S0" sourceRef="A" targetRef="A"/>
        </bpmn:subProcess>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="End"/>"#,
    );
    let e = bind_err(&p, &base(1, &fixed("A", 1)));
    assert!(
        e.contains("embedded subprocess has no none start event"),
        "{e}"
    );
}

#[test]
fn a_called_process_without_start_event_starts_its_free_activities() {
    let mut p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:callActivity id="Call" calledElement="Child"/>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Call"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Call" targetRef="End"/>"#,
    );
    p.add_source(
        "child.bpmn",
        &format!(
            r#"<bpmn:definitions {NS} id="C"><bpmn:process id="Child">
              <bpmn:task id="C_work"/>
            </bpmn:process></bpmn:definitions>"#
        ),
    )
    .unwrap();
    let r = run(&p, &base(2, &fixed("C_work", 10)));
    assert_eq!(r.cases_completed, 2);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert!((r.case_cycle_time.mean - 600.0).abs() < 1e-9);
    let work = QId::new("child.bpmn", "C_work");
    assert!(
        validate(&p).iter().all(|i| i.element != work),
        "{:?}",
        validate(&p)
    );
}

#[test]
fn the_token_game_starts_a_subprocess_without_start_event_at_its_free_activities() {
    let mut g = Game::new(Arc::new(startless_sub()));
    g.apply(manual::Action::Start { event: q("Start") })
        .unwrap();
    let mut at: Vec<String> = g.tokens().iter().map(|t| t.node.id.clone()).collect();
    at.sort();
    assert_eq!(at, ["A", "B", "Sub"]);
}

// ---------------------------------------------------------------- not supported

#[test]
fn an_event_subprocess_is_reported_as_unsupported() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="Work"/><bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Work"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Work" targetRef="End"/>
        <bpmn:subProcess id="EvSub" triggeredByEvent="true">
          <bpmn:startEvent id="EvStart" isInterrupting="false"><bpmn:timerEventDefinition/></bpmn:startEvent>
          <bpmn:task id="Handle"/><bpmn:endEvent id="EvEnd"/>
          <bpmn:sequenceFlow id="E0" sourceRef="EvStart" targetRef="Handle"/>
          <bpmn:sequenceFlow id="E1" sourceRef="Handle" targetRef="EvEnd"/>
        </bpmn:subProcess>"#,
    );
    assert_eq!(
        issues_of(&p, "EvSub"),
        ["event subprocesses are not supported yet"]
    );
    // Only the event subprocess itself is reported, not its start event or its tasks.
    let e = bind_err(&p, &base(1, &fixed("Work", 1)));
    assert!(
        e.contains("EvSub (EvSub): event subprocesses are not supported yet"),
        "{e}"
    );
    assert!(!e.contains("EvStart") && !e.contains("Handle"), "{e}");
}

#[test]
fn instantiating_receive_tasks_and_event_gateways_are_reported() {
    let p = model(
        r#"
        <bpmn:receiveTask id="Recv" instantiate="true" messageRef="Msg"/>
        <bpmn:eventBasedGateway id="Gw"/>
        <bpmn:intermediateCatchEvent id="Tm"><bpmn:timerEventDefinition/></bpmn:intermediateCatchEvent>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Recv" targetRef="End"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Gw" targetRef="Tm"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Tm" targetRef="End"/>"#,
    );
    let recv = issues_of(&p, "Recv");
    assert!(
        recv.iter()
            .any(|m| m.contains("receive tasks that start a process")),
        "{recv:?}"
    );
    assert_eq!(
        issues_of(&p, "Gw"),
        [
            "event-based gateways that start a process are not supported; only start events start one"
        ]
    );
}
