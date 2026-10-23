//! Diagram view (spec §7.5): the BPMNDI layout drawn with the egui painter, with pan and zoom,
//! a heat-map overlay and token replay from the event log.

use std::collections::BTreeMap;

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use sim_core::diagram::{HeatMetric, Occupancy, heat, snapshot};
use sim_core::{BpmnFile, EventDef, NodeKind, Project, QId, ReplicationResult};

use crate::app::App;

pub struct DiagramView {
    pub file: Option<String>,
    pub zoom: f32,
    pub pan: Vec2,
    pub fit: bool,
    pub metric: Option<HeatMetric>,
    pub replay: bool,
    pub playing: bool,
    pub time: f64,
}

impl Default for DiagramView {
    fn default() -> Self {
        Self {
            file: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            metric: None,
            replay: false,
            playing: false,
            time: 0.0,
        }
    }
}

const METRICS: [(Option<HeatMetric>, &str); 5] = [
    (None, "no heat map"),
    (Some(HeatMetric::Waiting), "waiting time"),
    (Some(HeatMetric::Processing), "processing time"),
    (Some(HeatMetric::Runs), "runs"),
    (Some(HeatMetric::Utilisation), "utilisation"),
];

/// Green (low) to red (high).
fn heat_color(f: f32) -> Color32 {
    let f = f.clamp(0.0, 1.0);
    let (r, g) = if f < 0.5 {
        (2.0 * f, 1.0)
    } else {
        (1.0, 2.0 * (1.0 - f))
    };
    Color32::from_rgba_unmultiplied((r * 230.0) as u8, (g * 200.0) as u8, 60, 170)
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let project = app.ws.project.clone();
    let files: Vec<String> = project
        .files
        .iter()
        .filter(|(_, f)| !f.diagram.shapes.is_empty())
        .map(|(n, _)| n.clone())
        .collect();
    if files.is_empty() {
        ui.label("None of the loaded files contains diagram information (BPMNDI).");
        return;
    }
    let v = &mut app.diagram;
    if v.file.as_ref().is_none_or(|f| !files.contains(f)) {
        v.file = files.first().cloned();
        v.fit = true;
    }
    let result = app.results.as_ref().and_then(|r| r.data.get(app.rep));
    let unit = app.results.as_ref().map_or(60.0, |r| r.unit.seconds());

    ui.horizontal(|ui| {
        let current = v.file.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("diagram-file")
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for f in &files {
                    if ui.selectable_label(*f == current, f).clicked() {
                        v.file = Some(f.clone());
                        v.fit = true;
                    }
                }
            });
        if ui.button("Fit").clicked() {
            v.fit = true;
        }
        ui.separator();
        ui.add_enabled_ui(result.is_some(), |ui| {
            let label = METRICS
                .iter()
                .find(|(m, _)| *m == v.metric)
                .map_or("", |(_, l)| l);
            egui::ComboBox::from_id_salt("heat")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (m, l) in METRICS {
                        ui.selectable_value(&mut v.metric, m, l);
                    }
                });
            ui.checkbox(&mut v.replay, "token replay");
        });
        if result.is_none() {
            ui.label("(run a simulation for heat map and replay)");
        }
    });
    let end = result.map_or(0.0, |r| r.end_time);
    if v.replay && result.is_some() {
        ui.horizontal(|ui| {
            if ui.button(if v.playing { "⏸" } else { "▶" }).clicked() {
                v.playing = !v.playing;
            }
            ui.add(egui::Slider::new(&mut v.time, 0.0..=end.max(1.0)).show_value(false));
            ui.label(format!(
                "t = {:.1} {}",
                v.time / unit,
                app.results.as_ref().map_or("", |r| r.unit.name())
            ));
        });
        if v.playing {
            v.time += end / 600.0;
            if v.time >= end {
                v.time = end;
                v.playing = false;
            }
            ui.ctx().request_repaint();
        }
    }

    let Some(file) = v.file.clone().and_then(|f| project.files.get(&f)) else {
        return;
    };
    let heat_values = match (v.metric, result) {
        (Some(m), Some(r)) => heat(&project, &file.name, r, m),
        _ => BTreeMap::new(),
    };
    let occupancy = match (v.replay, result) {
        (true, Some(r)) => snapshot(r, v.time),
        _ => BTreeMap::new(),
    };
    let hovered = draw(ui, v, &project, file, &heat_values, &occupancy);
    if let (Some(id), Some(r)) = (hovered, result) {
        tooltip(ui, &project, r, &id, unit);
    }
    if !heat_values.is_empty() {
        let max = heat_values.values().cloned().fold(0.0, f64::max);
        let shown = match v.metric {
            Some(HeatMetric::Waiting | HeatMetric::Processing) => format!("{:.2}", max / unit),
            Some(HeatMetric::Utilisation) => format!("{:.0} %", max * 100.0),
            _ => format!("{max:.0}"),
        };
        ui.label(format!("heat map: green = 0, red = {shown}"));
    }
}

