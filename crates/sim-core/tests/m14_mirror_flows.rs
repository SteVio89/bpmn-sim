//! Message flows at call activities and subprocesses (BPMN §9.3): they only show what the
//! elements inside send and receive, are never sent along, and must match the inside.

use std::path::PathBuf;

use sim_core::*;

const MIN: f64 = 60.0;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/mirror-flows")
        .join(name)
}

fn load(files: &[&str]) -> Project {
    let mut p = Project::new();
    for f in files {
        p.add_path(&fixture(f)).expect("fixture parses");
    }
    p
}

fn scenario(triggers: &str, extra: &str) -> Scenario {
    Scenario::from_toml(&format!(
        r#"
[simulation]
cases = 4
[[arrivals]]
inter_arrival = {{ dist = "fixed", value = "60m" }}
triggers = [{triggers}]
[resources.Staff]
count = 1
{extra}
"#
    ))
    .expect("scenario parses")
}

const ORDER: &str = r#"
[activities.Send_Order]
duration = { dist = "fixed", value = "5m" }
resource = "Staff"
[messages.black_box_reply]
"ordering.bpmn#Part_Sup" = { dist = "fixed", value = "20m" }
"#;

fn flows(r: &ReplicationResult) -> Vec<&str> {
    r.messages.iter().map(|m| m.flow.id.as_str()).collect()
}

