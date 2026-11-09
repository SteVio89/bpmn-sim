use eframe::egui::{self, RichText, Ui};
use egui_plot::{Bar, BarChart, Legend, Line, Plot};
use sim_core::insights::{Finding, Level, Load, Topic};
use sim_core::{Insights, Project, QId, ReplicationResult, Summary, export};

use crate::app::{App, CaseSort, ResultsTab};

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
        if ui.button("Export event log XES…").clicked() {
            save(
                ui,
                "event-log.xes",
                || Ok(export::event_log_xes(&res.project, &res.data)),
                &mut app.status,
            );
        }
        if res.data.len() > 1 && ui.button("Export aggregate CSV…").clicked() {
            save(
                ui,
                "aggregate.csv",
                || {
                    Ok(export::aggregate_csv(
                        &res.project,
                        &sim_core::aggregate(&res.data),
                    ))
                },
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
            (ResultsTab::Replications, "Replications"),
        ] {
            ui.selectable_value(&mut app.results_tab, t, n);
        }
    });
    ui.separator();

    let (Some(r), Some(ins)) = (res.data.get(app.rep), res.insights.get(app.rep)) else {
        return;
    };
    let t = |s: f64| format!("{:.2}", s / unit);
    egui::ScrollArea::vertical().show(ui, |ui| match app.results_tab {
        ResultsTab::Summary => {
            ui.heading("Findings");
            findings(ui, &ins.findings(r), None);
            ui.add_space(8.0);
            ui.heading("Figures");
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
                if !r.failed.is_empty() {
                    row("failed cases", r.failed.len().to_string());
                }
                if r.horizon_reached {
                    row(
                        "stopped at horizon",
                        format!("{} token(s) unfinished", r.unfinished.len()),
                    );
                }
            });
            if !r.processes.is_empty() {
                ui.heading("Process instances");
                egui::Grid::new("procs").striped(true).show(ui, |ui| {
                    for h in [
                        "process",
                        "instances",
                        &format!("mean cycle time ({uname})"),
                        &format!("p90 ({uname})"),
                    ] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (id, p) in &r.processes {
                        ui.label(id.to_string());
                        ui.label(p.instances.to_string());
                        ui.label(t(p.cycle_time.mean));
                        ui.label(t(p.cycle_time.p90));
                        ui.end_row();
                    }
                });
            }
            if !r.decisions.is_empty() {
                ui.heading("Decisions");
                for (id, d) in &r.decisions {
                    let outcomes: Vec<String> = d.outcomes.iter().map(|(o, n)| format!("{o}: {n}")).collect();
                    ui.label(format!("{id}: {} evaluations — {}", d.evaluations, outcomes.join(", ")));
                }
            }
            ui.heading("End events");
            for (id, n) in &r.end_events {
                ui.label(format!("{id}: {n}"));
            }
        }
        ResultsTab::Cases => cases_tab(
            ui,
            r,
            ins,
            &res.project,
            unit,
            uname,
            &mut app.case_sort,
            &mut app.selected_case,
        ),
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
        ResultsTab::Resources => resources_tab(ui, r, ins, &res.project, unit, uname),
        ResultsTab::Replications => replications(ui, &res.data, res.unit.seconds(), uname),
        ResultsTab::Problems => {
            if r.horizon_reached {
                ui.label(format!(
                    "The run was cut off at the horizon; {} token(s) were still unfinished (not a deadlock).",
                    r.unfinished.len()
                ));
            }
            if !r.failed.is_empty() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!(
                        "{} case(s) failed through an incident; their instances halted with their tokens:",
                        r.failed.len()
                    ),
                );
                egui::Grid::new("failed").striped(true).show(ui, |ui| {
                    for f in &r.failed {
                        ui.label(format!("case {}", f.case));
                        ui.label(format!("t={}", t(f.time)));
                        ui.label(format!("{} {}", f.element, f.name.as_deref().unwrap_or("")));
                        ui.label(format!("failed: {}", f.reason));
                        ui.end_row();
                    }
                });
            }
            if !r.uncaught_escalations.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Escalations nothing caught (they have no effect):",
                );
                for e in &r.uncaught_escalations {
                    ui.label(format!(
                        "t={} case {}: '{}' at {}",
                        t(e.time),
                        e.case,
                        e.code,
                        e.element
                    ));
                }
            }
            if r.stuck.is_empty() {
                ui.label("No stuck instances (deadlocks): every token of the other cases reached an end event.");
            } else {
                ui.colored_label(
                    ui.visuals().error_fg_color,
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
                    ui.visuals().error_fg_color,
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
                        "t={} case {}: {} -> {}",
                        t(m.time),
                        m.case,
                        m.source,
                        m.target
                    ));
                }
            }
            if !r.undelivered.is_empty() {
                ui.colored_label(ui.visuals().warn_fg_color, "Buffered messages nobody picked up:");
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
            for (id, d) in r.decisions.iter().filter(|(_, d)| d.no_match > 0 || d.violations > 0) {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "decision {id}: {} evaluation(s) matched no rule, {} violated the hit policy",
                        d.no_match, d.violations
                    ),
                );
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
        ui.label("count");
        ui.label(s.count.to_string());
        ui.end_row();
        for (k, v) in [
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
    match crate::platform::save(name, None, content) {
        Some(Ok(path)) => *status = format!("Saved {}", path.display()),
        Some(Err(e)) => *status = format!("Export failed: {e}"),
        None => {}
    }
}

