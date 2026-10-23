//! Gaps to BPMN 2.0.2 closed after M9: process levels without a start event, event
//! subprocesses, and clear reports for instantiating receive tasks and event-based gateways.

use std::sync::Arc;

use sim_core::manual::Game;
use sim_core::*;

const NS: &str = r#"xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL""#;
const ERRORS: &str =
    r#"<bpmn:error id="Err_A" errorCode="A"/><bpmn:escalation id="Esc_X" escalationCode="X"/>"#;

fn q(id: &str) -> QId {
    QId::new("m.bpmn", id)
}

fn model(process: &str) -> Project {
    let xml = format!(
        r#"<bpmn:definitions {NS} id="D"><bpmn:message id="Msg" name="M"/><bpmn:signal id="Sig" name="S"/>{ERRORS}<bpmn:process id="P">{process}</bpmn:process></bpmn:definitions>"#
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

// ---------------------------------------------------------------- event subprocesses

/// `Work` (30 min) at the top level, next to an event subprocess `EvSub` that starts at
/// `EvStart` (`start` is its event definition, `attrs` its attributes) and runs `Handle`
/// (1 min) to `EvEnd`.
fn with_event_sub(attrs: &str, start: &str) -> Project {
    model(&format!(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="Work"/><bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Work"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Work" targetRef="End"/>
        <bpmn:subProcess id="EvSub" triggeredByEvent="true">
          <bpmn:startEvent id="EvStart" {attrs}>{start}</bpmn:startEvent>
          <bpmn:task id="Handle"/><bpmn:endEvent id="EvEnd"/>
          <bpmn:sequenceFlow id="E0" sourceRef="EvStart" targetRef="Handle"/>
          <bpmn:sequenceFlow id="E1" sourceRef="Handle" targetRef="EvEnd"/>
        </bpmn:subProcess>"#
    ))
}

const TIMER: &str = "<bpmn:timerEventDefinition/>";

fn timer_scenario(minutes: u32) -> String {
    base(
        2,
        &format!(
            "{}{}[events.EvStart]\ndelay = {{ dist = \"fixed\", value = \"{minutes}m\" }}\n",
            fixed("Work", 30),
            fixed("Handle", 1)
        ),
    )
}

#[test]
fn a_non_interrupting_event_subprocess_runs_next_to_its_scope() {
    let p = with_event_sub(r#"isInterrupting="false""#, TIMER);
    assert!(validate(&p).is_empty(), "{:?}", validate(&p));
    let r = run(&p, &timer_scenario(5));
    assert_eq!(r.cases_completed, 2);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!((runs(&r, "Work"), runs(&r, "Handle")), (2, 2));
    assert_eq!((ends(&r, "End"), ends(&r, "EvEnd")), (2, 2));
    assert_eq!(r.boundary_fired[&q("EvStart")], 2);
    assert!((r.case_cycle_time.mean - 30.0 * 60.0).abs() < 1e-9);
}

#[test]
fn an_interrupting_event_subprocess_cancels_the_rest_of_its_scope() {
    let p = with_event_sub("", TIMER);
    let r = run(&p, &timer_scenario(5));
    assert_eq!(r.cases_completed, 2);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!(runs(&r, "Work"), 0);
    assert_eq!(r.activities[&q("Work")].cancelled, 2);
    assert_eq!((ends(&r, "End"), ends(&r, "EvEnd")), (0, 2));
    assert!((r.case_cycle_time.mean - 6.0 * 60.0).abs() < 1e-9);
}

#[test]
fn an_event_subprocess_only_listens_while_its_scope_runs() {
    let p = with_event_sub("", TIMER);
    let r = run(&p, &timer_scenario(45));
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!((runs(&r, "Work"), runs(&r, "Handle")), (2, 0));
    assert!(!r.boundary_fired.contains_key(&q("EvStart")));
}

#[test]
fn a_scope_where_only_event_subprocesses_run_starts_no_new_ones() {
    // EvSub (non-interrupting, after 5 min) runs `Handle` for 60 min; the main flow ends after
    // 30 min, so `Late` (after 40 min) must not start any more.
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
        </bpmn:subProcess>
        <bpmn:subProcess id="LateSub" triggeredByEvent="true">
          <bpmn:startEvent id="Late" isInterrupting="false"><bpmn:timerEventDefinition/></bpmn:startEvent>
          <bpmn:task id="LateWork"/>
          <bpmn:sequenceFlow id="L0" sourceRef="Late" targetRef="LateWork"/>
        </bpmn:subProcess>"#,
    );
    let scenario = |late: u32| {
        base(
            1,
            &format!(
                "{}{}{}[events.EvStart]\ndelay = {{ dist = \"fixed\", value = \"5m\" }}\n[events.Late]\ndelay = {{ dist = \"fixed\", value = \"{late}m\" }}\n",
                fixed("Work", 30),
                fixed("Handle", 60),
                fixed("LateWork", 1)
            ),
        )
    };
    let r = run(&p, &scenario(40));
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!(runs(&r, "LateWork"), 0);
    assert!((r.case_cycle_time.mean - 65.0 * 60.0).abs() < 1e-9);
    // While the main flow still runs, it does start.
    let r = run(&p, &scenario(20));
    assert_eq!(runs(&r, "LateWork"), 1);
}

#[test]
fn a_signal_starts_an_event_subprocess() {
    // Next to `Work` a timer waits 5 min and throws signal S.
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:parallelGateway id="Fork"/>
        <bpmn:task id="Work"/>
        <bpmn:intermediateCatchEvent id="Wait"><bpmn:timerEventDefinition/></bpmn:intermediateCatchEvent>
        <bpmn:intermediateThrowEvent id="Throw"><bpmn:signalEventDefinition signalRef="Sig"/></bpmn:intermediateThrowEvent>
        <bpmn:endEvent id="End"/><bpmn:endEvent id="End2"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Fork"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Fork" targetRef="Work"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Fork" targetRef="Wait"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Wait" targetRef="Throw"/>
        <bpmn:sequenceFlow id="F4" sourceRef="Throw" targetRef="End2"/>
        <bpmn:sequenceFlow id="F5" sourceRef="Work" targetRef="End"/>
        <bpmn:subProcess id="EvSub" triggeredByEvent="true">
          <bpmn:startEvent id="EvStart" isInterrupting="false"><bpmn:signalEventDefinition signalRef="Sig"/></bpmn:startEvent>
          <bpmn:task id="Handle"/><bpmn:endEvent id="EvEnd"/>
          <bpmn:sequenceFlow id="E0" sourceRef="EvStart" targetRef="Handle"/>
          <bpmn:sequenceFlow id="E1" sourceRef="Handle" targetRef="EvEnd"/>
        </bpmn:subProcess>"#,
    );
    let scenario = base(
        3,
        &format!(
            "{}{}[events.Wait]\ndelay = {{ dist = \"fixed\", value = \"5m\" }}\n",
            fixed("Work", 30),
            fixed("Handle", 1)
        ),
    );
    let r = run(&p, &scenario);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!(runs(&r, "Handle"), 3);
    assert_eq!(r.signals.iter().map(|s| s.receivers).sum::<usize>(), 3);
}

/// `Sub` runs `Work` (10 min) and ends with error A. Its event subprocess `Catch` (error A)
/// runs `Handle` (5 min) and ends at `CatchEnd` with `catch_end`. `Sub` also has an error
/// boundary event `B_A` (code A).
fn erroring_sub(catch_end: &str) -> Project {
    model(&format!(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:startEvent id="S_start"/><bpmn:task id="Work"/>
          <bpmn:endEvent id="S_err"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:endEvent>
          <bpmn:sequenceFlow id="S0" sourceRef="S_start" targetRef="Work"/>
          <bpmn:sequenceFlow id="S1" sourceRef="Work" targetRef="S_err"/>
          <bpmn:subProcess id="Catch" triggeredByEvent="true">
            <bpmn:startEvent id="C_start"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:startEvent>
            <bpmn:task id="Handle"/><bpmn:endEvent id="CatchEnd">{catch_end}</bpmn:endEvent>
            <bpmn:sequenceFlow id="C0" sourceRef="C_start" targetRef="Handle"/>
            <bpmn:sequenceFlow id="C1" sourceRef="Handle" targetRef="CatchEnd"/>
          </bpmn:subProcess>
        </bpmn:subProcess>
        <bpmn:boundaryEvent id="B_A" attachedToRef="Sub"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:boundaryEvent>
        <bpmn:endEvent id="E_ok"/><bpmn:endEvent id="E_caught"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="E_ok"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B_A" targetRef="E_caught"/>"#
    ))
}

#[test]
fn an_error_event_subprocess_handles_the_error_inside_its_subprocess() {
    let p = erroring_sub("");
    let r = run(&p, &base(2, &(fixed("Work", 10) + &fixed("Handle", 5))));
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    // The inner handler wins over the boundary event; the subprocess then completes normally.
    assert_eq!((ends(&r, "E_ok"), ends(&r, "E_caught")), (2, 0));
    assert_eq!(r.boundary_fired[&q("C_start")], 2);
    assert!((r.case_cycle_time.mean - 15.0 * 60.0).abs() < 1e-9);
}

#[test]
fn an_error_thrown_by_an_event_subprocess_goes_to_the_enclosing_activity() {
    let p = erroring_sub(r#"<bpmn:errorEventDefinition errorRef="Err_A"/>"#);
    let r = run(&p, &base(2, &(fixed("Work", 10) + &fixed("Handle", 5))));
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!((ends(&r, "E_ok"), ends(&r, "E_caught")), (0, 2));
    assert_eq!(r.boundary_fired[&q("C_start")], 2);
    assert_eq!(r.boundary_fired[&q("B_A")], 2);
}

#[test]
fn a_non_interrupting_escalation_event_subprocess_lets_the_thrower_go_on() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:intermediateThrowEvent id="Esc"><bpmn:escalationEventDefinition escalationRef="Esc_X"/></bpmn:intermediateThrowEvent>
        <bpmn:task id="Work"/><bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Esc"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Esc" targetRef="Work"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Work" targetRef="End"/>
        <bpmn:subProcess id="EvSub" triggeredByEvent="true">
          <bpmn:startEvent id="EvStart" isInterrupting="false"><bpmn:escalationEventDefinition/></bpmn:startEvent>
          <bpmn:task id="Handle"/>
          <bpmn:sequenceFlow id="E0" sourceRef="EvStart" targetRef="Handle"/>
        </bpmn:subProcess>"#,
    );
    let r = run(&p, &base(2, &(fixed("Work", 10) + &fixed("Handle", 20))));
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!((runs(&r, "Work"), runs(&r, "Handle")), (2, 2));
    assert_eq!(ends(&r, "End"), 2);
    assert!((r.case_cycle_time.mean - 20.0 * 60.0).abs() < 1e-9);
}

