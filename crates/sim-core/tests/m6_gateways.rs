//! Inclusive (OR) and event-based gateways (M6).

use std::collections::BTreeMap;
use std::path::PathBuf;

use sim_core::*;

const MIN: f64 = 60.0;

fn load(fixture: &str) -> Project {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(fixture);
    let mut p = Project::new();
    p.add_path(&path).expect("fixture parses");
    p
}

fn run(fixture: &str, scenario: &str) -> ReplicationResult {
    let project = load(fixture);
    let bound = Scenario::from_toml(scenario)
        .unwrap()
        .bind(&project)
        .unwrap_or_else(|e| panic!("{e}"));
    run_replications(&bound).remove(0)
}

fn bind_err(fixture: &str, scenario: &str) -> String {
    Scenario::from_toml(scenario)
        .unwrap()
        .bind(&load(fixture))
        .unwrap_err()
        .to_string()
}

fn fixed(name: &str, minutes: u32) -> String {
    format!("[activities.{name}]\nduration = {{ dist = \"fixed\", value = \"{minutes}m\" }}\n")
}

fn base(cases: u32, interval: &str, triggers: &str, extra: &str) -> String {
    format!(
        "[simulation]\ncases = {cases}\n[[arrivals]]\ninter_arrival = {{ dist = \"fixed\", value = \"{interval}\" }}\ntriggers = [{triggers}]\n{extra}"
    )
}

/// Activities per case, from the records.
fn ran(r: &ReplicationResult) -> BTreeMap<u64, Vec<&str>> {
    let mut m: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
    for x in &r.records {
        m.entry(x.case).or_default().push(x.activity.id.as_str());
    }
    m
}

fn ends(r: &ReplicationResult, id: &str) -> u64 {
    r.end_events
        .iter()
        .filter(|(k, _)| k.id == id)
        .map(|(_, n)| *n)
        .sum()
}

// ---------------------------------------------------------------- inclusive gateway

fn or_split(pa: f64, pb: f64, pc: f64, cases: u32) -> String {
    base(
        cases,
        "120m",
        "\"Start\"",
        &format!(
            "{}{}{}[gateways.Or_Split]\nprobabilities = {{ Flow_a = {pa}, Flow_b = {pb}, Flow_c = {pc} }}\n",
            fixed("Task_A", 10),
            fixed("Task_B", 20),
            fixed("Task_C", 30)
        ),
    )
}

#[test]
fn inclusive_split_takes_each_flow_with_its_own_probability() {
    let n = 6000;
    let r = run("or-split-join.bpmn", &or_split(0.5, 0.3, 0.2, n));
    let runs = |t: &str| r.activities[&QId::new("or-split-join.bpmn", t)].runs as f64;
    let n = f64::from(n);
    let within = |count: f64, p: f64| (count - n * p).abs() < 5.0 * (n * p * (1.0 - p)).sqrt();
    // A is also the fallback when nothing is chosen: P = 0.5 + 0.5 * 0.7 * 0.8 = 0.78.
    assert!(within(runs("Task_A"), 0.78), "A {}", runs("Task_A"));
    assert!(within(runs("Task_B"), 0.3), "B {}", runs("Task_B"));
    assert!(within(runs("Task_C"), 0.2), "C {}", runs("Task_C"));
    // B and C are drawn independently: both together about 6 % of the cases.
    let both = ran(&r)
        .values()
        .filter(|v| v.contains(&"Task_B") && v.contains(&"Task_C"))
        .count() as f64;
    assert!(within(both, 0.06), "B and C {both}");
}

#[test]
fn inclusive_join_waits_for_exactly_the_chosen_branches() {
    let r = run("or-split-join.bpmn", &or_split(0.5, 0.5, 0.5, 400));
    assert_eq!(r.cases_completed, 400);
    assert_eq!(ends(&r, "End"), 400, "one token leaves the join per case");
    assert!(r.stuck.is_empty());
    // The case ends when its slowest chosen branch arrives (A 10, B 20, C 30 minutes).
    for (case, acts) in ran(&r) {
        let longest = acts
            .iter()
            .map(|a| match *a {
                "Task_A" => 10.0,
                "Task_B" => 20.0,
                _ => 30.0,
            })
            .fold(0.0, f64::max);
        let c = &r.cases[case as usize];
        assert_eq!(
            c.completion.unwrap() - c.arrival,
            longest * MIN,
            "case {case}: {acts:?}"
        );
    }
}

