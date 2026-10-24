//! BPMN 2.0.2 conformance: files serialized the ways the standard allows (default namespace,
//! any prefix, QName references, root event definitions, any element order) parse, validate
//! and run. Every file in `tests/fixtures/conformance` must pass without any issue.

use std::path::PathBuf;

use sim_core::*;

const BPMN_NS: &str = "http://www.omg.org/spec/BPMN/20100524/MODEL";

fn conformance_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/conformance")
}

fn load(name: &str) -> Project {
    let mut p = Project::new();
    p.add_path(&conformance_dir().join(name))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    p
}

fn source(xml: &str) -> Result<Project, ParseError> {
    let mut p = Project::new();
    p.add_source("inline.bpmn", xml)?;
    Ok(p)
}

/// A file with one process whose flow elements are `body`.
fn process(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<definitions xmlns="{BPMN_NS}" id="Defs" targetNamespace="http://example.org">
  <process id="Process_1">{body}</process>
</definitions>"#
    )
}

fn node<'p>(p: &'p Project, file: &str, id: &str) -> &'p FlowNode {
    p.node(&QId::new(file, id))
        .unwrap_or_else(|| panic!("{file}#{id} not parsed"))
}

/// Skeleton scenario, round-tripped through TOML, bound and run.
fn run_skeleton(name: &str, p: &Project) -> ReplicationResult {
    let text = Scenario::skeleton(p, None).to_toml().unwrap();
    let bound = Scenario::from_toml(&text)
        .unwrap_or_else(|e| panic!("{name}: {e}\n{text}"))
        .bind(p)
        .unwrap_or_else(|e| panic!("{name}: {e}\n{text}"));
    run_replications(&bound).remove(0)
}

#[test]
fn every_conformance_fixture_parses_validates_cleanly_and_runs() {
    let mut names: Vec<String> = std::fs::read_dir(conformance_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".bpmn"))
        .collect();
    names.sort();
    assert!(names.len() >= 4, "{names:?}");
    for name in names {
        let p = load(&name);
        let issues: Vec<String> = validate(&p).iter().map(ToString::to_string).collect();
        assert!(issues.is_empty(), "{name}: {issues:#?}");
        let r = run_skeleton(&name, &p);
        assert_eq!(r.cases_started, 100, "{name}");
        assert_eq!(r.cases_completed, 100, "{name}");
        assert!(r.stuck.is_empty(), "{name}: {:?}", r.stuck);
    }
}

#[test]
fn default_namespace_with_redundant_incoming_and_outgoing_children() {
    let f = "default-namespace.bpmn";
    let p = load(f);
    assert_eq!(p.nodes().count(), 7);
    // `<incoming>`/`<outgoing>` repeat the sequence flows; the wiring comes from the flows only.
    let split = node(&p, f, "Split");
    assert_eq!(split.incoming.len(), 1);
    assert_eq!(split.outgoing.len(), 2);
    assert_eq!(split.default_flow, Some(QId::new(f, "F_other")));
    assert_eq!(node(&p, f, "Join").incoming.len(), 2);
    // CDATA content of a typed formal expression.
    let ok = p.flow(&QId::new(f, "F_ok")).unwrap();
    assert_eq!(
        ok.condition.as_deref(),
        Some("getDataObject('order')/amount < 1000")
    );
    assert_eq!(ok.name.as_deref(), Some("ok"));
}

#[test]
fn xpath_conditions_get_shares_in_the_skeleton() {
    // XPath is the default expression language of BPMN; only FEEL is evaluated, so the split
    // is decided by shares instead of failing to bind.
    let p = load("default-namespace.bpmn");
    let s = Scenario::skeleton(&p, None);
    assert!(
        s.gateways.contains_key("default-namespace.bpmn#Split"),
        "{:?}",
        s.gateways.keys()
    );
}

#[test]
fn qname_references_with_a_prefix_resolve() {
    let f = "qname-refs.bpmn";
    let p = load(f);
    let file = &p.files[f];
    let processes: Vec<_> = file
        .participants
        .values()
        .map(|x| x.process.as_ref().unwrap().id.as_str())
        .collect();
    assert_eq!(processes, ["Process_Customer", "Process_Shop"]);
    assert_eq!(
        node(&p, f, "Call_Pack").called_element.as_deref(),
        Some("Process_Pack")
    );
    assert_eq!(
        node(&p, f, "Start_Shop").event_defs,
        [EventDef::Message(Some("Order".into()))]
    );
    assert_eq!(
        node(&p, f, "Catch_Invoice").event_defs,
        [EventDef::Message(Some("Invoice".into()))]
    );
    assert_eq!(
        node(&p, f, "Throw_Shipped").event_defs,
        [EventDef::Signal(Some("Shipped".into()))]
    );
    assert_eq!(
        node(&p, f, "Err_OutOfStock").event_defs,
        [EventDef::Error(Some("STOCK".into()))]
    );
    assert_eq!(
        p.participant_name(&QId::new(f, "Process_Shop")),
        Some("Shop")
    );
}

