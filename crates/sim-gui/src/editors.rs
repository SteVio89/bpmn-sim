//! Editors for what one element takes from the scenario, shared by the configuration popover
//! in the diagram and the scenario tab.

use eframe::egui::{self, ComboBox, DragValue, Id, RichText, Sense, Ui};
use sim_core::needs::{
    DecisionOutput, Need, default_pool, is_arrival_start, key_of, match_flows, needs, outputs_for,
    suggest_output,
};
use sim_core::scenario::{
    ArrivalSpec, BoundarySpec, ChanceSpec, ChoiceSpec, CountSpec, EventSpec, GatewaySpec,
    InputSpec, LoopSpec, MultiInstanceSpec, NumberSpec, OutputValue, OutputValues, ResourceSpec,
};
use sim_core::{EventDef, FlowNode, NodeKind, Project, QId, Scenario};

use crate::configure::{find_node, node_kind_name};
use crate::diagram_panel::case_color;
use crate::widgets::{default_dist, dist_edit, dist_preview, rate_text};

/// How an outgoing flow reads: its name, else "to <target>".
pub fn flow_label(project: &Project, f: &QId) -> String {
    match project.flow(f) {
        Some(fl) => fl
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "to {}",
                    project
                        .node(&fl.target)
                        .map_or(fl.target.id.as_str(), |n| n.label())
                )
            }),
        None => f.id.clone(),
    }
}

/// Every parameter element `n` takes from the scenario. `preview` adds the distribution
/// previews (the popover has room for them, the table does not).
pub fn node_editor(ui: &mut Ui, project: &Project, s: &mut Scenario, n: &FlowNode, preview: bool) {
    let all = needs(project, n);
    let starts_cases = is_arrival_start(n);
    if all.is_empty() && !starts_cases {
        ui.label(RichText::new("Nothing to set for this element.").weak());
        return;
    }
    for need in all {
        need_editor(ui, project, s, n, need, preview);
    }
    if starts_cases {
        arrival_editor(ui, project, s, n, preview);
    }
}

/// The editor of one parameter of element `n`; nothing if the scenario has no entry for it.
pub fn need_editor(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    n: &FlowNode,
    need: Need,
    preview: bool,
) {
    let pools: Vec<String> = s.resources.keys().cloned().collect();
    match need {
        Need::Duration => {
            let Some(k) = key_of(&s.activities, project, &n.id) else {
                return;
            };
            let a = s.activities.get_mut(&k).expect("key exists");
            egui::Grid::new(("activity", &n.id))
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Duration");
                    dist_edit(ui, Id::unique(("dur", &n.id)), &mut a.duration);
                    ui.end_row();
                    if preview {
                        ui.label("");
                        dist_preview(ui, &a.duration);
                        ui.end_row();
                    }
                    ui.label("Resource");
                    resource_combo(ui, project, n, &mut a.resource, &pools);
                    ui.end_row();
                    ui.label("Fixed cost");
                    ui.add(
                        DragValue::new(&mut a.fixed_cost)
                            .speed(0.5)
                            .range(0.0..=f64::MAX),
                    );
                    ui.end_row();
                });
        }
        Need::Branching { inclusive } => {
            let Some(k) = key_of(&s.gateways, project, &n.id) else {
                return;
            };
            let g = s.gateways.get_mut(&k).expect("key exists");
            match decide_by(ui, project, n, g) {
                DecideBy::Output => output_editor(ui, project, n, g),
                DecideBy::History => history_editor(ui, project, n, g),
                DecideBy::Shares if inclusive => inclusive_editor(ui, project, n, g),
                DecideBy::Shares => xor_editor(ui, project, n, g),
            }
        }
        Need::Delay => {
            let Some(k) = key_of(&s.events, project, &n.id) else {
                return;
            };
            let e = s.events.get_mut(&k).expect("key exists");
            ui.horizontal(|ui| {
                ui.label("Fires after");
                dist_edit(ui, Id::unique(("event", &n.id)), &mut e.delay);
            });
            if preview {
                dist_preview(ui, &e.delay);
            }
        }
        Need::Boundary { probability } => {
            let Some(k) = key_of(&s.boundary, project, &n.id) else {
                return;
            };
            let b = s.boundary.get_mut(&k).expect("key exists");
            boundary_editor(ui, &n.id, b, probability, preview);
        }
        Need::InstanceCount => {
            let Some(k) = key_of(&s.multi_instance, project, &n.id) else {
                return;
            };
            let m = s.multi_instance.get_mut(&k).expect("key exists");
            count_editor(ui, &n.id, m);
        }
        Need::Repeat => {
            let Some(k) = key_of(&s.loops, project, &n.id) else {
                return;
            };
            repeat_editor(ui, s.loops.get_mut(&k).expect("key exists"));
        }
        Need::DecisionInputs { decision, inputs } => {
            ui.label(RichText::new(format!("Inputs of decision {decision}")).strong());
            egui::Grid::new(("inputs", &n.id))
                .num_columns(2)
                .show(ui, |ui| {
                    for i in inputs {
                        ui.label(&i);
                        match s.decision_input_mut(&i) {
                            Some((_, spec)) => input_edit(ui, (decision.as_str(), &i), spec),
                            None => {
                                ui.label(RichText::new("not set").weak());
                            }
                        }
                        ui.end_row();
                    }
                });
        }
    }
}

