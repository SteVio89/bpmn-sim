//! A receive task whose only sender is a black-box pool that never answers stops every token.
//! Binding warns and says what to configure instead.

use std::path::PathBuf;

use sim_core::*;

const MIN: f64 = 60.0;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/black-box-wait")
        .join(name)
}

fn load(files: &[&str]) -> Project {
    let mut p = Project::new();
    for f in files {
        p.add_path(&fixture(f)).expect("fixture parses");
    }
    p
}

fn scenario(task: &str, extra: &str) -> Scenario {
    Scenario::from_toml(&format!(
        r#"
[simulation]
cases = 3
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "60m" }}
triggers = ["shop.bpmn#Start_Shop"]
[resources.Staff]
count = 1
[activities.{task}]
duration = {{ dist = "fixed", value = "5m" }}
resource = "Staff"
{extra}
"#
    ))
    .expect("scenario parses")
}

const REPLY: &str = r#"
[messages.black_box_reply]
Part_Bank = { dist = "fixed", value = "20m" }
"#;

const FROM_OUTSIDE: &str = r#"
[events.Wait_Confirm]
delay = { dist = "fixed", value = "10m" }
"#;

fn wait_warnings(c: &scenario::Checked) -> Vec<String> {
    let wait = QId::new("payment.bpmn", "Wait_Confirm");
    let wait_asks = QId::new("payment-asks.bpmn", "Wait_Confirm");
    c.warnings
        .iter()
        .filter(|w| {
            w.element
                .as_ref()
                .is_some_and(|e| *e == wait || *e == wait_asks)
        })
        .map(|w| w.message.clone())
        .collect()
}

#[test]
fn a_black_box_nobody_contacts_never_answers_even_with_a_reply() {
    let p = load(&["shop.bpmn", "payment.bpmn"]);
    for extra in ["", REPLY] {
        let s = scenario("Task_Charge", extra);
        let c = s.check(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let w = wait_warnings(&c);
        assert_eq!(w.len(), 1, "{:?}", c.warnings);
        assert!(
            w[0].contains("Wait_Confirm (Receive confirmation): waits for a message only from black-box pool 'Bank', which answers only messages it gets, and nothing in the model sends it one"),
            "{}",
            w[0]
        );
        assert!(w[0].contains("[events]"), "{}", w[0]);

        let r = run_replications(&s.bind(&p).unwrap()).remove(0);
        assert_eq!(r.cases_completed, 0);
        assert_eq!(r.stuck.len(), 3);
        assert!(r.stuck.iter().all(|t| t.element.id == "Wait_Confirm"));
    }
}

#[test]
fn a_contacted_black_box_without_a_reply_is_named() {
    let p = load(&["shop.bpmn", "payment-asks.bpmn"]);
    let c = scenario("Send_Charge", "").check(&p);
    let w = wait_warnings(&c);
    assert_eq!(w.len(), 1, "{:?}", c.warnings);
    assert!(
        w[0].contains(
            "only from black-box pool 'Bank', which has no reply in [messages.black_box_reply]"
        ),
        "{}",
        w[0]
    );
}

#[test]
fn a_reply_or_a_trigger_from_outside_lets_the_token_go_on() {
    // The bank is asked and answers after 20m.
    let p = load(&["shop.bpmn", "payment-asks.bpmn"]);
    let s = scenario("Send_Charge", REPLY);
    let c = s.check(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    assert_eq!(r.cases_completed, 3);
    assert_eq!(r.case_cycle_time.mean, 25.0 * MIN);

    // Nobody asks the bank; the confirmation comes from outside the model after 10m.
    let p = load(&["shop.bpmn", "payment.bpmn"]);
    let s = scenario("Task_Charge", FROM_OUTSIDE);
    let c = s.check(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    assert_eq!(r.cases_completed, 3);
    assert!(r.stuck.is_empty());
    assert_eq!(r.case_cycle_time.mean, 15.0 * MIN);
}
