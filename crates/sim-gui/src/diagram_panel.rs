//! Diagram view (spec §7.5): the BPMNDI layout drawn with the egui painter, with pan and zoom,
//! a heat-map overlay, token replay from the event log, and navigation into called processes.

use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_2;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2, epaint::TextShape,
};
use sim_core::diagram::{HeatMetric, Occupancy, heat, snapshot};
use sim_core::{BpmnFile, EventDef, FlowNode, NodeKind, Project, QId, ReplicationResult, TaskKind};

use crate::app::App;
use crate::diagram_icons::{self, Icon};

pub struct DiagramView {
    pub file: Option<String>,
    /// Files we came from by opening call activities (for "Back").
    pub history: Vec<String>,
    pub zoom: f32,
    pub pan: Vec2,
    pub fit: bool,
    pub metric: Option<HeatMetric>,
    pub replay: bool,
    pub playing: bool,
    pub time: f64,
    /// Screen area of the last drawn diagram.
    pub area: Rect,
}

impl Default for DiagramView {
    fn default() -> Self {
        Self {
            file: None,
            history: Vec::new(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            metric: None,
            replay: false,
            playing: false,
            time: 0.0,
            area: Rect::NOTHING,
        }
    }
}

impl DiagramView {
    /// Screen position of a diagram coordinate (as last drawn).
    #[cfg(test)]
    pub fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        self.area.min + self.pan + Vec2::new(x as f32, y as f32) * self.zoom
    }

    fn open(&mut self, file: String) {
        if let Some(current) = self.file.replace(file) {
            self.history.push(current);
        }
        self.fit = true;
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

/// The file whose diagram shows the process a call activity calls.
fn called_file<'p>(project: &'p Project, n: &FlowNode) -> Option<(&'p QId, &'p BpmnFile)> {
    let q = project.processes.get(n.called_element.as_ref()?)?;
    Some((q, project.files.get(&q.file)?))
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
        v.history.clear();
        v.fit = true;
    }
    let result = app.results.as_ref().and_then(|r| r.data.get(app.rep));
    let unit = app.results.as_ref().map_or(60.0, |r| r.unit.seconds());

