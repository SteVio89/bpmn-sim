use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, ComboBox, DragValue, Id, RichText, Ui};
use sim_core::calendar::CalendarSpec;
use sim_core::dist::{Seconds, format_duration, parse_duration};
use sim_core::needs::{Need, default_pool, is_arrival_start, key_of, needs};
use sim_core::scenario::{
    ActivitySpec, ArrivalSpec, Problem, ResourceSpec, SignalScope, SimulationSection, TimeUnit,
};
use sim_core::{FlowNode, Project, Scenario};

use crate::app::App;
use crate::configure::{Status, Target, problem_target, statuses};
use crate::editors;
use crate::widgets::{default_dist, dist_edit, duration_edit, rate_text};

/// A section of elements: title, the elements, which of their needs it shows.
type Section<'a> = (&'a str, &'a [&'a FlowNode], fn(&Need) -> bool);

/// Run `edit` on the scenario; if it changed anything, `targets` count as checked.
fn tracked(
    s: &mut Scenario,
    unreviewed: &mut BTreeSet<Target>,
    targets: &[Target],
    edit: impl FnOnce(&mut Scenario),
) {
    let before = s.clone();
    edit(s);
    if *s != before {
        for t in targets {
            unreviewed.remove(t);
        }
    }
}

/// The name of something configurable, with a dot if it still needs input (red for errors).
/// Clicking it shows it in the diagram.
fn name_cell(
    ui: &mut Ui,
    name: &str,
    t: &Target,
    status: &BTreeMap<Target, Status>,
    jump: &mut Option<Target>,
) {
    ui.horizontal(|ui| {
        match status.get(t) {
            Some(Status::Error) => {
                dot(ui, ui.visuals().error_fg_color).on_hover_text("has errors");
            }
            Some(Status::Todo) => {
                dot(ui, ui.visuals().warn_fg_color)
                    .on_hover_text("placeholder, still to be checked");
            }
            None => {
                dot(ui, egui::Color32::TRANSPARENT);
            }
        }
        if ui.link(name).on_hover_text("show in the diagram").clicked() {
            *jump = Some(t.clone());
        }
    });
}

/// A small filled circle (the fonts have no bullet glyph).
fn dot(ui: &mut Ui, color: egui::Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    response
}

