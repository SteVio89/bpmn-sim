//! Replay camera (following cases): the main view follows one instance of the case and the
//! processes it calls, across diagrams; another instance gets a live close-up ("action cam") on
//! the right when it gets busy where no view shows it, and keeps it while it waits; the rest get
//! a one-line chip.

use std::collections::BTreeMap;

use eframe::egui::{Pos2, Rect, Vec2};
use sim_core::diagram::{TokenPlace, TokenView};
use sim_core::{EventDef, NodeKind, Project, QId, ReplicationResult, TaskKind};

use crate::diagram_panel::{DiagramView, point_along};

/// Zoom of an action cam (an element and its neighbourhood).
pub const CAM_ZOOM: f32 = 0.6;
/// At most this many action cams; further instances become chips.
pub const MAX_CAMS: usize = 4;
/// Share of the diagram area the column of action cams and chips takes.
const COLUMN_SHARE: f32 = 0.22;
/// Width limits of that column.
const COLUMN_MIN: f32 = 300.0;
const COLUMN_MAX: f32 = 560.0;
/// Height of an action cam's picture per width, and the least it gets.
const CAM_ASPECT: f32 = 0.62;
const CAM_MIN_HEIGHT: f32 = 120.0;
/// Space an action cam takes besides its picture (frame, title, status), and a chip.
const CAM_EXTRA: f32 = 64.0;
const CHIP_HEIGHT: f32 = 26.0;
/// An instance is busy while a token of it works or travels, or moved this long ago (on screen).
pub const BUSY_SECONDS: f64 = 3.0;
/// The followed tokens stay this far inside a view (fraction of its size per side).
const MARGIN: f32 = 0.15;

pub struct Camera {
    /// The main view follows; action cams and chips are shown.
    pub on: bool,
    /// Top-level instance the main view follows; `None`: the case's first one.
    pub main: Option<u64>,
    /// Moved by hand: the main view stays until "follow" is pressed.
    pub paused: bool,
    /// The action cams, top to bottom.
    cams: Vec<Cam>,
    /// Size of an action cam's picture (as last laid out).
    cam_size: Vec2,
    /// Diagram point the main view is gliding to.
    glide: Option<Pos2>,
    /// The case this state belongs to.
    case: Option<u64>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            on: true,
            main: None,
            paused: false,
            cams: Vec::new(),
            cam_size: Vec2::new(COLUMN_MIN, COLUMN_MIN * CAM_ASPECT),
            glide: None,
            case: None,
        }
    }
}

impl Camera {
    /// Start afresh for another case.
    pub fn reset_for(&mut self, case: u64) {
        if self.case.is_some_and(|c| c != case) {
            *self = Self {
                on: self.on,
                ..Self::default()
            };
        }
        self.case = Some(case);
    }

    /// Follow `root` with the main view.
    pub fn promote(&mut self, root: u64) {
        self.main = Some(root);
        self.paused = false;
        self.glide = None;
        self.cams.retain(|c| c.root != root);
    }

    /// The top-level instance the main view follows in `case`.
    pub fn main_root(&self, r: &ReplicationResult, case: u64) -> Option<u64> {
        self.main
            .filter(|m| r.instances.get(*m as usize).is_some_and(|i| i.case == case))
            .or_else(|| roots(r, case).first().copied())
    }

    /// The main view or an action cam is still gliding (needs another frame).
    pub fn moving(&self) -> bool {
        self.glide.is_some() || self.cams.iter().any(|c| c.glide.is_some())
    }

    /// The instances shown as action cams, top to bottom.
    #[cfg(test)]
    pub fn cam_roots(&self) -> Vec<u64> {
        self.cams.iter().map(|c| c.root).collect()
    }

    /// Lay out the side column in `area` (the main view and the column): the main view's part
    /// and the column's; remembers the size of an action cam's picture for the next frame.
    pub fn split(&mut self, area: Rect, side: &Side) -> (Rect, Rect) {
        let width = (area.width() * COLUMN_SHARE)
            .clamp(COLUMN_MIN, COLUMN_MAX)
            .min(area.width() - 200.0)
            .max(0.0);
        let split = area.right() - width;
        let main = Rect::from_min_max(area.min, Pos2::new(split - 6.0, area.bottom()));
        let column = Rect::from_min_max(Pos2::new(split, area.top()), area.max);
        // Cams fill the column's height (above the chips), up to their aspect ratio.
        let picture = column.width() - 16.0;
        let n = side.cams.len().max(1) as f32;
        let chips = side.chips.len() as f32 * CHIP_HEIGHT;
        let fit = (column.height() - chips) / n - CAM_EXTRA;
        self.cam_size = Vec2::new(
            picture,
            fit.clamp(CAM_MIN_HEIGHT, (picture * CAM_ASPECT).max(CAM_MIN_HEIGHT)),
        );
        (main, column)
    }