/// The resource pool a task uses: its lane's (or pool's) unless overridden.
pub fn resource_combo(
    ui: &mut Ui,
    project: &Project,
    n: &FlowNode,
    resource: &mut Option<String>,
    pools: &[String],
) {
    let lane = default_pool(project, n);
    let from_lane = match &lane {
        Some(l) => format!("{l} (lane)"),
        None => "none".into(),
    };
    ComboBox::from_id_salt(("res", &n.id))
        .selected_text(resource.clone().unwrap_or(from_lane.clone()))
        .show_ui(ui, |ui| {
            ui.selectable_value(resource, None, from_lane);
            for p in pools.iter().filter(|p| Some(*p) != lane.as_ref()) {
                ui.selectable_value(resource, Some(p.clone()), p);
            }
        });
}

/// How a split without conditions is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecideBy {
    Shares,
    /// What happened in the case (XOR split with two flows).
    History,
    /// A decision output set by a business rule task.
    Output,
}

impl DecideBy {
    fn text(self) -> &'static str {
        match self {
            DecideBy::Shares => "shares",
            DecideBy::History => "what happened in the case",
            DecideBy::Output => "a decision output",
        }
    }
}

/// Lets the user choose how split `n` is decided, among the ways that work for it, and
/// switches the entry over when the choice changes.
fn decide_by(ui: &mut Ui, project: &Project, n: &FlowNode, g: &mut GatewaySpec) -> DecideBy {
    let inclusive = n.kind == NodeKind::InclusiveGateway;
    let current = if g.by_output() {
        DecideBy::Output
    } else if g.if_completed.is_some() || g.then.is_some() {
        DecideBy::History
    } else {
        DecideBy::Shares
    };
    let outputs = outputs_for(project, n);
    let mut ways = vec![DecideBy::Shares];
    if !inclusive && n.outgoing.len() == 2 {
        ways.push(DecideBy::History);
    }
    if !outputs.is_empty() {
        ways.push(DecideBy::Output);
    }
    if ways.len() == 1 && current == DecideBy::Shares {
        return current;
    }
    let mut chosen = current;
    ui.horizontal(|ui| {
        ui.label("Decide by");
        ComboBox::from_id_salt(("decide-by", &n.id))
            .selected_text(current.text())
            .show_ui(ui, |ui| {
                for w in &ways {
                    ui.selectable_value(&mut chosen, *w, w.text());
                }
            });
    });
    if chosen != current {
        g.probabilities.clear();
        g.if_completed = None;
        g.then = None;
        g.output = None;
        g.values.clear();
        match chosen {
            DecideBy::Shares => {
                let flows: Vec<&QId> = n
                    .outgoing
                    .iter()
                    .filter(|f| !inclusive || n.default_flow.as_ref() != Some(*f))
                    .collect();
                let p = if inclusive {
                    0.5
                } else {
                    1.0 / flows.len() as f64
                };
                g.probabilities = flows.iter().map(|f| (f.id.clone(), p)).collect();
            }
            DecideBy::History => {
                // The default flow usually is the "yes, already done" answer.
                let then = n.default_flow.as_ref().unwrap_or(&n.outgoing[0]);
                g.then = Some(then.id.clone());
            }
            DecideBy::Output => {
                // The output whose values the flow names match, else the closest decision's.
                let (output, values) = suggest_output(project, n)
                    .unwrap_or_else(|| (outputs[0].variable().to_string(), Default::default()));
                g.output = Some(output);
                g.values = values;
            }
        }
    }
    chosen
}