pub fn show(app: &mut App, ui: &mut Ui) {
    toolbar(app, ui);
    let project = app.ws.project.clone();
    let checked = app.scenario.check(&project);
    let status = statuses(&project, &app.scenario, &app.unreviewed, &checked.errors);
    let mut jump = None;
    problems(ui, &project, &checked.errors, &checked.warnings, &mut jump);
    let todo = crate::configure::attention(&project, &status);
    if !todo.is_empty() {
        ui.horizontal(|ui| {
            dot(ui, ui.visuals().warn_fg_color);
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("{} element(s) still need input", todo.len()),
            );
            if ui
                .button("Check them in the diagram")
                .on_hover_text(
                    "open the first one in the diagram; \"Next\" there leads through the rest",
                )
                .clicked()
            {
                jump = todo.first().cloned();
            }
        });
    }
    ui.separator();

    let s = &mut app.scenario;
    let unreviewed = &mut app.unreviewed;
    // Elements that take each kind of parameter, in model order.
    let with = |pred: fn(&Need) -> bool| -> Vec<&FlowNode> {
        project
            .nodes()
            .filter(|n| needs(&project, n).iter().any(pred))
            .collect()
    };
    let tasks = with(|n| matches!(n, Need::Duration));
    let splits = with(|n| matches!(n, Need::Branching { .. }));
    let events = with(|n| matches!(n, Need::Delay | Need::Boundary { .. }));
    let repeats = with(|n| matches!(n, Need::InstanceCount | Need::Repeat));

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::CollapsingHeader::new("Arrivals")
            .default_open(true)
            .show(ui, |ui| arrivals(ui, &project, s, unreviewed, &status, &mut jump));

        egui::CollapsingHeader::new(format!("Resources ({})", s.resources.len()))
            .default_open(true)
            .show(ui, |ui| resources(ui, &project, s, unreviewed, &status, &mut jump));

        egui::CollapsingHeader::new(format!("Working hours ({})", s.calendars.len()))
            .show(ui, |ui| calendars(ui, s));

        egui::CollapsingHeader::new(format!("Tasks ({})", tasks.len()))
            .default_open(true)
            .show(ui, |ui| task_table(ui, &project, s, unreviewed, &status, &tasks, &mut jump));

        let sections: [Section; 3] = [
            ("Branching", &splits, |n| matches!(n, Need::Branching { .. })),
            ("Timers and boundary events", &events, |n| {
                matches!(n, Need::Delay | Need::Boundary { .. })
            }),
            ("Multi-instance and loops", &repeats, |n| {
                matches!(n, Need::InstanceCount | Need::Repeat)
            }),
        ];
        for (title, nodes, pred) in sections {
            if nodes.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{title} ({})", nodes.len()))
                .default_open(true)
                .show(ui, |ui| {
                    for n in nodes {
                        let t = Target::Node(n.id.clone());
                        name_cell(ui, n.label(), &t, &status, &mut jump);
                        ui.indent(("ind", &n.id), |ui| {
                            tracked(s, unreviewed, std::slice::from_ref(&t), |s| {
                                if s.missing(&project, n).iter().any(pred) {
                                    if ui.button("Set placeholder values").clicked() {
                                        s.fill(&project, n);
                                    }
                                    return;
                                }
                                for need in needs(&project, n).into_iter().filter(pred) {
                                    editors::need_editor(ui, &project, s, n, need, false);
                                }
                            });
                        });
                    }
                });
        }

        let inputs: Vec<(String, String)> = s
            .decisions
            .iter()
            .flat_map(|(d, spec)| spec.inputs.keys().map(move |i| (d.clone(), i.clone())))
            .collect();
        if !inputs.is_empty() {
            egui::CollapsingHeader::new(format!("Decision inputs ({})", inputs.len()))
                .default_open(true)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "Inputs are process variables: one distribution serves every decision that reads it.",
                        )
                        .small()
                        .weak(),
                    );
                    egui::Grid::new("decision-inputs").num_columns(2).striped(true).show(ui, |ui| {
                        for (d, i) in inputs {
                            ui.label(&i).on_hover_text(format!("defined under decision {d}"));
                            if let Some(spec) = s.decisions.get_mut(&d).and_then(|x| x.inputs.get_mut(&i)) {
                                editors::input_edit(ui, (d.as_str(), i.as_str()), spec);
                            }
                            ui.end_row();
                        }
                    });
                });
        }

        egui::CollapsingHeader::new("Messages and signals").show(ui, |ui| {
            let m = s.messages.get_or_insert_with(Default::default);
            ui.checkbox(
                &mut m.buffer_unconsumed,
                "keep messages that arrive before the receiver waits",
            );
            ui.horizontal(|ui| {
                ui.label("Signals reach");
                ui.selectable_value(&mut s.simulation.signal_scope, SignalScope::Case, "the same case");
                ui.selectable_value(&mut s.simulation.signal_scope, SignalScope::Global, "all cases");
            });
            let black_boxes: Vec<(sim_core::QId, String)> = project
                .files
                .values()
                .flat_map(|f| f.participants.values())
                .filter(|p| p.process.is_none())
                .map(|p| (p.id.clone(), p.name.clone().unwrap_or(p.id.id.clone())))
                .collect();
            for (id, name) in black_boxes {
                let t = Target::BlackBox(id.clone());
                name_cell(ui, &name, &t, &status, &mut jump);
                ui.indent(("bb", &id), |ui| {
                    tracked(s, unreviewed, std::slice::from_ref(&t), |s| {
                        editors::black_box_editor(ui, s, &id)
                    });
                });
            }
        });
    });
    if let Some(t) = jump {
        app.focus(t);
    }
}