#[test]
fn qname_references_in_message_flows_and_boundary_events_resolve() {
    let xml = format!(
        r#"<definitions xmlns="{BPMN_NS}" xmlns:tns="urn:t" id="D" targetNamespace="urn:t">
  <collaboration id="C">
    <participant id="P_A" processRef="tns:A"/>
    <participant id="P_B"/>
    <messageFlow id="M" sourceRef="tns:T" targetRef="tns:P_B"/>
  </collaboration>
  <process id="A">
    <startEvent id="S"/><task id="T"/><endEvent id="E"/><endEvent id="E2"/>
    <boundaryEvent id="B" attachedToRef="tns:T"><timerEventDefinition/></boundaryEvent>
    <sequenceFlow id="F1" sourceRef="S" targetRef="T"/>
    <sequenceFlow id="F2" sourceRef="T" targetRef="E"/>
    <sequenceFlow id="F3" sourceRef="B" targetRef="E2"/>
  </process>
</definitions>"#
    );
    let p = source(&xml).unwrap();
    let mf = &p.files["inline.bpmn"].message_flows[&QId::new("inline.bpmn", "M")];
    assert_eq!(mf.source.id, "T");
    assert_eq!(mf.target.id, "P_B");
    assert_eq!(
        node(&p, "inline.bpmn", "B").attached_to,
        Some(QId::new("inline.bpmn", "T"))
    );
}

#[test]
fn root_event_definitions_are_found_through_event_definition_ref() {
    let f = "root-event-definitions.bpmn";
    let p = load(f);
    assert_eq!(node(&p, f, "Wait").event_defs, [EventDef::Timer]);
    assert_eq!(node(&p, f, "End").event_defs, [EventDef::Terminate]);
}

#[test]
fn dangling_event_definition_ref_names_the_event() {
    let err = source(&process(
        r#"<startEvent id="S"/><endEvent id="E"><eventDefinitionRef>Nope</eventDefinitionRef></endEvent>
        <sequenceFlow id="F" sourceRef="S" targetRef="E"/>"#,
    ))
    .unwrap_err()
    .to_string();
    assert!(err.contains("E") && err.contains("Nope"), "{err}");
}

#[test]
fn full_palette_reads_every_supported_element() {
    let f = "full-palette.bpmn";
    let p = load(f);
    let kind = |id| node(&p, f, id).kind.clone();
    for (id, k) in [
        ("Task_User", TaskKind::User),
        ("Task_Manual", TaskKind::Manual),
        ("Task_Service", TaskKind::Service),
        ("Task_Script", TaskKind::Script),
        ("Task_Rule", TaskKind::BusinessRule),
        ("Task_Multi", TaskKind::Task),
    ] {
        assert_eq!(kind(id), NodeKind::Task(k), "{id}");
    }
    assert_eq!(kind("Or_Split"), NodeKind::InclusiveGateway);
    assert_eq!(kind("Fork"), NodeKind::ParallelGateway);
    assert_eq!(kind("Throw_None"), NodeKind::IntermediateThrowEvent);

    // Nested lanes: the innermost lane that lists a node wins.
    assert_eq!(node(&p, f, "Task_User").lane.as_deref(), Some("Office"));
    assert_eq!(node(&p, f, "Task_Multi").lane.as_deref(), Some("Clerks"));
    assert_eq!(node(&p, f, "End").lane.as_deref(), Some("Head office"));
    assert_eq!(p.files[f].lanes.len(), 3);

    // Subprocesses nest; an event subprocess is marked.
    assert_eq!(
        node(&p, f, "Inner_Task").parent,
        Some(QId::new(f, "Sub_Inner"))
    );
    assert_eq!(
        node(&p, f, "Sub_Inner").parent,
        Some(QId::new(f, "Sub_Handle"))
    );
    assert!(node(&p, f, "Sub_Escalate").triggered_by_event);
    assert!(!node(&p, f, "Esc_Start").cancel_activity);

    // Boundary events.
    let remind = node(&p, f, "Timer_Remind");
    assert!(!remind.cancel_activity);
    assert_eq!(remind.attached_to, Some(QId::new(f, "Sub_Handle")));
    assert!(node(&p, f, "Err_Rejected").cancel_activity);

    // Loops.
    assert_eq!(
        node(&p, f, "Task_Multi").looping,
        Some(Looping::MultiInstance {
            sequential: false,
            cardinality: Some("3".into()),
            input_collection: None,
            completion_condition: None,
        })
    );
    assert_eq!(
        node(&p, f, "Task_Loop").looping,
        Some(Looping::Standard {
            test_before: false,
            maximum: Some(2),
            condition: Some("false".into()),
        })
    );

    // Artifacts are kept for drawing; entities are decoded.
    let file = &p.files[f];
    let note = &file.artifacts[&QId::new(f, "Note_1")];
    assert_eq!(
        note.text.as_deref(),
        Some("Annotations & groups are artifacts")
    );
    assert_eq!(
        file.artifacts[&QId::new(f, "StoreRef_Crm")].kind,
        ArtifactKind::DataStore
    );
    assert!(file.artifacts.contains_key(&QId::new(f, "Group_1")));
    assert!(file.associations.contains_key(&QId::new(f, "DIA_1")));
    assert!(file.associations.contains_key(&QId::new(f, "DOA_1")));
    assert!(!file.associations[&QId::new(f, "Assoc_1")]);

    // Only the first diagram is drawn; labels and edges are read.
    let d = &file.diagram;
    assert!(d.shapes.contains_key(&QId::new(f, "Task_User")));
    assert!(!d.shapes.contains_key(&QId::new(f, "Sub_Start")));
    assert!(d.labels.contains_key(&QId::new(f, "Start")));
    assert_eq!(d.edges[&QId::new(f, "Flow_1")].len(), 2);
}

