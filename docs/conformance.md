# Conformance to BPMN 2.0.2

Where bpmn-sim stands against each rule of `docs/bpmn-2.0.2-rules.md`. The statuses were set by
reading the code: `parser.rs`, `validate.rs`, `scenario.rs` (binding), `project.rs` and `engine.rs`,
plus the tests and `docs/semantics.md`. Keep this file up to date when behaviour changes.

## Statuses

- **follows**: runs exactly as specified.
- **differs**: runs, but not as specified. **Must fix**: the spec behaviour becomes the default, the
  old one an option.
- **open (decision)**: the spec is ambiguous; it is decided under "Open decisions" in the catalogue,
  then either *follows* or *differs*.
- **not run, flagged**: not executed, and reported (a validation warning or a binding error).
- **not run, NOT flagged**: silently ignored or run wrongly. **Must fix** by reporting it.
- **structural, checked / not checked**: whether validation reports a violation.
- **scenario replaces**: a model value such as a condition, timer or loop condition is replaced by a
  scenario parameter. This is allowed as a simulation extension (§7.2) and documented in
  `docs/semantics.md`.
- **n/a**: non-operational (§13.1), or a feature we do not take up.

## Summary

| Status | Rules |
|---|---|
| follows | 68 |
| differs | 8 |
| open (decision) | 0 |
| not run, flagged | 18 |
| not run, NOT flagged | 11 |
| structural, checked | 10 |
| structural, not checked | 23 |
| scenario replaces | 8 |
| n/a | 19 |
| **total** | **165** |

## Must fix

| ID | Status | Milestone | Note |
|---|---|---|---|
| R-7.6.1-01 | structural, not checked | M20 | Not checked by validation. |
| R-7.6.1-02 | structural, not checked | M20 | The parser accepts any flow whose ends are nodes in the same file. |
| R-7.7-01 | not run, NOT flagged | M19 | All extensions are ignored silently, including mustUnderstand ones. |
| R-8.2.2-01 | not run, NOT flagged | M19 | `import` is not read. References resolve by id across the loaded files. |
| R-8.4.2-01 | not run, NOT flagged | M19 | Correlation elements are ignored. Messages are routed by element within the case (message groups). |
| R-8.4.2-02 | not run, NOT flagged | M19 | As R-8.4.2-01. |
| R-8.4.3-02 | structural, not checked | M20 |  |
| R-8.4.4-02 | structural, not checked | M20 |  |
| R-8.4.9-02 | structural, not checked | M20 | Not parsed. |
| R-8.4.13-03 | structural, not checked | M20 |  |
| R-8.4.13-04 | structural, not checked | M20 | The engine ignores such conditions without a warning. |
| R-8.4.13-05 | structural, not checked | M20 |  |
| R-9.4-01 | structural, not checked | M20 | Flows within one pool are not detected. |
| R-9.3.1-01 | not run, NOT flagged | M19 | Not parsed. |
| R-10.3-01 | not run, NOT flagged | M19 | Not parsed. Such an activity runs as an ordinary one or is started implicitly. |
| R-10.3-02 | not run, NOT flagged | M19 | Not parsed. A value >1 is ignored. |
| R-10.3-03 | not run, NOT flagged | M19 | Not parsed. |
| R-10.3-04 | differs | M18 | D-1: a top-level process without start events must start its free nodes (today it does not start at all). In a scope with start events, free nodes are a validation error (today a warning). |
| R-10.3.8-04 | not run, NOT flagged | M19 | Not parsed. |
| R-10.5.1-01 | differs | M18 | The default is the case. `signal_scope = "global"` gives the spec behaviour. |
| R-10.5.1-03 | differs | M18 | D-4: today only the thrower's token stays stuck. An uncaught error must halt the whole instance as an incident ("failed: uncaught error"). Uncaught escalations must be listed in the report. |
| R-10.5.2-01 | structural, not checked | M20 |  |
| R-10.5.2-04 | differs | M18 | D-2: all of them fire today. Exactly one must fire, the one the scenario names; without a choice, binding fails. |
| R-10.5.2-07 | structural, not checked | M20 | Only "no outgoing flow" is warned about. |
| R-10.5.3-02 | structural, not checked | M20 |  |
| R-10.5.3-07 | structural, not checked | M20 |  |
| R-10.5.4-03 | structural, not checked | M20 | Partly checked: "no outgoing" and "never reached" are warned about, a missing incoming flow is not. |
| R-10.5.4-04 | structural, not checked | M20 | Partly checked: only a missing target is an error. |
| R-10.5.4-05 | structural, not checked | M20 | The engine always interrupts on errors, but `cancelActivity="false"` on an error boundary event is not reported. |
| R-10.5.4-08 | structural, not checked | M20 |  |
| R-10.5.5-01 | structural, not checked | M20 | The timer content is not parsed. |
| R-10.5.5-03 | differs | M18 | It fires once. |
| R-10.5.5-04 | structural, not checked | M20 |  |
| R-10.5.6-03 | structural, not checked | M20 |  |
| R-10.6.1-01 | structural, not checked | M20 | 1:1 gateways are common in real models, so this should be a warning. |
| R-10.6.6-01 | structural, not checked | M20 |  |
| R-10.6.6-03 | structural, not checked | M20 |  |
| R-10.6.6-06 | not run, NOT flagged | M19 | Not parsed. |
| R-13.2-01 | differs | M18 | D-1: report as a warning for a top-level process without start events, and as an error for free nodes next to start events. |
| R-13.3.4-01 | differs | M18 | D-2: several none starts all fire today. In an embedded sub-process this must be a validation error. |
| R-13.3.4-03 | not run, NOT flagged | M19 | Flows crossing the boundary are neither checked nor defined. |
| R-13.4.3-01 | differs | M18 | The engine waits while any token can still reach the join (Camunda behaviour). M18: make the spec rule the default. |