/// A split that follows a decision output: which output, and which of its values take each
/// flow.
fn output_editor(ui: &mut Ui, project: &Project, n: &FlowNode, g: &mut GatewaySpec) {
    let outputs = outputs_for(project, n);
    let current = g
        .output
        .as_deref()
        .and_then(|v| outputs.iter().find(|o| o.names.iter().any(|x| x == v)));
    let task_label = |o: &DecisionOutput| {
        project
            .node(&o.task)
            .map_or(o.task.id.clone(), |t| t.label().to_string())
    };
    ui.horizontal(|ui| {
        ui.label("Output");
        ComboBox::from_id_salt(("output", &n.id))
            .width(240.0)
            .selected_text(g.output.clone().unwrap_or("choose an output".into()))
            .show_ui(ui, |ui| {
                for o in &outputs {
                    let on = current.is_some_and(|c| c == o);
                    if ui
                        .selectable_label(on, format!("{} ({})", o.variable(), task_label(o)))
                        .on_hover_text(format!("decision {}", o.decision))
                        .clicked()
                        && !on
                    {
                        g.output = Some(o.variable().to_string());
                        g.values = match_flows(project, n, o).unwrap_or_default();
                    }
                }
            });
    });
    let possible = current.and_then(|o| o.values.clone());
    egui::Grid::new(("output-values", &n.id))
        .num_columns(2)
        .show(ui, |ui| {
            for (i, f) in n
                .outgoing
                .iter()
                .filter(|f| n.default_flow.as_ref() != Some(*f))
                .enumerate()
            {
                ui.colored_label(case_color(i as u64), flow_label(project, f));
                let key = if g.values.contains_key(&f.to_string()) {
                    f.to_string()
                } else {
                    f.id.clone()
                };
                let mut chosen: Vec<OutputValue> = g
                    .values
                    .get(&key)
                    .map(|v| v.as_slice().to_vec())
                    .unwrap_or_default();
                let before = chosen.clone();
                match &possible {
                    // Toggle the values the decision can give (and keep any others).
                    Some(values) => {
                        ui.horizontal_wrapped(|ui| {
                            for v in values.iter().filter_map(OutputValue::from_value) {
                                let mut on = chosen.contains(&v);
                                if ui.toggle_value(&mut on, value_text(&v)).changed() {
                                    if on {
                                        chosen.push(v);
                                    } else {
                                        chosen.retain(|c| *c != v);
                                    }
                                }
                            }
                            for v in before.iter().filter(|v| !values.contains(&v.to_value())) {
                                ui.colored_label(ui.visuals().warn_fg_color, value_text(v))
                                    .on_hover_text("the decision never gives this value");
                            }
                        });
                    }
                    // Computed values (aggregation): typed in, separated by commas.
                    None => {
                        let id = Id::unique(("output-text", &n.id, &f.id));
                        let mut text = ui.data_mut(|d| {
                            d.get_temp::<String>(id).unwrap_or_else(|| {
                                chosen.iter().map(value_text).collect::<Vec<_>>().join(", ")
                            })
                        });
                        if ui
                            .add(egui::TextEdit::singleline(&mut text).desired_width(160.0))
                            .changed()
                        {
                            chosen = text
                                .split(',')
                                .map(str::trim)
                                .filter(|t| !t.is_empty())
                                .map(parse_value)
                                .collect();
                        }
                        ui.data_mut(|d| d.insert_temp(id, text));
                    }
                }
                if chosen != before {
                    if chosen.is_empty() {
                        g.values.remove(&key);
                    } else {
                        g.values.insert(key, OutputValues::from_vec(chosen));
                    }
                }
                ui.end_row();
            }
        });
    let otherwise = match &n.default_flow {
        Some(d) => format!(
            "A value no flow takes goes along the default flow ({}).",
            flow_label(project, d)
        ),
        None => "A value no flow takes leaves the token stuck at the gateway (there is no default flow).".into(),
    };
    let how = match (
        n.kind == NodeKind::InclusiveGateway,
        current.is_some_and(|o| o.list),
    ) {
        (true, true) => "The decision gives a list: every flow with a value in it is taken. ",
        (true, false) => "Every flow whose value matches is taken. ",
        (false, true) => "The decision gives a list: the first flow with a value in it is taken. ",
        (false, false) => "",
    };
    ui.label(RichText::new(format!("{how}{otherwise}")).small().weak());
}

