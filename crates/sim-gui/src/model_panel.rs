use std::collections::BTreeMap;

use eframe::egui::{self, RichText, Ui};
use sim_core::{NodeKind, Severity, validate};

use crate::app::App;
use crate::examples::EXAMPLES;
use crate::platform::{self, Purpose};

// The browser cannot look into the folder of an opened file, so there the user selects the
// main model together with the files it calls.
#[cfg(not(target_arch = "wasm32"))]
const OPEN_FILES: &str = "Open file…";
#[cfg(not(target_arch = "wasm32"))]
const OPEN_FILES_HINT: &str = "a BPMN file, with the files it calls from the same folder";
#[cfg(not(target_arch = "wasm32"))]
const MULTIPLE: bool = false;
#[cfg(not(target_arch = "wasm32"))]
const OPEN_FILTER: &platform::Filter = &platform::BPMN;
#[cfg(not(target_arch = "wasm32"))]
const OPEN_HINT: &str = "Open a BPMN file or a folder of models above, or drop files onto \
                         this window. Models from bpmn.io and Camunda Modeler work as they are.";
#[cfg(target_arch = "wasm32")]
const OPEN_FILES: &str = "Open files…";
#[cfg(target_arch = "wasm32")]
const OPEN_HINT: &str = "Open your BPMN files above (with the files they call and their DMN \
                         tables), or drop them onto this page. Models from bpmn.io and Camunda \
                         Modeler work as they are. Files stay in your browser.";
#[cfg(target_arch = "wasm32")]
const OPEN_FILES_HINT: &str = "the BPMN model together with the BPMN and DMN files it calls";
#[cfg(target_arch = "wasm32")]
const MULTIPLE: bool = true;
#[cfg(target_arch = "wasm32")]
const OPEN_FILTER: &platform::Filter = &platform::BPMN_DMN;

/// The built-in examples, each with a short description.
pub fn examples_menu(app: &mut App, ui: &mut Ui) {
    for e in EXAMPLES {
        if ui.button(e.title).on_hover_text(e.description).clicked() {
            app.open_example(e);
            ui.close();
        }
    }
}

/// Shown while no model is open: what the app does and the examples to start with.
fn welcome(app: &mut App, ui: &mut Ui) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.set_max_width(560.0);
        ui.heading("BPMN Sim");
        ui.add_space(8.0);
        ui.label(
            "Simulate BPMN 2.0 process models: cycle times, waiting times, resource \
             utilisation and costs. Find where cases get stuck, and play a case through the \
             diagram by hand.",
        );
        ui.add_space(8.0);
        ui.label(OPEN_HINT);
        ui.add_space(16.0);
        ui.strong("Or start with an example:");
        ui.add_space(4.0);
        for e in EXAMPLES {
            ui.add_space(4.0);
            if ui.button(e.title).clicked() {
                app.open_example(e);
            }
            ui.weak(e.description);
        }
    });
}

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.horizontal(|ui| {
        #[cfg(not(target_arch = "wasm32"))]
        if ui.button("Open folder…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            app.ws.open(&dir);
            app.after_load();
            app.scenario_candidates = find_scenarios(&dir);
            if app.ws.load_error.is_none()
                && let Some(first) = app.scenario_candidates.first().cloned()
            {
                app.load_scenario(first);
            }
        }
        if ui
            .button(OPEN_FILES)
            .on_hover_text(OPEN_FILES_HINT)
            .clicked()
        {
            app.files
                .pick(ui.ctx(), Purpose::OpenModel, OPEN_FILTER, MULTIPLE);
        }
        if ui.button("Add file…").clicked() {
            app.files
                .pick(ui.ctx(), Purpose::AddModel, &platform::BPMN_DMN, true);
        }
        ui.menu_button("Examples", |ui| examples_menu(app, ui));
    });
    if let Some(e) = &app.ws.load_error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    if app.ws.files.is_empty() {
        welcome(app, ui);
        return;
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Files");
        let mut remove = None;
        for (i, f) in app.ws.files.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui
                    .small_button("x")
                    .on_hover_text("Remove from project")
                    .clicked()
                {
                    remove = Some(i);
                }
                ui.label(f.label());
            });
        }
        if let Some(i) = remove {
            app.ws.remove(i);
            app.after_load();
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

#[cfg(not(target_arch = "wasm32"))]
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
