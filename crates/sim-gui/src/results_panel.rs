use eframe::egui::{self, RichText, Ui};
use egui_plot::{Bar, BarChart, Plot};
use sim_core::{ReplicationResult, Summary, export};

use crate::app::{App, ResultsTab};

pub fn show(app: &mut App, ui: &mut Ui) {
    let Some(res) = &app.results else {
        ui.label("No results yet. Run a simulation first.");
        return;
    };
    let unit = res.unit.seconds();
    let uname = res.unit.name();
    app.rep = app.rep.min(res.data.len().saturating_sub(1));

    ui.horizontal(|ui| {
        if res.data.len() > 1 {
            ui.label("replication:");
            for (i, r) in res.data.iter().enumerate() {
                ui.selectable_value(&mut app.rep, i, format!("#{} (seed {})", i + 1, r.seed));
            }
            ui.separator();
        }
        if ui.button("Export JSON…").clicked() {
            save(
                ui,
                "results.json",
                || serde_json::to_string_pretty(&res.data).map_err(|e| e.to_string()),
                &mut app.status,
            );
        }
        if ui.button("Export summary CSV…").clicked() {
            save(
                ui,
                "summary.csv",
                || Ok(export::summary_csv(&res.project, &res.data)),
                &mut app.status,
            );
        }
        if ui.button("Export event log CSV…").clicked() {
            save(
                ui,
                "event-log.csv",
                || Ok(export::event_log_csv(&res.project, &res.data)),
                &mut app.status,
            );
        }
    });
    ui.horizontal(|ui| {
        for (t, n) in [
            (ResultsTab::Summary, "Summary"),
            (ResultsTab::Cases, "Cases"),
            (ResultsTab::Activities, "Activities"),
            (ResultsTab::Resources, "Resources"),
            (ResultsTab::Problems, "Problems"),
        ] {
            ui.selectable_value(&mut app.results_tab, t, n);
        }
    });
    ui.separator();

    let Some(r) = res.data.get(app.rep) else {
        return;
    };
    let t = |s: f64| format!("{:.2}", s / unit);
    egui::ScrollArea::vertical().show(ui, |ui| match app.results_tab {
        ResultsTab::Summary => {
            egui::Grid::new("summary").striped(true).show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                };
                row("seed", r.seed.to_string());
                row(
                    "cases started / completed",
                    format!("{} / {}", r.cases_started, r.cases_completed),
                );
                row(&format!("run length ({uname})"), t(r.end_time));
                row(
                    &format!("mean cycle time ({uname})"),
                    t(r.case_cycle_time.mean),
                );
                row("mean case cost", format!("{:.2}", r.case_cost.mean));
                row("stuck tokens", r.stuck.len().to_string());
            });
            ui.heading("End events");
            for (id, n) in &r.end_events {
                ui.label(format!("{id}: {n}"));
            }
        }
        ResultsTab::Cases => {
            summary_table(
                ui,
                &format!("Cycle time ({uname})"),
                &r.case_cycle_time,
                unit,
            );
            histogram(ui, r, unit, uname);
        }
        ResultsTab::Activities => {
            egui::Grid::new("acts").striped(true).show(ui, |ui| {
                for h in [
                    "activity",
                    "runs",
                    "cancelled",
                    &format!("wait ({uname})"),
                    &format!("processing ({uname})"),
                    "cost",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (id, a) in &r.activities {
                    let name = res
                        .project
                        .node(id)
                        .map(|n| n.label().to_string())
                        .unwrap_or_default();
                    ui.label(format!("{id}  {name}"));
                    ui.label(a.runs.to_string());
                    ui.label(a.cancelled.to_string());
                    ui.label(t(a.waiting.mean));
                    ui.label(t(a.processing.mean));
                    ui.label(format!("{:.2}", a.total_cost));
                    ui.end_row();
                }
            });
            let bars = r
                .activities
                .iter()
                .enumerate()
                .map(|(i, (id, a))| {
                    Bar::new(i as f64, a.waiting.mean / unit)
                        .name(id.to_string())
                        .width(0.7)
                })
                .collect();
            Plot::new("wait")
                .height(220.0)
                .y_axis_label(format!("mean wait ({uname})"))
                .show(ui, |p| p.bar_chart(BarChart::new("waiting", bars)));
        }
        ResultsTab::Resources => {
            egui::Grid::new("res").striped(true).show(ui, |ui| {
                for h in [
                    "pool",
                    "count",
                    "tasks",
                    "utilisation",
                    &format!("busy ({uname})"),
                    "cost",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (name, p) in &r.resources {
                    ui.label(name);
                    ui.label(p.count.to_string());
                    ui.label(p.tasks.to_string());
                    ui.label(format!("{:.1}%", p.utilisation * 100.0));
                    ui.label(t(p.busy_time));
                    ui.label(format!("{:.2}", p.cost));
                    ui.end_row();
                }
            });
            let bars = r
                .resources
                .iter()
                .enumerate()
                .map(|(i, (n, p))| {
                    Bar::new(i as f64, p.utilisation * 100.0)
                        .name(n.as_str())
                        .width(0.7)
                })
                .collect();
            Plot::new("util")
                .height(220.0)
                .y_axis_label("utilisation %")
                .include_y(100.0)
                .show(ui, |p| p.bar_chart(BarChart::new("utilisation", bars)));
        }
        ResultsTab::Problems => {
            if r.stuck.is_empty() {
                ui.label("No stuck instances: every token reached an end event.");
            } else {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!(
                        "{} token(s) were still waiting when the run ended:",
                        r.stuck.len()
                    ),
                );
                egui::Grid::new("stuck").striped(true).show(ui, |ui| {
                    for s in &r.stuck {
                        ui.label(format!("case {}", s.case));
                        ui.label(s.process.to_string());
                        ui.label(format!("{} {}", s.element, s.name.as_deref().unwrap_or("")));
                        ui.label(&s.state);
                        ui.end_row();
                    }
                });
            }
            if r.messages_lost > 0 {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!(
                        "{} lost message(s) (nobody was waiting and buffering is off):",
                        r.messages_lost
                    ),
                );
                for m in r
                    .messages
                    .iter()
                    .filter(|m| m.outcome == sim_core::MessageOutcome::Lost)
                {
                    ui.label(format!(
                        "t={} case {}: {} → {}",
                        t(m.time),
                        m.case,
                        m.source,
                        m.target
                    ));
                }
            }
            if !r.undelivered.is_empty() {
                ui.colored_label(egui::Color32::YELLOW, "Buffered messages nobody picked up:");
                for u in &r.undelivered {
                    ui.label(format!("case {}: {} × {}", u.case, u.count, u.target));
                }
            }
            let missed = r.signals.iter().filter(|s| s.receivers == 0).count();
            if missed > 0 {
                ui.label(format!(
                    "{missed} signal broadcast(s) reached nobody (signals are not stored)."
                ));
            }
            if !r.boundary_fired.is_empty() {
                ui.heading("Boundary events fired");
                for (id, n) in &r.boundary_fired {
                    ui.label(format!("{id}: {n}"));
                }
            }
            let incomplete = r.cases_started - r.cases_completed;
            if incomplete > 0 {
                ui.label(format!("{incomplete} case(s) did not complete."));
            }
        }
    });
}