/// A value as written in the scenario: strings in quotes.
pub fn value_text(v: &OutputValue) -> String {
    match v {
        OutputValue::Str(s) => format!("\"{s}\""),
        OutputValue::Num(x) => x.to_string(),
        OutputValue::Bool(b) => b.to_string(),
    }
}

/// A typed value: true/false, a number, else a string (quotes optional).
fn parse_value(t: &str) -> OutputValue {
    match t {
        "true" => OutputValue::Bool(true),
        "false" => OutputValue::Bool(false),
        _ => t.parse().map_or_else(
            |_| OutputValue::Str(t.trim_matches('"').to_string()),
            OutputValue::Num,
        ),
    }
}

/// XOR split asking whether an element has completed in the case: one flow once it has, the
/// other until then.
fn history_editor(ui: &mut Ui, project: &Project, n: &FlowNode, g: &mut GatewaySpec) {
    let asked = g
        .if_completed
        .as_deref()
        .and_then(|k| find_node(project, k));
    let mut candidates: Vec<(String, &FlowNode)> = project
        .nodes()
        .filter(|e| {
            e.id != n.id
                && !matches!(
                    e.kind,
                    NodeKind::ExclusiveGateway
                        | NodeKind::InclusiveGateway
                        | NodeKind::ParallelGateway
                        | NodeKind::EventBasedGateway
                        | NodeKind::Unsupported(_)
                )
        })
        .map(|e| {
            let pool = project
                .participant_name(&e.process)
                .unwrap_or(e.id.file.as_str());
            (format!("{pool}: {}", e.label()), e)
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let then = g
        .then
        .as_deref()
        .and_then(|t| n.outgoing.iter().find(|f| f.id == t || f.to_string() == t))
        .cloned();
    egui::Grid::new(("history", &n.id))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Once");
            ComboBox::from_id_salt(("asked", &n.id))
                .width(240.0)
                .selected_text(
                    asked.map_or("choose an element".to_string(), |e| e.label().to_string()),
                )
                .show_ui(ui, |ui| {
                    for (text, e) in &candidates {
                        let on = asked.is_some_and(|a| a.id == e.id);
                        if ui
                            .selectable_label(on, text)
                            .on_hover_text(format!("{} {}", node_kind_name(&e.kind), e.id))
                            .clicked()
                        {
                            g.if_completed = Some(e.id.to_string());
                        }
                    }
                });
            ui.end_row();
            ui.label("has completed, take");
            ComboBox::from_id_salt(("then", &n.id))
                .selected_text(
                    then.as_ref()
                        .map_or("choose a flow".to_string(), |f| flow_label(project, f)),
                )
                .show_ui(ui, |ui| {
                    for f in &n.outgoing {
                        if ui
                            .selectable_label(then.as_ref() == Some(f), flow_label(project, f))
                            .clicked()
                        {
                            g.then = Some(f.id.clone());
                        }
                    }
                });
            ui.end_row();
            ui.label("until then");
            match n.outgoing.iter().find(|f| Some(*f) != then.as_ref()) {
                Some(other) if then.is_some() => ui.label(flow_label(project, other)),
                _ => ui.label(""),
            };
            ui.end_row();
        });
    ui.label(
        RichText::new(
            "Only this case counts: what the pools started with it, and the processes they \
             called, have done before the token reaches the gateway.",
        )
        .small()
        .weak(),
    );
}

