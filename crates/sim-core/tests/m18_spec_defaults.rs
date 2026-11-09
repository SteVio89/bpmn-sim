//! M18: where bpmn-sim differed from BPMN 2.0.2, the spec behaviour is now the default and the
//! old behaviour a scenario option. Every test names the rule (docs/bpmn-2.0.2-rules.md) or the
//! decision (D-n) it covers.

use std::sync::Arc;

use sim_core::manual::{Action, Game, Pending};
use sim_core::*;

const NS: &str = r#"xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL""#;
const ROOT: &str = r#"<bpmn:error id="Err_A" errorCode="A"/><bpmn:escalation id="Esc_X" escalationCode="X"/><bpmn:signal id="Sig" name="S"/>"#;
const MIN: f64 = 60.0;

fn q(id: &str) -> QId {
    QId::new("m.bpmn", id)
}

/// A file with root elements and the given processes (`<bpmn:process ...>...</bpmn:process>`).
fn file(processes: &str) -> Project {
    let xml = format!(r#"<bpmn:definitions {NS} id="D">{ROOT}{processes}</bpmn:definitions>"#);
    let mut p = Project::new();
    p.add_source("m.bpmn", &xml).expect("model parses");
    p
}

/// One process `P` with the given content.
fn model(content: &str) -> Project {
    file(&format!(r#"<bpmn:process id="P">{content}</bpmn:process>"#))
}

fn bind<'p>(p: &'p Project, scenario: &str) -> BoundModel<'p> {
    Scenario::from_toml(scenario)
        .unwrap()
        .bind(p)
        .unwrap_or_else(|e| panic!("{e}"))
}

fn run(p: &Project, scenario: &str) -> ReplicationResult {
    run_replications(&bind(p, scenario)).remove(0)
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

fn with_pool(name: &str, minutes: u32, pool: &str) -> String {
    format!(
        "[activities.{name}]\nduration = {{ dist = \"fixed\", value = \"{minutes}m\" }}\nresource = \"{pool}\"\n"
    )
}

fn base(cases: u32, interval: &str, triggers: &str, extra: &str) -> String {
    format!(
        "[simulation]\ncases = {cases}\n[[arrivals]]\ninter_arrival = {{ dist = \"fixed\", value = \"{interval}\" }}\ntriggers = [{triggers}]\n{extra}"
    )
}

fn runs(r: &ReplicationResult, task: &str) -> usize {
    r.activities.get(&q(task)).map_or(0, |a| a.runs)
}

fn cancelled(r: &ReplicationResult, task: &str) -> usize {
    r.activities.get(&q(task)).map_or(0, |a| a.cancelled)
}

fn ends(r: &ReplicationResult, id: &str) -> u64 {
    r.end_events.get(&q(id)).copied().unwrap_or(0)
}

fn issues_of(p: &Project, id: &str) -> Vec<(Severity, String)> {
    validate(p)
        .into_iter()
        .filter(|i| i.element.id == id)
        .map(|i| (i.severity, i.message))
        .collect()
}

// ---------------------------------------------------------------- D-3: runtime exceptions

/// An AND split: the second branch reaches an XOR gateway whose only condition is false and that has
/// no default flow; the first runs a long task on pool "R" (it has started when the exception comes).
fn xor_without_way(kind: &str) -> Project {
    model(&format!(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:parallelGateway id="Split"/>
        <bpmn:{kind} id="Gw"/>
        <bpmn:task id="Long"/><bpmn:task id="After"/>
        <bpmn:endEvent id="E1"/><bpmn:endEvent id="E2"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Split"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Split" targetRef="Long"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Split" targetRef="Gw"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Gw" targetRef="After">
          <bpmn:conditionExpression>= x &gt; 1</bpmn:conditionExpression>
        </bpmn:sequenceFlow>
        <bpmn:sequenceFlow id="F3b" sourceRef="Gw" targetRef="E1">
          <bpmn:conditionExpression>= x &lt; 0</bpmn:conditionExpression>
        </bpmn:sequenceFlow>
        <bpmn:sequenceFlow id="F4" sourceRef="After" targetRef="E1"/>
        <bpmn:sequenceFlow id="F5" sourceRef="Long" targetRef="E2"/>"#
    ))
}

fn pool_scenario(extra: &str) -> String {
    base(
        3,
        "10m",
        "\"Start\"",
        &format!(
            "[resources.R]\ncount = 1\n{}{}{extra}",
            with_pool("Long", 30, "R"),
            fixed("After", 5)
        ),
    )
}

#[test]
fn d3_r_13_4_2_01_xor_without_true_condition_fails_the_case_and_halts_the_instance() {
    let p = xor_without_way("exclusiveGateway");
    let r = run(&p, &pool_scenario(""));
    assert_eq!(r.cases_completed, 0);
    assert!(r.stuck.is_empty(), "not a deadlock: {:?}", r.stuck);
    assert_eq!(r.failed.len(), 3);
    let f = &r.failed[0];
    assert_eq!(f.element, q("Gw"));
    assert_eq!(
        f.reason,
        FailureReason::RuntimeException {
            rule: "R-13.4.2-01".into()
        }
    );
    assert_eq!(f.reason.to_string(), "runtime exception (R-13.4.2-01)");
    // The instance halts: the other branch's task never completes; its token is kept.
    assert_eq!(runs(&r, "Long"), 0);
    assert_eq!(cancelled(&r, "Long"), 3);
    assert_eq!(ends(&r, "E2"), 0);
    assert!(f.tokens.iter().any(|t| t.element == q("Gw")));
    assert!(f.tokens.iter().any(|t| t.element == q("Long")));
    // The resource was released: no case waits for it (all three cancelled at once, no queue).
    let waits: Vec<f64> = r
        .records
        .iter()
        .filter(|x| x.activity.id == "Long")
        .map(|x| x.waiting())
        .collect();
    assert!(waits.iter().all(|w| *w == 0.0), "{waits:?}");
    assert!(r.cases.iter().all(|c| c.failed && c.completion.is_none()));
}

#[test]
fn d3_r_10_6_3_01_inclusive_split_without_true_condition_fails_the_case() {
    let p = xor_without_way("inclusiveGateway");
    let r = run(&p, &pool_scenario(""));
    assert_eq!(r.failed.len(), 3);
    assert_eq!(
        r.failed[0].reason.to_string(),
        "runtime exception (R-10.6.3-01)"
    );
    assert!(r.stuck.is_empty());
}

#[test]
fn d3_a_runtime_exception_is_not_caught_by_error_boundary_events() {
    // D-3: a runtime exception is no BPMN error.
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:startEvent id="S_start"/>
          <bpmn:exclusiveGateway id="Gw"/>
          <bpmn:endEvent id="S_end"/>
          <bpmn:sequenceFlow id="S0" sourceRef="S_start" targetRef="Gw"/>
          <bpmn:sequenceFlow id="S1" sourceRef="Gw" targetRef="S_end">
            <bpmn:conditionExpression>= x &gt; 1</bpmn:conditionExpression>
          </bpmn:sequenceFlow>
          <bpmn:sequenceFlow id="S2" sourceRef="Gw" targetRef="S_end">
            <bpmn:conditionExpression>= x &lt; 0</bpmn:conditionExpression>
          </bpmn:sequenceFlow>
        </bpmn:subProcess>
        <bpmn:boundaryEvent id="Catch" attachedToRef="Sub"><bpmn:errorEventDefinition/></bpmn:boundaryEvent>
        <bpmn:endEvent id="End"/><bpmn:endEvent id="Caught"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="End"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Catch" targetRef="Caught"/>"#,
    );
    let r = run(&p, &base(2, "1h", "\"Start\"", ""));
    assert_eq!(ends(&r, "Caught"), 0);
    assert_eq!(r.failed.len(), 2);
    assert!(r.boundary_fired.is_empty());
}

// ---------------------------------------------------------------- D-4: uncaught errors

fn error_in_branch() -> Project {
    model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:parallelGateway id="Split"/>
        <bpmn:task id="Short"/><bpmn:task id="Long"/>
        <bpmn:endEvent id="ErrEnd"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:endEvent>
        <bpmn:endEvent id="E2"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Split"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Split" targetRef="Short"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Split" targetRef="Long"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Short" targetRef="ErrEnd"/>
        <bpmn:sequenceFlow id="F4" sourceRef="Long" targetRef="E2"/>"#,
    )
}