#[test]
fn byte_order_mark_character_references_and_non_ascii_names() {
    let xml = format!(
        "\u{feff}{}",
        process(
            r#"<startEvent id="S" name="Antrag eingegangen &#x2192; pr&#xFC;fen"/>
            <task id="T" name="Prüfen &amp; freigeben — 日本"/>
            <endEvent id="E"/>
            <sequenceFlow id="F1" sourceRef="S" targetRef="T"/>
            <sequenceFlow id="F2" sourceRef="T" targetRef="E"/>"#
        )
    );
    let p = source(&xml).unwrap();
    assert_eq!(
        node(&p, "inline.bpmn", "S").name.as_deref(),
        Some("Antrag eingegangen → prüfen")
    );
    assert_eq!(
        node(&p, "inline.bpmn", "T").name.as_deref(),
        Some("Prüfen & freigeben — 日本")
    );
    assert!(validate(&p).is_empty());
}

#[test]
fn comments_processing_instructions_and_foreign_elements_are_ignored() {
    let p = source(&process(
        r#"<!-- a comment --><?vendor hint?>
        <startEvent id="S"/>
        <foo:task xmlns:foo="urn:not-bpmn" id="NotATask"/>
        <task id="T"><extensionElements><foo:task xmlns:foo="urn:x" id="Nested"/></extensionElements></task>
        <endEvent id="E"/>
        <sequenceFlow id="F1" sourceRef="S" targetRef="T"/>
        <sequenceFlow id="F2" sourceRef="T" targetRef="E"/>"#,
    ))
    .unwrap();
    let ids: Vec<_> = p.nodes().map(|n| n.id.id.as_str()).collect();
    assert_eq!(ids, ["E", "S", "T"]);
}

#[test]
fn other_root_elements_of_the_standard_do_not_break_parsing() {
    let xml = format!(
        r#"<definitions xmlns="{BPMN_NS}" id="D" targetNamespace="urn:t">
  <import importType="http://www.w3.org/2001/XMLSchema" location="types.xsd" namespace="urn:types"/>
  <itemDefinition id="Item"/>
  <message id="Msg" itemRef="Item"/>
  <interface id="If"><operation id="Op"><inMessageRef>Msg</inMessageRef></operation></interface>
  <resource id="Res" name="Clerk"/>
  <partnerRole id="Role" name="Buyer"/>
  <dataStore id="Store"/>
  <globalTask id="GT"/>
  <correlationProperty id="CP"/>
  <choreography id="Chor"/>
  <conversation id="Conv"/>
  <process id="Process_1">
    <ioSpecification><inputSet/><outputSet/></ioSpecification>
    <property id="Prop"/>
    <dataObject id="DO"/>
    <startEvent id="S"/>
    <task id="T">
      <performer><resourceRef>Res</resourceRef></performer>
    </task>
    <endEvent id="E"/>
    <sequenceFlow id="F1" sourceRef="S" targetRef="T"/>
    <sequenceFlow id="F2" sourceRef="T" targetRef="E"/>
  </process>
</definitions>"#
    );
    let p = source(&xml).unwrap();
    assert_eq!(p.nodes().count(), 3);
    assert!(validate(&p).is_empty());
    assert_eq!(run_skeleton("inline", &p).cases_completed, 100);
}