    /// Size of an action cam's picture.
    pub fn cam_size(&self) -> Vec2 {
        self.cam_size
    }
}

/// An action cam: the instance it shows and the diagram point it looks at.
struct Cam {
    root: u64,
    /// Empty until it is first placed.
    file: String,
    center: Pos2,
    glide: Option<Pos2>,
}

/// The top-level instances of `case` (its pools and what messages and signals started).
pub fn roots(r: &ReplicationResult, case: u64) -> Vec<u64> {
    r.instances
        .iter()
        .filter(|i| i.case == case && i.parent.is_none())
        .map(|i| i.id)
        .collect()
}

/// The top-level instance `inst` belongs to (itself, or the one whose call activities called it).
pub fn root_of(r: &ReplicationResult, mut inst: u64) -> u64 {
    while let Some(p) = r.instances.get(inst as usize).and_then(|i| i.parent) {
        inst = p;
    }
    inst
}

/// Name of a top-level instance: its pool, and its start event if the pool has several
/// instances in the case.
pub fn name_of(project: &Project, r: &ReplicationResult, root: u64) -> String {
    let Some(i) = r.instances.get(root as usize) else {
        return format!("instance {root}");
    };
    let pool = project
        .files
        .get(&i.process.file)
        .and_then(|f| {
            f.participants
                .values()
                .find(|p| p.process.as_ref() == Some(&i.process))
        })
        .and_then(|p| p.name.clone())
        .unwrap_or_else(|| i.process.id.clone());
    let siblings = r
        .instances
        .iter()
        .filter(|o| o.case == i.case && o.parent.is_none() && o.process == i.process)
        .count();
    let start = r.trace.iter().find_map(|e| match &e.kind {
        sim_core::TraceKind::Arrive { node, via: None } if e.instance == root => {
            project.node(node).map(|n| n.label().to_string())
        }
        _ => None,
    });
    match start {
        Some(s) if siblings > 1 => format!("{pool} ({s})"),
        _ => pool,
    }
}

/// The tokens of one top-level instance and the processes it called.
pub struct Lineage {
    pub root: u64,
    pub name: String,
    /// The token that moved last: the one the camera shows.
    pub focus: TokenView,
    /// How many further tokens it has.
    pub more: usize,
    pub busy: bool,
    /// Its tokens, each with whether it is busy.
    tokens: Vec<(TokenView, bool)>,
}

impl Lineage {
    /// Diagram and token to show, given the diagram a view shows now: the one the last-moved
    /// token is in, unless a token is busy in the current one (parallel work in several files
    /// does not make the view jump back and forth).
    fn view(&self, current: Option<&str>) -> (&str, &TokenView) {
        let focus_file = file_of(&self.focus.place);
        if let Some(cur) = current
            && cur != focus_file
            && let Some((t, _)) = self
                .tokens
                .iter()
                .filter(|(t, _)| file_of(&t.place) == cur)
                .filter(|(_, busy)| *busy)
                .max_by(|(a, _), (b, _)| a.since.total_cmp(&b.since))
        {
            return (file_of(&t.place), t);
        }
        (focus_file, &self.focus)
    }

    /// Diagram points of its tokens in `file`.
    fn points(&self, project: &Project, file: &str) -> Vec<Pos2> {
        self.tokens
            .iter()
            .filter_map(|(t, _)| token_point(project, &t.place))
            .filter(|(f, _)| f == file)
            .map(|(_, p)| p)
            .collect()
    }
}

/// The file a token is in.
fn file_of(place: &TokenPlace) -> &str {
    match place {
        TokenPlace::OnFlow { flow, .. } => &flow.file,
        TokenPlace::Waiting { node } | TokenPlace::Working { node } | TokenPlace::At { node } => {
            &node.file
        }
    }
}

/// A token waiting at a call activity or subprocess stands for the tokens inside it.
fn waits_for_inside(project: &Project, t: &TokenView) -> bool {
    matches!(&t.place, TokenPlace::At { node } if project.node(node)
        .is_some_and(|n| matches!(n.kind, NodeKind::CallActivity | NodeKind::SubProcess)))
}