#[test]
fn an_error_in_a_called_process_is_caught_by_an_event_subprocess_of_the_caller() {
    let mut p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:callActivity id="Call" calledElement="Child"/>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Call"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Call" targetRef="End"/>
        <bpmn:subProcess id="EvSub" triggeredByEvent="true">
          <bpmn:startEvent id="EvStart"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:startEvent>
          <bpmn:task id="Handle"/><bpmn:endEvent id="EvEnd"/>
          <bpmn:sequenceFlow id="E0" sourceRef="EvStart" targetRef="Handle"/>
          <bpmn:sequenceFlow id="E1" sourceRef="Handle" targetRef="EvEnd"/>
        </bpmn:subProcess>"#,
    );
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
    let r = run(&p, &base(2, &(fixed("C_work", 10) + &fixed("Handle", 5))));
    assert_eq!(r.cases_completed, 2);
    assert!(r.stuck.is_empty(), "{:?}", r.stuck);
    assert_eq!((ends(&r, "End"), ends(&r, "EvEnd")), (0, 2));
    assert!((r.case_cycle_time.mean - 15.0 * 60.0).abs() < 1e-9);
}

#[test]
fn malformed_event_subprocesses_are_binding_errors() {
    let none = with_event_sub("", "");
    let e = bind_err(&none, &base(1, &(fixed("Work", 1) + &fixed("Handle", 1))));
    assert!(e.contains("needs a trigger"), "{e}");
    let error = with_event_sub(
        r#"isInterrupting="false""#,
        r#"<bpmn:errorEventDefinition errorRef="Err_A"/>"#,
    );
    let e = bind_err(&error, &base(1, &(fixed("Work", 1) + &fixed("Handle", 1))));
    assert!(e.contains("always interrupting"), "{e}");
    let timer = with_event_sub("", TIMER);
    let e = bind_err(&timer, &base(1, &(fixed("Work", 1) + &fixed("Handle", 1))));
    assert!(
        e.contains("EvStart (EvStart): timer event needs an [events] entry"),
        "{e}"
    );
}