fn summary_table(ui: &mut Ui, title: &str, s: &Summary, unit: f64) {
    ui.heading(title);
    egui::Grid::new(title).show(ui, |ui| {
        for (k, v) in [
            ("count", s.count as f64 * unit),
            ("mean", s.mean),
            ("std dev", s.std_dev),
            ("min", s.min),
            ("median", s.median),
            ("p90", s.p90),
            ("max", s.max),
        ] {
            ui.label(k);
            ui.label(format!("{:.2}", v / unit));
            ui.end_row();
        }
    });
}

fn histogram(ui: &mut Ui, r: &ReplicationResult, unit: f64, uname: &str) {
    let times: Vec<f64> = r
        .cases
        .iter()
        .filter(|c| !c.warm_up)
        .filter_map(|c| c.completion.map(|e| (e - c.arrival) / unit))
        .collect();
    if times.is_empty() {
        return;
    }
    let (lo, hi) = times
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    let bins = ((times.len() as f64).sqrt().ceil() as usize).clamp(1, 60);
    let width = ((hi - lo) / bins as f64).max(1e-9);
    let mut counts = vec![0usize; bins];
    for x in &times {
        counts[(((x - lo) / width) as usize).min(bins - 1)] += 1;
    }
    let bars = counts
        .iter()
        .enumerate()
        .map(|(i, &c)| Bar::new(lo + (i as f64 + 0.5) * width, c as f64).width(width * 0.95))
        .collect();
    ui.heading("Cycle time histogram");
    Plot::new("hist")
        .height(240.0)
        .x_axis_label(format!("cycle time ({uname})"))
        .show(ui, |p| p.bar_chart(BarChart::new("cases", bars)));
}

fn save(
    ui: &Ui,
    name: &str,
    content: impl FnOnce() -> Result<String, String>,
    status: &mut String,
) {
    let _ = ui;
    if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
        *status = match content().and_then(|c| std::fs::write(&path, c).map_err(|e| e.to_string()))
        {
            Ok(()) => format!("Saved {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }
}
