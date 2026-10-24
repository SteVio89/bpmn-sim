//! Configure mode of the diagram: which elements still need input, badges with the values set,
//! the popover editor next to the selected element, and the pilot run in the background.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{self, Rect, RichText, Ui};
use sim_core::needs::{Need, is_arrival_start, key_of, needs};
use sim_core::pilot::{Pilot, pilot};
use sim_core::scenario::{CountSpec, NumberSpec, Problem};
use sim_core::{NodeKind, Project, QId, Scenario};

use crate::editors::value_text;
use crate::widgets::dist_short;

/// Something in the diagram that takes parameters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Node(QId),
    /// A resource pool, drawn as a lane or a pool with a process.
    Pool(String),
    /// A black-box pool (it may reply to messages).
    BlackBox(QId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not looked at yet: missing, or a placeholder value.
    Todo,
    Error,
}

/// What the diagram draws in configure mode.
#[derive(Default)]
pub struct ConfigOverlay {
    /// Element -> short summary of its parameters.
    pub badges: BTreeMap<QId, String>,
    /// Outgoing flow of a split -> its share.
    pub flow_badges: BTreeMap<QId, String>,
    /// Resource pool -> summary, and whether it is overloaded.
    pub pool_badges: BTreeMap<String, (String, bool)>,
    pub status: BTreeMap<Target, Status>,
    /// Sequence flow -> share of the busiest flow's traffic in the pilot run (0..1).
    pub traffic: BTreeMap<QId, f32>,
    pub selected: Option<Target>,
    /// Elements that open an editor when clicked.
    pub clickable: BTreeSet<QId>,
}

/// The target an error or warning is about.
pub fn problem_target(project: &Project, p: &Problem) -> Option<Target> {
    if let Some(pool) = &p.pool {
        return Some(Target::Pool(pool.clone()));
    }
    let q = p.element.clone()?;
    Some(if project.node(&q).is_some() {
        Target::Node(q)
    } else {
        Target::BlackBox(q)
    })
}

/// Every target and its status: errors, then elements that are missing parameters or still
/// have placeholders.
pub fn statuses(
    project: &Project,
    s: &Scenario,
    unreviewed: &BTreeSet<Target>,
    errors: &[Problem],
) -> BTreeMap<Target, Status> {
    let mut out: BTreeMap<Target, Status> = unreviewed
        .iter()
        .map(|t| (t.clone(), Status::Todo))
        .collect();
    for n in project.nodes() {
        if !s.missing(project, n).is_empty() {
            out.insert(Target::Node(n.id.clone()), Status::Todo);
        }
    }
    for e in errors {
        if let Some(t) = problem_target(project, e) {
            out.insert(t, Status::Error);
        }
    }
    out
}

/// The element a scenario key names: a qualified id, or a bare id unique in the project.
pub fn find_node<'p>(project: &'p Project, key: &str) -> Option<&'p sim_core::FlowNode> {
    match key.split_once('#') {
        Some((f, i)) => project.node(&QId::new(f, i)),
        None => {
            let mut it = project.nodes().filter(|n| n.id.id == key);
            let one = it.next()?;
            it.next().is_none().then_some(one)
        }
    }
}

/// `text` cut to at most `max` characters, with an ellipsis.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

fn percent(p: f64) -> String {
    let v = p * 100.0;
    if (v - v.round()).abs() < 0.05 {
        format!("{v:.0} %")
    } else {
        format!("{v:.1} %")
    }
}

fn count_text(c: &CountSpec) -> String {
    match c {
        CountSpec::Fixed(n) => format!("x{n}"),
        CountSpec::Dist(NumberSpec::Uniform { min, max }) => format!("x{min}-{max}"),
        CountSpec::Dist(NumberSpec::Fixed { value }) => format!("x{value}"),
        CountSpec::Dist(_) => "x random".into(),
    }
}