## All rules

| ID | Status | Where | Tests | Milestone | Note |
|---|---|---|---|---|---|
| R-2.1-01 | follows | docs/semantics.md (M10) |  |  | bpmn-sim claims "based on". |
| R-2.2.7-01 | n/a | this matrix |  |  | The goal this matrix tracks. |
| R-2.2.3-01 | follows | sim-gui diagram_panel.rs, diagram_icons.rs |  |  | Tokens and heat maps are colour extensions. |
| R-2.3-01 | n/a |  |  |  | Not claimed; see R-2.2.7-01. |
| R-2.3.2-01 | follows | parser.rs | m3_collab, m17_conformance |  |  |
| R-7.2-01 | n/a | scenario.rs |  |  | So durations, probabilities, resources, calendars and costs are a legitimate extension. |
| R-7.2.1-01 | follows | engine.rs | m1_engine::stuck_instances_are_reported_with_location |  |  |
| R-7.2.1-02 | follows | engine.rs (messages) | m3_collab |  |  |
| R-7.3-01 | follows | engine.rs | m1_engine, m9_semantics |  |  |
| R-7.3-02 | follows | engine.rs | m4b_multifile::xor_loop_with_link_events_repeats_the_task |  |  |
| R-7.6.1-01 | structural, not checked |  |  | M20 | Not checked by validation. |
| R-7.6.1-02 | structural, not checked |  |  | M20 | The parser accepts any flow whose ends are nodes in the same file. |
| R-7.6.1-03 | structural, checked | parser.rs check_references |  |  | Parse error: "is not a flow element". |
| R-7.6.2-01 | structural, checked | scenario.rs (binding) | m3_collab |  | Binding error that names what to model instead. |
| R-7.7-01 | not run, NOT flagged | parser.rs |  | M19 | All extensions are ignored silently, including mustUnderstand ones. |
| R-8.2.1-01 | scenario replaces | scenario.rs, feel.rs | m17_conformance::xpath_conditions_get_shares_in_the_skeleton |  | Only FEEL is evaluated. Other languages are replaced by shares. |
| R-8.2.2-01 | not run, NOT flagged | parser.rs |  | M19 | `import` is not read. References resolve by id across the loaded files. |
| R-8.3.1-01 | structural, checked | parser.rs | m17_conformance::duplicate_ids_in_nested_scopes_are_rejected |  |  |
| R-8.3.5-01 | follows | parser.rs | m17_conformance::root_event_definitions_are_found_through_event_definition_ref |  |  |
| R-8.4.1-01 | follows | parser.rs |  |  | Drawn, never simulated. |
| R-8.4.2-01 | not run, NOT flagged |  |  | M19 | Correlation elements are ignored. Messages are routed by element within the case (message groups). |
| R-8.4.2-02 | not run, NOT flagged |  |  | M19 | As R-8.4.2-01. |
| R-8.4.3-01 | follows | engine.rs | m9_semantics::an_exact_error_code_wins_over_a_catch_all_boundary_event |  |  |
| R-8.4.3-02 | structural, not checked |  |  | M20 |  |
| R-8.4.4-01 | follows | engine.rs | m9_semantics |  |  |
| R-8.4.4-02 | structural, not checked |  |  | M20 |  |
| R-8.4.6-01 | scenario replaces | scenario.rs |  |  | Conditions that can't be evaluated need a [gateways] entry. |
| R-8.4.9-01 | follows | engine.rs |  |  |  |
| R-8.4.9-02 | structural, not checked |  |  | M20 | Not parsed. |
| R-8.4.13-01 | structural, checked | parser.rs, scenario.rs event_sub_problems | m10_spec::malformed_event_subprocesses_are_binding_errors |  |  |
| R-8.4.13-02 | follows | engine.rs, scenario.rs | m9_semantics::conditional_flows_of_a_task_follow_their_conditions_and_the_default |  | With a [gateways] entry, probabilities replace the conditions (S). |
| R-8.4.13-03 | structural, not checked |  |  | M20 |  |
| R-8.4.13-04 | structural, not checked |  |  | M20 | The engine ignores such conditions without a warning. |
| R-8.4.13-05 | structural, not checked |  |  | M20 |  |
| R-8.4.13-06 | follows | scenario.rs | m6_gateways::inclusive_split_falls_back_to_the_default_flow |  |  |
| R-8.4.13-07 | follows | parser.rs |  |  | Model order. |
| R-8.4.13-08 | n/a |  |  |  | Non-operational (§13.1). |
| R-8.4.12-01 | n/a | scenario.rs |  |  | Resource pools are a scenario extension. |
| R-8.5-01 | n/a |  |  |  | Service tasks are simulated with durations. |
| R-9.3-01 | follows | parser.rs, engine.rs | m3_collab::black_box_messages_are_logged_and_can_be_answered |  | How a black box answers is our extension (`black_box_reply`). |
| R-9.4-01 | structural, not checked |  |  | M20 | Flows within one pool are not detected. |
| R-9.4-02 | follows | parser.rs |  |  |  |
| R-9.3.1-01 | not run, NOT flagged |  |  | M19 | Not parsed. |
| R-9.1-01 | n/a |  |  |  | Non-operational. External triggers ([events]) correspond to it. |
| R-9.5-01 | follows | parser.rs |  |  | Ignored. |
| R-10.2-01 | scenario replaces | scenario.rs |  |  | Probabilities or manual choice. |
| R-10.3-01 | not run, NOT flagged |  |  | M19 | Not parsed. Such an activity runs as an ordinary one or is started implicitly. |
| R-10.3-02 | not run, NOT flagged |  |  | M19 | Not parsed. A value >1 is ignored. |
| R-10.3-03 | not run, NOT flagged |  |  | M19 | Not parsed. |
| R-10.3-04 | differs | project.rs start_nodes, validate.rs | m10_spec | M18 | D-1: a top-level process without start events must start its free nodes (today it does not start at all). In a scope with start events, free nodes are a validation error (today a warning). |
| R-10.3-05 | follows | engine.rs |  |  |  |
| R-10.3.3-01 | follows | engine.rs | m3_collab |  |  |
| R-10.3.3-02 | follows | engine.rs | m3_collab::v7_receiver_continues_only_after_the_message |  |  |
| R-10.3.3-03 | not run, flagged | validate.rs | m10_spec::instantiating_receive_tasks_and_event_gateways_are_reported | on demand | Warning. |
| R-10.3.3-04 | n/a |  |  |  | Every task takes a scenario duration and resource. |
| R-10.3.5-01 | follows | project.rs start_nodes | m10_spec::a_subprocess_without_start_event_starts_every_activity_without_incoming_flow |  |  |
| R-10.3.5-02 | structural, checked | scenario.rs event_sub_problems | m10_spec::malformed_event_subprocesses_are_binding_errors |  | Binding error. |
| R-10.3.5-03 | follows | engine.rs | m10_spec::a_non_interrupting_event_subprocess_runs_next_to_its_scope |  | A timer fires once. For timeCycle see R-10.5.5-03. |
| R-10.3.5-04 | not run, flagged | parser.rs (Unsupported) | m17_conformance::elements_the_simulator_does_not_run_are_reported_not_rejected | on demand |  |
| R-10.3.5-05 | not run, flagged | parser.rs |  | on demand | Every cancel definition is unsupported. |
| R-10.3.5-06 | not run, flagged | parser.rs (Unsupported) |  | on demand |  |
| R-10.3.6-01 | follows | engine.rs | m4b_multifile::v10_call_activity_runs_the_child_in_the_same_case_and_sums_costs |  |  |
| R-10.3.6-02 | follows | engine.rs | m9_semantics::an_error_in_a_called_process_is_caught_at_the_call_activity |  |  |
| R-10.3.6-03 | structural, checked | validate.rs |  |  | Error. |
| R-10.3.6-04 | not run, flagged | validate.rs | m4b_multifile::unresolved_call_activities_are_reported_not_fatal | on demand | Not found, so a warning. With a duration it runs as a task. |
| R-10.3.6-05 | n/a |  |  |  | Data is not run. |
| R-10.3.8-01 | scenario replaces | scenario.rs, engine.rs | m9_semantics::a_loop_repeats_with_its_probability_up_to_the_maximum |  | `repeat_probability` replaces `loopCondition`. `testBefore` and the maximum are honoured. |
| R-10.3.8-02 | follows | scenario.rs, engine.rs | m9_semantics::the_count_comes_from_the_model_or_from_a_distribution |  | The scenario count is an extension (S). |
| R-10.3.8-03 | not run, flagged | scenario.rs |  | on demand | Warning only: all instances still run. M19 makes it unmistakable. |
| R-10.3.8-04 | not run, NOT flagged |  |  | M19 | Not parsed. |
| R-10.3.8-05 | n/a |  |  |  | Data is not run. A decision list can give the count (S). |
| R-10.4-01 | n/a |  |  |  | Data is not run. If it is ever needed: the data layer, switchable. |
| R-10.4-02 | n/a |  |  |  | Activities never wait for data. |
| R-10.4-03 | n/a |  |  |  |  |
| R-10.4-04 | n/a |  |  |  |  |
| R-10.4-05 | n/a |  |  |  |  |
| R-10.4-06 | n/a |  |  |  |  |
| R-10.5.1-01 | differs | engine.rs | m3_collab::signals_stay_within_the_case_unless_global | M18 | The default is the case. `signal_scope = "global"` gives the spec behaviour. |
| R-10.5.1-02 | follows | engine.rs | m9_semantics::an_error_travels_up_through_nested_subprocesses |  |  |
| R-10.5.1-03 | differs | engine.rs | m9_semantics::an_uncaught_error_leaves_the_token_stuck_at_the_end_event | M18 | D-4: today only the thrower's token stays stuck. An uncaught error must halt the whole instance as an incident ("failed: uncaught error"). Uncaught escalations must be listed in the report. |
| R-10.5.1-04 | follows | engine.rs terminate | m4a_events::terminate_end_event_cancels_the_rest_of_the_instance |  |  |
| R-10.5.1-05 | not run, flagged | scenario.rs unsupported | m1_engine::unsupported_elements_fail_binding | on demand | Binding error. A top-level start with only none and timer definitions is accepted, because arrivals start it. |
| R-10.5.2-01 | structural, not checked |  |  | M20 |  |
| R-10.5.2-02 | follows | project.rs start_nodes | m10_spec |  | Except `isForCompensation` (R-10.3-01). |
| R-10.5.2-03 | follows | engine.rs |  |  |  |
| R-10.5.2-04 | differs | project.rs start_nodes |  | M18 | D-2: all of them fire today. Exactly one must fire, the one the scenario names; without a choice, binding fails. |
| R-10.5.2-05 | structural, checked | scenario.rs unsupported, event_sub_problems | m10_spec::malformed_event_subprocesses_are_binding_errors |  | Conditional, Multiple and Compensation are flagged as unsupported. |
| R-10.5.2-06 | follows | parser.rs |  |  |  |
| R-10.5.2-07 | structural, not checked |  |  | M20 | Only "no outgoing flow" is warned about. |
| R-10.5.2-08 | not run, flagged | parser.rs |  | on demand |  |
| R-10.5.2-09 | follows | engine.rs | m3_collab::message_start_event_creates_an_instance_in_the_same_case, m4a_events::signal_start_event_creates_an_instance_in_the_senders_case |  |  |
| R-10.5.2-10 | scenario replaces | scenario.rs arrivals |  |  | Arrivals and calendars replace timer values. |
| R-10.5.3-01 | follows | engine.rs |  |  |  |
| R-10.5.3-02 | structural, not checked |  |  | M20 |  |
| R-10.5.3-03 | follows | engine.rs | m3_collab |  |  |
| R-10.5.3-04 | follows | engine.rs | m9_semantics::a_thrown_error_is_caught_by_the_boundary_event_of_its_subprocess |  |  |
| R-10.5.3-05 | follows | engine.rs | m9_semantics::a_non_interrupting_escalation_starts_its_boundary_path_and_the_thrower_goes_on |  |  |
| R-10.5.3-06 | not run, flagged | parser.rs |  | on demand |  |
| R-10.5.3-07 | structural, not checked |  |  | M20 |  |
| R-10.5.4-01 | follows | engine.rs | m4a_events::timer_catch_event_waits_for_its_configured_delay |  |  |
| R-10.5.4-02 | structural, checked | scenario.rs unsupported |  |  |  |
| R-10.5.4-03 | structural, not checked | validate.rs |  | M20 | Partly checked: "no outgoing" and "never reached" are warned about, a missing incoming flow is not. |
| R-10.5.4-04 | structural, not checked | validate.rs | m4a_events::v11_link_events_behave_like_a_sequence_flow | M20 | Partly checked: only a missing target is an error. |
| R-10.5.4-05 | structural, not checked | engine.rs |  | M20 | The engine always interrupts on errors, but `cancelActivity="false"` on an error boundary event is not reported. |
| R-10.5.4-06 | follows | parser.rs |  |  | See D-5 for escalation. |
| R-10.5.4-07 | follows | engine.rs |  |  |  |
| R-10.5.4-08 | structural, not checked |  |  | M20 |  |
| R-10.5.5-01 | structural, not checked |  |  | M20 | The timer content is not parsed. |
| R-10.5.5-02 | scenario replaces | scenario.rs [events]/[boundary] |  |  | The scenario delay replaces the timer value. |
| R-10.5.5-03 | differs | engine.rs |  | M18 | It fires once. |
| R-10.5.5-04 | structural, not checked |  |  | M20 |  |
| R-10.5.5-05 | not run, flagged | parser.rs |  | on demand |  |
| R-10.5.5-06 | not run, flagged | parser.rs |  | on demand |  |
| R-10.5.6-01 | follows | engine.rs | m4a_events::interrupting_boundary_event_cancels_a_whole_subprocess |  |  |
| R-10.5.6-02 | follows | engine.rs | m4a_events::non_interrupting_boundary_adds_a_second_token_and_keeps_the_task |  |  |
| R-10.5.6-03 | structural, not checked |  |  | M20 |  |
| R-10.5.6-04 | follows | engine.rs | m10_spec |  |  |
| R-10.5.6-05 | follows | engine.rs | m10_spec::an_error_thrown_by_an_event_subprocess_goes_to_the_enclosing_activity |  |  |
| R-10.6.1-01 | structural, not checked |  |  | M20 | 1:1 gateways are common in real models, so this should be a warning. |
| R-10.6.1-02 | follows | project.rs start_nodes | m10_spec |  |  |
| R-10.6.2-01 | follows | engine.rs, scenario.rs | m9_semantics::a_task_without_a_true_condition_and_default_flow_gets_stuck | M18 (report) | D-3: the incident (token stays) is today's behaviour. M18 reports it as "failed: runtime exception", separately from deadlocks. |
| R-10.6.2-02 | follows | engine.rs |  |  |  |
| R-10.6.3-01 | follows | engine.rs | m6_gateways::inclusive_split_falls_back_to_the_default_flow | M18 (report) | D-3: as R-10.6.2-01. |
| R-10.6.4-01 | follows | engine.rs | m1_engine::v5_and_split_join_takes_the_longer_branch |  |  |
| R-10.6.5-01 | not run, flagged | parser.rs (Unsupported) |  | on demand |  |
| R-10.6.6-01 | structural, not checked |  |  | M20 |  |
| R-10.6.6-02 | structural, checked | scenario.rs | m6_gateways::event_based_gateway_targets_are_checked |  |  |
| R-10.6.6-03 | structural, not checked |  |  | M20 |  |
| R-10.6.6-04 | follows | engine.rs | m6_gateways::event_based_gateway_takes_the_first_event_and_cancels_the_others |  |  |
| R-10.6.6-05 | not run, flagged | validate.rs | m10_spec::instantiating_receive_tasks_and_event_gateways_are_reported | on demand | Warning. |
| R-10.6.6-06 | not run, NOT flagged |  |  | M19 | Not parsed. |
| R-10.7-01 | not run, flagged | parser.rs |  | on demand | Every compensation definition is flagged. `isForCompensation` is not (R-10.3-01). |
| R-10.8-01 | follows | scenario.rs |  |  | Using a lane name as a resource pool is our extension. |
| R-10.9-01 | n/a |  |  |  | Non-operational. |
| R-13.1-01 | n/a |  |  |  | Manual and abstract tasks get scenario durations. |
| R-13.2-01 | differs | validate.rs |  | M18 | D-1: report as a warning for a top-level process without start events, and as an error for free nodes next to start events. |
| R-13.2-02 | not run, flagged | validate.rs | m10_spec::instantiating_receive_tasks_and_event_gateways_are_reported | on demand |  |
| R-13.2-03 | follows | engine.rs |  |  |  |
| R-13.3.1-01 | follows | engine.rs | m9_semantics::conditional_flows_of_a_task_follow_their_conditions_and_the_default |  |  |
| R-13.3.1-02 | follows | engine.rs |  |  | It takes zero time. |
| R-13.3.2-01 | follows | engine.rs | m6_gateways |  |  |
| R-13.3.2-02 | follows | engine.rs | m10_spec::a_scope_where_only_event_subprocesses_run_starts_no_new_ones |  |  |
| R-13.3.2-03 | follows | engine.rs | m10_spec::an_interrupting_event_subprocess_cancels_the_rest_of_its_scope |  |  |
| R-13.3.2-04 | n/a |  |  |  | Data is not run. |
| R-13.3.3-01 | scenario replaces | scenario.rs [boundary] | m4a_events::error_boundary_fires_with_its_probability |  | A probability on the error boundary stands for the fault. |
| R-13.3.4-01 | differs | project.rs start_nodes | m4a_events::embedded_subprocess_runs_inline | M18 | D-2: several none starts all fire today. In an embedded sub-process this must be a validation error. |
| R-13.3.4-02 | structural, checked | scenario.rs unsupported |  |  |  |
| R-13.3.4-03 | not run, NOT flagged |  |  | M19 | Flows crossing the boundary are neither checked nor defined. |
| R-13.3.4-04 | follows | engine.rs | m4a_events |  | D-10: the sub-process ends and the parent continues, as decided. |
| R-13.3.4-05 | follows | project.rs start_nodes |  |  |  |
| R-13.3.6-01 | scenario replaces | engine.rs | m9_semantics::a_loop_repeats_with_its_probability_up_to_the_maximum |  | See R-10.3.8-01. |
| R-13.3.7-01 | follows | engine.rs | m9_semantics::parallel_instances_run_at_once_and_compete_for_resources |  |  |
| R-13.4.1-01 | follows | engine.rs | m1_engine::v6_and_join_never_mixes_instances |  |  |
| R-13.4.2-01 | follows | engine.rs | m1_engine::v4_xor_frequencies | M18 (report) | D-3: as R-10.6.2-01. |
| R-13.4.3-01 | differs | engine.rs fire_or_joins | m6_gateways::inclusive_join_waits_only_while_another_token_can_still_arrive | M18 | The engine waits while any token can still reach the join (Camunda behaviour). M18: make the spec rule the default. |
| R-13.4.4-01 | follows | engine.rs | m6_gateways |  | The parallel variant is not parsed (R-10.6.6-06). |
| R-13.4.5-01 | not run, flagged | parser.rs (Unsupported) |  | on demand |  |
| R-13.5.2-01 | follows | engine.rs | m3_collab::v7_early_message_is_buffered_by_default |  | Messages that arrive earlier are buffered by default. The spec does not define this, so it is our extension (`buffer_unconsumed`). |
| R-13.5.3-01 | follows | engine.rs | m9_semantics::a_boundary_timer_cancels_every_instance |  |  |
| R-13.5.4-01 | follows | engine.rs | m10_spec |  |  |
| R-13.5.4-02 | follows | engine.rs | m10_spec::an_event_subprocess_only_listens_while_its_scope_runs |  |  |
| R-13.5.5-01 | not run, flagged | parser.rs |  | on demand |  |
| R-13.5.6-01 | follows | engine.rs terminate | m4a_events::terminate_end_event_cancels_the_rest_of_the_instance |  |  |
| R-13.5.6-02 | follows | engine.rs |  |  |  |
| R-13.5.6-03 | not run, flagged | parser.rs |  | on demand |  |