/// XOR split: one share per flow, always adding up to 100 % (the last flow takes the rest),
/// shown as a stacked bar whose dividers can be dragged.
fn xor_editor(ui: &mut Ui, project: &Project, n: &FlowNode, g: &mut GatewaySpec) {
    let keys = flow_keys(n, g, false);
    let sum: f64 = keys.iter().map(|k| g.probabilities[k]).sum();
    // Stacked bar with draggable dividers.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(280.0, 18.0), Sense::hover());
    let mut x = rect.left();
    let scale = if sum > 0.0 { sum } else { 1.0 };
    let mut edges = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        let w = (g.probabilities[k] / scale) as f32 * rect.width();
        let seg =
            egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w, rect.bottom()));
        ui.painter()
            .rect_filled(seg, 2.0, case_color(i as u64).gamma_multiply(0.8));
        x += w;
        edges.push(x);
    }
    for (i, ex) in edges.iter().take(keys.len().saturating_sub(1)).enumerate() {
        let handle =
            egui::Rect::from_center_size(egui::pos2(*ex, rect.center().y), egui::vec2(8.0, 22.0));
        let r = ui.interact(handle, Id::unique(("divider", &n.id, i)), Sense::drag());
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        ui.painter().rect_filled(
            egui::Rect::from_center_size(handle.center(), egui::vec2(3.0, 20.0)),
            1.0,
            ui.visuals().strong_text_color(),
        );
        if r.dragged() {
            let d = f64::from(r.drag_delta().x / rect.width()) * scale;
            let (a, b) = (&keys[i], &keys[i + 1]);
            let d = d.clamp(-g.probabilities[a], g.probabilities[b]);
            *g.probabilities.get_mut(a).expect("flow") += d;
            *g.probabilities.get_mut(b).expect("flow") -= d;
        }
    }
    egui::Grid::new(("xor", &n.id))
        .num_columns(2)
        .show(ui, |ui| {
            let last = keys.len() - 1;
            for (i, k) in keys.iter().enumerate() {
                let f = QId::new(&n.id.file, k.rsplit('#').next().unwrap_or(k));
                ui.colored_label(case_color(i as u64), flow_label(project, &f));
                let mut pct = g.probabilities[k] * 100.0;
                if i == last {
                    ui.label(format!("{pct:.1} % (the rest)"));
                } else if ui
                    .add(
                        DragValue::new(&mut pct)
                            .range(0.0..=100.0)
                            .speed(0.5)
                            .suffix(" %"),
                    )
                    .changed()
                {
                    // The last flow takes what the others leave.
                    let others: f64 = keys
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i && *j != last)
                        .map(|(_, k)| g.probabilities[k])
                        .sum();
                    let p = (pct / 100.0).clamp(0.0, 1.0 - others);
                    g.probabilities.insert(k.clone(), p);
                    g.probabilities
                        .insert(keys[last].clone(), (1.0 - others - p).max(0.0));
                }
                ui.end_row();
            }
        });
    if (sum - 1.0).abs() > 1e-9 {
        ui.horizontal(|ui| {
            ui.colored_label(
                ui.visuals().error_fg_color,
                format!("shares add up to {:.1} %", sum * 100.0),
            );
            if sum > 0.0 && ui.button("Scale to 100 %").clicked() {
                g.probabilities.values_mut().for_each(|p| *p /= sum);
            }
        });
    }
}

/// Inclusive split: each non-default flow taken on its own with its chance.
fn inclusive_editor(ui: &mut Ui, project: &Project, n: &FlowNode, g: &mut GatewaySpec) {
    let keys = flow_keys(n, g, true);
    egui::Grid::new(("or", &n.id))
        .num_columns(2)
        .show(ui, |ui| {
            for (i, k) in keys.iter().enumerate() {
                let f = QId::new(&n.id.file, k.rsplit('#').next().unwrap_or(k));
                ui.colored_label(case_color(i as u64), flow_label(project, &f));
                let p = g.probabilities.get_mut(k).expect("flow");
                let mut pct = *p * 100.0;
                if ui
                    .add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix(" %"))
                    .changed()
                {
                    *p = pct / 100.0;
                }
                ui.end_row();
            }
        });
    let fallback = match &n.default_flow {
        Some(d) => format!("the default flow ({})", flow_label(project, d)),
        None => "the most likely flow".into(),
    };
    ui.label(
        RichText::new(format!(
            "Each flow is taken on its own with its chance; if none is, {fallback}."
        ))
        .small()
        .weak(),
    );
}

/// The probability keys of `n`'s flows in model order, adding missing flows with 0 (an
/// inclusive split leaves out its default flow).
fn flow_keys(n: &FlowNode, g: &mut GatewaySpec, inclusive: bool) -> Vec<String> {
    n.outgoing
        .iter()
        .filter(|f| !inclusive || n.default_flow.as_ref() != Some(*f))
        .map(|f| {
            let full = f.to_string();
            let key = if g.probabilities.contains_key(&full) {
                full
            } else {
                f.id.clone()
            };
            g.probabilities.entry(key.clone()).or_insert(0.0);
            key
        })
        .collect()
}