#[test]
fn inclusive_split_falls_back_to_the_default_flow() {
    let s = |pa: f64| {
        base(
            50,
            "60m",
            "\"Start\"",
            &format!(
                "{}{}[gateways.Or_Split]\nprobabilities = {{ Flow_a = {pa} }}\n",
                fixed("Task_A", 5),
                fixed("Task_D", 5)
            ),
        )
    };
    let r = run("or-default.bpmn", &s(0.0));
    let runs = |r: &ReplicationResult, t: &str| {
        r.activities
            .get(&QId::new("or-default.bpmn", t))
            .map_or(0, |a| a.runs)
    };
    assert_eq!((runs(&r, "Task_A"), runs(&r, "Task_D")), (0, 50));
    let r = run("or-default.bpmn", &s(1.0));
    assert_eq!(
        (runs(&r, "Task_A"), runs(&r, "Task_D")),
        (50, 0),
        "the default flow is only used when nothing else is chosen"
    );
    let e = bind_err(
        "or-default.bpmn",
        &s(0.5).replace("Flow_a = 0.5", "Flow_a = 0.5, Flow_d = 0.5"),
    );
    assert!(e.contains("default flow") && e.contains("Flow_d"), "{e}");
    let e = bind_err(
        "or-split-join.bpmn",
        &or_split(0.5, 0.5, 0.5, 1).replace(", Flow_c = 0.5", ""),
    );
    assert!(e.contains("Flow_c"), "{e}");
}

fn upstream(p_b: f64, interval: &str, cases: u32) -> String {
    base(
        cases,
        interval,
        "\"Start\"",
        &format!(
            "{}{}{}{}{}[gateways.Xor]\nprobabilities = {{ Flow_toB = {p_b}, Flow_toX = {} }}\n",
            fixed("Task_A", 10),
            fixed("Task_Pre", 15),
            fixed("Task_B", 5),
            fixed("Task_X", 30),
            fixed("Task_After", 1),
            1.0 - p_b
        ),
    )
}

#[test]
fn inclusive_join_waits_only_while_another_token_can_still_arrive() {
    // A arrives at the join at 10. The other token is still in Task_Pre (it may come), so the
    // join waits. At 15 the XOR decides: towards B the join fires at 20; towards X the other
    // token can no longer arrive and the join fires at once, at 15 (not at 45 when X ends).
    let r = run("or-upstream.bpmn", &upstream(0.5, "100m", 400));
    assert_eq!(r.cases_completed, 400);
    assert!(r.stuck.is_empty());
    for (case, acts) in ran(&r) {
        let arrival = r.cases[case as usize].arrival;
        let after = r
            .records
            .iter()
            .find(|x| x.case == case && x.activity.id == "Task_After")
            .unwrap();
        let expected = if acts.contains(&"Task_B") { 20.0 } else { 15.0 };
        assert_eq!(
            after.start - arrival,
            expected * MIN,
            "case {case}: {acts:?}"
        );
    }
    assert_eq!(ends(&r, "End"), 400);
}

#[test]
fn inclusive_join_never_mixes_instances() {
    // Cases arrive every 3 minutes, so several instances wait at the join at the same time.
    let r = run("or-upstream.bpmn", &upstream(0.5, "3m", 300));
    assert_eq!(r.cases_completed, 300);
    for (case, acts) in ran(&r) {
        let arrival = r.cases[case as usize].arrival;
        let after = r
            .records
            .iter()
            .find(|x| x.case == case && x.activity.id == "Task_After")
            .unwrap();
        let expected = if acts.contains(&"Task_B") { 20.0 } else { 15.0 };
        assert_eq!(after.start - arrival, expected * MIN, "case {case}");
    }
}

// ---------------------------------------------------------------- event-based gateway

fn ebg(b: u32, c: u32) -> String {
    base(
        5,
        "200m",
        "\"Start_A\", \"Start_B\", \"Start_C\"",
        &format!(
            "[resources.B]\ncount = 1\n[resources.C]\ncount = 1\n{}{}[events.Wait_Timer]\ndelay = {{ dist = \"fixed\", value = \"30m\" }}\n",
            fixed("Task_B", b),
            fixed("Task_C", c)
        ),
    )
}

#[test]
fn event_based_gateway_takes_the_first_event_and_cancels_the_others() {
    // Message at 10, signal at 20, timer at 30: the message wins.
    let r = run("event-gateway.bpmn", &ebg(10, 20));
    assert_eq!(
        (ends(&r, "End_M"), ends(&r, "End_S"), ends(&r, "End_T")),
        (5, 0, 0)
    );
    assert!(
        r.signals.iter().all(|s| s.receivers == 0),
        "the signal subscription was cancelled"
    );
    assert!(r.stuck.is_empty());
    assert_eq!(r.cases_completed, 5);

    // Signal at 20 wins; the timer at 30 is cancelled; the message at 50 finds nobody waiting.
    let r = run("event-gateway.bpmn", &ebg(50, 20));
    assert_eq!(
        (ends(&r, "End_M"), ends(&r, "End_S"), ends(&r, "End_T")),
        (0, 5, 0)
    );
    assert_eq!(
        r.undelivered.iter().map(|u| u.count).sum::<usize>(),
        5,
        "buffered, never picked up"
    );

    // Timer at 30 wins.
    let r = run("event-gateway.bpmn", &ebg(50, 40));
    assert_eq!(
        (ends(&r, "End_M"), ends(&r, "End_S"), ends(&r, "End_T")),
        (0, 0, 5)
    );
    // Each case ends when the last pool is done (B at 50).
    assert_eq!(r.case_cycle_time.mean, 50.0 * MIN);
}

