//! Replay camera (following cases): the main view follows one instance of the case and the
//! processes it calls, across diagrams; the other instances whose tokens it does not show get
//! a live close-up ("action cam") on the right while they are busy, and a one-line chip while
//! they only wait.

use std::collections::BTreeMap;

use eframe::egui::{Pos2, Rect, Vec2};
use sim_core::diagram::{TokenPlace, TokenView};
use sim_core::{EventDef, NodeKind, Project, QId, ReplicationResult, TaskKind};

use crate::diagram_panel::{DiagramView, point_along};

/// Zoom of an action cam (a close-up of the element and its neighbours).
pub const CAM_ZOOM: f32 = 0.8;
/// At most this many action cams; further busy instances become chips.
pub const MAX_CAMS: usize = 4;
/// Width of the column of action cams and chips.
pub const COLUMN: f32 = 280.0;
/// Height of an action cam's picture.
pub const CAM_HEIGHT: f32 = 150.0;
/// An instance is busy while a token of it works or travels, or moved this long ago (on screen).
pub const BUSY_SECONDS: f64 = 3.0;
/// The followed tokens stay this far inside the main view (fraction of its size per side).
const MARGIN: f32 = 0.15;

pub struct Camera {
    /// The main view follows; action cams and chips are shown.
    pub on: bool,
    /// Top-level instance the main view follows; `None`: the case's first one.
    pub main: Option<u64>,
    /// Moved by hand: the main view stays until "follow" is pressed.
    pub paused: bool,
    /// Instances shown as action cams, in the order they appeared.
    cams: Vec<u64>,
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
        self.cams.retain(|c| *c != root);
    }

    /// The top-level instance the main view follows in `case`.
    pub fn main_root(&self, r: &ReplicationResult, case: u64) -> Option<u64> {
        self.main
            .filter(|m| r.instances.get(*m as usize).is_some_and(|i| i.case == case))
            .or_else(|| roots(r, case).first().copied())
    }

    /// The main view is still gliding (needs another frame).
    pub fn moving(&self) -> bool {
        self.glide.is_some()
    }
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
    tokens: Vec<TokenView>,
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
            let busy = shown.iter().any(|t| {
                matches!(
                    t.place,
                    TokenPlace::OnFlow { .. } | TokenPlace::Working { .. }
                ) || t.since >= now - busy_for
            });
            Some(Lineage {
                root,
                name: name_of(project, r, root),
                focus: focus.clone(),
                more: shown.len() - 1,
                busy,
                tokens: shown.into_iter().cloned().collect(),
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
    let Some((file, p)) = token_point(project, &l.focus.place) else {
        return;
    };
    if project
        .files
        .get(&file)
        .is_none_or(|f| f.diagram.shapes.is_empty())
    {
        return;
    }
    let size = v.area.size();
    let pan_to = |c: Pos2, zoom: f32| size / 2.0 - c.to_vec2() * zoom;
    if v.file.as_ref() != Some(&file) {
        let mut history = callers(r, l.focus.instance);
        while history.last() == Some(&file) {
            history.pop();
        }
        v.history = history;
        v.file = Some(file);
        v.fit = false;
        v.pan = pan_to(p, v.zoom);
        v.camera.glide = None;
        return;
    }
    let screen = |w: Pos2| (v.pan + w.to_vec2() * v.zoom).to_pos2();
    let inner = Rect::from_min_size(Pos2::ZERO, size).shrink2(size * MARGIN);
    let pts: Vec<Pos2> = l
        .tokens
        .iter()
        .filter_map(|t| token_point(project, &t.place))
        .filter(|(f, _)| *f == file)
        .map(|(_, p)| p)
        .collect();
    let bbox = Rect::from_points(&pts);
    let on_screen = Rect::from_min_max(screen(bbox.min), screen(bbox.max));
    // All its tokens if they fit, else the one that moved last.
    let target = if on_screen.width() <= inner.width() && on_screen.height() <= inner.height() {
        (!inner.contains_rect(on_screen)).then(|| bbox.center())
    } else {
        (!inner.contains(screen(p))).then_some(p)
    };
    if target.is_some() {
        v.camera.glide = target;
    }
    if let Some(g) = v.camera.glide {
        let want = pan_to(g, v.zoom);
        let k = 1.0 - (-dt * 6.0).exp();
        v.pan += (want - v.pan) * k;
        if (want - v.pan).length() < 0.5 {
            v.pan = want;
            v.camera.glide = None;
        }
    }
}

/// The other instances whose last-moved token the main view does not show: busy ones as
/// action cams (in the order they appeared, at most `MAX_CAMS`), the rest as chips.
pub struct Side {
    pub cams: Vec<Lineage>,
    pub chips: Vec<Lineage>,
}

impl Side {
    pub fn is_empty(&self) -> bool {
        self.cams.is_empty() && self.chips.is_empty()
    }
}

pub fn side(
    v: &mut DiagramView,
    project: &Project,
    r: &ReplicationResult,
    lineages: Vec<Lineage>,
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
        .filter(|l| Some(l.root) != main && !visible(l))
        .collect();
    let busy: Vec<u64> = others.iter().filter(|l| l.busy).map(|l| l.root).collect();
    let cams = &mut v.camera.cams;
    cams.retain(|c| busy.contains(c));
    for b in busy {
        if !cams.contains(&b) {
            cams.push(b);
        }
    }
    let shown: Vec<u64> = cams.iter().take(MAX_CAMS).copied().collect();
    let (mut cams, chips): (Vec<Lineage>, Vec<Lineage>) =
        others.into_iter().partition(|l| shown.contains(&l.root));
    cams.sort_by_key(|l| shown.iter().position(|c| *c == l.root));
    Side { cams, chips }
}

/// Where an action cam showing a token at diagram point `p` puts its view.
pub fn cam_pan(size: Vec2, p: Pos2) -> Vec2 {
    size / 2.0 - p.to_vec2() * CAM_ZOOM
}
