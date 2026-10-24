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
            app.scenario_candidates = find_scenarios(&dir);
            if app.ws.load_error.is_none()
                && let Some(first) = app.scenario_candidates.first().cloned()
            {
                app.load_scenario(first);
            }
        }
        if ui.button("Open file…").clicked()
            && let Some(f) = rfd::FileDialog::new()
                .add_filter("BPMN", &["bpmn"])
                .pick_file()
        {
            app.ws.open(&f);
            app.scenario_candidates.clear();
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
        ui.colored_label(ui.visuals().error_fg_color, e);
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Files");
        let mut remove = None;
        for (i, p) in app.ws.paths.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("x")
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
                Severity::Error => ui.visuals().error_fg_color,
                Severity::Warning => ui.visuals().warn_fg_color,
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
                                                text += &format!(
                                                    "  -> decision {d} ({file}, evaluated)"
                                                )
                                            }
                                            None => {
                                                text += &format!("  -> decision {d} (NOT FOUND)")
                                            }
                                        }
                                    }
                                    if n.kind == NodeKind::CallActivity
                                        && let Some(c) = &n.called_element
                                    {
                                        match project.processes.get(c) {
                                            Some(q) => text += &format!("  -> {c} ({})", q.file),
                                            None => text += &format!("  -> {c} (NOT FOUND)"),
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
    if app.ws.load_error.is_some() {
        return;
    }
    if app.scenario_is_pristine() {
        app.regenerate_skeleton();
    } else {
        // Keep what was set; add placeholders for new elements and report removed ones.
        let u = app.update_from_model();
        if !u.is_empty() {
            app.update_note = Some(u);
        }
    }
}

/// The `*.toml` files in `dir` that parse as a scenario, in name order. Other TOML files
/// (a `Cargo.toml`, say) are skipped.
fn find_scenarios(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                .filter(|p| {
                    std::fs::read_to_string(p)
                        .is_ok_and(|t| sim_core::Scenario::from_toml(&t).is_ok())
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