#[test]
fn d4_r_10_5_1_03_an_uncaught_error_halts_the_whole_instance() {
    // Cases every 10 minutes; Long needs pool R (one unit) for 30 minutes. The error at t+5
    // cancels Long and releases R, so the next case's Long never waits.
    let p = error_in_branch();
    let s = base(
        3,
        "10m",
        "\"Start\"",
        &format!(
            "[resources.R]\ncount = 1\n{}{}",
            fixed("Short", 5),
            with_pool("Long", 30, "R")
        ),
    );
    let r = run(&p, &s);
    assert_eq!(r.cases_completed, 0);
    assert_eq!(r.failed.len(), 3);
    for f in &r.failed {
        assert_eq!(f.reason, FailureReason::UncaughtError { code: "A".into() });
        assert_eq!(f.reason.to_string(), "uncaught error 'A' (R-10.5.1-03)");
        assert_eq!(f.element, q("ErrEnd"));
        assert_eq!(f.time - r.cases[f.case as usize].arrival, 5.0 * MIN);
    }
    assert_eq!(runs(&r, "Long"), 0);
    assert_eq!(cancelled(&r, "Long"), 3);
    let long: Vec<&ActivityRecord> = r
        .records
        .iter()
        .filter(|x| x.activity.id == "Long")
        .collect();
    assert!(
        long.iter()
            .all(|x| x.waiting() == 0.0 && x.work == 5.0 * MIN)
    );
    assert!(r.stuck.is_empty());
    assert_eq!(r.resources["R"].busy_time, 15.0 * MIN);
}