/// Badges and statuses for configure mode.
pub fn overlay(
    project: &Project,
    s: &Scenario,
    status: BTreeMap<Target, Status>,
    pilot: Option<&Pilot>,
    selected: Option<Target>,
) -> ConfigOverlay {
    let mut o = ConfigOverlay {
        status,
        selected,
        ..Default::default()
    };
    for n in project.nodes() {
        let mut parts: Vec<String> = Vec::new();
        for need in needs(project, n) {
            match need {
                Need::Duration => {
                    if let Some(a) =
                        key_of(&s.activities, project, &n.id).map(|k| &s.activities[&k])
                    {
                        parts.push(dist_short(&a.duration));
                        if let Some(r) = &a.resource {
                            parts.push(r.clone());
                        }
                    }
                }
                Need::Branching { inclusive } => {
                    if let Some(g) = key_of(&s.gateways, project, &n.id).map(|k| &s.gateways[&k]) {
                        if g.by_output() {
                            let var = g.output.as_deref().unwrap_or("?");
                            for f in &n.outgoing {
                                let values =
                                    g.values.get(&f.id).or_else(|| g.values.get(&f.to_string()));
                                let text = match values {
                                    Some(vs) => {
                                        let parts: Vec<String> =
                                            vs.as_slice().iter().map(value_text).collect();
                                        short(&format!("{var} = {}", parts.join(" | ")), 32)
                                    }
                                    None if n.default_flow.as_ref() == Some(f) => {
                                        "otherwise".into()
                                    }
                                    None => format!("{var} = ?"),
                                };
                                o.flow_badges.insert(f.clone(), text);
                            }
                            continue;
                        }
                        if g.if_completed.is_some() || g.then.is_some() {
                            let asked = g
                                .if_completed
                                .as_deref()
                                .and_then(|k| find_node(project, k))
                                .map_or("?".to_string(), |e| short(e.label(), 24));
                            for f in &n.outgoing {
                                let then = g
                                    .then
                                    .as_deref()
                                    .is_some_and(|t| t == f.id || *t == f.to_string());
                                let text = if then {
                                    format!("once {asked} done")
                                } else {
                                    "until then".into()
                                };
                                o.flow_badges.insert(f.clone(), text);
                            }
                            continue;
                        }
                        for f in &n.outgoing {
                            let p = g
                                .probabilities
                                .get(&f.id)
                                .or_else(|| g.probabilities.get(&f.to_string()));
                            match p {
                                Some(p) => {
                                    o.flow_badges.insert(f.clone(), percent(*p));
                                }
                                None if inclusive && n.default_flow.as_ref() == Some(f) => {
                                    o.flow_badges.insert(f.clone(), "otherwise".into());
                                }
                                None => {}
                            }
                        }
                    }
                }
                Need::Delay => {
                    if let Some(e) = key_of(&s.events, project, &n.id).map(|k| &s.events[&k]) {
                        parts.push(dist_short(&e.delay));
                    }
                }
                Need::Boundary { probability } => {
                    if let Some(b) = key_of(&s.boundary, project, &n.id).map(|k| &s.boundary[&k]) {
                        if probability && let Some(p) = b.probability {
                            parts.push(percent(p));
                        }
                        if let Some(d) = &b.delay {
                            parts.push(format!("after {}", dist_short(d)));
                        }
                    }
                }
                Need::InstanceCount => {
                    if let Some(m) =
                        key_of(&s.multi_instance, project, &n.id).map(|k| &s.multi_instance[&k])
                    {
                        parts.push(count_text(&m.count));
                    }
                }
                Need::Repeat => {
                    if let Some(l) = key_of(&s.loops, project, &n.id).map(|k| &s.loops[&k]) {
                        parts.push(format!("again {}", percent(l.repeat_probability)));
                    }
                }
                Need::DecisionInputs { .. } => {}
            }
        }
        if is_arrival_start(n) {
            for i in s.arrivals_of(project, &n.id) {
                let a = &s.arrivals[i];
                let mut text = format!("every {}", dist_short(&a.inter_arrival));
                if let Some(c) = &a.calendar {
                    text.push_str(&format!(" · {c}"));
                }
                parts.push(text);
            }
            if let Some(e) = key_of(&s.events, project, &n.id).map(|k| &s.events[&k]) {
                parts.push(format!("starts after {}", dist_short(&e.delay)));
            }
            o.clickable.insert(n.id.clone());
        }
        if !needs(project, n).is_empty() {
            o.clickable.insert(n.id.clone());
        }
        if !parts.is_empty() {
            o.badges.insert(n.id.clone(), parts.join(" · "));
        }
    }
    for (name, r) in &s.resources {
        let mut text = format!("{}x", r.count);
        if r.cost_per_hour > 0.0 {
            text.push_str(&format!(" · {}/h", r.cost_per_hour));
        }
        if let Some(c) = &r.calendar {
            text.push_str(&format!(" · {c}"));
        }
        let mut overloaded = false;
        if let Some(l) = pilot.and_then(|p| p.pools.get(name)) {
            text.push_str(&format!(" · load {:.0} %", l.load * 100.0));
            overloaded = l.load > 1.0;
        }
        o.pool_badges.insert(name.clone(), (text, overloaded));
    }
    if let Some(p) = pilot {
        let max = p.flows.values().copied().max().unwrap_or(0).max(1) as f32;
        o.traffic = p
            .flows
            .iter()
            .map(|(f, c)| (f.clone(), *c as f32 / max))
            .collect();
    }
    o
}

