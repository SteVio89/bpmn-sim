use std::path::PathBuf;

use sim_core::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn load(name: &str) -> Project {
    let mut p = Project::new();
    p.add_path(&fixture(name)).expect("fixture parses");
    p
}

fn messages(issues: &[Issue]) -> Vec<String> {
    issues
        .iter()
        .map(|i| format!("{}: {}", i.element.id, i.message))
        .collect()
}

#[test]
fn parses_single_task_and_ignores_extensions() {
    let p = load("v1-single-task.bpmn");
    let task = p
        .node(&QId::new("v1-single-task.bpmn", "Task_Work"))
        .unwrap();
    assert_eq!(task.kind, NodeKind::Task(TaskKind::User));
    assert_eq!(task.lane.as_deref(), Some("Dev"));
    assert_eq!(task.incoming.len(), 1);
    assert_eq!(task.outgoing.len(), 1);
    assert_eq!(
        p.nodes().count(),
        3,
        "data store and annotation are not flow nodes"
    );
    assert!(validate(&p).is_empty());
}

#[test]
fn matches_by_namespace_not_prefix() {
    assert_eq!(load("prefix-variants.bpmn").nodes().count(), 2);
    assert_eq!(load("v11-link.bpmn").nodes().count(), 5);
}

#[test]
fn link_events_validate_cleanly() {
    assert!(validate(&load("v11-link.bpmn")).is_empty());
}

#[test]
fn validation_reports_problems_with_element_ids() {
    let msgs = messages(&validate(&load("bad-model.bpmn")));
    let has = |id: &str, part: &str| msgs.iter().any(|m| m.starts_with(id) && m.contains(part));
    assert!(has("Task_Orphan", "no incoming"));
    assert!(has("Task_Orphan", "no outgoing"));
    assert!(has("Throw", "no link catch event named 'Missing'"));
    assert!(has("Catch_Never", "can never be reached"));
    assert!(has("Call_Unknown", "Process_Nope"));
    assert!(has("Complex", "unsupported element 'complexGateway'"));
    assert!(!msgs.iter().any(|m| m.starts_with("Task_Dead")), "{msgs:?}");
}

#[test]
fn dangling_reference_error_names_file_and_element() {
    let mut p = Project::new();
    let err = p
        .add_path(&fixture("dangling-flow.bpmn"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("dangling-flow.bpmn") && err.contains("F1") && err.contains("Ghost"),
        "{err}"
    );
}

#[test]
fn rejects_non_bpmn_and_bad_xml() {
    let mut p = Project::new();
    assert!(matches!(
        p.add_source("a.bpmn", "<foo/>"),
        Err(ParseError::NotBpmn { .. })
    ));
    assert!(matches!(
        p.add_source("b.bpmn", "<foo>"),
        Err(ParseError::Xml { .. })
    ));
}

#[test]
fn multi_file_keeps_reused_ids_apart() {
    let p = Project::load_dir(&fixture("multi-file")).unwrap();
    assert_eq!(p.files.len(), 4, "main, two subprocesses, unrelated");
    assert_eq!(p.dmn.len(), 3);
    let a = p
        .node(&QId::new("subprocess-a.bpmn", "Task_Analyse"))
        .unwrap();
    let b = p
        .node(&QId::new("subprocess-b.bpmn", "Task_Analyse"))
        .unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(a.lane.as_deref(), Some("Ops"));
    assert_eq!(a.process.id, "Process_SubA");
    assert_eq!(b.process.id, "Process_SubB");
    // Message flow to a black-box pool stays inside its own file.
    assert_eq!(p.files["subprocess-a.bpmn"].message_flows.len(), 1);
    assert!(
        p.files["subprocess-a.bpmn"]
            .participants
            .values()
            .any(|x| x.process.is_none())
    );
    // zeebe:calledElement and calledElement are both resolved.
    let call_a = p.node(&QId::new("main.bpmn", "Call_A")).unwrap();
    let call_b = p.node(&QId::new("main.bpmn", "Call_B")).unwrap();
    assert_eq!(call_a.called_element.as_deref(), Some("Process_SubA"));
    assert_eq!(call_b.called_element.as_deref(), Some("Process_SubB"));
    // zeebe:calledDecision is read and resolved against the DMN files.
    let rule = p
        .node(&QId::new("subprocess-b.bpmn", "Task_Review"))
        .unwrap();
    assert_eq!(rule.called_decision.as_deref(), Some("decision-risk"));
    assert_eq!(p.decisions["decision-risk"], "decision-risk.dmn");
    assert!(validate(&p).is_empty(), "{:?}", messages(&validate(&p)));
}

#[test]
fn duplicate_process_id_names_both_files() {
    let err = Project::load_dir(&fixture("dup-process"))
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Process_SubA")
            && err.contains("main.bpmn")
            && err.contains("subprocess-a.bpmn"),
        "{err}"
    );
}

#[test]
fn unresolved_call_activity_is_an_error_without_the_child_file() {
    let mut p = Project::new();
    p.add_path(&fixture("multi-file/main.bpmn")).unwrap();
    let issues = validate(&p);
    assert_eq!(
        issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count(),
        2
    );
}