/// The tokens of `case`, grouped by top-level instance (in the order the instances were created).
pub fn lineages(
    project: &Project,
    r: &ReplicationResult,
    case: u64,
    tokens: &[TokenView],
    now: f64,
    busy_for: f64,
) -> Vec<Lineage> {
    let mut by_root: BTreeMap<u64, Vec<&TokenView>> = BTreeMap::new();
    for t in tokens.iter().filter(|t| t.case == case) {
        by_root.entry(root_of(r, t.instance)).or_default().push(t);
    }
    by_root
        .into_iter()
        .filter_map(|(root, all)| {
            let inside: Vec<&TokenView> = all
                .iter()
                .copied()
                .filter(|t| !waits_for_inside(project, t))
                .collect();
            let shown = if inside.is_empty() { all } else { inside };
            let focus = shown.iter().copied().max_by(|a, b| {
                a.since
                    .total_cmp(&b.since)
                    .then(a.instance.cmp(&b.instance))
                    .then(a.token.cmp(&b.token))
            })?;
            let tokens: Vec<(TokenView, bool)> = shown
                .iter()
                .map(|t| {
                    let busy = matches!(
                        t.place,
                        TokenPlace::OnFlow { .. } | TokenPlace::Working { .. }
                    ) || t.since >= now - busy_for;
                    ((*t).clone(), busy)
                })
                .collect();
            Some(Lineage {
                root,
                name: name_of(project, r, root),
                focus: focus.clone(),
                more: shown.len() - 1,
                busy: tokens.iter().any(|(_, b)| *b),
                tokens,
            })
        })
        .collect()
}

/// File and diagram point of a token.
pub fn token_point(project: &Project, place: &TokenPlace) -> Option<(String, Pos2)> {
    match place {
        TokenPlace::OnFlow { flow, fraction } => {
            let pts: Vec<Pos2> = project
                .files
                .get(&flow.file)?
                .diagram
                .edges
                .get(flow)?
                .iter()
                .map(|(x, y)| Pos2::new(*x as f32, *y as f32))
                .collect();
            Some((flow.file.clone(), point_along(&pts, *fraction)))
        }
        TokenPlace::Waiting { node } | TokenPlace::Working { node } | TokenPlace::At { node } => {
            let b = project.files.get(&node.file)?.diagram.shapes.get(node)?;
            Some((
                node.file.clone(),
                Pos2::new((b.x + b.width / 2.0) as f32, (b.y + b.height / 2.0) as f32),
            ))
        }
    }
}

/// What a token is doing, in words.
pub fn status(project: &Project, t: &TokenView) -> String {
    let label = |q: &QId| {
        project
            .node(q)
            .map_or(q.id.clone(), |n| n.label().to_string())
    };
    match &t.place {
        TokenPlace::OnFlow { flow, .. } => match project.flow(flow) {
            Some(f) => format!("on the way to '{}'", label(&f.target)),
            None => "moving".into(),
        },
        TokenPlace::Working { node } => format!("working on '{}'", label(node)),
        TokenPlace::Waiting { node } => format!("waiting for staff at '{}'", label(node)),
        TokenPlace::At { node } => {
            let Some(n) = project.node(node) else {
                return label(node);
            };
            let what = match (&n.kind, n.event_defs.first()) {
                (NodeKind::Task(TaskKind::Receive), _) | (_, Some(EventDef::Message(_))) => {
                    "waiting for a message at"
                }
                (_, Some(EventDef::Signal(_))) => "waiting for a signal at",
                (_, Some(EventDef::Timer)) => "waiting for a timer at",
                (k, _) if k.is_gateway() => "waiting at",
                _ => "at",
            };
            format!("{what} '{}'", n.label())
        }
    }
}

/// Files of the call activities that lead to instance `inst`, outermost first.
fn callers(r: &ReplicationResult, mut inst: u64) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(i) = r.instances.get(inst as usize) {
        match (i.parent, &i.call) {
            (Some(p), Some(c)) => {
                out.push(c.file.clone());
                inst = p;
            }
            _ => break,
        }
    }
    out.reverse();
    out.dedup();
    out
}

/// Where a view of `size` at `zoom` first looks at `pts` (diagram points of an instance's
/// tokens, `p` the one shown): at all of them if they fit, else at `p`.
fn frame(size: Vec2, zoom: f32, pts: &[Pos2], p: Pos2) -> Pos2 {
    let inner = size / zoom * (1.0 - 2.0 * MARGIN);
    let bbox = Rect::from_points(pts);
    if bbox.width() <= inner.x && bbox.height() <= inner.y {
        bbox.center()
    } else {
        p
    }
}