fn boundary_editor(ui: &mut Ui, id: &QId, b: &mut BoundarySpec, probability: bool, preview: bool) {
    if probability {
        let p = b.probability.get_or_insert(0.1);
        let mut pct = *p * 100.0;
        ui.horizontal(|ui| {
            ui.label("Happens in");
            if ui
                .add(egui::Slider::new(&mut pct, 0.0..=100.0).suffix(" %"))
                .changed()
            {
                *p = pct / 100.0;
            }
            ui.label("of the runs");
        });
    }
    let d = b.delay.get_or_insert_with(|| default_dist(0, 3600.0));
    ui.horizontal(|ui| {
        ui.label(if probability { "after" } else { "Fires after" });
        dist_edit(ui, Id::unique(("boundary", id)), d);
    });
    if preview {
        dist_preview(ui, d);
    }
}

fn count_editor(ui: &mut Ui, id: &QId, m: &mut MultiInstanceSpec) {
    let kind = match &m.count {
        CountSpec::Fixed(_) => 0,
        CountSpec::Dist(NumberSpec::Uniform { .. }) => 1,
        CountSpec::Dist(_) => 2,
    };
    ui.horizontal(|ui| {
        ui.label("Instances");
        let mut chosen = kind;
        ComboBox::from_id_salt(("count-kind", id))
            .width(90.0)
            .selected_text(["exactly", "between", "distribution"][kind])
            .show_ui(ui, |ui| {
                for (i, k) in ["exactly", "between", "distribution"].iter().enumerate() {
                    ui.selectable_value(&mut chosen, i, *k);
                }
            });
        if chosen != kind {
            m.count = match chosen {
                0 => CountSpec::Fixed(3),
                1 => CountSpec::Dist(NumberSpec::Uniform { min: 1.0, max: 5.0 }),
                _ => CountSpec::Dist(NumberSpec::Normal {
                    mean: 3.0,
                    stdev: 1.0,
                }),
            };
        }
        match &mut m.count {
            CountSpec::Fixed(n) => {
                ui.add(DragValue::new(n).range(0..=10_000));
            }
            CountSpec::Dist(NumberSpec::Uniform { min, max }) => {
                ui.add(DragValue::new(min).range(0.0..=10_000.0));
                ui.label("and");
                ui.add(DragValue::new(max).range(0.0..=10_000.0));
            }
            CountSpec::Dist(n) => number_fields(ui, n),
        }
    });
}

fn repeat_editor(ui: &mut Ui, l: &mut LoopSpec) {
    let mut pct = l.repeat_probability * 100.0;
    ui.horizontal(|ui| {
        ui.label("Runs again in");
        if ui
            .add(egui::Slider::new(&mut pct, 0.0..=99.0).suffix(" %"))
            .changed()
        {
            l.repeat_probability = pct / 100.0;
        }
    });
    let p = l.repeat_probability;
    if p < 1.0 {
        ui.label(
            RichText::new(format!(
                "so it runs {:.1} times on average",
                1.0 / (1.0 - p)
            ))
            .small()
            .weak(),
        );
    }
}

