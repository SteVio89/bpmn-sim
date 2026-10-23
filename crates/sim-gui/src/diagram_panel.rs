//! Diagram view (spec §7.5): the BPMNDI layout drawn with the egui painter, with pan and zoom,
//! a heat-map overlay, token replay from the event log, and navigation into called processes.

use std::collections::BTreeMap;
use std::f32::consts::FRAC_PI_2;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2, epaint::TextShape,
};
use std::collections::BTreeSet;
use std::sync::Arc;

use sim_core::calendar::SimClock;
use sim_core::diagram::{
    HeatMetric, Occupancy, TokenPlace, TokenView, heat, messages_in_flight, snapshot,
    token_positions,
};
use sim_core::{BpmnFile, EventDef, FlowNode, NodeKind, Project, QId, ReplicationResult, TaskKind};

use crate::app::App;
use crate::diagram_icons::{self, Icon};
use crate::manual_panel::{ACCENT, ManualState, Marks};

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
    /// Playback speed in simulated seconds per real second; `None` = automatic.
    pub speed: Option<f64>,
    /// Follow individual (traced) cases instead of showing queues of all cases.
    pub follow: bool,
    pub case: u64,
    pub cases_shown: u64,
    /// Manual token game (when switched on).
    pub manual: Option<ManualState>,
    /// Screen area of the last drawn diagram.
    pub area: Rect,
}

/// What a frame of the diagram reports back.
pub struct DrawOut {
    pub hovered: Option<QId>,
    pub clicked_node: Option<QId>,
    pub clicked_flow: Option<QId>,
}

/// What the replay draws on top of the diagram.
pub enum Overlay {
    None,
    /// All cases: waiting (queue bar) and working counts per task.
    Queues(BTreeMap<QId, Occupancy>),
    /// Followed cases: individual tokens and messages in flight.
    Tokens {
        tokens: Vec<TokenView>,
        messages: Vec<(QId, f64)>,
    },
    /// Manual token game: (node, instance) of every token.
    Game(Vec<(QId, usize)>),
}

const SPEEDS: [(f64, &str); 7] = [
    (60.0, "1 min/s"),
    (600.0, "10 min/s"),
    (3600.0, "1 h/s"),
    (8.0 * 3600.0, "8 h/s"),
    (86_400.0, "1 day/s"),
    (7.0 * 86_400.0, "1 week/s"),
    (30.0 * 86_400.0, "30 days/s"),
];

/// Colour of a case's tokens.
pub fn case_color(case: u64) -> Color32 {
    const P: [Color32; 8] = [
        Color32::from_rgb(66, 135, 245),
        Color32::from_rgb(240, 120, 40),
        Color32::from_rgb(60, 180, 90),
        Color32::from_rgb(220, 60, 120),
        Color32::from_rgb(150, 100, 230),
        Color32::from_rgb(200, 180, 40),
        Color32::from_rgb(40, 190, 200),
        Color32::from_rgb(230, 80, 60),
    ];
    P[(case % P.len() as u64) as usize]
}

/// Default speed: a typical task takes about 1.5 s on screen.
fn auto_speed(r: &ReplicationResult) -> f64 {
    let mut d: Vec<f64> = r
        .activities
        .values()
        .map(|a| a.processing.mean)
        .filter(|x| *x > 0.0)
        .collect();
    d.sort_by(f64::total_cmp);
    d.get(d.len() / 2).map_or(60.0, |m| (m / 1.5).max(1.0))
}

fn format_span(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let (d, h, m) = (
        (s / 86_400.0).floor(),
        ((s % 86_400.0) / 3600.0).floor(),
        ((s % 3600.0) / 60.0).floor(),
    );
    if d > 0.0 {
        format!("{d}d {h:02}:{m:02}")
    } else {
        format!("{h:02}:{m:02}:{:02}", (s % 60.0).floor())
    }
}

impl DiagramView {
    fn followed(&self) -> BTreeSet<u64> {
        (self.case..self.case + self.cases_shown.max(1)).collect()
    }

    /// Start and end of the followed cases (arrival of the first, completion of the last).
    fn follow_window(&self, r: &ReplicationResult) -> (f64, f64) {
        let cases: Vec<_> = self
            .followed()
            .into_iter()
            .filter_map(|c| r.cases.get(c as usize))
            .collect();
        let start = cases.iter().map(|c| c.arrival).fold(f64::MAX, f64::min);
        let end = cases
            .iter()
            .map(|c| c.completion.unwrap_or(r.end_time))
            .fold(0.0, f64::max);
        (if start == f64::MAX { 0.0 } else { start }, end)
    }

