use eframe::egui::{self, ComboBox, DragValue, Id, RichText, Ui};
use sim_core::calendar::CalendarSpec;
use sim_core::scenario::{
    ActivitySpec, ArrivalSpec, GatewaySpec, ResourceSpec, SignalScope, SimulationSection, TimeUnit,
};
use sim_core::{NodeKind, Project, Scenario};

use crate::app::App;
use crate::widgets::{default_dist, dist_edit};

pub fn show(app: &mut App, ui: &mut Ui) {
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
                    app.status = format!("Loaded {}", f.display());
                    app.scenario_path = Some(f);
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
            .button("Generate skeleton")
            .on_hover_text("Replaces the scenario with placeholders for every task and XOR split")
            .clicked()
        {
            app.regenerate_skeleton();
            app.status = "Skeleton generated.".into();
        }
    });
    problems(app, ui);
    ui.separator();

    let project = app.ws.project.clone();
    let pools: Vec<String> = app.scenario.resources.keys().cloned().collect();
    let calendar_names: Vec<String> = app.scenario.calendars.keys().cloned().collect();
    let starts: Vec<String> = project
        .nodes()
        .filter(|n| n.kind == NodeKind::StartEvent)
        .map(|n| n.id.to_string())
        .collect();

    egui::ScrollArea::vertical().show(ui, |ui| {
        let s = &mut app.scenario;
        egui::CollapsingHeader::new("Simulation")
            .default_open(true)
            .show(ui, |ui| simulation_editor(ui, &mut s.simulation));

        egui::CollapsingHeader::new("Arrivals")
            .default_open(true)
            .show(ui, |ui| {
                let mut remove = None;
                for (i, a) in s.arrivals.iter_mut().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(
                                    a.label
                                        .clone()
                                        .unwrap_or_else(|| format!("Arrival {}", i + 1)),
                                )
                                .strong(),
                            );
                            if ui.small_button("remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("inter-arrival:");
                            dist_edit(ui, Id::unique(("arrival", i)), &mut a.inter_arrival);
                        });
                        ui.label("Start events fired together:");
                        let mut drop = None;
                        for (t, trig) in a.triggers.iter().enumerate() {
                            ui.horizontal(|ui| {
                                if ui.small_button("✕").clicked() {
                                    drop = Some(t);
                                }
                                ui.label(trig);
                            });
                        }
                        if let Some(t) = drop {
                            a.triggers.remove(t);
                        }
                        let mut add = None;
                        ComboBox::from_id_salt(("addtrigger", i))
                            .selected_text("add start event…")
                            .show_ui(ui, |ui| {
                                for st in starts.iter().filter(|st| !a.triggers.contains(st)) {
                                    if ui.selectable_label(false, st).clicked() {
                                        add = Some(st.clone());
                                    }
                                }
                            });
                        a.triggers.extend(add);
                    });
                }
                if let Some(i) = remove {
                    s.arrivals.remove(i);
                }
                if ui.button("Add arrival generator").clicked() {
                    s.arrivals.push(ArrivalSpec {
                        label: None,
                        inter_arrival: default_dist(0, 3600.0),
                        triggers: starts.first().cloned().into_iter().collect(),
                    });
                }
            });

        egui::CollapsingHeader::new("Calendars").show(ui, |ui| {
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
                ui.text_edit_singleline(&mut name);
                if ui.button("Add calendar").clicked() && !name.trim().is_empty() {
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
        });

        egui::CollapsingHeader::new("Resources")
            .default_open(true)
            .show(ui, |ui| {
                let mut remove = None;
                egui::Grid::new("resources").striped(true).show(ui, |ui| {
                    ui.label(RichText::new("pool").strong());
                    ui.label(RichText::new("count").strong());
                    ui.label(RichText::new("cost / hour").strong());
                    ui.label(RichText::new("calendar").strong());
                    ui.end_row();
                    for (name, r) in s.resources.iter_mut() {
                        ui.label(name);
                        ui.add(DragValue::new(&mut r.count).range(1..=10_000));
                        ui.add(
                            DragValue::new(&mut r.cost_per_hour)
                                .speed(0.5)
                                .range(0.0..=f64::MAX),
                        );
                        let shown = r.calendar.clone().unwrap_or_else(|| "(always open)".into());
                        ComboBox::from_id_salt(("cal", name))
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut r.calendar, None, "(always open)");
                                for c in &calendar_names {
                                    ui.selectable_value(&mut r.calendar, Some(c.clone()), c);
                                }
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
                let id = ui.scope_id().with("new-pool");
                let mut name: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut name);
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
                });
                ui.data_mut(|d| d.insert_temp(id, name));
            });

        egui::CollapsingHeader::new("Messages and signals").show(ui, |ui| {
            let m = s.messages.get_or_insert_with(Default::default);
            ui.checkbox(
                &mut m.buffer_unconsumed,
                "buffer messages that arrive before the receiver waits",
            );
            ui.horizontal(|ui| {
                ui.label("signal scope");
                ui.selectable_value(
                    &mut s.simulation.signal_scope,
                    SignalScope::Case,
                    "same case",
                );
                ui.selectable_value(
                    &mut s.simulation.signal_scope,
                    SignalScope::Global,
                    "all cases",
                );
            });
            let black_boxes: Vec<String> = project
                .files
                .values()
                .flat_map(|f| f.participants.values())
                .filter(|p| p.process.is_none())
                .map(|p| p.id.to_string())
                .collect();
            for bb in black_boxes {
                let mut replies = m.black_box_reply.contains_key(&bb);
                if ui
                    .checkbox(
                        &mut replies,
                        format!("black box {bb} replies after a delay"),
                    )
                    .changed()
                {
                    if replies {
                        m.black_box_reply
                            .insert(bb.clone(), default_dist(0, 1800.0));
                    } else {
                        m.black_box_reply.remove(&bb);
                    }
                }
                if let Some(d) = m.black_box_reply.get_mut(&bb) {
                    dist_edit(ui, Id::unique(("reply", &bb)), d);
                }
            }
        });

        egui::CollapsingHeader::new(format!("Activities ({})", s.activities.len())).show(
            ui,
            |ui| {
                for (key, a) in s.activities.iter_mut() {
                    activity_editor(ui, &project, key, a, &pools);
                }
            },
        );

        if !s.events.is_empty() || !s.boundary.is_empty() {
            egui::CollapsingHeader::new("Timer events and boundary events")
                .default_open(true)
                .show(ui, |ui| {
                    for (key, e) in s.events.iter_mut() {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}  delay:", display(&project, key, &e.label)));
                            dist_edit(ui, Id::unique(("event", key)), &mut e.delay);
                        });
                    }
                    for (key, b) in s.boundary.iter_mut() {
                        ui.horizontal(|ui| {
                            ui.label(display(&project, key, &b.label));
                            if let Some(p) = b.probability.as_mut() {
                                ui.add(egui::Slider::new(p, 0.0..=1.0).text("probability"));
                            }
                            if let Some(d) = b.delay.as_mut() {
                                ui.label("delay:");
                                dist_edit(ui, Id::unique(("boundary", key)), d);
                            }
                        });
                    }
                });
        }

        egui::CollapsingHeader::new(format!("XOR gateways ({})", s.gateways.len())).show(
            ui,
            |ui| {
                for (key, g) in s.gateways.iter_mut() {
                    gateway_editor(ui, &project, key, g);
                }
            },
        );
    });
}