/// Mean and 95% confidence interval across replications.
fn replications(ui: &mut Ui, data: &[ReplicationResult], unit: f64, uname: &str) {
    if data.len() < 2 {
        ui.label("Run at least 2 replications to get confidence intervals.");
        return;
    }
    let a = sim_core::aggregate(data);
    let pm = |e: &sim_core::Estimate, scale: f64| match e.ci95 {
        Some(c) => format!("{:.2} ± {:.2}", e.mean / scale, c / scale),
        None => format!("{:.2}", e.mean / scale),
    };
    ui.label(format!(
        "{} replications, mean ± half width of the 95% confidence interval",
        a.replications
    ));
    egui::Grid::new("agg").striped(true).show(ui, |ui| {
        let mut row = |k: String, v: String| {
            ui.label(k);
            ui.label(v);
            ui.end_row();
        };
        row("cases completed".into(), pm(&a.cases_completed, 1.0));
        row(
            format!("cycle time, mean ({uname})"),
            pm(&a.case_cycle_time_mean, unit),
        );
        row(
            format!("cycle time, median ({uname})"),
            pm(&a.case_cycle_time_median, unit),
        );
        row(
            format!("cycle time, p90 ({uname})"),
            pm(&a.case_cycle_time_p90, unit),
        );
        row("case cost, mean".into(), pm(&a.case_cost_mean, 1.0));
        for (id, x) in &a.activities {
            row(format!("{id}: wait ({uname})"), pm(&x.waiting_mean, unit));
            row(
                format!("{id}: processing ({uname})"),
                pm(&x.processing_mean, unit),
            );
        }
        for (name, x) in &a.resources {
            row(format!("{name}: utilisation (%)"), pm(&x.utilisation, 0.01));
        }
        for (id, x) in &a.process_cycle_time_mean {
            row(format!("{id}: instance cycle time ({uname})"), pm(x, unit));
        }
    });
}

/// Findings in plain language, problems first; `topic` limits them to one tab.
fn findings(ui: &mut Ui, list: &[Finding], topic: Option<Topic>) {
    let mut any = false;
    for f in list.iter().filter(|f| topic.is_none_or(|t| f.topic == t)) {
        any = true;
        let (prefix, color) = match f.level {
            Level::Problem => ("Problem: ", Some(ui.visuals().error_fg_color)),
            Level::Warning => ("Warning: ", Some(ui.visuals().warn_fg_color)),
            Level::Info => ("", None),
        };
        let mut text = RichText::new(format!("{prefix}{}", f.text));
        if let Some(c) = color {
            text = text.color(c);
        }
        ui.horizontal_top(|ui| {
            ui.label("•");
            ui.add(egui::Label::new(text).wrap());
        });
    }
    if !any && topic.is_none() {
        ui.label("Nothing to report.");
    }
}

fn activity_name(project: &Project, id: &QId) -> String {
    project
        .node(id)
        .map(|n| n.label().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| id.id.to_string())
}