fn toolbar(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("Load scenario…").clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("TOML", &["toml"])
                .pick_file()
        {
            match std::fs::read_to_string(&f)
                .map_err(|e| e.to_string())
                .and_then(|t| Scenario::from_toml(&t).map_err(|e| e.to_string()))
            {
                Ok(s) => {
                    app.scenario = s;
                    // A saved scenario counts as checked; only what it lacks is new.
                    app.unreviewed.clear();
                    let u = app.update_from_model();
                    app.status = format!("Loaded {}", f.display());
                    app.scenario_path = Some(f);
                    app.update_note = Some(u);
                }
                Err(e) => app.status = format!("Could not load scenario: {e}"),
            }
        }
        if ui.button("Save scenario…").clicked() {
            let mut dialog = rfd::FileDialog::new()
                .add_filter("TOML", &["toml"])
                .set_file_name("scenario.toml");
            if let Some(p) = &app.scenario_path {
                dialog = dialog.set_file_name(
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
            }
            if let Some(f) = dialog.save_file() {
                app.status = match app
                    .scenario
                    .to_toml()
                    .map_err(|e| e.to_string())
                    .and_then(|t| std::fs::write(&f, t).map_err(|e| e.to_string()))
                {
                    Ok(()) => format!("Saved {}", f.display()),
                    Err(e) => format!("Could not save: {e}"),
                };
                app.scenario_path = Some(f);
            }
        }
        if ui
            .button("Update from model")
            .on_hover_text("add placeholders for elements the scenario does not cover yet; keeps everything you set")
            .clicked()
        {
            app.update_note = Some(app.update_from_model());
        }
        let confirm_id = Id::unique("confirm-reset");
        let mut confirm = ui.data(|d| d.get_temp::<bool>(confirm_id)).unwrap_or(false);
        if confirm {
            ui.colored_label(ui.visuals().warn_fg_color, "Replace everything with placeholders?");
            if ui.button("Replace").clicked() {
                app.regenerate_skeleton();
                app.update_note = None;
                app.status = "Scenario reset to placeholders.".into();
                confirm = false;
            }
            if ui.button("Cancel").clicked() {
                confirm = false;
            }
        } else if ui
            .button("Start over…")
            .on_hover_text("replace the whole scenario with placeholders for the model")
            .clicked()
        {
            confirm = true;
        }
        ui.data_mut(|d| d.insert_temp(confirm_id, confirm));
    });
    if let Some(u) = app.update_note.clone() {
        ui.horizontal_wrapped(|ui| {
            let added = u.added.len() + u.pools_added.len() + usize::from(u.arrival_added);
            if added == 0 && u.orphaned.is_empty() {
                ui.label("The scenario covers the whole model.");
            } else if added > 0 {
                ui.label(format!(
                    "Added placeholders for {added} element(s), marked as still to check."
                ));
            }
            if !u.orphaned.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "{} entr(ies) for elements no longer in the model: {}",
                        u.orphaned.len(),
                        u.orphaned.join(", ")
                    ),
                );
                if ui.button("Remove them").clicked() {
                    app.scenario.remove_orphans(&app.ws.project);
                    app.update_note = None;
                }
            }
            if ui.small_button("x").clicked() {
                app.update_note = None;
            }
        });
    }
}

/// Errors (red) and warnings (yellow); those about an element show it in the diagram.
fn problems(
    ui: &mut Ui,
    project: &Project,
    errors: &[Problem],
    warnings: &[Problem],
    jump: &mut Option<Target>,
) {
    let show = |ui: &mut Ui, p: &Problem, color: egui::Color32, jump: &mut Option<Target>| {
        match problem_target(project, p) {
            Some(t) => {
                if ui
                    .link(RichText::new(&p.message).color(color))
                    .on_hover_text("show in the diagram")
                    .clicked()
                {
                    *jump = Some(t);
                }
            }
            None => {
                ui.colored_label(color, &p.message);
            }
        }
    };
    if errors.len() + warnings.len() > 6 {
        egui::ScrollArea::vertical()
            .id_salt("problems")
            .max_height(110.0)
            .show(ui, |ui| {
                for e in errors {
                    show(ui, e, ui.visuals().error_fg_color, jump);
                }
                for w in warnings {
                    show(ui, w, ui.visuals().warn_fg_color, jump);
                }
            });
    } else {
        for e in errors {
            show(ui, e, ui.visuals().error_fg_color, jump);
        }
        for w in warnings {
            show(ui, w, ui.visuals().warn_fg_color, jump);
        }
    }
}