#[test]
fn d4_an_uncaught_error_in_a_called_process_halts_the_caller_too() {
    let p = file(
        r#"
        <bpmn:process id="P">
          <bpmn:startEvent id="Start"/>
          <bpmn:parallelGateway id="Split"/>
          <bpmn:callActivity id="Call" calledElement="Q"/>
          <bpmn:task id="Long"/>
          <bpmn:endEvent id="E1"/><bpmn:endEvent id="E2"/>
          <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Split"/>
          <bpmn:sequenceFlow id="F1" sourceRef="Split" targetRef="Call"/>
          <bpmn:sequenceFlow id="F2" sourceRef="Split" targetRef="Long"/>
          <bpmn:sequenceFlow id="F3" sourceRef="Call" targetRef="E1"/>
          <bpmn:sequenceFlow id="F4" sourceRef="Long" targetRef="E2"/>
        </bpmn:process>
        <bpmn:process id="Q">
          <bpmn:startEvent id="Q_start"/>
          <bpmn:task id="Q_task"/>
          <bpmn:endEvent id="Q_err"><bpmn:errorEventDefinition errorRef="Err_A"/></bpmn:endEvent>
          <bpmn:sequenceFlow id="Q0" sourceRef="Q_start" targetRef="Q_task"/>
          <bpmn:sequenceFlow id="Q1" sourceRef="Q_task" targetRef="Q_err"/>
        </bpmn:process>"#,
    );
    let r = run(
        &p,
        &base(
            2,
            "1h",
            "\"Start\"",
            &format!("{}{}", fixed("Q_task", 5), fixed("Long", 30)),
        ),
    );
    assert_eq!(r.failed.len(), 2);
    assert_eq!(r.failed[0].element, q("Q_err"));
    assert_eq!(r.failed[0].process.id, "Q");
    // The caller halted as well: Long was cancelled, nothing reached an end event.
    assert_eq!(cancelled(&r, "Long"), 2);
    assert_eq!(ends(&r, "E1") + ends(&r, "E2"), 0);
    assert!(r.stuck.is_empty());
}