/// A share as a small bar with its percentage.
fn share_bar(ui: &mut Ui, part: f64, whole: f64) {
    let f = if whole > 0.0 {
        (part / whole) as f32
    } else {
        0.0
    };
    ui.add(
        egui::ProgressBar::new(f.clamp(0.0, 1.0))
            .desired_width(160.0)
            .text(format!("{:.0}%", 100.0 * f)),
    );
}

#[allow(clippy::too_many_arguments)]
fn cases_tab(
    ui: &mut Ui,
    r: &ReplicationResult,
    ins: &Insights,
    project: &Project,
    unit: f64,
    uname: &str,
    sort: &mut CaseSort,
    selected: &mut Option<u64>,
) {
    let t = |s: f64| format!("{:.2}", s / unit);
    findings(ui, &ins.findings(r), Some(Topic::Cases));
    ui.add_space(8.0);

    let b = &ins.time;
    if b.cases > 0 {
        ui.heading("Where the time of an average case goes");
        egui::Grid::new("breakdown").striped(true).show(ui, |ui| {
            for (k, v, hint) in [
                (
                    "worked on",
                    b.work,
                    "working time of its tasks: shortened by faster work",
                ),
                (
                    "waiting for a resource",
                    b.wait,
                    "tasks waited for a free person (or for working hours to begin): \
                     shortened by more people in the pool the case waits for",
                ),
                (
                    "paused outside working hours",
                    b.paused,
                    "tasks started but interrupted by the end of the working day",
                ),
                (
                    "other",
                    b.other,
                    "timers, messages, other pools, tasks without a resource",
                ),
            ] {
                if v <= 0.005 * b.cycle && k != "worked on" {
                    continue;
                }
                ui.label(k).on_hover_text(hint);
                ui.label(format!("{} {uname}", t(v)));
                share_bar(ui, v, b.cycle);
                ui.end_row();
            }
            ui.label(RichText::new("cycle time").strong());
            ui.label(RichText::new(format!("{} {uname}", t(b.cycle))).strong());
            ui.end_row();
        });
        if b.parallel {
            ui.label(
                RichText::new(
                    "Tasks of a case run in parallel, so the parts add up to more than the \
                     cycle time.",
                )
                .weak(),
            );
        }
        ui.add_space(8.0);
    }

    ui.columns(2, |cols| {
        summary_table(
            &mut cols[0],
            &format!("Cycle time ({uname})"),
            &r.case_cycle_time,
            unit,
        );
        cols[0].label(
            RichText::new(
                "From arrival to the end of the case; warm-up and unfinished cases are left out. \
                 The median is a typical case, p90 what 9 in 10 cases manage.",
            )
            .weak(),
        );
        histogram(&mut cols[1], r, unit, uname);
    });

    if !ins.wip.is_empty() {
        ui.heading("Cases in progress over time");
        ui.label(
            RichText::new(
                "How many cases were open at once. A level that settles means the process \
                 keeps up; a line that keeps rising while cases arrive means queues grow.",
            )
            .weak(),
        );
        let points: Vec<[f64; 2]> = ins.wip.iter().map(|p| [p[0] / unit, p[1]]).collect();
        Plot::new("wip")
            .height(180.0)
            .x_axis_label(format!("simulated time ({uname})"))
            .y_axis_label("open cases")
            .include_y(0.0)
            .show(ui, |p| p.line(Line::new("open cases", points)));
    }

    ui.add_space(8.0);
    ui.heading("Individual cases");
    ui.horizontal(|ui| {
        ui.label("show first:");
        for (s, n) in [
            (CaseSort::Slowest, "slowest"),
            (CaseSort::MostWaiting, "most waiting"),
            (CaseSort::MostExpensive, "most expensive"),
            (CaseSort::Arrival, "in order of arrival"),
        ] {
            ui.selectable_value(sort, s, n);
        }
    });
    ui.label(RichText::new("Click a case to see its tasks.").weak());
    let mut rows: Vec<&sim_core::insights::CaseRow> = ins.cases.iter().collect();
    let open = |c: &sim_core::insights::CaseRow| c.cycle_time().unwrap_or(r.end_time - c.arrival);
    match sort {
        CaseSort::Slowest => rows.sort_by(|a, b| open(b).total_cmp(&open(a))),
        CaseSort::MostWaiting => rows.sort_by(|a, b| b.wait.total_cmp(&a.wait)),
        CaseSort::MostExpensive => rows.sort_by(|a, b| b.cost.total_cmp(&a.cost)),
        CaseSort::Arrival => {}
    }
    const SHOWN: usize = 200;
    egui::ScrollArea::vertical()
        .id_salt("case-list-scroll")
        .max_height(320.0)
        .show(ui, |ui| {
            egui::Grid::new("case-list").striped(true).show(ui, |ui| {
                for h in [
                    "case".to_string(),
                    "status".into(),
                    format!("arrival ({uname})"),
                    format!("cycle time ({uname})"),
                    format!("worked ({uname})"),
                    format!("waited ({uname})"),
                    "cost".into(),
                    "waited longest at".into(),
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for c in rows.iter().take(SHOWN) {
                    let on = *selected == Some(c.id);
                    if ui.selectable_label(on, format!("case {}", c.id)).clicked() {
                        *selected = if on { None } else { Some(c.id) };
                    }
                    ui.label(if c.failed {
                        "failed"
                    } else if c.completion.is_none() {
                        "open at end"
                    } else if c.warm_up {
                        "done (warm-up)"
                    } else {
                        "done"
                    });
                    ui.label(t(c.arrival));
                    ui.label(match c.cycle_time() {
                        Some(x) => t(x),
                        None => format!("> {}", t(r.end_time - c.arrival)),
                    });
                    ui.label(t(c.work));
                    ui.label(t(c.wait));
                    ui.label(format!("{:.2}", c.cost));
                    ui.label(match &c.longest_wait {
                        Some((a, w)) => format!("{} ({})", activity_name(project, a), t(*w)),
                        None => String::new(),
                    });
                    ui.end_row();
                }
            });
        });
    if rows.len() > SHOWN {
        ui.label(
            RichText::new(format!(
                "{} more; all cases are in the event log export.",
                rows.len() - SHOWN
            ))
            .weak(),
        );
    }

    if let Some(id) = *selected
        && let Some(c) = ins.cases.iter().find(|c| c.id == id)
    {
        ui.add_space(8.0);
        ui.heading(format!("Tasks of case {id}"));
        let mut recs: Vec<&sim_core::ActivityRecord> =
            r.records.iter().filter(|x| x.case == id).collect();
        recs.sort_by(|a, b| a.queued.total_cmp(&b.queued));
        if recs.is_empty() {
            ui.label("No task of this case finished.");
        }
        egui::Grid::new("case-tasks").striped(true).show(ui, |ui| {
            for h in [
                "task".to_string(),
                "resource".into(),
                format!("reached ({uname})"),
                format!("waited ({uname})"),
                format!("worked ({uname})"),
                format!("finished ({uname})"),
                "cost".into(),
            ] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for x in recs {
                let name = activity_name(project, &x.activity);
                ui.label(if x.cancelled {
                    format!("{name} (cancelled)")
                } else {
                    name
                });
                ui.label(x.resource.as_deref().unwrap_or("-"));
                ui.label(t(x.queued));
                ui.label(t(x.waiting()));
                ui.label(t(x.work));
                ui.label(t(x.end));
                ui.label(format!("{:.2}", x.cost));
                ui.end_row();
            }
        });
        if let Some(end) = c.completion {
            ui.label(format!(
                "Arrived at {}, finished at {}: {} {uname} in total.",
                t(c.arrival),
                t(end),
                t(end - c.arrival)
            ));
        }
    }
}

fn resources_tab(
    ui: &mut Ui,
    r: &ReplicationResult,
    ins: &Insights,
    project: &Project,
    unit: f64,
    uname: &str,
) {
    let t = |s: f64| format!("{:.2}", s / unit);
    if ins.pools.is_empty() {
        ui.label("The scenario has no resource pools.");
        return;
    }
    findings(ui, &ins.findings(r), Some(Topic::Resources));
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Utilisation is the share of its working time a pool was busy. Below about 70% \
             queues stay short; above 80% waiting rises steeply; near 100% the queue grows \
             without limit. \"Waited\" is how long tasks waited for a free person of the pool, \
             \"queue\" how many tasks waited at once.",
        )
        .weak(),
    );
    ui.add_space(4.0);
    egui::Grid::new("res").striped(true).show(ui, |ui| {
        for h in [
            "pool".to_string(),
            "people".into(),
            "utilisation".into(),
            "load".into(),
            "tasks".into(),
            format!("waited, mean ({uname})"),
            format!("waited, p90 ({uname})"),
            "queue, mean".into(),
            "queue, max".into(),
            "with one more".into(),
            "with one less".into(),
            "cost".into(),
        ] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for p in &ins.pools {
            let bottleneck = ins.bottleneck.as_deref() == Some(p.name.as_str());
            ui.label(if bottleneck {
                RichText::new(format!("{} (bottleneck)", p.name)).strong()
            } else {
                RichText::new(&p.name)
            });
            ui.label(p.count.to_string());
            share_bar(ui, p.utilisation, 1.0);
            let load = RichText::new(p.load.label());
            ui.label(match p.load {
                Load::Saturated => load.color(ui.visuals().error_fg_color),
                Load::High => load.color(ui.visuals().warn_fg_color),
                _ => load,
            });
            ui.label(p.tasks.to_string());
            ui.label(t(p.wait.mean));
            ui.label(t(p.wait.p90));
            ui.label(format!("{:.2}", p.mean_queue));
            ui.label(p.max_queue.to_string());
            ui.label(format!("{:.0}%", 100.0 * p.utilisation_with(p.count + 1)))
                .on_hover_text("utilisation for the same work with one more person");
            if p.count > 1 {
                ui.label(format!("{:.0}%", 100.0 * p.utilisation_with(p.count - 1)))
                    .on_hover_text("utilisation for the same work with one person less");
            } else {
                ui.label("-");
            }
            ui.label(format!(
                "{:.2}",
                r.resources.get(&p.name).map_or(0.0, |x| x.cost)
            ));
            ui.end_row();
        }
    });
    ui.label(
        RichText::new(
            "\"With one more / less\" is a rough estimate. Waiting times change much more than \
             utilisation: run the scenario again with the new number to see them.",
        )
        .weak(),
    );

    ui.add_space(8.0);
    ui.heading("Tasks waiting over time");
    ui.label(
        RichText::new(
            "A queue that empties again and again is fine; one that keeps rising means the \
             pool cannot keep up.",
        )
        .weak(),
    );
    Plot::new("queues")
        .height(200.0)
        .legend(Legend::default())
        .x_axis_label(format!("simulated time ({uname})"))
        .y_axis_label("tasks waiting")
        .include_y(0.0)
        .show(ui, |plot| {
            for p in &ins.pools {
                let points: Vec<[f64; 2]> = p.queue.iter().map(|q| [q[0] / unit, q[1]]).collect();
                plot.line(Line::new(p.name.clone(), points));
            }
        });

    ui.add_space(8.0);
    ui.heading("What each pool works on");
    for p in &ins.pools {
        let people = if p.count == 1 { "person" } else { "people" };
        egui::CollapsingHeader::new(format!("{} ({} {people})", p.name, p.count))
            .id_salt(("pool-work", &p.name))
            .default_open(ins.bottleneck.as_deref() == Some(p.name.as_str()))
            .show(ui, |ui| {
                if p.activities.is_empty() {
                    ui.label("No task used this pool.");
                    return;
                }
                egui::Grid::new(("pool-acts", &p.name))
                    .striped(true)
                    .show(ui, |ui| {
                        for h in [
                            "task".to_string(),
                            "runs".into(),
                            format!("waited, mean ({uname})"),
                            format!("worked, mean ({uname})"),
                        ] {
                            ui.label(RichText::new(h).strong());
                        }
                        ui.end_row();
                        for (id, n) in &p.activities {
                            ui.label(activity_name(project, id));
                            ui.label(n.to_string());
                            let a = r.activities.get(id);
                            ui.label(a.map_or(String::new(), |a| t(a.waiting.mean)));
                            ui.label(a.map_or(String::new(), |a| t(a.processing.mean)));
                            ui.end_row();
                        }
                    });
            });
    }
}