#[test]
fn the_token_game_offers_and_runs_event_subprocesses() {
    let mut g = Game::new(Arc::new(with_event_sub("", TIMER)));
    g.apply(manual::Action::Start { event: q("Start") })
        .unwrap();
    let (token, events) = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            manual::Pending::Fire { token, events, .. } => Some((token, events)),
            _ => None,
        })
        .expect("the event subprocess is offered");
    assert_eq!(events, [q("EvStart")]);
    g.apply(manual::Action::Fire {
        token,
        event: q("EvStart"),
    })
    .unwrap();
    let mut at: Vec<String> = g.tokens().iter().map(|t| t.node.id.clone()).collect();
    at.sort();
    // `Work` is cancelled; the event subprocess waits while `Handle` runs.
    assert_eq!(at, ["EvSub", "Handle"]);
    assert!(
        g.log
            .iter()
            .any(|l| l == "event subprocess 'EvSub' starts and interrupts its scope"),
        "{:?}",
        g.log
    );
}

#[test]
fn the_token_game_catches_errors_in_event_subprocesses() {
    let mut g = Game::new(Arc::new(erroring_sub("")));
    g.stop_at_events = false;
    g.apply(manual::Action::Start { event: q("Start") })
        .unwrap();
    let work = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            manual::Pending::Complete { token, task } if task == q("Work") => Some(token),
            _ => None,
        })
        .expect("Work waits");
    g.apply(manual::Action::Complete { token: work }).unwrap();
    let mut at: Vec<String> = g.tokens().iter().map(|t| t.node.id.clone()).collect();
    at.sort();
    assert_eq!(at, ["Catch", "Handle", "Sub"]);
    assert!(
        g.log
            .iter()
            .any(|l| l == "error 'A' caught by event subprocess 'Catch'"),
        "{:?}",
        g.log
    );
}

#[test]
fn the_skeleton_covers_event_subprocesses() {
    let p = with_event_sub(r#"isInterrupting="false""#, TIMER);
    let text = Scenario::skeleton(&p, None).to_toml().unwrap();
    assert!(text.contains("[events.\"m.bpmn#EvStart\""), "{text}");
    let bound = Scenario::from_toml(&text).unwrap().bind(&p).unwrap();
    assert_eq!(run_replications(&bound)[0].cases_completed, 100);
}