fn arrivals(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    unreviewed: &mut BTreeSet<Target>,
    status: &BTreeMap<Target, Status>,
    jump: &mut Option<Target>,
) {
    let starts: Vec<&FlowNode> = project.nodes().filter(|n| is_arrival_start(n)).collect();
    let find = |t: &str| {
        starts
            .iter()
            .find(|n| n.id.to_string() == t || n.id.id == t)
            .copied()
    };
    let mut remove = None;
    for i in 0..s.arrivals.len() {
        let targets: Vec<Target> = s.arrivals[i]
            .triggers
            .iter()
            .filter_map(|t| find(t))
            .map(|n| Target::Node(n.id.clone()))
            .collect();
        ui.group(|ui| {
            ui.horizontal(|ui| {
                let label = s.arrivals[i]
                    .label
                    .clone()
                    .unwrap_or(format!("Arrivals {}", i + 1));
                ui.label(RichText::new(label).strong());
                if ui.small_button("remove").clicked() {
                    remove = Some(i);
                }
            });
            tracked(s, unreviewed, &targets, |s| {
                let a = &mut s.arrivals[i];
                ui.horizontal(|ui| {
                    ui.label("A new case every");
                    dist_edit(ui, Id::unique(("arrival", i)), &mut a.inter_arrival);
                    ui.label(RichText::new(rate_text(&a.inter_arrival)).small().weak());
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Starts");
                    let mut drop = None;
                    for (k, t) in a.triggers.iter().enumerate() {
                        match find(t) {
                            Some(n) => {
                                name_cell(ui, n.label(), &Target::Node(n.id.clone()), status, jump)
                            }
                            None => {
                                ui.colored_label(ui.visuals().error_fg_color, t);
                            }
                        }
                        if ui
                            .small_button("x")
                            .on_hover_text("do not start here")
                            .clicked()
                        {
                            drop = Some(k);
                        }
                    }
                    if let Some(k) = drop {
                        a.triggers.remove(k);
                    }
                    let mut add = None;
                    ComboBox::from_id_salt(("addtrigger", i))
                        .selected_text("add start event…")
                        .show_ui(ui, |ui| {
                            for n in &starts {
                                if find_in(&a.triggers, n) {
                                    continue;
                                }
                                if ui
                                    .selectable_label(false, n.label())
                                    .on_hover_text(n.id.to_string())
                                    .clicked()
                                {
                                    add = Some(n.id.to_string());
                                }
                            }
                        });
                    a.triggers.extend(add);
                });
            });
        });
    }
    if let Some(i) = remove {
        s.arrivals.remove(i);
    }
    if ui.button("Add arrival generator").clicked() {
        s.arrivals.push(ArrivalSpec {
            label: None,
            inter_arrival: default_dist(3, 3600.0),
            triggers: starts
                .first()
                .map(|n| n.id.to_string())
                .into_iter()
                .collect(),
        });
    }
}

fn find_in(triggers: &[String], n: &FlowNode) -> bool {
    triggers
        .iter()
        .any(|t| *t == n.id.to_string() || *t == n.id.id)
}

fn resources(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    unreviewed: &mut BTreeSet<Target>,
    status: &BTreeMap<Target, Status>,
    jump: &mut Option<Target>,
) {
    let calendars: Vec<String> = s.calendars.keys().cloned().collect();
    let names: Vec<String> = s.resources.keys().cloned().collect();
    let mut remove = None;
    egui::Grid::new("resources").striped(true).show(ui, |ui| {
        ui.label(RichText::new("pool").strong());
        ui.label(RichText::new("resources").strong());
        ui.label(RichText::new("cost / hour").strong());
        ui.label(RichText::new("working hours").strong());
        ui.end_row();
        for name in names {
            let t = Target::Pool(name.clone());
            name_cell(ui, &name, &t, status, jump);
            tracked(s, unreviewed, std::slice::from_ref(&t), |s| {
                let r = s.resources.get_mut(&name).expect("listed");
                ui.add(DragValue::new(&mut r.count).range(1..=10_000));
                ui.add(
                    DragValue::new(&mut r.cost_per_hour)
                        .speed(0.5)
                        .range(0.0..=f64::MAX),
                );
                ComboBox::from_id_salt(("cal", &name))
                    .selected_text(r.calendar.clone().unwrap_or_else(|| "always".into()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.calendar, None, "always");
                        for c in &calendars {
                            ui.selectable_value(&mut r.calendar, Some(c.clone()), c);
                        }
                    });
            });
            if ui.small_button("remove").clicked() {
                remove = Some(name.clone());
            }
            ui.end_row();
        }
    });
    if let Some(n) = remove {
        s.resources.remove(&n);
    }
    let unused_lanes: Vec<String> = project
        .nodes()
        .filter(|n| needs(project, n).contains(&Need::Duration))
        .filter_map(|n| default_pool(project, n))
        .filter(|p| !s.resources.contains_key(p))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let id = ui.scope_id().with("new-pool");
    let mut name: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("pool name")
                .desired_width(140.0),
        );
        if ui.button("Add pool").clicked() && !name.trim().is_empty() {
            s.resources
                .entry(name.trim().to_string())
                .or_insert(ResourceSpec {
                    count: 1,
                    cost_per_hour: 0.0,
                    calendar: None,
                });
            name.clear();
        }
        for l in unused_lanes {
            if ui
                .small_button(format!("add {l}"))
                .on_hover_text("a lane without a pool")
                .clicked()
            {
                s.resources.insert(
                    l,
                    ResourceSpec {
                        count: 1,
                        cost_per_hour: 0.0,
                        calendar: None,
                    },
                );
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(id, name));
}

