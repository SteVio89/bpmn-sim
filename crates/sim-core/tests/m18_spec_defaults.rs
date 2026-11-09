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

// ---------------------------------------------------------------- D-1: R-10.3-04, R-13.2-01

/// A process without start events: A and B have no incoming flow.
fn startless() -> Project {
    model(
        r#"
        <bpmn:task id="A"/><bpmn:task id="B"/>
        <bpmn:endEvent id="EA"/><bpmn:endEvent id="EB"/>
        <bpmn:sequenceFlow id="F1" sourceRef="A" targetRef="EA"/>
        <bpmn:sequenceFlow id="F2" sourceRef="B" targetRef="EB"/>"#,
    )
}

#[test]
fn d1_r_10_3_04_a_top_level_process_without_start_events_starts_its_free_nodes() {
    let p = startless();
    let s = base(
        4,
        "1h",
        "\"P\"",
        &format!("{}{}", fixed("A", 10), fixed("B", 20)),
    );
    let r = run(&p, &s);
    assert_eq!(r.cases_completed, 4);
    assert_eq!((runs(&r, "A"), runs(&r, "B")), (4, 4));
    assert_eq!(r.case_cycle_time.mean, 20.0 * MIN);
    assert_eq!(r.instances.len(), 4, "one instance per case");
    // A qualified process id works too.
    let r = run(&p, &s.replace("\"P\"", "\"m.bpmn#P\""));
    assert_eq!(r.cases_completed, 4);
}

#[test]
fn d1_r_13_2_01_a_process_without_start_events_is_a_warning() {
    let p = startless();
    let issues = issues_of(&p, "P");
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].0, Severity::Warning);
    assert!(issues[0].1.contains("R-13.2-01"), "{issues:?}");
    // Its free nodes are no "no incoming sequence flow" findings: they start the process.
    assert!(issues_of(&p, "A").is_empty());
}

#[test]
fn d1_only_processes_without_start_events_can_be_triggered_by_id() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="End"/>"#,
    );
    let e = bind_err(&p, &base(1, "1h", "\"P\"", ""));
    assert!(e.contains("is a process with start events"), "{e}");
}

#[test]
fn d1_skeleton_and_token_game_start_a_process_without_start_events() {
    let p = startless();
    let s = Scenario::skeleton(&p, None);
    assert_eq!(s.arrivals[0].triggers, ["m.bpmn#P"]);
    assert!(s.bind(&p).is_ok());
    let mut g = Game::new(Arc::new(startless()));
    assert_eq!(g.start_events(), [q("P")]);
    g.apply(Action::Start { event: q("P") }).unwrap();
    let mut at: Vec<String> = g.tokens().iter().map(|t| t.node.id.to_string()).collect();
    at.sort();
    assert_eq!(at, ["A", "B"]);
}

#[test]
fn d1_r_10_3_04_free_nodes_next_to_start_events_are_an_error_and_never_run() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="T"/><bpmn:task id="Free"/>
        <bpmn:exclusiveGateway id="FreeGw"/>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="T"/>
        <bpmn:sequenceFlow id="F1" sourceRef="T" targetRef="End"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Free" targetRef="End"/>
        <bpmn:sequenceFlow id="F3" sourceRef="FreeGw" targetRef="End"/>"#,
    );
    for id in ["Free", "FreeGw"] {
        let issues = issues_of(&p, id);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].0, Severity::Error);
        assert!(
            issues[0].1.contains("R-10.3-04") && issues[0].1.contains("R-13.2-01"),
            "{issues:?}"
        );
    }
    let r = run(
        &p,
        &base(
            2,
            "1h",
            "\"Start\"",
            &format!("{}{}", fixed("T", 5), fixed("Free", 5)),
        ),
    );
    assert_eq!((runs(&r, "T"), runs(&r, "Free")), (2, 0));
    assert_eq!(r.cases_completed, 2);
}

// ---------------------------------------------------------------- D-2: R-10.5.2-04, R-13.3.4-01