## Roadmap

Conformance first. These milestones close every *differs*, *open*, *NOT flagged* and *not checked*
row:

1. **M18: fix the differences.** The spec rule becomes the default and today's behaviour stays a
   scenario option. Covers every *differs* and *open* row, once its decision is made:
   - the exact inclusive join (R-13.4.3-01);
   - global signals (R-10.5.1-01);
   - repeating timer cycles (R-10.5.5-03);
   - D-1 (decided): top-level processes without start events start their free nodes; free nodes next to start events are an error;
   - D-2 (decided): several none starts in an embedded sub-process are an error; for a called process the scenario names the one start event the call uses;
   - D-3 (decided): a runtime exception is an incident; the case is reported as "failed: runtime exception" with the rule ID, separately from deadlocks;
   - D-4 (decided): an uncaught error halts the whole instance as an incident; uncaught escalations are listed in the report;
   - D-10 (decided): no change; terminate inside a sub-process lets the parent continue.
2. **M19: report everything that is not run.** Every *NOT flagged* row gets a validation message:
   - an error where running it would be wrong, e.g. `startQuantity` > 1, `isForCompensation`,
     `eventGatewayType="Parallel"`, a sub-process entered through inner start events;
   - a warning where the scenario legitimately stands in, e.g. correlation, imports, mustUnderstand
     extensions, ParticipantMultiplicity, multi-instance `behavior`.