fn display(project: &Project, key: &str, label: &Option<String>) -> String {
    let name = label.clone().or_else(|| {
        project
            .nodes()
            .find(|n| n.id.to_string() == key || n.id.id == key)
            .and_then(|n| n.name.clone())
    });
    match name {
        Some(n) => format!("{key}  \"{n}\""),
        None => key.to_string(),
    }
}

fn activity_editor(
    ui: &mut Ui,
    project: &Project,
    key: &str,
    a: &mut ActivitySpec,
    pools: &[String],
) {
    egui::CollapsingHeader::new(display(project, key, &a.label))
        .id_salt(("act", key))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("duration:");
                dist_edit(ui, Id::unique(("dur", key)), &mut a.duration);
            });
            ui.horizontal(|ui| {
                ui.label("fixed cost:");
                ui.add(
                    DragValue::new(&mut a.fixed_cost)
                        .speed(0.5)
                        .range(0.0..=f64::MAX),
                );
                ui.label("resource override:");
                let shown = a
                    .resource
                    .clone()
                    .unwrap_or_else(|| "(from lane / pool)".into());
                ComboBox::from_id_salt(("res", key))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut a.resource, None, "(from lane / pool)");
                        for p in pools {
                            ui.selectable_value(&mut a.resource, Some(p.clone()), p);
                        }
                    });
            });
        });
}