#[test]
fn d2_r_13_3_4_01_an_embedded_subprocess_with_several_none_starts_is_an_error() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/>
        <bpmn:subProcess id="Sub">
          <bpmn:startEvent id="S1"/><bpmn:startEvent id="S2"/>
          <bpmn:endEvent id="SE"/>
          <bpmn:sequenceFlow id="S1e" sourceRef="S1" targetRef="SE"/>
          <bpmn:sequenceFlow id="S2e" sourceRef="S2" targetRef="SE"/>
        </bpmn:subProcess>
        <bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Sub"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Sub" targetRef="End"/>"#,
    );
    let issues = issues_of(&p, "Sub");
    assert!(
        issues
            .iter()
            .any(|(s, m)| *s == Severity::Error && m.contains("R-13.3.4-01")),
        "{issues:?}"
    );
    let e = bind_err(&p, &base(1, "1h", "\"Start\"", ""));
    assert!(
        e.contains("several none start events") && e.contains("R-13.3.4-01"),
        "{e}"
    );
}

/// P calls Q, which has two None start events: S1 -> T1 and S2 -> T2.
fn call_with_two_starts() -> Project {
    file(
        r#"
        <bpmn:process id="P">
          <bpmn:startEvent id="Start"/>
          <bpmn:callActivity id="Call" calledElement="Q"/>
          <bpmn:endEvent id="End"/>
          <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Call"/>
          <bpmn:sequenceFlow id="F1" sourceRef="Call" targetRef="End"/>
        </bpmn:process>
        <bpmn:process id="Q">
          <bpmn:startEvent id="S1"/><bpmn:startEvent id="S2"/>
          <bpmn:task id="T1"/><bpmn:task id="T2"/>
          <bpmn:endEvent id="E1"/><bpmn:endEvent id="E2"/>
          <bpmn:sequenceFlow id="Q1" sourceRef="S1" targetRef="T1"/>
          <bpmn:sequenceFlow id="Q2" sourceRef="T1" targetRef="E1"/>
          <bpmn:sequenceFlow id="Q3" sourceRef="S2" targetRef="T2"/>
          <bpmn:sequenceFlow id="Q4" sourceRef="T2" targetRef="E2"/>
        </bpmn:process>"#,
    )
}

fn call_scenario(calls: &str) -> String {
    base(
        3,
        "1h",
        "\"Start\"",
        &format!("{}{}{calls}", fixed("T1", 10), fixed("T2", 20)),
    )
}

#[test]
fn d2_r_10_5_2_04_a_call_needs_to_name_one_of_several_none_starts() {
    let p = call_with_two_starts();
    let e = bind_err(&p, &call_scenario(""));
    assert!(
        e.contains("several none start events ('S1', 'S2')")
            && e.contains("R-10.5.2-04")
            && e.contains("[calls.\"Call\"] start"),
        "{e}"
    );
    let e = bind_err(&p, &call_scenario("[calls.Call]\nstart = \"T1\"\n"));
    assert!(e.contains("choose one of 'S1', 'S2'"), "{e}");
}

#[test]
fn d2_r_10_5_2_04_exactly_the_chosen_start_fires() {
    let p = call_with_two_starts();
    let r = run(&p, &call_scenario("[calls.Call]\nstart = \"S2\"\n"));
    assert_eq!(r.cases_completed, 3);
    assert_eq!((runs(&r, "T1"), runs(&r, "T2")), (0, 3));
    assert_eq!(r.case_cycle_time.mean, 20.0 * MIN);
    let r = run(&p, &call_scenario("[calls.Call]\nstart = \"m.bpmn#S1\"\n"));
    assert_eq!((runs(&r, "T1"), runs(&r, "T2")), (3, 0));
}

#[test]
fn d2_skeleton_and_update_from_model_offer_the_choice() {
    let p = call_with_two_starts();
    let s = Scenario::skeleton(&p, None);
    assert_eq!(s.calls["m.bpmn#Call"].start, "S1");
    // The called process starts through the call, not by arrivals.
    assert_eq!(s.arrivals[0].triggers, ["m.bpmn#Start"]);
    assert!(s.bind(&p).is_ok());
    let needs = sim_core::needs::needs(&p, p.node(&q("Call")).unwrap());
    assert_eq!(
        needs,
        [sim_core::needs::Need::CallStart {
            choices: vec![q("S1"), q("S2")]
        }]
    );
    let toml = s.to_toml().unwrap();
    assert!(toml.contains("[calls.\"m.bpmn#Call\"]"), "{toml}");
}

