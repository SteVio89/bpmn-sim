use std::collections::BTreeMap;

use eframe::egui::{self, RichText, Ui};
use sim_core::{NodeKind, Severity, validate};

use crate::app::App;

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("Open folder…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            app.ws.open(&dir);
            after_load(app);
        }
        if ui.button("Open file…").clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("BPMN", &["bpmn"])
                .pick_file()
        {
            app.ws.open(&f);
            after_load(app);
        }
        if ui.button("Add file…").clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("BPMN", &["bpmn", "dmn"])
                .pick_file()
        {
            app.ws.add(f);
            after_load(app);
        }
    });
    if let Some(e) = &app.ws.load_error {
        ui.colored_label(egui::Color32::LIGHT_RED, e);
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Files");
        let mut remove = None;
        for (i, p) in app.ws.paths.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("✕")
                    .on_hover_text("Remove from project")
                    .clicked()
                {
                    remove = Some(i);
                }
                ui.label(p.display().to_string());
            });
        }
        if let Some(i) = remove {
            app.ws.remove(i);
            after_load(app);
        }

        let project = app.ws.project.clone();
        ui.heading("Validation");
        let issues = validate(&project);
        if issues.is_empty() && !project.files.is_empty() {
            ui.label("No problems found.");
        }
        for issue in &issues {
            let color = match issue.severity {
                Severity::Error => egui::Color32::LIGHT_RED,
                Severity::Warning => egui::Color32::YELLOW,
            };
            if ui
                .selectable_label(
                    app.selected.as_ref() == Some(&issue.element),
                    RichText::new(issue.to_string()).color(color),
                )
                .clicked()
            {
                app.selected = Some(issue.element.clone());
            }
        }
        if let Some(node) = app.selected.as_ref().and_then(|s| project.node(s)) {
            ui.group(|ui| {
                ui.label(RichText::new(format!("{} ({})", node.id, node.label())).strong());
                ui.label(format!("kind: {:?}", node.kind));
                if let Some(l) = &node.lane {
                    ui.label(format!("lane: {l}"));
                }
                ui.label(format!(
                    "{} incoming, {} outgoing flow(s)",
                    node.incoming.len(),
                    node.outgoing.len()
                ));
            });
        }

        ui.heading("Resource pools from lanes and pools");
        let mut pools: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (fname, f) in &project.files {
            for p in f.processes.values() {
                for l in &p.lanes {
                    pools
                        .entry(l.clone())
                        .or_default()
                        .push(format!("{fname} (lane)"));
                }
            }
            for part in f.participants.values().filter(|p| p.process.is_some()) {
                if let Some(n) = &part.name {
                    pools
                        .entry(n.clone())
                        .or_default()
                        .push(format!("{fname} (pool)"));
                }
            }
        }
        egui::Grid::new("pools").striped(true).show(ui, |ui| {
            for (pool, from) in &pools {
                ui.label(pool);
                ui.label(from.join(", "));
                ui.end_row();
            }
        });

        ui.heading("Model tree");
        for (fname, f) in &project.files {
            egui::CollapsingHeader::new(fname)
                .id_salt(("file", fname))
                .show(ui, |ui| {
                    for (pid, process) in &f.processes {
                        let title = format!(
                            "process {} {}",
                            pid.id,
                            process.name.as_deref().unwrap_or("")
                        );
                        egui::CollapsingHeader::new(title)
                            .id_salt(pid.to_string())
                            .show(ui, |ui| {
                                for n in f.nodes.values().filter(|n| n.process == *pid) {
                                    let mut text = format!("{:?}  {}", n.kind, n.id.id);
                                    if let Some(name) = &n.name {
                                        text += &format!("  \"{name}\"");
                                    }
                                    if let Some(d) = &n.called_decision {
                                        match project.decisions.get(d) {
                                            Some(file) => {
                                                text +=
                                                    &format!("  → decision {d} ({file}, evaluated)")
                                            }
                                            None => {
                                                text += &format!("  → decision {d} (NOT FOUND)")
                                            }
                                        }
                                    }
                                    if n.kind == NodeKind::CallActivity
                                        && let Some(c) = &n.called_element
                                    {
                                        match project.processes.get(c) {
                                            Some(q) => text += &format!("  → {c} ({})", q.file),
                                            None => text += &format!("  → {c} (NOT FOUND)"),
                                        }
                                    }
                                    if ui
                                        .selectable_label(
                                            app.selected.as_ref() == Some(&n.id),
                                            text,
                                        )
                                        .clicked()
                                    {
                                        app.selected = Some(n.id.clone());
                                    }
                                }
                            });
                    }
                });
        }
    });
}

fn after_load(app: &mut App) {
    app.selected = None;
    app.status = match &app.ws.load_error {
        None => format!("Loaded {} file(s).", app.ws.project.files.len()),
        Some(_) => "Loading failed, see the Model tab.".into(),
    };
    if app.scenario_is_pristine() && app.ws.load_error.is_none() {
        app.regenerate_skeleton();
    }
}