/// The arrival generators that start cases at start event `n`.
pub fn arrival_editor(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    n: &FlowNode,
    preview: bool,
) {
    let mine = s.arrivals_of(project, &n.id);
    if mine.is_empty() {
        ui.label(RichText::new("No cases start here.").weak());
        if ui.button("Start cases here").clicked() {
            s.arrivals.push(ArrivalSpec {
                label: n.name.clone(),
                inter_arrival: default_dist(3, 3600.0),
                triggers: vec![n.id.to_string()],
            });
        }
        return;
    }
    let mut stop = None;
    for i in mine {
        let a = &mut s.arrivals[i];
        ui.horizontal(|ui| {
            ui.label("A new case every");
            dist_edit(ui, Id::unique(("arrival", i)), &mut a.inter_arrival);
        });
        ui.label(RichText::new(rate_text(&a.inter_arrival)).small());
        if preview {
            dist_preview(ui, &a.inter_arrival);
        }
        let others: Vec<&String> = a
            .triggers
            .iter()
            .filter(|t| **t != n.id.to_string() && **t != n.id.id)
            .collect();
        if !others.is_empty() {
            ui.label(
                RichText::new(format!(
                    "also starts {} in the same case",
                    others
                        .iter()
                        .map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .small()
                .weak(),
            );
        }
        if ui.small_button("Stop starting cases here").clicked() {
            stop = Some(i);
        }
    }
    if n.event_defs.iter().any(|d| matches!(d, EventDef::Timer)) {
        start_delay_editor(ui, project, s, n, preview);
    }
    if let Some(i) = stop {
        let full = n.id.to_string();
        s.arrivals[i]
            .triggers
            .retain(|t| *t != full && *t != n.id.id);
        if s.arrivals[i].triggers.is_empty() {
            s.arrivals.remove(i);
        }
    }
}

/// A timer start event fired by an arrival may start later than the case (`[events]` delay),
/// e.g. when each pool of a collaboration begins at its own time after a common date.
fn start_delay_editor(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    n: &FlowNode,
    preview: bool,
) {
    let key = key_of(&s.events, project, &n.id);
    let mut later = key.is_some();
    if ui
        .checkbox(&mut later, "starts later than the case")
        .changed()
    {
        match &key {
            Some(k) if !later => {
                s.events.remove(k);
            }
            None if later => {
                s.events.insert(
                    n.id.to_string(),
                    EventSpec {
                        label: n.name.clone(),
                        delay: default_dist(1, 86_400.0),
                    },
                );
            }
            _ => {}
        }
    }
    let Some(e) = key_of(&s.events, project, &n.id).and_then(|k| s.events.get_mut(&k)) else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label("after");
        dist_edit(ui, Id::unique(("start-delay", &n.id)), &mut e.delay);
    });
    if preview {
        dist_preview(ui, &e.delay);
    }
}

/// A resource pool: number of resources, cost and calendar; lists the tasks using it.
pub fn pool_editor(ui: &mut Ui, project: &Project, s: &mut Scenario, name: &str) {
    let calendars: Vec<String> = s.calendars.keys().cloned().collect();
    let users: Vec<&str> = project
        .nodes()
        .filter(|n| needs(project, n).contains(&Need::Duration))
        .filter(|n| {
            let over = key_of(&s.activities, project, &n.id)
                .and_then(|k| s.activities.get(&k))
                .and_then(|a| a.resource.clone());
            over.or_else(|| default_pool(project, n)).as_deref() == Some(name)
        })
        .map(|n| n.label())
        .collect();
    let Some(r) = s.resources.get_mut(name) else {
        ui.label(RichText::new("Not a resource pool of the scenario.").weak());
        if ui.button("Add as resource pool").clicked() {
            s.resources.insert(
                name.to_string(),
                ResourceSpec {
                    count: 1,
                    cost_per_hour: 0.0,
                    calendar: None,
                },
            );
        }
        return;
    };
    egui::Grid::new(("pool", name))
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Resources");
            ui.add(DragValue::new(&mut r.count).range(1..=10_000));
            ui.end_row();
            ui.label("Cost per hour");
            ui.add(
                DragValue::new(&mut r.cost_per_hour)
                    .speed(0.5)
                    .range(0.0..=f64::MAX),
            );
            ui.end_row();
            ui.label("Working hours");
            ComboBox::from_id_salt(("cal", name))
                .selected_text(r.calendar.clone().unwrap_or_else(|| "always".into()))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.calendar, None, "always");
                    for c in &calendars {
                        ui.selectable_value(&mut r.calendar, Some(c.clone()), c);
                    }
                });
            ui.end_row();
        });
    if users.is_empty() {
        ui.label(RichText::new("No task uses this pool.").small().weak());
    } else {
        ui.label(
            RichText::new(format!("Used by {}", users.join(", ")))
                .small()
                .weak(),
        );
    }
}

/// A black-box pool: whether (and after how long) it replies along its message flows.
pub fn black_box_editor(ui: &mut Ui, s: &mut Scenario, id: &QId) {
    let m = s.messages.get_or_insert_with(Default::default);
    let key = [id.to_string(), id.id.clone()]
        .into_iter()
        .find(|k| m.black_box_reply.contains_key(k))
        .unwrap_or(id.id.clone());
    let mut replies = m.black_box_reply.contains_key(&key);
    if ui
        .checkbox(&mut replies, "replies along its outgoing message flows")
        .changed()
    {
        if replies {
            m.black_box_reply
                .insert(key.clone(), default_dist(0, 1800.0));
        } else {
            m.black_box_reply.remove(&key);
        }
    }
    if let Some(d) = m.black_box_reply.get_mut(&key) {
        ui.horizontal(|ui| {
            ui.label("after");
            dist_edit(ui, Id::unique(("reply", id)), d);
        });
    }
}