#[test]
fn d4_an_uncaught_escalation_has_no_effect_but_is_reported() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:intermediateThrowEvent id="Esc"><bpmn:escalationEventDefinition escalationRef="Esc_X"/></bpmn:intermediateThrowEvent>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Esc"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Esc" targetRef="End"/>"#,
    );
    let r = run(&p, &base(3, "1h", "\"Start\"", ""));
    assert_eq!(r.cases_completed, 3);
    assert!(r.failed.is_empty());
    assert_eq!(r.uncaught_escalations.len(), 3);
    assert_eq!(r.uncaught_escalations[0].code, "X");
    assert_eq!(r.uncaught_escalations[0].element, q("Esc"));
}

#[test]
fn d4_the_token_game_halts_on_an_uncaught_error() {
    let mut g = Game::new(Arc::new(error_in_branch()));
    g.apply(Action::Start { event: q("Start") }).unwrap();
    assert!(g.failure().is_none());
    let short = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::Complete { token, task } if task == q("Short") => Some(token),
            _ => None,
        })
        .unwrap();
    g.apply(Action::Complete { token: short }).unwrap();
    // The error end event stops before it throws (stop at events).
    let ready = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::Continue { token, .. } => Some(token),
            _ => None,
        })
        .unwrap();
    g.apply(Action::Continue { token: ready }).unwrap();
    assert_eq!(g.failure(), Some("uncaught error 'A' (R-10.5.1-03)"));
    // Nothing can be done any more; the tokens are kept (at Long and at the end event).
    assert!(g.pending().is_empty());
    assert_eq!(g.tokens().len(), 2);
    assert!(g.finished().is_empty());
    let long = g
        .tokens()
        .into_iter()
        .find(|t| t.node == q("Long"))
        .unwrap();
    assert!(g.apply(Action::Complete { token: long.id }).is_err());
}

// ---------------------------------------------------------------- D-10 (no change)

#[test]
fn d10_r_13_3_4_04_terminate_inside_a_subprocess_lets_the_parent_continue() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:startEvent id="S_start"/>
          <bpmn:parallelGateway id="S_split"/>
          <bpmn:task id="Slow"/>
          <bpmn:endEvent id="Term"><bpmn:terminateEventDefinition/></bpmn:endEvent>
          <bpmn:endEvent id="S_end"/>
          <bpmn:sequenceFlow id="S0" sourceRef="S_start" targetRef="S_split"/>
          <bpmn:sequenceFlow id="S1" sourceRef="S_split" targetRef="Slow"/>
          <bpmn:sequenceFlow id="S2" sourceRef="S_split" targetRef="Term"/>
          <bpmn:sequenceFlow id="S3" sourceRef="Slow" targetRef="S_end"/>
        </bpmn:subProcess>
        <bpmn:task id="After"/>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="After"/>
        <bpmn:sequenceFlow id="F2" sourceRef="After" targetRef="End"/>"#,
    );
    let r = run(
        &p,
        &base(
            2,
            "1h",
            "\"Start\"",
            &format!("{}{}", fixed("Slow", 30), fixed("After", 5)),
        ),
    );
    assert_eq!(r.cases_completed, 2);
    assert_eq!(r.case_cycle_time.mean, 5.0 * MIN);
    assert_eq!(cancelled(&r, "Slow"), 2);
    assert_eq!(ends(&r, "End"), 2);
    assert!(r.failed.is_empty() && r.stuck.is_empty());
}