3. **M20: structural validation that cites the spec.** Every *not checked* row becomes a validation
   finding carrying its rule ID and page, e.g. "R-10.6.6-01: an event-based gateway needs at least two
   outgoing flows (BPMN 2.0.2 p.297)". It is an error when the process is executable, otherwise a
   warning. An optional set of style checks (7PMG, Mendling et al.) is kept apart from spec findings.

Analysis features:

4. **M21: exhaustive soundness check.** Explore the whole state space of the token game
   (`manual.rs`) instead of sampling runs. It proves:
   - every case can complete;
   - nothing is left behind when it does;
   - no element is dead;

   across pools, including message deadlocks. Data and timers are abstracted to "may happen".
   Unbounded models get "undecided" once a bound is reached. A counterexample can be replayed in the
   token game.
5. **M22: scenario comparison.** Two scenarios with paired replications (common random numbers); a
   confidence interval for each difference and a significance verdict. GUI, CLI and export.
6. **M23: model metrics.** Size and element counts, control-flow complexity, density, structuredness,
   and paths/variants from the M21 state space. GUI, CLI and export.

Added only when a model needs them, and then strictly by the spec (rows *not run, flagged*):
- ISO-8601 timers;
- conditional and Multiple events;
- complex gateway;
- compensation and transactions;
- ad-hoc sub-processes;
- the data layer (switchable per element, D-12);
- correlation keys.