#[test]
fn d2_the_token_game_lets_the_player_pick_the_start_or_follows_the_scenario() {
    let mut g = Game::new(Arc::new(call_with_two_starts()));
    g.apply(Action::Start { event: q("Start") }).unwrap();
    let (token, starts) = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::StartCall { token, starts, .. } => Some((token, starts)),
            _ => None,
        })
        .expect("the player picks the start event");
    assert_eq!(starts, [q("S1"), q("S2")]);
    g.apply(Action::StartCall {
        token,
        start: q("S2"),
    })
    .unwrap();
    let at: Vec<QId> = g.tokens().into_iter().map(|t| t.node).collect();
    assert!(at.contains(&q("T2")) && !at.contains(&q("T1")), "{at:?}");

    // With a scenario's [calls] entry the game starts there by itself.
    let mut g = Game::new(Arc::new(call_with_two_starts()));
    g.use_scenario(&Scenario::from_toml(&call_scenario("[calls.Call]\nstart = \"S1\"\n")).unwrap());
    g.apply(Action::Start { event: q("Start") }).unwrap();
    let at: Vec<QId> = g.tokens().into_iter().map(|t| t.node).collect();
    assert!(at.contains(&q("T1")) && !at.contains(&q("T2")), "{at:?}");
}

// ---------------------------------------------------------------- R-10.5.5-03: timeCycle

/// Work (45m) with a non-interrupting timer boundary event `timer` (its definition content)
/// that leads to Remind (1m).
fn with_reminder(timer: &str) -> Project {
    model(&format!(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="Work"/><bpmn:task id="Remind"/>
        <bpmn:boundaryEvent id="Tick" attachedToRef="Work" cancelActivity="false">
          <bpmn:timerEventDefinition>{timer}</bpmn:timerEventDefinition>
        </bpmn:boundaryEvent>
        <bpmn:endEvent id="End"/><bpmn:endEvent id="RemEnd"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Work"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Work" targetRef="End"/>
        <bpmn:sequenceFlow id="F2" sourceRef="Tick" targetRef="Remind"/>
        <bpmn:sequenceFlow id="F3" sourceRef="Remind" targetRef="RemEnd"/>"#
    ))
}

fn reminder_scenario(boundary: &str, sim: &str) -> String {
    base(
        2,
        "2h",
        "\"Start\"",
        &format!(
            "{}{}[boundary.Tick]\ndelay = {{ dist = \"fixed\", value = \"10m\" }}\n{boundary}",
            fixed("Work", 45),
            fixed("Remind", 1)
        ),
    )
    .replace("cases = 2", &format!("cases = 2\n{sim}"))
}

#[test]
fn r_10_5_5_03_the_parser_records_the_timer_kind() {
    let kind = |t: &str| with_reminder(t).node(&q("Tick")).unwrap().timer;
    assert_eq!(
        kind("<bpmn:timeDate>2024-01-01T00:00:00Z</bpmn:timeDate>"),
        Some(TimerKind::Date)
    );
    assert_eq!(
        kind("<bpmn:timeDuration>PT10M</bpmn:timeDuration>"),
        Some(TimerKind::Duration)
    );
    assert_eq!(
        kind("<bpmn:timeCycle>R3/PT10M</bpmn:timeCycle>"),
        Some(TimerKind::Cycle { repeat: Some(3) })
    );
    assert_eq!(
        kind("<bpmn:timeCycle>R/PT10M</bpmn:timeCycle>"),
        Some(TimerKind::Cycle { repeat: None })
    );
    assert_eq!(
        kind("<bpmn:timeCycle>0 0 9 * * ?</bpmn:timeCycle>"),
        Some(TimerKind::Cycle { repeat: None })
    );
    assert_eq!(kind(""), None);
}

#[test]
fn r_10_5_5_03_a_non_interrupting_time_cycle_boundary_fires_repeatedly() {
    // R3/PT10M on a 45-minute task: at 10, 20 and 30 minutes.
    let r = run(
        &with_reminder("<bpmn:timeCycle>R3/PT10M</bpmn:timeCycle>"),
        &reminder_scenario("", ""),
    );
    assert_eq!(r.boundary_fired[&q("Tick")], 6);
    assert_eq!(runs(&r, "Remind"), 6);
    let starts: Vec<f64> = r
        .records
        .iter()
        .filter(|x| x.activity.id == "Remind" && x.case == 0)
        .map(|x| x.start / MIN)
        .collect();
    assert_eq!(starts, [10.0, 20.0, 30.0]);
    assert_eq!(r.cases_completed, 2);
    // A scenario repeat overrides the model's count.
    let r = run(
        &with_reminder("<bpmn:timeCycle>R3/PT10M</bpmn:timeCycle>"),
        &reminder_scenario("repeat = 2\n", ""),
    );
    assert_eq!(r.boundary_fired[&q("Tick")], 4);
}