#[test]
fn message_flows_at_a_call_activity_are_allowed_and_never_sent() {
    let p = load(&["shop.bpmn", "ordering.bpmn"]);
    let issues = validate(&p);
    assert!(issues.is_empty(), "{issues:?}");

    // A reply configured for the parent's pool must not travel along the mirror flow either.
    let extra =
        format!("{ORDER}\"shop.bpmn#Part_Supplier\" = {{ dist = \"fixed\", value = \"1m\" }}\n");
    let s = scenario(r#""shop.bpmn#Start_Shop""#, &extra);
    let c = s.check(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);

    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    assert_eq!(r.cases_completed, 4);
    // Send the order (5m), the supplier answers after 20m.
    assert_eq!(r.case_cycle_time.mean, 25.0 * MIN);
    let f = flows(&r);
    assert!(
        f.iter().all(|f| *f == "MF_Order" || *f == "MF_Goods"),
        "{f:?}"
    );
    assert_eq!(f.len(), 8);
}

#[test]
fn parent_and_called_process_must_show_the_same_parties() {
    let p = load(&["shop-mismatch.bpmn", "ordering.bpmn"]);
    let issues = validate(&p);
    let call = QId::new("shop-mismatch.bpmn", "Call_Order");
    let at_call: Vec<&str> = issues
        .iter()
        .filter(|i| i.element == call)
        .map(|i| i.message.as_str())
        .collect();
    assert!(
        issues
            .iter()
            .filter(|i| i.element == call)
            .all(|i| i.severity == Severity::Warning)
    );
    assert_eq!(
        at_call,
        [
            "message flow 'MF_Audit' from 'Auditor' ends at this call activity, but nothing in the called process receives messages from 'Auditor'",
            "message flow 'MF_Notice' goes from this call activity to 'Warehouse', but nothing in the called process sends messages to 'Warehouse'",
            "the called process receives messages from 'supplier' ('Receive goods'), but no message flow from 'supplier' ends at this call activity; show that pool and the message flow here too",
        ]
    );
}

#[test]
fn a_mirror_flow_to_an_element_of_a_modelled_process_delivers_nothing() {
    let p = load(&["shop-mismatch.bpmn", "ordering.bpmn"]);
    let s = scenario(
        r#""shop-mismatch.bpmn#Start_Shop", "shop-mismatch.bpmn#Start_Warehouse""#,
        ORDER,
    );
    let c = s.check(&p);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let notice = QId::new("shop-mismatch.bpmn", "Wait_Notice");
    assert!(
        c.warnings
            .iter()
            .any(|w| w.element.as_ref() == Some(&notice)
                && w.message
                    .contains("'Await delivery notice' never gets this message")),
        "{:?}",
        c.warnings
    );

    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    assert!(!flows(&r).contains(&"MF_Notice"));
    assert!(!r.stuck.is_empty());
    assert!(r.stuck.iter().all(|s| s.element == notice));
}

#[test]
fn message_flows_at_an_embedded_subprocess_match_its_inside() {
    let p = load(&["clinic.bpmn"]);
    let issues = validate(&p);
    assert!(issues.is_empty(), "{issues:?}");

    let s = scenario(
        r#""clinic.bpmn#Start_Clinic""#,
        "[activities.Send_Sample]\nduration = { dist = \"fixed\", value = \"5m\" }\nresource = \"Staff\"\n",
    );
    let r = run_replications(&s.bind(&p).unwrap()).remove(0);
    assert_eq!(r.cases_completed, 4);
    assert!(flows(&r).iter().all(|f| *f == "MF_Sample"));

    // Without the message flow at the collapsed subprocess, the lab is not shown on this level.
    let without = std::fs::read_to_string(fixture("clinic.bpmn"))
        .unwrap()
        .replace(
            r#"<bpmn:messageFlow id="MF_Visit" sourceRef="Sub_Visit" targetRef="Part_Lab"/>"#,
            "",
        );
    let validate_xml = |xml: &str| {
        let mut p = Project::new();
        p.add_source("clinic.bpmn", xml).unwrap();
        validate(&p)
    };
    let issues = validate_xml(&without);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].element, QId::new("clinic.bpmn", "Sub_Visit"));
    assert_eq!(
        issues[0].message,
        "the subprocess sends messages to 'Lab' ('Send sample'), but no message flow goes from this subprocess to 'Lab'; show that pool and the message flow here too"
    );

    // Drawn expanded, the subprocess is no level of its own: the flow from inside is enough.
    let expanded = without.replace(
        "    </bpmndi:BPMNPlane>",
        r#"      <bpmndi:BPMNShape id="Send_Sample_di" bpmnElement="Send_Sample"><dc:Bounds x="280" y="250" width="60" height="40"/></bpmndi:BPMNShape>
    </bpmndi:BPMNPlane>"#,
    );
    let issues = validate_xml(&expanded);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn send_and_receive_mix_ups_say_what_to_model_instead() {
    let p = load(&["helper.bpmn"]);
    let s = scenario(
        r#""helper.bpmn#Start_Team""#,
        r#"
[activities.Task_Call]
duration = { dist = "fixed", value = "5m" }
resource = "Staff"
[activities.Send_Ask]
duration = { dist = "fixed", value = "5m" }
resource = "Staff"
[messages.black_box_reply]
Part_Helper = { dist = "fixed", value = "10m" }
"#,
    );
    let errors: Vec<String> = s.check(&p).errors.into_iter().map(|e| e.message).collect();
    let has = |text: &str| {
        assert!(
            errors.iter().any(|e| e.contains(text)),
            "{text}\n{errors:#?}"
        );
    };
    has(
        "message flow helper.bpmn#MF_Back: target 'helper.bpmn#Send_Ask' cannot receive a message: it is a send task, which only sends; to wait for the answer, add a receive task or message catch event after it and let the message flow end there",
    );
    has(
        "message flow helper.bpmn#MF_Ack: source 'helper.bpmn#Wait_Ack' cannot send a message: it is a receive task, which only receives",
    );
    has(
        "message flow helper.bpmn#MF_Call: source 'helper.bpmn#Task_Call' cannot send a message: only send tasks and message throw/end events send; make it a send task",
    );
}

#[test]
fn in_the_manual_game_a_receiver_behind_a_mirror_flow_is_fired_by_hand() {
    use sim_core::manual::{Action, Game, Pending};
    use std::sync::Arc;

    let mut g = Game::new(Arc::new(load(&["shop-mismatch.bpmn", "ordering.bpmn"])));
    g.stop_at_tasks = false;
    g.stop_at_events = false;
    let notice = QId::new("shop-mismatch.bpmn", "Wait_Notice");
    g.apply(Action::Start {
        event: QId::new("shop-mismatch.bpmn", "Start_Warehouse"),
    })
    .unwrap();
    // Nothing is ever sent along the mirror flow, so the player has to bring the message.
    let token = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::Fire { token, events, .. } if events.contains(&notice) => Some(token),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{:?}", g.pending()));
    g.apply(Action::Fire {
        token,
        event: notice,
    })
    .unwrap();
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
}