/// Where a target is drawn: its file and bounds (the first lane or pool for a resource pool).
pub fn locate(project: &Project, t: &Target) -> Option<(String, sim_core::Bounds)> {
    match t {
        Target::Node(q) | Target::BlackBox(q) => {
            let f = project.files.get(&q.file)?;
            Some((q.file.clone(), *f.diagram.shapes.get(q)?))
        }
        Target::Pool(name) => project.files.values().find_map(|f| {
            let lane = f.lanes.iter().find(|(_, n)| *n == name).map(|(id, _)| id);
            let pool = || {
                f.participants
                    .values()
                    .find(|p| p.process.is_some() && p.name.as_deref() == Some(name))
                    .map(|p| &p.id)
            };
            let id = lane.or_else(pool)?;
            Some((f.name.clone(), *f.diagram.shapes.get(id)?))
        }),
    }
}

/// Targets that need attention, in reading order (file, then top to bottom, left to right).
pub fn attention(project: &Project, status: &BTreeMap<Target, Status>) -> Vec<Target> {
    let mut v: Vec<(String, i64, i64, Target)> = status
        .keys()
        .map(|t| match locate(project, t) {
            Some((f, b)) => (f, b.y as i64, b.x as i64, t.clone()),
            None => ("~".into(), 0, 0, t.clone()),
        })
        .collect();
    v.sort();
    v.into_iter().map(|(.., t)| t).collect()
}

/// Display name of a target.
pub fn target_name(project: &Project, t: &Target) -> String {
    match t {
        Target::Node(q) => project
            .node(q)
            .map_or(q.id.clone(), |n| n.label().to_string()),
        Target::Pool(name) => name.clone(),
        Target::BlackBox(q) => project
            .files
            .get(&q.file)
            .and_then(|f| f.participants.get(q))
            .and_then(|p| p.name.clone())
            .unwrap_or(q.id.clone()),
    }
}

fn kind_name(project: &Project, t: &Target) -> &'static str {
    match t {
        Target::Pool(_) => "resource pool",
        Target::BlackBox(_) => "black-box pool",
        Target::Node(q) => project
            .node(q)
            .map_or("element", |n| node_kind_name(&n.kind)),
    }
}

/// What kind of element a node is, in words.
pub fn node_kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Task(_) => "task",
        NodeKind::ExclusiveGateway => "exclusive gateway",
        NodeKind::InclusiveGateway => "inclusive gateway",
        NodeKind::StartEvent => "start event",
        NodeKind::EndEvent => "end event",
        NodeKind::BoundaryEvent => "boundary event",
        NodeKind::IntermediateCatchEvent | NodeKind::IntermediateThrowEvent => "intermediate event",
        NodeKind::SubProcess => "subprocess",
        NodeKind::CallActivity => "call activity",
        _ => "element",
    }
}

/// What the user did in the popover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverAction {
    None,
    Close,
    Next,
}

/// The editor of `target` in a floating window next to `anchor` (kept inside `bounds`).
/// Edits mark the target as reviewed.
#[allow(clippy::too_many_arguments)]
pub fn popover(
    ctx: &egui::Context,
    project: &Project,
    s: &mut Scenario,
    unreviewed: &mut BTreeSet<Target>,
    target: &Target,
    anchor: Rect,
    bounds: Rect,
    problems: &[&Problem],
    more: usize,
) -> PopoverAction {
    const WIDTH: f32 = 380.0;
    let mut pos = anchor.right_top() + egui::vec2(12.0, 0.0);
    if pos.x + WIDTH > bounds.right() {
        pos.x = (anchor.left() - WIDTH - 12.0).max(bounds.left());
    }
    pos.y = pos
        .y
        .clamp(bounds.top(), (bounds.bottom() - 120.0).max(bounds.top()));
    let mut action = PopoverAction::None;
    let before = s.clone();
    egui::Area::new(egui::Id::unique("configure-popover"))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .fixed_pos(pos)
        .constrain_to(bounds)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(WIDTH);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(target_name(project, target)).strong());
                    ui.label(RichText::new(kind_name(project, target)).small().weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("x").on_hover_text("close (Esc)").clicked() {
                            action = PopoverAction::Close;
                        }
                    });
                });
                for p in problems {
                    ui.colored_label(ui.visuals().error_fg_color, &p.message);
                }
                ui.separator();
                body(ui, project, s, target);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .button("Done")
                        .on_hover_text("mark as checked and close")
                        .clicked()
                    {
                        unreviewed.remove(target);
                        action = PopoverAction::Close;
                    }
                    if more > 0
                        && ui
                            .button(format!("Next ({more} left)"))
                            .on_hover_text(
                                "mark as checked and go to the next element that needs input",
                            )
                            .clicked()
                    {
                        unreviewed.remove(target);
                        action = PopoverAction::Next;
                    }
                });
            });
        });
    if *s != before {
        unreviewed.remove(target);
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = PopoverAction::Close;
    }
    action
}