/// Where a view of `size` at `zoom` looking at `center` has to glide so the tokens stay inside
/// it (`pts` and `p` as in [`frame`]); `None` while they are.
pub fn glide_target(center: Pos2, size: Vec2, zoom: f32, pts: &[Pos2], p: Pos2) -> Option<Pos2> {
    let inner = Rect::from_center_size(center, size / zoom * (1.0 - 2.0 * MARGIN));
    let bbox = Rect::from_points(pts);
    if bbox.width() <= inner.width() && bbox.height() <= inner.height() {
        (!inner.contains_rect(bbox)).then(|| bbox.center())
    } else {
        (!inner.contains(p)).then_some(p)
    }
}

/// Move `center` a step of `dt` seconds towards `glide`; the glide ends on arrival.
fn glide_step(center: &mut Pos2, glide: &mut Option<Pos2>, zoom: f32, dt: f32) {
    let Some(g) = *glide else {
        return;
    };
    let k = 1.0 - (-dt * 6.0).exp();
    *center += (g - *center) * k;
    if (g - *center).length() * zoom < 0.5 {
        *center = g;
        *glide = None;
    }
}

/// Move the main view along with the followed instance: into the file its last-moved token is
/// in, and, within a file, only when its tokens get close to the edge (gliding there).
pub fn follow_main(
    v: &mut DiagramView,
    project: &Project,
    r: &ReplicationResult,
    lineages: &[Lineage],
    dt: f32,
) {
    let main = v.camera.main_root(r, v.case);
    let Some(l) = lineages.iter().find(|l| Some(l.root) == main) else {
        return;
    };
    if v.camera.paused || !v.area.is_positive() {
        return;
    }
    let (file, focus) = l.view(v.file.as_deref());
    let Some((_, p)) = token_point(project, &focus.place) else {
        return;
    };
    if project
        .files
        .get(file)
        .is_none_or(|f| f.diagram.shapes.is_empty())
    {
        return;
    }
    let size = v.area.size();
    let pts = l.points(project, file);
    if v.file.as_deref() != Some(file) {
        let mut history = callers(r, focus.instance);
        while history.last().map(String::as_str) == Some(file) {
            history.pop();
        }
        v.history = history;
        v.file = Some(file.to_string());
        v.fit = false;
        v.pan = size / 2.0 - frame(size, v.zoom, &pts, p).to_vec2() * v.zoom;
        v.camera.glide = None;
        return;
    }
    let mut center = ((size / 2.0 - v.pan) / v.zoom).to_pos2();
    if let Some(t) = glide_target(center, size, v.zoom, &pts, p) {
        v.camera.glide = Some(t);
    }
    if v.camera.glide.is_some() {
        glide_step(&mut center, &mut v.camera.glide, v.zoom, dt);
        v.pan = size / 2.0 - center.to_vec2() * v.zoom;
    }
}

/// An action cam ready to draw: the instance, the token it shows and where it looks.
pub struct Shot {
    pub lineage: Lineage,
    pub focus: TokenView,
    pub file: String,
    pub center: Pos2,
}

/// The other instances: those with an action cam, and the rest as chips.
pub struct Side {
    pub cams: Vec<Shot>,
    pub chips: Vec<Lineage>,
}

impl Side {
    pub fn is_empty(&self) -> bool {
        self.cams.is_empty() && self.chips.is_empty()
    }
}