#[test]
fn several_processes_without_a_collaboration() {
    let xml = format!(
        r#"<definitions xmlns="{BPMN_NS}" id="D" targetNamespace="urn:t">
  <process id="A"><startEvent id="SA"/><endEvent id="EA"/><sequenceFlow id="FA" sourceRef="SA" targetRef="EA"/></process>
  <process id="B"><startEvent id="SB"/><endEvent id="EB"/><sequenceFlow id="FB" sourceRef="SB" targetRef="EB"/></process>
</definitions>"#
    );
    let p = source(&xml).unwrap();
    assert_eq!(p.processes.len(), 2);
    assert_eq!(node(&p, "inline.bpmn", "SB").process.id, "B");
}

#[test]
fn definitions_without_processes_parse_to_an_empty_model() {
    let p = source(&format!(
        r#"<definitions xmlns="{BPMN_NS}" id="D" targetNamespace="urn:t"/>"#
    ))
    .unwrap();
    assert_eq!(p.nodes().count(), 0);
    assert!(validate(&p).is_empty());
}

#[test]
fn elements_the_simulator_does_not_run_are_reported_not_rejected() {
    let p = source(&process(
        r#"<startEvent id="S"/>
        <complexGateway id="Complex"/>
        <transaction id="Tx"/>
        <adHocSubProcess id="AdHoc"/>
        <intermediateCatchEvent id="Cond"><conditionalEventDefinition><condition>x</condition></conditionalEventDefinition></intermediateCatchEvent>
        <intermediateThrowEvent id="Comp"><compensateEventDefinition/></intermediateThrowEvent>
        <endEvent id="E"/>
        <sequenceFlow id="F1" sourceRef="S" targetRef="Complex"/>
        <sequenceFlow id="F2" sourceRef="Complex" targetRef="Tx"/>
        <sequenceFlow id="F3" sourceRef="Tx" targetRef="AdHoc"/>
        <sequenceFlow id="F4" sourceRef="AdHoc" targetRef="Cond"/>
        <sequenceFlow id="F5" sourceRef="Cond" targetRef="Comp"/>
        <sequenceFlow id="F6" sourceRef="Comp" targetRef="E"/>"#,
    ))
    .unwrap();
    let f = "inline.bpmn";
    for id in ["Complex", "Tx", "AdHoc"] {
        assert!(
            matches!(node(&p, f, id).kind, NodeKind::Unsupported(_)),
            "{id}"
        );
    }
    assert_eq!(
        node(&p, f, "Cond").event_defs,
        [EventDef::Unsupported("conditionalEventDefinition".into())]
    );
    let issues: Vec<String> = validate(&p).iter().map(ToString::to_string).collect();
    for id in ["Complex", "Tx", "AdHoc"] {
        assert!(
            issues.iter().any(|i| i.contains(&format!("#{id}"))),
            "{id}: {issues:#?}"
        );
    }
    // Binding names the elements instead of running something else.
    let err = Scenario::skeleton(&p, None)
        .bind(&p)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Complex") && err.contains("Cond"), "{err}");
}

#[test]
fn duplicate_ids_in_nested_scopes_are_rejected() {
    let err = source(&process(
        r#"<task id="T"/><subProcess id="Sub"><task id="T"/></subProcess>"#,
    ))
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("duplicate element id") && err.contains("T"),
        "{err}"
    );
}

#[test]
fn flow_node_without_id_is_an_error_naming_the_element() {
    let err = source(&process("<userTask name='x'/>"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("userTask") && err.contains("id"), "{err}");
}

#[test]
fn only_the_bpmn_20_model_namespace_is_accepted() {
    for ns in [
        "http://www.omg.org/spec/BPMN/20100524/MODEL/",
        "http://www.omg.org/spec/BPMN/20100524/DI",
        "http://schema.omg.org/spec/BPMN/2.0",
    ] {
        let xml = format!(r#"<definitions xmlns="{ns}" id="D"/>"#);
        assert!(
            matches!(source(&xml), Err(ParseError::NotBpmn { .. })),
            "{ns}"
        );
    }
}