/// Draw the diagram; returns the element under the pointer.
fn draw(
    ui: &mut Ui,
    v: &mut DiagramView,
    project: &Project,
    file: &BpmnFile,
    heat_values: &BTreeMap<QId, f64>,
    occupancy: &BTreeMap<QId, Occupancy>,
) -> Option<QId> {
    let size = ui.available_size().max(Vec2::new(200.0, 200.0));
    let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let area = response.rect;
    let d = &file.diagram;

    // World bounding box.
    let (mut lo, mut hi) = (Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN));
    for b in d.shapes.values() {
        lo = lo.min(Pos2::new(b.x as f32, b.y as f32));
        hi = hi.max(Pos2::new((b.x + b.width) as f32, (b.y + b.height) as f32));
    }
    if v.fit && hi.x > lo.x && hi.y > lo.y {
        let w = (hi - lo).max(Vec2::splat(1.0));
        v.zoom = (area.width() / w.x).min(area.height() / w.y) * 0.95;
        v.pan = (area.size() - w * v.zoom) / 2.0 - lo.to_vec2() * v.zoom;
        v.fit = false;
    }
    if response.dragged() {
        v.pan += response.drag_delta();
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0
            && let Some(p) = response.hover_pos()
        {
            let factor = (scroll * 0.002).exp();
            let local = p - area.min;
            v.pan = local - (local - v.pan) * factor;
            v.zoom = (v.zoom * factor).clamp(0.05, 20.0);
        }
    }
    let (zoom, pan) = (v.zoom, v.pan);
    let to_screen = |x: f64, y: f64| area.min + pan + Vec2::new(x as f32, y as f32) * zoom;
    let rect_of = |b: &sim_core::Bounds| {
        Rect::from_min_max(
            to_screen(b.x, b.y),
            to_screen(b.x + b.width, b.y + b.height),
        )
    };
    let painter = painter.with_clip_rect(area);
    let ink = ui.visuals().text_color();
    let paper = ui.visuals().extreme_bg_color;
    let thin = Stroke::new(1.0, ink);
    let font = FontId::proportional((11.0 * zoom).clamp(6.0, 28.0));
    let max_heat = heat_values.values().cloned().fold(0.0, f64::max);
    let heat_of = |id: &QId| {
        heat_values.get(id).map(|h| {
            heat_color(if max_heat > 0.0 {
                (*h / max_heat) as f32
            } else {
                0.0
            })
        })
    };

    painter.rect_filled(area, 0.0, paper);

    // Pools and lanes first (they are containers).
    for (id, b) in &d.shapes {
        let is_pool = file.participants.contains_key(id);
        let is_lane = file.lanes.contains_key(id);
        if !is_pool && !is_lane {
            continue;
        }
        let r = rect_of(b);
        if let Some(c) = heat_of(id) {
            painter.rect_filled(r, 0.0, c.gamma_multiply(0.35));
        }
        painter.rect_stroke(r, 0.0, thin, StrokeKind::Inside);
        let name = if is_pool {
            file.participants.get(id).and_then(|p| p.name.clone())
        } else {
            file.lanes.get(id).cloned()
        };
        if let Some(n) = name {
            painter.text(
                r.left_center() + Vec2::new(4.0 * zoom, 0.0),
                Align2::LEFT_CENTER,
                n,
                font.clone(),
                ink,
            );
        }
    }

    // Flows.
    for (id, points) in &d.edges {
        let pts: Vec<Pos2> = points.iter().map(|(x, y)| to_screen(*x, *y)).collect();
        if file.message_flows.contains_key(id) {
            painter.extend(Shape::dashed_line(
                &pts,
                Stroke::new(1.0, ink.gamma_multiply(0.7)),
                6.0 * zoom,
                4.0 * zoom,
            ));
        } else {
            painter.add(Shape::line(pts.clone(), thin));
        }
        if let [.., a, b] = pts.as_slice() {
            let dir = (*b - *a).normalized() * 8.0 * zoom.min(2.0);
            let side = Vec2::new(-dir.y, dir.x) * 0.5;
            painter.add(Shape::convex_polygon(
                vec![*b, *b - dir + side, *b - dir - side],
                ink,
                Stroke::NONE,
            ));
        }
    }

    // Flow nodes.
    let pointer = response.hover_pos();
    let mut hovered = None;
    for (id, b) in &d.shapes {
        let Some(n) = project.node(id) else { continue };
        let r = rect_of(b);
        if pointer.is_some_and(|p| r.contains(p)) {
            hovered = Some(id.clone());
        }
        let fill = heat_of(id).unwrap_or(paper);
        let label = n.name.clone().unwrap_or_default();
        match &n.kind {
            k if k.is_event() => {
                let end = *k == NodeKind::EndEvent;
                let stroke = Stroke::new(if end { 3.0 } else { 1.0 } * zoom.clamp(0.5, 2.0), ink);
                painter.circle(r.center(), r.width().min(r.height()) / 2.0, fill, stroke);
                if matches!(
                    k,
                    NodeKind::IntermediateCatchEvent
                        | NodeKind::IntermediateThrowEvent
                        | NodeKind::BoundaryEvent
                ) {
                    painter.circle_stroke(
                        r.center(),
                        r.width().min(r.height()) / 2.0 - 3.0 * zoom,
                        thin,
                    );
                }
                let symbol = match n.event_defs.first() {
                    Some(EventDef::Message(_)) => "✉",
                    Some(EventDef::Timer) => "⏱",
                    Some(EventDef::Signal(_)) => "△",
                    Some(EventDef::Link(_)) => "➜",
                    Some(EventDef::Terminate) => "●",
                    Some(EventDef::Error) => "⚡",
                    _ => "",
                };
                painter.text(r.center(), Align2::CENTER_CENTER, symbol, font.clone(), ink);
                if !label.is_empty() {
                    painter.text(
                        r.center_bottom() + Vec2::new(0.0, 2.0),
                        Align2::CENTER_TOP,
                        label,
                        font.clone(),
                        ink,
                    );
                }
            }
            k if k.is_gateway() => {
                let c = r.center();
                let (hw, hh) = (r.width() / 2.0, r.height() / 2.0);
                let diamond = vec![
                    c - Vec2::new(0.0, hh),
                    c + Vec2::new(hw, 0.0),
                    c + Vec2::new(0.0, hh),
                    c - Vec2::new(hw, 0.0),
                ];
                painter.add(Shape::convex_polygon(diamond, fill, thin));
                let symbol = match k {
                    NodeKind::ParallelGateway => "+",
                    NodeKind::InclusiveGateway => "○",
                    NodeKind::EventBasedGateway => "⬠",
                    _ => "×",
                };
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    symbol,
                    FontId::proportional((18.0 * zoom).clamp(6.0, 40.0)),
                    ink,
                );
                if !label.is_empty() {
                    painter.text(
                        r.center_bottom() + Vec2::new(0.0, 2.0),
                        Align2::CENTER_TOP,
                        label,
                        font.clone(),
                        ink,
                    );
                }
            }
            _ => {
                let rounding = 8.0 * zoom;
                let stroke = Stroke::new(
                    if n.kind == NodeKind::CallActivity {
                        3.0
                    } else {
                        1.0
                    },
                    ink,
                );
                // Expanded subprocesses only get an outline: their content is drawn on top.
                let fill = if n.kind == NodeKind::SubProcess {
                    Color32::TRANSPARENT
                } else {
                    fill
                };
                painter.rect(r, rounding, fill, stroke, StrokeKind::Inside);
                if n.kind == NodeKind::SubProcess {
                    painter.text(
                        r.left_top() + Vec2::splat(4.0 * zoom),
                        Align2::LEFT_TOP,
                        label,
                        font.clone(),
                        ink,
                    );
                } else {
                    let galley =
                        painter.layout(label, font.clone(), ink, (r.width() - 6.0).max(10.0));
                    painter.galley(r.center() - galley.size() / 2.0, galley, ink);
                }
            }
        }
        // Replay: tokens waiting for a resource (orange) and working (blue).
        if let Some(o) = occupancy.get(id) {
            let badge = |offset: f32, count: usize, color: Color32| {
                if count > 0 {
                    let c = r.right_top() + Vec2::new(-offset * zoom, 0.0);
                    painter.circle_filled(c, 8.0 * zoom.clamp(0.6, 2.0), color);
                    painter.text(
                        c,
                        Align2::CENTER_CENTER,
                        count.to_string(),
                        FontId::proportional((10.0 * zoom).clamp(6.0, 20.0)),
                        Color32::WHITE,
                    );
                }
            };
            badge(0.0, o.working, Color32::from_rgb(40, 110, 220));
            badge(20.0, o.waiting, Color32::from_rgb(230, 130, 20));
        }
    }
    hovered
}

fn tooltip(ui: &Ui, project: &Project, r: &ReplicationResult, id: &QId, unit: f64) {
    let Some(n) = project.node(id) else { return };
    let a = r.activities.get(id);
    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        egui::Id::unique("diagram-tip"),
        egui::PopupAnchor::Pointer,
    )
    .show(|ui| {
        ui.label(format!("{} ({})", n.label(), id));
        if let Some(a) = a {
            ui.label(format!("runs {}, cancelled {}", a.runs, a.cancelled));
            ui.label(format!(
                "waiting {:.2}, processing {:.2}",
                a.waiting.mean / unit,
                a.processing.mean / unit
            ));
        }
        if let Some(c) = r.end_events.get(id) {
            ui.label(format!("{c} token(s) ended here"));
        }
        if let Some(f) = r.boundary_fired.get(id) {
            ui.label(format!("fired {f} time(s)"));
        }
    });
}