fn calendars(ui: &mut Ui, s: &mut Scenario) {
    let mut remove = None;
    for (name, c) in s.calendars.iter_mut() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(name.as_str()).strong());
                if ui.small_button("remove").clicked() {
                    remove = Some(name.clone());
                }
            });
            ui.horizontal(|ui| {
                for day in ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] {
                    let mut on = c.days.iter().any(|d| d.eq_ignore_ascii_case(day));
                    if ui.checkbox(&mut on, day).changed() {
                        c.days.retain(|d| !d.eq_ignore_ascii_case(day));
                        if on {
                            c.days.push(day.to_string());
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("hours");
                ui.text_edit_singleline(&mut c.hours)
                    .on_hover_text("e.g. 09:00-12:00,13:00-17:00");
            });
        });
    }
    if let Some(n) = remove {
        s.calendars.remove(&n);
    }
    let id = ui.scope_id().with("new-calendar");
    let mut name: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text("name, e.g. office")
                .desired_width(140.0),
        );
        if ui.button("Add working hours").clicked() && !name.trim().is_empty() {
            s.calendars
                .entry(name.trim().to_string())
                .or_insert(CalendarSpec {
                    days: ["mon", "tue", "wed", "thu", "fri"]
                        .map(String::from)
                        .to_vec(),
                    hours: "09:00-17:00".into(),
                });
            name.clear();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, name));
}

/// One row per task, grouped by file and lane.
fn task_table(
    ui: &mut Ui,
    project: &Project,
    s: &mut Scenario,
    unreviewed: &mut BTreeSet<Target>,
    status: &BTreeMap<Target, Status>,
    tasks: &[&FlowNode],
    jump: &mut Option<Target>,
) {
    let mut groups: BTreeMap<(String, String), Vec<&FlowNode>> = BTreeMap::new();
    for n in tasks {
        let lane = default_pool(project, n).unwrap_or_default();
        groups.entry((n.id.file.clone(), lane)).or_default().push(n);
    }
    let files = project.files.len();
    let pools: Vec<String> = s.resources.keys().cloned().collect();
    egui::Grid::new("tasks").num_columns(4).show(ui, |ui| {
        for ((file, lane), nodes) in groups {
            let title = match (files > 1, lane.is_empty()) {
                (true, true) => file.clone(),
                (true, false) => format!("{file} · {lane}"),
                (false, true) => "(no lane)".into(),
                (false, false) => lane.clone(),
            };
            ui.label(RichText::new(title).strong());
            ui.label(RichText::new("duration").weak());
            ui.label(RichText::new("resource").weak());
            ui.label(RichText::new("fixed cost").weak());
            ui.end_row();
            for n in nodes {
                let t = Target::Node(n.id.clone());
                name_cell(ui, n.label(), &t, status, jump);
                match key_of(&s.activities, project, &n.id) {
                    Some(k) => tracked(s, unreviewed, std::slice::from_ref(&t), |s| {
                        let a: &mut ActivitySpec = s.activities.get_mut(&k).expect("key");
                        dist_edit(ui, Id::unique(("dur", &n.id)), &mut a.duration);
                        editors::resource_combo(ui, project, n, &mut a.resource, &pools);
                        ui.add(
                            DragValue::new(&mut a.fixed_cost)
                                .speed(0.5)
                                .range(0.0..=f64::MAX),
                        );
                    }),
                    None => {
                        if ui.button("Set placeholder values").clicked() {
                            s.fill(project, n);
                            unreviewed.insert(t);
                        }
                    }
                }
                ui.end_row();
            }
        }
    });
}