fn number_fields(ui: &mut Ui, n: &mut NumberSpec) {
    let num = |ui: &mut Ui, label: &str, v: &mut f64| {
        ui.label(label);
        ui.add(DragValue::new(v).speed(0.5));
    };
    match n {
        NumberSpec::Fixed { value } => num(ui, "value", value),
        NumberSpec::Uniform { min, max } => {
            num(ui, "min", min);
            num(ui, "max", max);
        }
        NumberSpec::Normal { mean, stdev } | NumberSpec::Lognormal { mean, stdev } => {
            num(ui, "mean", mean);
            num(ui, "stdev", stdev);
        }
        NumberSpec::Exponential { mean } => num(ui, "mean", mean),
        NumberSpec::Triangular { min, mode, max } => {
            num(ui, "min", min);
            num(ui, "mode", mode);
            num(ui, "max", max);
        }
    }
}

/// Edit a decision input: number distribution, categorical choice or boolean.
pub fn input_edit(ui: &mut Ui, id: (&str, &str), spec: &mut InputSpec) {
    ui.horizontal(|ui| {
        let kind = match spec {
            InputSpec::Number(_) => 0,
            InputSpec::Choice(_) => 1,
            InputSpec::Chance(_) => 2,
        };
        let mut chosen = kind;
        ComboBox::from_id_salt(("input-kind", id))
            .width(80.0)
            .selected_text(["number", "choice", "true/false"][kind])
            .show_ui(ui, |ui| {
                for (i, k) in ["number", "choice", "true/false"].iter().enumerate() {
                    ui.selectable_value(&mut chosen, i, *k);
                }
            });
        if chosen != kind {
            *spec = match chosen {
                0 => InputSpec::Number(NumberSpec::Uniform {
                    min: 0.0,
                    max: 100.0,
                }),
                1 => InputSpec::Choice(ChoiceSpec {
                    choice: [("a".to_string(), 0.5), ("b".to_string(), 0.5)].into(),
                }),
                _ => InputSpec::Chance(ChanceSpec { p_true: 0.5 }),
            };
        }
        match spec {
            InputSpec::Number(n) => {
                let kinds = [
                    "fixed",
                    "uniform",
                    "normal",
                    "exponential",
                    "triangular",
                    "lognormal",
                ];
                let k = match n {
                    NumberSpec::Fixed { .. } => 0,
                    NumberSpec::Uniform { .. } => 1,
                    NumberSpec::Normal { .. } => 2,
                    NumberSpec::Exponential { .. } => 3,
                    NumberSpec::Triangular { .. } => 4,
                    NumberSpec::Lognormal { .. } => 5,
                };
                let mut c = k;
                ComboBox::from_id_salt(("input-dist", id))
                    .width(90.0)
                    .selected_text(kinds[k])
                    .show_ui(ui, |ui| {
                        for (i, name) in kinds.iter().enumerate() {
                            ui.selectable_value(&mut c, i, *name);
                        }
                    });
                if c != k {
                    *n = match c {
                        0 => NumberSpec::Fixed { value: 50.0 },
                        1 => NumberSpec::Uniform {
                            min: 0.0,
                            max: 100.0,
                        },
                        2 => NumberSpec::Normal {
                            mean: 50.0,
                            stdev: 10.0,
                        },
                        3 => NumberSpec::Exponential { mean: 50.0 },
                        4 => NumberSpec::Triangular {
                            min: 0.0,
                            mode: 50.0,
                            max: 100.0,
                        },
                        _ => NumberSpec::Lognormal {
                            mean: 50.0,
                            stdev: 10.0,
                        },
                    };
                }
                number_fields(ui, n);
            }
            InputSpec::Choice(c) => {
                ui.vertical(|ui| {
                    for (value, p) in c.choice.iter_mut() {
                        ui.add(egui::Slider::new(p, 0.0..=1.0).text(value.as_str()));
                    }
                    let sum: f64 = c.choice.values().sum();
                    if (sum - 1.0).abs() > 1e-9 && ui.button("Normalise").clicked() && sum > 0.0 {
                        c.choice.values_mut().for_each(|p| *p /= sum);
                    }
                });
            }
            InputSpec::Chance(c) => {
                ui.add(egui::Slider::new(&mut c.p_true, 0.0..=1.0).text("P(true)"));
            }
        }
    });
}