    pub fn speed_of(&self, r: &ReplicationResult) -> f64 {
        self.speed.unwrap_or_else(|| auto_speed(r))
    }

    /// Simulated time a token is shown travelling along a flow (0.6 s on screen).
    pub fn travel(&self, r: &ReplicationResult) -> f64 {
        self.speed_of(r) * 0.6
    }

    /// Move the replay clock by `dt` real seconds.
    pub fn advance(&mut self, r: &ReplicationResult, dt: f64) {
        let end = if self.follow {
            self.follow_window(r).1 + self.travel(r)
        } else {
            r.end_time
        };
        self.time += self.speed_of(r) * dt;
        if self.time >= end {
            self.time = end;
            self.playing = false;
        }
    }

    fn jump_to_case(&mut self, r: &ReplicationResult, case: u64) {
        self.case = case.min(r.cases.len().saturating_sub(1) as u64);
        self.time = self.follow_window(r).0;
    }

    /// Jump to the next moment something happens (for the followed cases, or any task).
    fn next_event(&mut self, r: &ReplicationResult) {
        let t = self.time;
        let travel = self.travel(r);
        let next = if self.follow {
            let cases = self.followed();
            r.trace
                .iter()
                .filter(|e| cases.contains(&e.case))
                .map(|e| e.time + travel)
                .find(|x| *x > t + 1e-9)
        } else {
            r.records
                .iter()
                .flat_map(|x| [x.queued, x.start, x.end])
                .filter(|x| *x > t + 1e-9)
                .min_by(f64::total_cmp)
        };
        if let Some(n) = next {
            self.time = n;
        }
    }
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
            speed: None,
            follow: true,
            case: 0,
            cases_shown: 1,
            manual: None,
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
                .button(format!("Back to {back}"))
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
            if ui.checkbox(&mut v.replay, "token replay").changed() && v.replay {
                v.manual = None;
            }
        });
        let mut manual = v.manual.is_some();
        if ui
            .checkbox(&mut manual, "manual simulation")
            .on_hover_text("play one case by hand: start tokens and decide at every gateway")
            .changed()
        {
            v.manual = manual.then(|| ManualState::new(project.clone()));
            if manual {
                v.replay = false;
            }
        }
        if result.is_none() {
            ui.label("(run a simulation for heat map and replay)");
        }
    });
    if v.replay
        && let Some(r) = result
    {
        replay_controls(ui, v, r);
    }

    let Some(file) = v.file.clone().and_then(|f| project.files.get(&f)) else {
        return;
    };
    let heat_values = match (v.metric, result) {
        (Some(m), Some(r)) => heat(&project, &file.name, r, m),
        _ => BTreeMap::new(),
    };
    // A manual game belongs to the loaded model; start afresh when the model changes.
    if v.manual
        .as_ref()
        .is_some_and(|m| !Arc::ptr_eq(&m.project, &project))
    {
        v.manual = Some(ManualState::new(project.clone()));
    }
    if let Some(m) = v.manual.as_mut() {
        m.controls(ui);
    }
    follow_manual_focus(v, &project);
    let marks = v
        .manual
        .as_ref()
        .map(ManualState::marks)
        .unwrap_or_default();
    let overlay = match (v.replay, result) {
        _ if v.manual.is_some() => Overlay::Game(
            v.manual
                .as_ref()
                .map(|m| {
                    m.game
                        .tokens()
                        .into_iter()
                        .map(|t| (t.node, t.instance))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        (true, Some(r)) => {
            let cases = v.followed();
            let traced = v.follow && cases.iter().any(|c| r.trace.iter().any(|e| e.case == *c));
            if traced {
                let travel = v.travel(r);
                Overlay::Tokens {
                    tokens: token_positions(r, &cases, v.time, travel),
                    messages: messages_in_flight(r, &cases, v.time, travel),
                }
            } else {
                Overlay::Queues(snapshot(r, v.time))
            }
        }
        _ => Overlay::None,
    };
    let legend = !heat_values.is_empty();
    let out = draw(ui, v, &project, file, &heat_values, &overlay, &marks);
    if let Some(id) = &out.hovered {
        tooltip(ui, &project, result, id, unit);
    }
    let used = match v.manual.as_mut() {
        Some(m) if out.clicked_node.is_some() || out.clicked_flow.is_some() => {
            m.click(out.clicked_node.as_ref(), out.clicked_flow.as_ref())
        }
        _ => false,
    };
    let clicked = out.clicked_node.filter(|_| !used).filter(|id| {
        project
            .node(id)
            .is_some_and(|n| n.kind == NodeKind::CallActivity)
    });
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
    overlay: &Overlay,
    marks: &Marks,
) -> DrawOut {
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
    let pointer = response.hover_pos();
    let mut hovered_flow = None;
    for (id, points) in &d.edges {
        let pts: Vec<Pos2> = points.iter().map(|(x, y)| to_screen(*x, *y)).collect();
        if marks.flows.contains(id) {
            // A flow the player can choose: highlighted (thicker when already ticked).
            let w = if marks.selected.contains(id) {
                6.0
            } else {
                3.5
            };
            painter.add(Shape::line(
                pts.clone(),
                Stroke::new(w, ACCENT.gamma_multiply(0.8)),
            ));
            if pointer.is_some_and(|p| distance_to_polyline(&pts, p) < 7.0) {
                hovered_flow = Some(id.clone());
                painter.add(Shape::line(pts.clone(), Stroke::new(w + 2.0, ACCENT)));
            }
        }
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

    // Names of events and gateways: where the modeller put them (BPMNLabel), else below.
    let name_label = |painter: &egui::Painter, id: &QId, below: Pos2, text: String| {
        if text.is_empty() {
            return;
        }
        match d.labels.get(id) {
            Some(b) => {
                let lr = rect_of(b);
                let galley = painter.layout(text, font.clone(), ink, lr.width().max(20.0) + 4.0);
                painter.galley(lr.center() - galley.size() / 2.0, galley, ink);
            }
            None => {
                painter.text(below, Align2::CENTER_TOP, text, font.clone(), ink);
            }
        }
    };

    // Flow nodes.
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
        if marks.nodes.contains(id) {
            // Something the player can click here: a halo around the element.
            let halo = Stroke::new(3.0, ACCENT);
            if n.kind.is_event() {
                painter.circle_stroke(r.center(), r.width().min(r.height()) / 2.0 + 4.0, halo);
            } else {
                painter.rect_stroke(r.expand(4.0), 10.0 * zoom, halo, StrokeKind::Outside);
            }
        }
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
                name_label(&painter, id, r.center_bottom() + Vec2::new(0.0, 2.0), label);
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
                name_label(&painter, id, r.center_bottom() + Vec2::new(0.0, 2.0), label);
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
        // Replay of all cases: a queue bar in front of each task and the number being worked on.
        if let Overlay::Queues(occupancy) = overlay
            && let Some(o) = occupancy.get(id)
        {
            let small = FontId::proportional((10.0 * zoom).clamp(6.0, 20.0));
            if o.waiting > 0 {
                let h = (r.height() * (o.waiting as f32 / 10.0).min(1.0)).max(4.0 * zoom);
                let bar = Rect::from_min_max(
                    Pos2::new(r.left() - 10.0 * zoom, r.bottom() - h),
                    Pos2::new(r.left() - 3.0 * zoom, r.bottom()),
                );
                painter.rect_filled(bar, 1.0, Color32::from_rgb(230, 130, 20));
                painter.text(
                    bar.center_top() - Vec2::new(0.0, 2.0),
                    Align2::CENTER_BOTTOM,
                    o.waiting.to_string(),
                    small.clone(),
                    Color32::from_rgb(230, 130, 20),
                );
            }
            if o.working > 0 {
                let c = r.right_top() + Vec2::new(-2.0 * zoom, 2.0 * zoom);
                painter.circle_filled(
                    c,
                    8.0 * zoom.clamp(0.6, 2.0),
                    Color32::from_rgb(40, 110, 220),
                );
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    o.working.to_string(),
                    small,
                    Color32::WHITE,
                );
            }
        }
    }

    // Names of sequence and message flows: at their BPMNLabel, else next to the middle of the
    // flow, on a backing so they stay readable across lines.
    for (id, points) in &d.edges {
        let name = file
            .sequence_flows
            .get(id)
            .and_then(|f| f.name.clone())
            .or_else(|| file.message_flows.get(id).and_then(|f| f.name.clone()));
        let Some(name) = name.filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        let pts: Vec<Pos2> = points.iter().map(|(x, y)| to_screen(*x, *y)).collect();
        let galley = painter.layout(name, font.clone(), ink, (120.0 * zoom).max(40.0));
        let pos = match d.labels.get(id) {
            Some(b) => rect_of(b).center() - galley.size() / 2.0,
            None => point_along(&pts, 0.5) + Vec2::new(4.0, -3.0 - galley.size().y),
        };
        painter.rect_filled(
            Rect::from_min_size(pos, galley.size()).expand(1.5),
            2.0,
            paper.gamma_multiply(0.85),
        );
        painter.galley(pos, galley, ink);
    }

    // Manual game: tokens where they wait.
    if let Overlay::Game(tokens) = overlay {
        let radius = (7.0 * zoom).clamp(4.0, 14.0);
        let mut stacked: BTreeMap<QId, usize> = BTreeMap::new();
        for (node, inst) in tokens {
            let (Some(b), Some(n)) = (d.shapes.get(node), project.node(node)) else {
                continue;
            };
            let r = rect_of(b);
            let k = stacked.entry(node.clone()).or_default();
            let i = *k as f32;
            *k += 1;
            let p = match n.kind {
                NodeKind::SubProcess => {
                    r.left_top() + Vec2::splat(radius + 2.0) + Vec2::new(i * radius * 2.2, 0.0)
                }
                NodeKind::Task(_) | NodeKind::CallActivity => Pos2::new(
                    r.right() - (radius + 3.0) * (1.0 + 2.0 * i),
                    r.bottom() - radius - 3.0,
                ),
                _ => r.center() + Vec2::new(i * radius * 1.6, -i * radius * 1.6),
            };
            painter.circle(p, radius, case_color(*inst as u64), Stroke::new(1.5, ink));
        }
    }

    // Replay of followed cases: tokens and messages.
    if let Overlay::Tokens { tokens, messages } = overlay {
        let radius = (6.0 * zoom).clamp(3.0, 12.0);
        let edge = |flow: &QId| {
            d.edges.get(flow).map(|p| {
                p.iter()
                    .map(|(x, y)| to_screen(*x, *y))
                    .collect::<Vec<Pos2>>()
            })
        };
        for (flow, f) in messages {
            if let Some(pts) = edge(flow) {
                let p = point_along(&pts, *f);
                diagram_icons::draw(
                    &painter,
                    Icon::Message { filled: true },
                    p,
                    radius * 3.0,
                    ink,
                    paper,
                );
            }
        }
        let mut stacked: BTreeMap<(QId, u8), usize> = BTreeMap::new();
        for tv in tokens {
            let mut slot = |node: &QId, kind: u8| {
                let k = stacked.entry((node.clone(), kind)).or_default();
                *k += 1;
                (*k - 1) as f32
            };
            let pos = match &tv.place {
                TokenPlace::OnFlow { flow, fraction } => {
                    edge(flow).map(|pts| point_along(&pts, *fraction))
                }
                TokenPlace::Waiting { node } => d.shapes.get(node).map(|b| {
                    let r = rect_of(b);
                    let i = slot(node, 0);
                    Pos2::new(r.left() - (radius + 2.0) * (1.0 + 2.0 * i), r.center().y)
                }),
                TokenPlace::Working { node } => d.shapes.get(node).map(|b| {
                    let r = rect_of(b);
                    let i = slot(node, 1);
                    Pos2::new(
                        r.right() - (radius + 3.0) * (1.0 + 2.0 * i),
                        r.bottom() - radius - 3.0,
                    )
                }),
                TokenPlace::At { node } => d.shapes.get(node).map(|b| {
                    let r = rect_of(b);
                    let i = slot(node, 2);
                    r.center() + Vec2::new(i * radius * 1.5, -i * radius * 1.5)
                }),
            };
            if let Some(p) = pos {
                painter.circle(p, radius, case_color(tv.case), Stroke::new(1.0, ink));
            }
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
    let clickable =
        hovered_flow.is_some() || hovered.as_ref().is_some_and(|h| marks.nodes.contains(h));
    if clickable {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let clicked = response.clicked();
    DrawOut {
        clicked_node: hovered.clone().filter(|h| {
            clicked && (marks.nodes.contains(h) || openable.is_some_and(|n| n.id == *h))
        }),
        clicked_flow: hovered_flow.filter(|_| clicked),
        hovered,
    }
}

fn distance_to_polyline(pts: &[Pos2], p: Pos2) -> f32 {
    pts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = if ab.length_sq() > 0.0 {
                ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (a + ab * t - p).length()
        })
        .fold(f32::MAX, f32::min)
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

/// The point at `fraction` (0..1) of the length of a polyline.
fn point_along(pts: &[Pos2], fraction: f64) -> Pos2 {
    let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let mut left = total * fraction.clamp(0.0, 1.0) as f32;
    for w in pts.windows(2) {
        let len = (w[1] - w[0]).length();
        if left <= len && len > 0.0 {
            return w[0] + (w[1] - w[0]) * (left / len);
        }
        left -= len;
    }
    pts.last().copied().unwrap_or_default()
}

/// Play/pause, speed, clock, case selection and the time slider.
fn replay_controls(ui: &mut Ui, v: &mut DiagramView, r: &ReplicationResult) {
    let traced: u64 = r.trace.iter().map(|e| e.case + 1).max().unwrap_or(0);
    ui.horizontal(|ui| {
        if ui
            .button(if v.playing { "Pause" } else { "Play" })
            .on_hover_text("play / pause")
            .clicked()
        {
            v.playing = !v.playing;
        }
        if ui
            .button("Restart")
            .on_hover_text("back to the start")
            .clicked()
        {
            v.time = if v.follow { v.follow_window(r).0 } else { 0.0 };
        }
        if ui
            .button("Next event")
            .on_hover_text("jump to the next moment something happens")
            .clicked()
        {
            v.playing = false;
            v.next_event(r);
        }
        let auto = auto_speed(r);
        let shown = match v.speed {
            None => format!("auto ({})", speed_label(auto)),
            Some(s) => speed_label(s),
        };
        egui::ComboBox::from_id_salt("replay-speed")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut v.speed, None, format!("auto ({})", speed_label(auto)));
                for (s, l) in SPEEDS {
                    ui.selectable_value(&mut v.speed, Some(s), l);
                }
            });
        ui.separator();
        let clock = SimClock {
            epoch: r.start_epoch,
        };
        ui.label(format!(
            "{}  (+{})",
            clock.format(v.time).replace('T', " ").trim_end_matches('Z'),
            format_span(v.time)
        ));
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut v.follow, false, "all cases (queues)");
        ui.radio_value(&mut v.follow, true, "follow cases");
        if v.follow {
            let before = (v.case, v.cases_shown);
            if ui.button("< Prev").on_hover_text("previous case").clicked() && v.case > 0 {
                v.jump_to_case(r, v.case - 1);
            }
            ui.add(egui::DragValue::new(&mut v.case).range(0..=r.cases.len().saturating_sub(1) as u64).prefix("case "));
            if ui.button("Next >").on_hover_text("next case").clicked() {
                v.jump_to_case(r, v.case + 1);
            }
            ui.add(egui::DragValue::new(&mut v.cases_shown).range(1..=20).prefix("show ").suffix(" case(s)"));
            if (v.case, v.cases_shown) != before {
                v.jump_to_case(r, v.case);
            }
            if v.case >= traced {
                ui.colored_label(
                    Color32::YELLOW,
                    format!("only the first {traced} cases are traced (simulation.trace_cases); showing queues"),
                );
            } else {
                for c in v.followed().into_iter().take(8) {
                    ui.colored_label(case_color(c), format!("case {c}"));
                }
            }
        }
    });
    let (lo, hi) = if v.follow {
        let (a, b) = v.follow_window(r);
        (a, b + v.travel(r))
    } else {
        (0.0, r.end_time)
    };
    let width = ui.available_width();
    ui.spacing_mut().slider_width = (width - 20.0).max(100.0);
    ui.add(egui::Slider::new(&mut v.time, lo..=hi.max(lo + 1.0)).show_value(false));
    if v.playing {
        let dt = f64::from(ui.input(|i| i.stable_dt).min(0.1));
        v.advance(r, dt);
        ui.ctx().request_repaint();
    }
}

fn speed_label(s: f64) -> String {
    if s >= 86_400.0 {
        format!("{:.1} days/s", s / 86_400.0)
    } else if s >= 3600.0 {
        format!("{:.1} h/s", s / 3600.0)
    } else if s >= 60.0 {
        format!("{:.1} min/s", s / 60.0)
    } else {
        format!("{s:.0} s/s")
    }
}

/// After a manual step, show the file where the token that just came to rest waits for the
/// player: into a called process when a token entered it, back to the caller when it ended.
/// "Back" then leads along the chain of calling files.
fn follow_manual_focus(v: &mut DiagramView, project: &Project) {
    let Some(m) = v.manual.as_mut() else { return };
    if !std::mem::take(&mut m.moved) {
        return;
    }
    let Some(target) = m.game.focus().map(|n| n.file) else {
        return;
    };
    if v.file.as_ref() == Some(&target)
        || project
            .files
            .get(&target)
            .is_none_or(|f| f.diagram.shapes.is_empty())
    {
        return;
    }
    v.history = m.game.focus_calls().into_iter().map(|c| c.file).collect();
    v.file = Some(target);
    v.fit = true;
}