fn body(ui: &mut Ui, project: &Project, s: &mut Scenario, target: &Target) {
    match target {
        Target::Node(q) => match project.node(q) {
            Some(n) => crate::editors::node_editor(ui, project, s, n, true),
            None => {
                ui.label("This element is not in the model any more.");
            }
        },
        Target::Pool(name) => crate::editors::pool_editor(ui, project, s, name),
        Target::BlackBox(q) => crate::editors::black_box_editor(ui, s, q),
    }
}

/// Runs the pilot of the current scenario in the background, shortly after it last changed.
pub struct PilotRunner {
    pub enabled: bool,
    seen: Option<(Arc<Project>, Scenario)>,
    changed: Option<Instant>,
    rx: Option<Receiver<Result<Pilot, String>>>,
    pub result: Option<Result<Pilot, String>>,
}

impl Default for PilotRunner {
    fn default() -> Self {
        Self {
            enabled: true,
            seen: None,
            changed: None,
            rx: None,
            result: None,
        }
    }
}

/// Quiet time after the last edit before the pilot runs.
const SETTLE: Duration = Duration::from_millis(400);

impl PilotRunner {
    /// Whether a newer pilot is pending or running (the shown result may be out of date).
    pub fn busy(&self) -> bool {
        self.changed.is_some() || self.rx.is_some()
    }

    /// Call every frame: collects a finished pilot and starts the next one when due.
    pub fn tick(&mut self, ctx: &egui::Context, project: &Arc<Project>, s: &Scenario) {
        if let Some(rx) = &self.rx
            && let Ok(r) = rx.try_recv()
        {
            self.result = Some(r);
            self.rx = None;
        }
        if !self.enabled || project.files.is_empty() {
            return;
        }
        let same = self
            .seen
            .as_ref()
            .is_some_and(|(p, sc)| Arc::ptr_eq(p, project) && sc == s);
        if !same {
            self.seen = Some((project.clone(), s.clone()));
            self.changed = Some(Instant::now());
        }
        let Some(at) = self.changed else { return };
        let waited = at.elapsed();
        if waited < SETTLE || self.rx.is_some() {
            ctx.request_repaint_after(SETTLE.saturating_sub(waited).max(Duration::from_millis(50)));
            return;
        }
        self.changed = None;
        let (tx, rx) = channel();
        let (project, s, ctx) = (project.clone(), s.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(pilot(&project, &s));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Wait for the pilot to settle and finish (tests).
    #[cfg(test)]
    pub fn finish(&mut self, ctx: &egui::Context, project: &Arc<Project>, s: &Scenario) {
        let deadline = Instant::now() + Duration::from_secs(20);
        self.tick(ctx, project, s);
        while self.busy() {
            assert!(Instant::now() < deadline, "pilot did not finish");
            std::thread::sleep(Duration::from_millis(20));
            self.tick(ctx, project, s);
        }
    }

    pub fn pilot(&self) -> Option<&Pilot> {
        self.result.as_ref().and_then(|r| r.as_ref().ok())
    }
}

/// One line about the pilot run: cycle time and the busiest pools.
pub fn pilot_summary(ui: &mut Ui, runner: &mut PilotRunner) {
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut runner.enabled, "Quick check")
            .on_hover_text(format!(
                "after each change, simulate {} cases in the background",
                sim_core::pilot::PILOT_CASES
            ));
        if !runner.enabled {
            return;
        }
        match &runner.result {
            None => {
                ui.label(RichText::new("waiting for a valid scenario…").weak());
            }
            Some(Err(_)) => {
                ui.label(RichText::new("fix the problems above to see a quick check").weak());
            }
            Some(Ok(p)) => {
                ui.label(format!(
                    "{} cases: cycle time avg {}, 90 % within {}",
                    p.cases,
                    crate::widgets::short_duration(p.cycle_time.mean),
                    crate::widgets::short_duration(p.cycle_time.p90)
                ));
                for (name, l) in &p.pools {
                    let text = format!("{name} {:.0} %", l.load * 100.0);
                    if l.load > 1.0 {
                        ui.colored_label(ui.visuals().error_fg_color, format!("{text} overloaded"))
                            .on_hover_text(
                                "more work arrives than this pool can do: the queue keeps growing",
                            );
                    } else {
                        ui.label(text).on_hover_text(
                            "load: share of the pool's capacity the arriving work needs",
                        );
                    }
                }
                if p.stuck > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("{} token(s) got stuck", p.stuck),
                    );
                }
            }
        }
        if runner.busy() {
            ui.spinner();
        }
    });
}
