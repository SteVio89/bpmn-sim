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

// ---------------------------------------------------------------- R-13.4.3-01: inclusive join

/// T1 (10m) reaches the join over F1. T2 (30m) is followed by an XOR that leads back to T1 (so
/// to F1, the filled flow) or to T3 (5m, then F2, the empty flow). When T1's token arrives,
/// T2's token can reach the empty flow F2 but also the filled flow F1.
fn or_join_with_shared_upstream() -> Project {
    model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:parallelGateway id="And"/>
        <bpmn:task id="T1"/><bpmn:task id="T2"/><bpmn:task id="T3"/><bpmn:task id="After"/>
        <bpmn:exclusiveGateway id="Xor"/>
        <bpmn:inclusiveGateway id="Join"/>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="And"/>
        <bpmn:sequenceFlow id="Fa" sourceRef="And" targetRef="T1"/>
        <bpmn:sequenceFlow id="Fb" sourceRef="And" targetRef="T2"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T1" targetRef="Join"/>
        <bpmn:sequenceFlow id="Fx" sourceRef="T2" targetRef="Xor"/>
        <bpmn:sequenceFlow id="To_T1" sourceRef="Xor" targetRef="T1"/>
        <bpmn:sequenceFlow id="To_T3" sourceRef="Xor" targetRef="T3"/>
        <bpmn:sequenceFlow id="F2" sourceRef="T3" targetRef="Join"/>
        <bpmn:sequenceFlow id="Fo" sourceRef="Join" targetRef="After"/>
        <bpmn:sequenceFlow id="Fe" sourceRef="After" targetRef="End"/>"#,
    )
}

fn or_join_scenario(option: &str) -> String {
    base(
        3,
        "1h",
        "\"Start\"",
        &format!(
            "{}{}{}{}[gateways.Xor]\nprobabilities = {{ To_T1 = 0.0, To_T3 = 1.0 }}\n",
            fixed("T1", 10),
            fixed("T2", 30),
            fixed("T3", 5),
            fixed("After", 1)
        ),
    )
    .replace("cases = 3", &format!("cases = 3\n{option}"))
}

fn after_starts(r: &ReplicationResult) -> Vec<f64> {
    r.records
        .iter()
        .filter(|x| x.activity.id == "After")
        .map(|x| (x.start - r.cases[x.case as usize].arrival) / MIN)
        .collect()
}

#[test]
fn r_13_4_3_01_inclusive_join_fires_when_waiting_tokens_can_also_reach_a_filled_flow() {
    // Spec (default): at t=10 T2's token can reach the empty F2 but also the filled F1, so the
    // join fires at once. T3's token reaches the join at 35 and fires it again on its own.
    let r = run(&or_join_with_shared_upstream(), &or_join_scenario(""));
    assert_eq!(r.cases_completed, 3);
    assert!(r.stuck.is_empty() && r.failed.is_empty());
    assert_eq!(runs(&r, "After"), 6);
    assert_eq!(after_starts(&r), vec![10.0, 35.0, 10.0, 35.0, 10.0, 35.0]);
    // Explicitly set, the same.
    let r2 = run(
        &or_join_with_shared_upstream(),
        &or_join_scenario("inclusive_join = \"spec\""),
    );
    assert_eq!(after_starts(&r2), after_starts(&r));
}

#[test]
fn r_13_4_3_01_the_old_rule_waits_while_any_token_can_still_arrive() {
    // inclusive_join = "wait_for_upstream" (before M18, Camunda): the join waits for T2's token
    // and fires once at 35 with both flows.
    let r = run(
        &or_join_with_shared_upstream(),
        &or_join_scenario("inclusive_join = \"wait_for_upstream\""),
    );
    assert_eq!(r.cases_completed, 3);
    assert_eq!(after_starts(&r), vec![35.0, 35.0, 35.0]);
}