/// Update the action cams: an instance that gets busy where no view shows it gets one (taking
/// the slot of the one that has waited longest when all are taken and one waits), and keeps it
/// while it waits, until it ends or the main view follows it. Each cam glides like the main view.
pub fn side(
    v: &mut DiagramView,
    project: &Project,
    r: &ReplicationResult,
    lineages: Vec<Lineage>,
    dt: f32,
) -> Side {
    let main = v.camera.main_root(r, v.case);
    let visible = |l: &Lineage| {
        token_point(project, &l.focus.place).is_some_and(|(f, p)| {
            v.file.as_ref() == Some(&f)
                && v.area.contains(v.area.min + v.pan + p.to_vec2() * v.zoom)
        })
    };
    let others: Vec<Lineage> = lineages
        .into_iter()
        .filter(|l| Some(l.root) != main)
        .collect();
    let seen: Vec<u64> = others
        .iter()
        .filter(|l| visible(l))
        .map(|l| l.root)
        .collect();
    let fresh: Vec<u64> = others
        .iter()
        .filter(|l| l.busy && !seen.contains(&l.root))
        .filter(|l| !v.camera.cams.iter().any(|c| c.root == l.root))
        .map(|l| l.root)
        .collect();
    let of = |root: u64| others.iter().find(|l| l.root == root);
    let size = v.camera.cam_size;
    let cams = &mut v.camera.cams;
    cams.retain(|c| of(c.root).is_some());
    for root in fresh {
        let cam = Cam {
            root,
            file: String::new(),
            center: Pos2::ZERO,
            glide: None,
        };
        if cams.len() < MAX_CAMS {
            cams.push(cam);
        } else if let Some(slot) = cams
            .iter()
            .enumerate()
            .filter_map(|(i, c)| of(c.root).filter(|l| !l.busy).map(|l| (i, l.focus.since)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
        {
            cams[slot] = cam;
        }
    }
    let mut placed = Vec::new();
    for cam in cams.iter_mut() {
        let Some(l) = of(cam.root) else { continue };
        let current = (!cam.file.is_empty()).then_some(cam.file.as_str());
        let (file, focus) = l.view(current);
        let Some((_, p)) = token_point(project, &focus.place) else {
            continue;
        };
        let pts = l.points(project, file);
        if cam.file != file {
            cam.file = file.to_string();
            cam.center = frame(size, CAM_ZOOM, &pts, p);
            cam.glide = None;
        } else {
            if let Some(t) = glide_target(cam.center, size, CAM_ZOOM, &pts, p) {
                cam.glide = Some(t);
            }
            glide_step(&mut cam.center, &mut cam.glide, CAM_ZOOM, dt);
        }
        placed.push((cam.root, focus.clone(), cam.file.clone(), cam.center));
    }
    let mut others = others;
    let cams = placed
        .into_iter()
        .filter_map(|(root, focus, file, center)| {
            let i = others.iter().position(|l| l.root == root)?;
            Some(Shot {
                lineage: others.remove(i),
                focus,
                file,
                center,
            })
        })
        .collect();
    others.retain(|l| !seen.contains(&l.root));
    Side {
        cams,
        chips: others,
    }
}

/// Where an action cam of `size` looking at diagram point `center` puts its view.
pub fn cam_pan(size: Vec2, center: Pos2) -> Vec2 {
    size / 2.0 - center.to_vec2() * CAM_ZOOM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_glide_only_when_tokens_near_the_edge() {
        let size = Vec2::new(300.0, 180.0);
        let c = Pos2::new(500.0, 300.0);
        // 500 x 300 diagram units at 0.6, 350 x 210 of them inside the margin.
        let near = [Pos2::new(600.0, 350.0), Pos2::new(400.0, 260.0)];
        assert_eq!(glide_target(c, size, 0.6, &near, near[0]), None);
        // Close to the edge: to the middle of all tokens if they fit.
        let edge = [Pos2::new(700.0, 300.0), Pos2::new(600.0, 300.0)];
        assert_eq!(
            glide_target(c, size, 0.6, &edge, edge[0]),
            Some(Pos2::new(650.0, 300.0))
        );
        // Too far apart: to the token shown, and only once it is close to the edge.
        let apart = [Pos2::new(100.0, 300.0), Pos2::new(900.0, 300.0)];
        assert_eq!(
            glide_target(c, size, 0.6, &apart, Pos2::new(520.0, 300.0)),
            None
        );
        assert_eq!(glide_target(c, size, 0.6, &apart, apart[1]), Some(apart[1]));
        // The glide eases in and ends on arrival.
        let (mut at, mut g) = (c, Some(Pos2::new(650.0, 300.0)));
        glide_step(&mut at, &mut g, 0.6, 0.1);
        assert!(at.x > 500.0 && at.x < 650.0 && g.is_some(), "{at:?}");
        for _ in 0..100 {
            glide_step(&mut at, &mut g, 0.6, 0.1);
        }
        assert_eq!((at, g), (Pos2::new(650.0, 300.0), None));
    }

    #[test]
    fn the_column_grows_with_the_window_and_cams_fill_it() {
        let mut cam = Camera::default();
        let side = Side {
            cams: Vec::new(),
            chips: Vec::new(),
        };
        let (main, column) = cam.split(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, 900.0)),
            &side,
        );
        assert_eq!(column.width(), 440.0);
        assert_eq!(main.right(), 2000.0 - 440.0 - 6.0);
        // One cam: as tall as its aspect ratio allows.
        assert_eq!(cam.cam_size(), Vec2::new(424.0, 424.0 * CAM_ASPECT));
        // A small window keeps the column usable and the main view at least 200 wide.
        let (main, column) = cam.split(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(450.0, 600.0)),
            &side,
        );
        assert_eq!(column.width(), 250.0);
        assert!(main.width() >= 190.0);
    }
}