/// When and how long to run: cases or simulated time, replications, and the details.
pub fn run_settings(ui: &mut Ui, s: &mut SimulationSection) {
    egui::Grid::new("run").num_columns(2).show(ui, |ui| {
        ui.label("Stop after");
        ui.horizontal(|ui| {
            let mut on = s.cases.is_some();
            if ui.checkbox(&mut on, "").changed() {
                s.cases = on.then_some(100);
            }
            let mut cases = s.cases.unwrap_or(100);
            ui.add_enabled_ui(on, |ui| {
                if ui
                    .add(DragValue::new(&mut cases).range(1..=100_000_000u64))
                    .changed()
                {
                    s.cases = Some(cases);
                }
            });
            ui.label("cases");
        });
        ui.end_row();
        ui.label(if s.cases.is_some() {
            "or after"
        } else {
            "Stop after"
        });
        ui.horizontal(|ui| {
            let mut on = s.horizon.is_some();
            if ui.checkbox(&mut on, "").changed() {
                s.horizon = on.then(|| "30d".to_string());
            }
            let mut secs = Seconds(
                s.horizon
                    .as_deref()
                    .and_then(|h| parse_duration(h).ok())
                    .unwrap_or(30.0 * 86_400.0),
            );
            ui.add_enabled_ui(on, |ui| {
                let before = secs;
                duration_edit(ui, Id::unique("horizon"), &mut secs);
                if secs != before && secs.0 > 0.0 {
                    s.horizon = Some(format_duration(secs.0));
                }
            });
            ui.label("of simulated time");
        });
        ui.end_row();
        ui.label("Replications");
        ui.horizontal(|ui| {
            ui.add(DragValue::new(&mut s.replications).range(1..=1000));
            ui.label(
                RichText::new("independent runs; several give confidence intervals")
                    .small()
                    .weak(),
            );
        });
        ui.end_row();
    });
    egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
        egui::Grid::new("run-advanced").num_columns(2).show(ui, |ui| {
            ui.label("Random seed");
            ui.add(DragValue::new(&mut s.seed));
            ui.end_row();
            ui.label("Warm-up cases");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut s.warm_up_cases));
                ui.label(RichText::new("left out of the statistics").small().weak());
            });
            ui.end_row();
            ui.label("Start (UTC)");
            let mut start = s.start.clone().unwrap_or_default();
            if ui
                .add(egui::TextEdit::singleline(&mut start).hint_text("2024-01-01T00:00"))
                .on_hover_text("date and time of the simulation start; decides the weekday working hours start on")
                .changed()
            {
                s.start = (!start.trim().is_empty()).then(|| start.trim().to_string());
            }
            ui.end_row();
            ui.label("Report times in");
            ComboBox::from_id_salt("unit")
                .selected_text(s.time_unit.name())
                .show_ui(ui, |ui| {
                    for u in [
                        TimeUnit::Seconds,
                        TimeUnit::Minutes,
                        TimeUnit::Hours,
                        TimeUnit::Days,
                    ] {
                        ui.selectable_value(&mut s.time_unit, u, u.name());
                    }
                });
            ui.end_row();
            ui.label("Cases to replay");
            ui.horizontal(|ui| {
                ui.add(DragValue::new(&mut s.trace_cases).range(0..=100_000u64));
                ui.label(RichText::new("recorded step by step for the token replay").small().weak());
            });
            ui.end_row();
        });
    });
}

pub fn show_run(app: &mut App, ui: &mut Ui) {
    run_settings(ui, &mut app.scenario.simulation);
    ui.separator();
    let project = app.ws.project.clone();
    let checked = app.scenario.check(&project);
    let mut jump = None;
    problems(ui, &project, &checked.errors, &checked.warnings, &mut jump);
    if let Some(e) = &app.run_error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    let running = app.job.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                checked.bound.is_some() && !running,
                egui::Button::new("Run simulation"),
            )
            .clicked()
        {
            let ctx = ui.ctx().clone();
            app.start_run(&ctx);
            app.status = "Running…".into();
        }
        if running && let Some(j) = &app.job {
            ui.add(
                egui::ProgressBar::new(j.done as f32 / j.total.max(1) as f32)
                    .text(format!("{}/{} replications", j.done, j.total)),
            );
        }
    });
    ui.separator();
    crate::configure::pilot_summary(ui, &mut app.pilot);
    if let Some(t) = jump {
        app.focus(t);
    }
}