#[test]
fn event_based_gateway_uses_an_already_buffered_message() {
    // B sends its message straight away. With B's trigger first, the message is sent (and
    // buffered) before A's instance reaches the gateway; the gateway then takes it at once.
    let text = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/event-gateway.bpmn"),
    )
    .unwrap()
    .replace("<bpmn:task id=\"Task_B\"/>", "")
    .replace(
        "sourceRef=\"Start_B\" targetRef=\"Task_B\"",
        "sourceRef=\"Start_B\" targetRef=\"Throw_Msg\"",
    )
    .replace(
        "<bpmn:sequenceFlow id=\"B1\" sourceRef=\"Task_B\" targetRef=\"Throw_Msg\"/>",
        "",
    );
    let mut p = Project::new();
    p.add_source("event-gateway.bpmn", &text).unwrap();
    let s = ebg(0, 20)
        .replace(
            "[activities.Task_B]\nduration = { dist = \"fixed\", value = \"0m\" }\n",
            "",
        )
        .replace(
            "\"Start_A\", \"Start_B\", \"Start_C\"",
            "\"Start_B\", \"Start_C\", \"Start_A\"",
        );
    let r = run_replications(
        &Scenario::from_toml(&s)
            .unwrap()
            .bind(&p)
            .unwrap_or_else(|e| panic!("{e}")),
    )
    .remove(0);
    assert_eq!(ends(&r, "End_M"), 5);
    assert!(
        r.messages
            .iter()
            .all(|m| m.outcome == MessageOutcome::Buffered)
    );
    assert!(
        r.undelivered.is_empty(),
        "the gateway consumed the buffered message"
    );
    assert_eq!(r.case_cycle_time.mean, 20.0 * MIN, "C still runs until 20");
}

#[test]
fn event_based_gateway_targets_are_checked() {
    let text = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/event-gateway.bpmn"))
        .unwrap()
        .replace("<bpmn:intermediateCatchEvent id=\"Catch_Sig\"><bpmn:signalEventDefinition signalRef=\"S\"/></bpmn:intermediateCatchEvent>", "<bpmn:task id=\"Catch_Sig\"/>");
    let mut p = Project::new();
    p.add_source("event-gateway.bpmn", &text).unwrap();
    let s = format!(
        "{}[activities.Catch_Sig]\nduration = {{ dist = \"fixed\", value = \"1m\" }}\n",
        ebg(10, 20)
    );
    let e = Scenario::from_toml(&s)
        .unwrap()
        .bind(&p)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("Ebg") && e.contains("event-based gateway"),
        "{e}"
    );
    // A timer behind the gateway needs its delay.
    let e = bind_err(
        "event-gateway.bpmn",
        &ebg(10, 20).replace(
            "[events.Wait_Timer]\ndelay = { dist = \"fixed\", value = \"30m\" }\n",
            "",
        ),
    );
    assert!(e.contains("Wait_Timer"), "{e}");
}

#[test]
fn gateway_fixtures_validate_and_skeletons_run() {
    for f in [
        "or-split-join.bpmn",
        "or-default.bpmn",
        "or-upstream.bpmn",
        "event-gateway.bpmn",
    ] {
        let p = load(f);
        let errors: Vec<_> = validate(&p)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{f}: {errors:?}");
        let sk = Scenario::from_toml(&Scenario::skeleton(&p, None).to_toml().unwrap()).unwrap();
        let r = run_replications(&sk.bind(&p).unwrap_or_else(|e| panic!("{f}: {e}"))).remove(0);
        assert_eq!(r.cases_completed, 100, "{f}");
    }
}

#[test]
fn gateway_runs_are_deterministic() {
    let s = or_split(0.4, 0.4, 0.4, 300).replace(
        "\"fixed\", value = \"10m\"",
        "\"exponential\", mean = \"10m\"",
    );
    let a = serde_json::to_string(&run("or-split-join.bpmn", &s)).unwrap();
    let b = serde_json::to_string(&run("or-split-join.bpmn", &s)).unwrap();
    assert_eq!(a, b);
}