fn gateway_editor(ui: &mut Ui, project: &Project, key: &str, g: &mut GatewaySpec) {
    egui::CollapsingHeader::new(display(project, key, &g.label))
        .id_salt(("gw", key))
        .default_open(true)
        .show(ui, |ui| {
            for (flow, p) in g.probabilities.iter_mut() {
                ui.add(egui::Slider::new(p, 0.0..=1.0).text(flow.as_str()));
            }
            let sum: f64 = g.probabilities.values().sum();
            ui.horizontal(|ui| {
                let ok = (sum - 1.0).abs() <= 1e-9;
                let text = RichText::new(format!("sum = {sum:.4}"));
                ui.label(if ok {
                    text
                } else {
                    text.color(egui::Color32::LIGHT_RED)
                });
                if !ok && ui.button("Normalise").clicked() && sum > 0.0 {
                    g.probabilities.values_mut().for_each(|p| *p /= sum);
                }
            });
        });
}

pub fn simulation_editor(ui: &mut Ui, s: &mut SimulationSection) {
    egui::Grid::new("sim").num_columns(2).show(ui, |ui| {
        ui.label("seed");
        ui.add(DragValue::new(&mut s.seed));
        ui.end_row();
        ui.label("replications");
        ui.add(DragValue::new(&mut s.replications).range(1..=1000));
        ui.end_row();
        ui.label("cases per replication");
        let mut cases = s.cases.unwrap_or(0);
        if ui
            .add(DragValue::new(&mut cases).range(0..=100_000_000u64))
            .changed()
        {
            s.cases = (cases > 0).then_some(cases);
        }
        ui.end_row();
        ui.label("horizon (e.g. 365d)");
        let mut horizon = s.horizon.clone().unwrap_or_default();
        if ui.text_edit_singleline(&mut horizon).changed() {
            s.horizon = (!horizon.trim().is_empty()).then(|| horizon.trim().to_string());
        }
        ui.end_row();
        ui.label("start (UTC date and time)");
        let mut start = s.start.clone().unwrap_or_default();
        if ui
            .add(egui::TextEdit::singleline(&mut start).hint_text("2024-01-01T00:00"))
            .changed()
        {
            s.start = (!start.trim().is_empty()).then(|| start.trim().to_string());
        }
        ui.end_row();
        ui.label("warm-up cases");
        ui.add(DragValue::new(&mut s.warm_up_cases));
        ui.end_row();
        ui.label("report time unit");
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
    });
}

/// Scenario errors (red) and warnings (yellow) from binding the scenario to the model.
fn problems(app: &App, ui: &mut Ui) {
    match app.scenario.bind(&app.ws.project) {
        Ok(b) => {
            for w in &b.warnings {
                ui.colored_label(egui::Color32::YELLOW, format!("warning: {w}"));
            }
        }
        Err(e) => {
            ui.colored_label(egui::Color32::LIGHT_RED, e.to_string());
        }
    }
}

pub fn show_run(app: &mut App, ui: &mut Ui) {
    simulation_editor(ui, &mut app.scenario.simulation);
    ui.separator();
    let bound_ok = app.scenario.bind(&app.ws.project).is_ok();
    problems(app, ui);
    if let Some(e) = &app.run_error {
        ui.colored_label(egui::Color32::LIGHT_RED, e);
    }
    let running = app.job.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(bound_ok && !running, egui::Button::new("▶ Run simulation"))
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
}