#[test]
fn r_13_4_3_01_a_token_that_can_reach_only_an_empty_flow_blocks_the_join() {
    // or-upstream-like shape: T2's XOR leads only to T3 or to an end event, never to F1.
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:parallelGateway id="And"/>
        <bpmn:task id="T1"/><bpmn:task id="T2"/><bpmn:task id="T3"/><bpmn:task id="After"/>
        <bpmn:exclusiveGateway id="Xor"/>
        <bpmn:inclusiveGateway id="Join"/>
        <bpmn:endEvent id="End"/><bpmn:endEvent id="End_X"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="And"/>
        <bpmn:sequenceFlow id="Fa" sourceRef="And" targetRef="T1"/>
        <bpmn:sequenceFlow id="Fb" sourceRef="And" targetRef="T2"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T1" targetRef="Join"/>
        <bpmn:sequenceFlow id="Fx" sourceRef="T2" targetRef="Xor"/>
        <bpmn:sequenceFlow id="To_T1" sourceRef="Xor" targetRef="End_X"/>
        <bpmn:sequenceFlow id="To_T3" sourceRef="Xor" targetRef="T3"/>
        <bpmn:sequenceFlow id="F2" sourceRef="T3" targetRef="Join"/>
        <bpmn:sequenceFlow id="Fo" sourceRef="Join" targetRef="After"/>
        <bpmn:sequenceFlow id="Fe" sourceRef="After" targetRef="End"/>"#,
    );
    let r = run(&p, &or_join_scenario(""));
    assert_eq!(after_starts(&r), vec![35.0, 35.0, 35.0]);
}

fn complete(g: &mut Game, task: &str) {
    let token = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::Complete { token, task: t } if t == q(task) => Some(token),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{task} is not waiting: {:?}", g.pending()));
    g.apply(Action::Complete { token }).unwrap();
}

#[test]
fn r_13_4_3_01_the_token_game_uses_the_same_join_rule() {
    let at_after = |g: &Game| g.tokens().iter().any(|t| t.node == q("After"));
    let mut g = Game::new(Arc::new(or_join_with_shared_upstream()));
    g.apply(Action::Start { event: q("Start") }).unwrap();
    complete(&mut g, "T1");
    assert!(at_after(&g), "spec: the join fires while T2 still runs");

    let mut g = Game::new(Arc::new(or_join_with_shared_upstream()));
    g.inclusive_join = InclusiveJoin::WaitForUpstream;
    g.apply(Action::Start { event: q("Start") }).unwrap();
    complete(&mut g, "T1");
    assert!(!at_after(&g), "old rule: the join waits for T2's token");
}

// ---------------------------------------------------------------- R-10.5.1-01: signals

/// A throws signal S after T (10m); the other branch waits for S.
fn signal_model() -> Project {
    model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:parallelGateway id="And"/>
        <bpmn:task id="T"/>
        <bpmn:intermediateThrowEvent id="Throw"><bpmn:signalEventDefinition signalRef="Sig"/></bpmn:intermediateThrowEvent>
        <bpmn:intermediateCatchEvent id="Catch"><bpmn:signalEventDefinition signalRef="Sig"/></bpmn:intermediateCatchEvent>
        <bpmn:endEvent id="E1"/><bpmn:endEvent id="E2"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="And"/>
        <bpmn:sequenceFlow id="F1" sourceRef="And" targetRef="T"/>
        <bpmn:sequenceFlow id="F2" sourceRef="T" targetRef="Throw"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Throw" targetRef="E1"/>
        <bpmn:sequenceFlow id="F4" sourceRef="And" targetRef="Catch"/>
        <bpmn:sequenceFlow id="F5" sourceRef="Catch" targetRef="E2"/>"#,
    )
}

#[test]
fn r_10_5_1_01_signals_are_broadcast_across_cases_by_default() {
    // Cases at 0 and 5 minutes. Case 0's signal (at 10) also reaches case 1's waiting catch
    // event, which then goes on at 10 instead of 15.
    let s = |opt: &str| {
        base(2, "5m", "\"Start\"", &fixed("T", 10))
            .replace("cases = 2", &format!("cases = 2\n{opt}"))
    };
    let r = run(&signal_model(), &s(""));
    assert_eq!(r.signals[0].receivers, 2);
    assert_eq!(r.signals[1].receivers, 0);
    assert_eq!(r.cases_completed, 2);
    assert_eq!(
        Scenario::from_toml(&s("")).unwrap().simulation.signal_scope,
        SignalScope::Global
    );
    // signal_scope = "case": the behaviour before M18.
    let r = run(&signal_model(), &s("signal_scope = \"case\""));
    assert_eq!(r.signals[0].receivers, 1);
    assert_eq!(r.signals[1].receivers, 1);
}