    ui.horizontal(|ui| {
        if let Some(back) = v.history.last().cloned()
            && ui
                .button(format!("◀ {back}"))
                .on_hover_text("Back to the calling diagram")
                .clicked()
        {
            v.history.pop();
            v.file = Some(back);
            v.fit = true;
        }
        let current = v.file.clone().unwrap_or_default();
        egui::ComboBox::from_id_salt("diagram-file")
            .selected_text(&current)
            .show_ui(ui, |ui| {
                for f in &files {
                    if ui.selectable_label(*f == current, f).clicked() {
                        v.file = Some(f.clone());
                        v.history.clear();
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
    let legend = !heat_values.is_empty();
    let (hovered, clicked) = draw(ui, v, &project, file, &heat_values, &occupancy);
    if let Some(id) = &hovered {
        tooltip(ui, &project, result, id, unit);
    }
    if let Some(n) = clicked.as_ref().and_then(|id| project.node(id)) {
        match called_file(&project, n) {
            Some((_, f)) if !f.diagram.shapes.is_empty() => v.open(f.name.clone()),
            Some((q, f)) => {
                app.status = format!("{} ({}) has no diagram information", q.id, f.name)
            }
            None => {}
        }
    }
    if legend {
        let max = heat_values.values().cloned().fold(0.0, f64::max);
        let shown = match app.diagram.metric {
            Some(HeatMetric::Waiting | HeatMetric::Processing) => format!("{:.2}", max / unit),
            Some(HeatMetric::Utilisation) => format!("{:.0} %", max * 100.0),
            _ => format!("{max:.0}"),
        };
        ui.label(format!("heat map: green = 0, red = {shown}"));
    }
}

/// Text rotated by -90° (reading upwards), centred at `c`.
fn vertical_text(painter: &egui::Painter, c: Pos2, text: String, font: FontId, color: Color32) {
    let galley = painter.layout_no_wrap(text, font, color);
    let size = galley.size();
    let pos = Pos2::new(c.x - size.y / 2.0, c.y + size.x / 2.0);
    painter.add(TextShape::new(pos, galley, color).with_angle(-FRAC_PI_2));
}

/// The marker of a task, or the symbol of an event.
fn icon_of(n: &FlowNode) -> Option<Icon> {
    let throwing = matches!(
        n.kind,
        NodeKind::EndEvent | NodeKind::IntermediateThrowEvent
    );
    match &n.kind {
        NodeKind::Task(k) => match k {
            TaskKind::User => Some(Icon::User),
            TaskKind::Service => Some(Icon::Service),
            TaskKind::Send => Some(Icon::Message { filled: true }),
            TaskKind::Receive => Some(Icon::Message { filled: false }),
            TaskKind::Script => Some(Icon::Script),
            TaskKind::BusinessRule => Some(Icon::BusinessRule),
            TaskKind::Manual => Some(Icon::Manual),
            TaskKind::Task => None,
        },
        k if k.is_event() => match n.event_defs.first()? {
            EventDef::Message(_) => Some(Icon::Message { filled: throwing }),
            EventDef::Timer => Some(Icon::Timer),
            EventDef::Signal(_) => Some(Icon::Signal { filled: throwing }),
            EventDef::Link(_) => Some(Icon::Link { filled: throwing }),
            EventDef::Error => Some(Icon::Error),
            EventDef::Escalation => Some(Icon::Escalation),
            EventDef::Terminate => Some(Icon::Terminate),
            _ => None,
        },
        _ => None,
    }
}

/// Draw the diagram; returns the element under the pointer and a clicked call activity.
fn draw(
    ui: &mut Ui,
    v: &mut DiagramView,
    project: &Project,
    file: &BpmnFile,
    heat_values: &BTreeMap<QId, f64>,
    occupancy: &BTreeMap<QId, Occupancy>,
) -> (Option<QId>, Option<QId>) {
    let size = ui.available_size().max(Vec2::new(200.0, 200.0));
    let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let area = response.rect;
    v.area = area;
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
    let header = 30.0 * zoom; // width of the name band of pools and lanes

    painter.rect_filled(area, 0.0, paper);

    // Pools and lanes first (they are containers); names read upwards in their header band.
    for (id, b) in &d.shapes {
        let pool = file.participants.get(id);
        let lane = file.lanes.get(id);
        if pool.is_none() && lane.is_none() {
            continue;
        }
        let r = rect_of(b);
        if let Some(c) = heat_of(id) {
            painter.rect_filled(r, 0.0, c.gamma_multiply(0.35));
        }
        painter.rect_stroke(r, 0.0, thin, StrokeKind::Inside);
        // A black-box pool (no process) is a plain box with its name in the middle; a pool with
        // a process has a header band on the left with the name reading upwards, as lanes do.
        let black_box = pool.is_some_and(|p| p.process.is_none());
        if pool.is_some() && !black_box {
            painter.line_segment(
                [
                    Pos2::new(r.left() + header, r.top()),
                    Pos2::new(r.left() + header, r.bottom()),
                ],
                thin,
            );
        }
        let name = pool.and_then(|p| p.name.clone()).or_else(|| lane.cloned());
        if let Some(n) = name {
            if black_box {
                painter.text(r.center(), Align2::CENTER_CENTER, n, font.clone(), ink);
            } else {
                let c = Pos2::new(r.left() + header / 2.0, r.center().y);
                vertical_text(&painter, c, n, font.clone(), ink);
            }
        }
    }

    // Flows.
    for (id, points) in &d.edges {
        let pts: Vec<Pos2> = points.iter().map(|(x, y)| to_screen(*x, *y)).collect();
        let (Some(first), Some(last)) = (pts.first(), pts.last()) else {
            continue;
        };
        let before = &pts[pts.len().saturating_sub(2)];
        let dir = (*last - *before).normalized() * 9.0 * zoom.clamp(0.5, 2.0);
        let side = Vec2::new(-dir.y, dir.x) * 0.5;
        let head = vec![*last, *last - dir + side, *last - dir - side];
        if file.message_flows.contains_key(id) {
            // Message flow: dashed, small circle at the start, open arrow head.
            let grey = Stroke::new(1.0, ink.gamma_multiply(0.7));
            painter.extend(Shape::dashed_line(&pts, grey, 6.0 * zoom, 4.0 * zoom));
            painter.circle(*first, 3.0 * zoom.clamp(0.5, 2.0), paper, grey);
            painter.add(Shape::convex_polygon(head, paper, grey));
        } else {
            painter.add(Shape::line(pts.clone(), thin));
            painter.add(Shape::convex_polygon(head, ink, Stroke::NONE));
        }
    }

    // Flow nodes.
    let pointer = response.hover_pos();
    let mut hovered = None;
    // Boundary events last, so they sit on top of their activity's border.
    let mut order: Vec<(&QId, &sim_core::Bounds, &FlowNode)> = d
        .shapes
        .iter()
        .filter_map(|(id, b)| project.node(id).map(|n| (id, b, n)))
        .collect();
    order.sort_by_key(|(_, _, n)| n.kind == NodeKind::BoundaryEvent);
    for (id, b, n) in order {
        let r = rect_of(b);
        if pointer.is_some_and(|p| r.contains(p)) {
            // Inner elements (drawn later) win over an enclosing subprocess.
            hovered = Some(id.clone());
        }
        let fill = heat_of(id).unwrap_or(paper);
        let label = n.name.clone().unwrap_or_default();
        let icon = icon_of(n);
        match &n.kind {
            k if k.is_event() => {
                let radius = r.width().min(r.height()) / 2.0;
                let end = *k == NodeKind::EndEvent;
                let w = if end { 3.0 } else { 1.0 } * zoom.clamp(0.5, 2.0);
                let stroke = Stroke::new(w, ink);
                if *k == NodeKind::BoundaryEvent && !n.cancel_activity {
                    // Non-interrupting: dashed rings.
                    painter.circle_filled(r.center(), radius, fill);
                    let ring = |rad: f32| -> Vec<Pos2> {
                        (0..=40)
                            .map(|i| {
                                r.center()
                                    + Vec2::angled(i as f32 / 40.0 * std::f32::consts::TAU) * rad
                            })
                            .collect()
                    };
                    painter.extend(Shape::dashed_line(
                        &ring(radius),
                        thin,
                        3.0 * zoom,
                        2.0 * zoom,
                    ));
                    painter.extend(Shape::dashed_line(
                        &ring(radius - 3.0 * zoom),
                        thin,
                        3.0 * zoom,
                        2.0 * zoom,
                    ));
                } else {
                    painter.circle(r.center(), radius, fill, stroke);
                    if matches!(
                        k,
                        NodeKind::IntermediateCatchEvent
                            | NodeKind::IntermediateThrowEvent
                            | NodeKind::BoundaryEvent
                    ) {
                        painter.circle_stroke(r.center(), radius - 3.0 * zoom, thin);
                    }
                }
                if let Some(i) = icon {
                    diagram_icons::draw(&painter, i, r.center(), radius * 1.3, ink, fill);
                }
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
                let s = hw.min(hh) * 0.45;
                let bold = Stroke::new((3.0 * zoom).clamp(1.0, 5.0), ink);
                match k {
                    NodeKind::ParallelGateway => {
                        painter.line_segment([c - Vec2::new(s, 0.0), c + Vec2::new(s, 0.0)], bold);
                        painter.line_segment([c - Vec2::new(0.0, s), c + Vec2::new(0.0, s)], bold);
                    }
                    NodeKind::InclusiveGateway => {
                        painter.circle_stroke(c, s, bold);
                    }
                    NodeKind::EventBasedGateway => {
                        painter.circle_stroke(c, s * 1.15, thin);
                        painter.circle_stroke(c, s * 0.95, thin);
                        let pent: Vec<Pos2> = (0..5)
                            .map(|i| {
                                c + Vec2::angled(
                                    -FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 5.0,
                                ) * s
                                    * 0.6
                            })
                            .collect();
                        painter.add(Shape::closed_line(pent, thin));
                    }
                    _ => {
                        let d = s * 0.75;
                        painter.line_segment([c - Vec2::new(d, d), c + Vec2::new(d, d)], bold);
                        painter.line_segment([c - Vec2::new(d, -d), c + Vec2::new(d, -d)], bold);
                    }
                }
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
                let call = n.kind == NodeKind::CallActivity;
                let stroke = Stroke::new(if call { 3.0 } else { 1.0 } * zoom.clamp(0.5, 1.5), ink);
                if n.kind == NodeKind::SubProcess {
                    // Expanded subprocess: only an outline, its content is drawn on top.
                    painter.rect(
                        r,
                        rounding,
                        Color32::TRANSPARENT,
                        stroke,
                        StrokeKind::Inside,
                    );
                    painter.text(
                        r.left_top() + Vec2::splat(4.0 * zoom),
                        Align2::LEFT_TOP,
                        label,
                        font.clone(),
                        ink,
                    );
                } else {
                    painter.rect(r, rounding, fill, stroke, StrokeKind::Inside);
                    let galley = painter.layout(
                        label,
                        font.clone(),
                        ink,
                        (r.width() - 8.0 * zoom).max(10.0),
                    );
                    painter.galley(r.center() - galley.size() / 2.0, galley, ink);
                    let s = 18.0 * zoom;
                    if let Some(i) = icon {
                        diagram_icons::draw(
                            &painter,
                            i,
                            r.left_top() + Vec2::new(4.0 * zoom + s / 2.0, 4.0 * zoom + s / 2.0),
                            s,
                            ink,
                            fill,
                        );
                    }
                    if call {
                        diagram_icons::draw(
                            &painter,
                            Icon::Plus,
                            r.center_bottom() - Vec2::new(0.0, 4.0 * zoom + s * 0.4),
                            s,
                            ink,
                            fill,
                        );
                    }
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

    // A call activity whose process has a diagram can be opened with a click.
    let openable = hovered
        .as_ref()
        .and_then(|id| project.node(id))
        .filter(|n| n.kind == NodeKind::CallActivity && called_file(project, n).is_some());
    if openable.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let clicked = if response.clicked() {
        openable.map(|n| n.id.clone())
    } else {
        None
    };
    (hovered, clicked)
}

fn tooltip(ui: &Ui, project: &Project, r: Option<&ReplicationResult>, id: &QId, unit: f64) {
    let Some(n) = project.node(id) else { return };
    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        egui::Id::unique("diagram-tip"),
        egui::PopupAnchor::Pointer,
    )
    .show(|ui| {
        ui.label(format!("{} ({})", n.label(), id));
        if n.kind == NodeKind::CallActivity {
            match called_file(project, n) {
                Some((q, f)) if !f.diagram.shapes.is_empty() => {
                    ui.label(format!("click to open {} ({})", q.id, f.name));
                }
                Some((q, f)) => {
                    ui.label(format!("calls {} ({}, no diagram)", q.id, f.name));
                }
                None => {
                    ui.label("called process not loaded");
                }
            }
        }
        let Some(r) = r else { return };
        if let Some(a) = r.activities.get(id) {
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