#[test]
fn r_10_5_5_03_other_timers_fire_once() {
    let r = run(
        &with_reminder("<bpmn:timeDuration>PT10M</bpmn:timeDuration>"),
        &reminder_scenario("", ""),
    );
    assert_eq!(r.boundary_fired[&q("Tick")], 2);
    let r = run(&with_reminder(""), &reminder_scenario("", ""));
    assert_eq!(r.boundary_fired[&q("Tick")], 2);
    // repeat is only for non-interrupting timeCycle timers.
    let p = with_reminder("");
    let b = bind(&p, &reminder_scenario("repeat = 3\n", ""));
    assert!(
        b.warnings.iter().any(|w| w.contains("repeat is ignored")),
        "{:?}",
        b.warnings
    );
}

#[test]
fn r_10_5_5_03_an_unbounded_cycle_needs_a_count_or_a_horizon() {
    let p = with_reminder("<bpmn:timeCycle>R/PT10M</bpmn:timeCycle>");
    let e = bind_err(&p, &reminder_scenario("", ""));
    assert!(
        e.contains("no repeat count") && e.contains("simulation.horizon"),
        "{e}"
    );
    // With a horizon it fires as long as the task runs: at 10, 20, 30 and 40 minutes.
    let r = run(&p, &reminder_scenario("", "horizon = \"10h\""));
    assert_eq!(r.boundary_fired[&q("Tick")], 8);
    assert_eq!(r.cases_completed, 2);
    // Or a count from the scenario.
    let r = run(&p, &reminder_scenario("repeat = 1\n", ""));
    assert_eq!(r.boundary_fired[&q("Tick")], 2);
}

#[test]
fn r_10_5_5_03_a_non_interrupting_time_cycle_event_subprocess_starts_repeatedly() {
    let p = model(
        r#"
        <bpmn:startEvent id="Start"/><bpmn:task id="Work"/><bpmn:endEvent id="End"/>
        <bpmn:sequenceFlow id="F0" sourceRef="Start" targetRef="Work"/>
        <bpmn:sequenceFlow id="F1" sourceRef="Work" targetRef="End"/>
        <bpmn:subProcess id="EvSub" triggeredByEvent="true">
          <bpmn:startEvent id="EvStart" isInterrupting="false">
            <bpmn:timerEventDefinition><bpmn:timeCycle>R2/PT10M</bpmn:timeCycle></bpmn:timerEventDefinition>
          </bpmn:startEvent>
          <bpmn:endEvent id="EvEnd"/>
          <bpmn:sequenceFlow id="E0" sourceRef="EvStart" targetRef="EvEnd"/>
        </bpmn:subProcess>"#,
    );
    let s = base(
        2,
        "2h",
        "\"Start\"",
        &format!(
            "{}[events.EvStart]\ndelay = {{ dist = \"fixed\", value = \"10m\" }}\n",
            fixed("Work", 45)
        ),
    );
    let r = run(&p, &s);
    assert_eq!(r.boundary_fired[&q("EvStart")], 4);
    assert_eq!(r.cases_completed, 2);
    let r = run(
        &p,
        &s.replace("value = \"10m\" }", "value = \"10m\" }\nrepeat = 3"),
    );
    assert_eq!(r.boundary_fired[&q("EvStart")], 6);
}

#[test]
fn r_10_5_5_03_the_token_game_fires_timers_as_often_as_the_engine() {
    let fire_count = |timer: &str| {
        let mut g = Game::new(Arc::new(with_reminder(timer)));
        g.stop_at_tasks = true;
        g.apply(Action::Start { event: q("Start") }).unwrap();
        let mut n = 0;
        while let Some(token) = g.pending().into_iter().find_map(|p| match p {
            Pending::Fire { token, events, .. } if events.contains(&q("Tick")) => Some(token),
            _ => None,
        }) {
            g.apply(Action::Fire {
                token,
                event: q("Tick"),
            })
            .unwrap();
            n += 1;
            if n > 10 {
                break;
            }
        }
        n
    };
    assert_eq!(
        fire_count("<bpmn:timeDuration>PT10M</bpmn:timeDuration>"),
        1
    );
    assert_eq!(fire_count("<bpmn:timeCycle>R3/PT10M</bpmn:timeCycle>"), 3);
    assert_eq!(fire_count("<bpmn:timeCycle>R/PT10M</bpmn:timeCycle>"), 11);
}
