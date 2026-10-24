//! Replay camera (following cases): the main view follows one instance of the case and the
//! processes it calls, across diagrams; another instance gets a live close-up ("action cam") on
//! the right when it gets busy where no view shows it, and keeps it while it waits; the rest get
//! a one-line chip. In auto mode the main view instead picks the instance with the most going on
//! by itself, without action cams. Views glide (eased) within a diagram and slide over to another
//! diagram; the main view's moves hold the replay clock so nothing happens unseen meanwhile.

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
/// Auto mode stays with an instance at least this long (on screen) while something happens there.
pub const HOLD_SECONDS: f32 = 3.0;
/// Auto mode switches only to an instance this much more interesting than the current one.
const SWITCH_MARGIN: f32 = 0.5;
/// Pause (on screen, clock held) before the main view switches diagrams, so the token can be
/// seen reaching the call activity or end event that makes it switch.
const BEAT_SECONDS: f32 = 0.35;
/// Length of the slide from one diagram to the next (clock held for the main view).
pub const CUT_SECONDS: f32 = 0.6;
/// The next diagram slides in from this share of the view's width.
pub const SLIDE: f32 = 0.25;
/// A glide further than this share of the view's width holds the clock (a jump in all but name).
const FAR: f32 = 0.5;
/// A glide keeps its target while the new one is closer than this (screen points).
const RETARGET: f32 = 40.0;
/// The followed tokens stay this far inside a view (fraction of its size per side).
const MARGIN: f32 = 0.15;

pub struct Camera {
    /// The main view follows; action cams and chips are shown.
    pub on: bool,
    /// Top-level instance the main view follows; `None`: the case's first one.
    pub main: Option<u64>,
    /// The main view picks the instance to follow by itself (no action cams).
    pub auto: bool,
    /// Seconds (on screen) the main view has followed `main` in auto mode.
    held: f32,
    /// Moved by hand: the main view stays until "follow" is pressed.
    pub paused: bool,
    /// The action cams, top to bottom.
    cams: Vec<Cam>,
    /// Size of an action cam's picture (as last laid out).
    cam_size: Vec2,
    /// The main view's glide within its diagram.
    glide: Option<Glide>,
    /// Seconds waited before switching the main view to another diagram.
    beat: Option<f32>,
    /// The main view slides in over the diagram it showed before.
    cut: Option<Cut>,
    /// The instance the main view followed last frame.
    following: Option<u64>,
    /// The case this state belongs to.
    case: Option<u64>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            on: true,
            main: None,
            auto: false,
            held: HOLD_SECONDS,
            paused: false,
            cams: Vec::new(),
            cam_size: Vec2::new(COLUMN_MIN, COLUMN_MIN * CAM_ASPECT),
            glide: None,
            beat: None,
            cut: None,
            following: None,
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
                auto: self.auto,
                ..Self::default()
            };
        }
        self.case = Some(case);
    }

    /// Follow `root` with the main view.
    pub fn promote(&mut self, root: u64) {
        self.auto = false;
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

    /// Let the main view pick the instance to follow.
    pub fn set_auto(&mut self) {
        self.auto = true;
        self.held = HOLD_SECONDS;
        self.paused = false;
        self.cams.clear();
    }

    /// The main view or an action cam is still moving (needs another frame).
    pub fn moving(&self) -> bool {
        self.glide.is_some()
            || self.beat.is_some()
            || self.cut.is_some()
            || self
                .cams
                .iter()
                .any(|c| c.glide.is_some() || c.cut.is_some())
    }

    /// The main view is about to switch diagrams, slides or makes a long glide: the replay
    /// clock waits for it.
    pub fn holds_clock(&self) -> bool {
        self.beat.is_some() || self.cut.is_some() || self.glide.as_ref().is_some_and(|g| g.hold)
    }

    /// The slide of the main view in progress.
    pub fn cut(&self) -> Option<&Cut> {
        self.cut.as_ref()
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
    glide: Option<Glide>,
    cut: Option<Cut>,
}

/// An eased move of a view's centre within a diagram.
struct Glide {
    from: Pos2,
    to: Pos2,
    t: f32,
    seconds: f32,
    /// Starts slowly (a fresh move); a new aim while moving keeps the speed.
    ease_in: bool,
    /// Holds the replay clock.
    hold: bool,
}

/// A view sliding in over the diagram it showed before: that diagram's file, view and the
/// direction it slides in from (1: from the right).
#[derive(Clone, Debug)]
pub struct Cut {
    pub file: String,
    pub pan: Vec2,
    pub zoom: f32,
    pub dir: f32,
    t: f32,
}

impl Cut {
    fn new(file: String, pan: Vec2, zoom: f32, dir: f32) -> Self {
        Self {
            file,
            pan,
            zoom,
            dir,
            t: 0.0,
        }
    }

    /// How far it is, from 0 to 1 (eased).
    pub fn progress(&self) -> f32 {
        smoothstep(self.t / CUT_SECONDS)
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
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
    /// Its tokens, each with whether it arrived just now.
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
                .filter(|(t, recent)| is_busy(t, *recent))
                .max_by(|(a, _), (b, _)| a.since.total_cmp(&b.since))
        {
            return (file_of(&t.place), t);
        }
        (focus_file, &self.focus)
    }

    /// How much is going on: travelling tokens most, then tokens that just arrived (more at
    /// events and gateways), tokens being worked on, and least tokens queued for staff.
    fn interest(&self, project: &Project) -> f32 {
        self.tokens
            .iter()
            .map(|(t, recent)| match &t.place {
                TokenPlace::OnFlow { .. } => 3.0,
                TokenPlace::Waiting { node }
                | TokenPlace::Working { node }
                | TokenPlace::At { node }
                    if *recent =>
                {
                    let event = project.node(node).is_some_and(|n| {
                        n.kind.is_gateway()
                            || matches!(
                                n.kind,
                                NodeKind::StartEvent
                                    | NodeKind::EndEvent
                                    | NodeKind::IntermediateCatchEvent
                                    | NodeKind::IntermediateThrowEvent
                                    | NodeKind::BoundaryEvent
                            )
                    });
                    if event { 3.0 } else { 2.0 }
                }
                TokenPlace::Working { .. } => 1.0,
                TokenPlace::Waiting { .. } => 0.3,
                TokenPlace::At { .. } => 0.0,
            })
            .fold(0.0, f32::max)
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

/// A token is busy while it travels or is worked on, or just after it arrived.
fn is_busy(t: &TokenView, recent: bool) -> bool {
    recent
        || matches!(
            t.place,
            TokenPlace::OnFlow { .. } | TokenPlace::Working { .. }
        )
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
                .map(|t| ((*t).clone(), t.since >= now - busy_for))
                .collect();
            Some(Lineage {
                root,
                name: name_of(project, r, root),
                focus: focus.clone(),
                more: shown.len() - 1,
                busy: tokens.iter().any(|(t, recent)| is_busy(t, *recent)),
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

/// Glide a view of `width` at `zoom` from `center` to `to`: longer moves take longer (up to
/// 1 s), and a move further than `FAR` or a `hold` one holds the replay clock. A glide under way
/// keeps its target unless the new one is clearly elsewhere.
fn aim(glide: &mut Option<Glide>, center: Pos2, to: Pos2, zoom: f32, width: f32, hold: bool) {
    if glide
        .as_ref()
        .is_some_and(|g| (g.to - to).length() * zoom < RETARGET)
    {
        return;
    }
    let d = (to - center).length() * zoom;
    *glide = Some(Glide {
        from: center,
        to,
        t: 0.0,
        seconds: (0.35 + d / 2000.0).min(1.0),
        ease_in: glide.is_none(),
        hold: hold || d > width * FAR,
    });
}

/// Move `center` along `glide` by `dt` seconds; the glide ends on arrival.
fn glide_step(center: &mut Pos2, glide: &mut Option<Glide>, dt: f32) {
    let Some(g) = glide else {
        return;
    };
    g.t += dt;
    let x = (g.t / g.seconds).min(1.0);
    let k = if g.ease_in {
        smoothstep(x)
    } else {
        1.0 - (1.0 - x).powi(3)
    };
    *center = g.from + (g.to - g.from) * k;
    if x >= 1.0 {
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
        v.camera.beat = None;
        return;
    }
    // Another instance (picked, or by auto mode): the clock waits for the view to get there.
    let switched = v.camera.following.is_some_and(|f| Some(f) != main);
    v.camera.following = main;
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
        // A moment to see the token arrive before the view switches.
        if v.file.is_some() {
            let beat = v.camera.beat.get_or_insert(0.0);
            *beat += dt;
            if *beat < BEAT_SECONDS {
                return;
            }
        }
        v.camera.beat = None;
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
    v.camera.beat = None;
    let mut center = ((size / 2.0 - v.pan) / v.zoom).to_pos2();
    if let Some(t) = glide_target(center, size, v.zoom, &pts, p) {
        aim(&mut v.camera.glide, center, t, v.zoom, size.x, switched);
    }
    if v.camera.glide.is_some() {
        glide_step(&mut center, &mut v.camera.glide, dt);
        v.pan = size / 2.0 - center.to_vec2() * v.zoom;
    }
}

/// The main view showed `before` (file, pan, zoom) last frame and shows `v.file` now:
/// start sliding over if that is another diagram, and move a slide on by `dt`.
pub fn track_cut(v: &mut DiagramView, before: Option<(String, Vec2, f32)>, dt: f32) {
    if let Some(c) = &mut v.camera.cut {
        c.t += dt;
        if c.t >= CUT_SECONDS {
            v.camera.cut = None;
        }
    }
    if let (Some((file, pan, zoom)), Some(now)) = (before, &v.file)
        && file != *now
    {
        // Into a called diagram from the right, back out from the left.
        let dir = if v.history.contains(&file) { 1.0 } else { -1.0 };
        v.camera.cut = Some(Cut::new(file, pan, zoom, dir));
    }
}

/// The main view shows the last-moved token of `l`.
fn shows(v: &DiagramView, project: &Project, l: &Lineage) -> bool {
    token_point(project, &l.focus.place).is_some_and(|(f, p)| {
        v.file.as_ref() == Some(&f) && v.area.contains(v.area.min + v.pan + p.to_vec2() * v.zoom)
    })
}

/// Auto mode: pick the instance the main view follows. It stays with the current one for
/// `HOLD_SECONDS` while something happens there, and then switches only to one with clearly
/// more going on (one the view already shows counts a little more, it needs no cut).
pub fn direct(v: &mut DiagramView, project: &Project, lineages: &[Lineage], dt: f32) {
    if v.camera.paused {
        return;
    }
    v.camera.held += dt;
    let score = |l: &Lineage| l.interest(project) + if shows(v, project, l) { 0.5 } else { 0.0 };
    let Some(best) = lineages.iter().max_by(|a, b| score(a).total_cmp(&score(b))) else {
        return;
    };
    let current = lineages.iter().find(|l| Some(l.root) == v.camera.main);
    let switch = match current {
        None => true,
        Some(c) if c.root == best.root => false,
        Some(c) => {
            let now = score(c);
            let free = v.camera.held >= HOLD_SECONDS || c.interest(project) == 0.0;
            free && score(best) > now + SWITCH_MARGIN
        }
    };
    if switch {
        v.camera.main = Some(best.root);
        v.camera.held = 0.0;
        v.camera.glide = None;
    }
}

/// An action cam ready to draw: the instance, the token it shows and where it looks.
pub struct Shot {
    pub lineage: Lineage,
    pub focus: TokenView,
    pub file: String,
    pub center: Pos2,
    /// Sliding in over the diagram it showed before.
    pub cut: Option<Cut>,
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
    let visible = |l: &Lineage| shows(v, project, l);
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
            cut: None,
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
        if let Some(c) = &mut cam.cut {
            c.t += dt;
            if c.t >= CUT_SECONDS {
                cam.cut = None;
            }
        }
        if cam.file != file {
            if !cam.file.is_empty() {
                let into = callers(r, focus.instance).contains(&cam.file);
                let pan = cam_pan(size, cam.center);
                let old = std::mem::take(&mut cam.file);
                cam.cut = Some(Cut::new(old, pan, CAM_ZOOM, if into { 1.0 } else { -1.0 }));
            }
            cam.file = file.to_string();
            cam.center = frame(size, CAM_ZOOM, &pts, p);
            cam.glide = None;
        } else {
            if let Some(t) = glide_target(cam.center, size, CAM_ZOOM, &pts, p) {
                aim(&mut cam.glide, cam.center, t, CAM_ZOOM, size.x, false);
            }
            glide_step(&mut cam.center, &mut cam.glide, dt);
        }
        let cut = cam.cut.clone();
        placed.push((cam.root, focus.clone(), cam.file.clone(), cam.center, cut));
    }
    let mut others = others;
    let cams = placed
        .into_iter()
        .filter_map(|(root, focus, file, center, cut)| {
            let i = others.iter().position(|l| l.root == root)?;
            Some(Shot {
                lineage: others.remove(i),
                focus,
                file,
                center,
                cut,
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
        // The glide eases in, takes longer the further it goes, and ends on arrival.
        let (mut at, mut g) = (c, None);
        aim(&mut g, at, Pos2::new(650.0, 300.0), 0.6, 300.0, false);
        let first = g.as_ref().map(|g| (g.seconds, g.hold)).unwrap();
        assert_eq!(first, (0.35 + 90.0 / 2000.0, false));
        glide_step(&mut at, &mut g, 0.05);
        assert!(at.x > 500.0 && at.x < 510.0 && g.is_some(), "{at:?}");
        // A target close to the old one keeps the glide, one elsewhere starts a new one.
        aim(&mut g, at, Pos2::new(660.0, 300.0), 0.6, 300.0, false);
        assert_eq!(g.as_ref().unwrap().to, Pos2::new(650.0, 300.0));
        for _ in 0..20 {
            glide_step(&mut at, &mut g, 0.05);
        }
        assert_eq!((at, g.is_none()), (Pos2::new(650.0, 300.0), true));
        // Far (more than half the view): the replay clock waits for it.
        aim(&mut g, at, Pos2::new(1000.0, 300.0), 0.6, 300.0, false);
        assert!(g.as_ref().unwrap().hold);
    }

    fn lineage(root: u64, place: TokenPlace, recent: bool) -> Lineage {
        let t = TokenView {
            case: 0,
            instance: root,
            token: 0,
            place,
            since: 0.0,
        };
        Lineage {
            root,
            name: String::new(),
            focus: t.clone(),
            more: 0,
            busy: is_busy(&t, recent),
            tokens: vec![(t, recent)],
        }
    }

    #[test]
    fn auto_mode_holds_a_shot_and_then_cuts_to_more_action() {
        let project = Project::default();
        let q = |id: &str| QId {
            file: "m.bpmn".into(),
            id: id.into(),
        };
        let mut v = DiagramView::default();
        v.camera.set_auto();
        let working = lineage(1, TokenPlace::Working { node: q("Task") }, false);
        let moving = lineage(
            2,
            TokenPlace::OnFlow {
                flow: q("Flow"),
                fraction: 0.5,
            },
            false,
        );
        let ls = [working, moving];
        // Nothing followed yet: the most interesting one at once.
        direct(&mut v, &project, &ls, 0.1);
        assert_eq!(v.camera.main, Some(2));
        // Following the worked-on one: it stays for the hold time, then cuts to the moving one.
        v.camera.main = Some(1);
        v.camera.held = 0.0;
        for _ in 0..29 {
            direct(&mut v, &project, &ls, 0.1);
            assert_eq!(v.camera.main, Some(1));
        }
        direct(&mut v, &project, &ls, 0.2);
        assert_eq!(v.camera.main, Some(2));
        assert_eq!(v.camera.held, 0.0);
        // Nothing happens at the followed one any more: no need to hold.
        let idle = lineage(1, TokenPlace::At { node: q("Catch") }, false);
        let ls = [
            idle,
            lineage(2, TokenPlace::Working { node: q("Task") }, false),
        ];
        v.camera.main = Some(1);
        direct(&mut v, &project, &ls, 0.1);
        assert_eq!(v.camera.main, Some(2));
        // About as much going on: no cut back and forth.
        let ls = [
            lineage(1, TokenPlace::Working { node: q("A") }, true),
            lineage(2, TokenPlace::Working { node: q("B") }, true),
        ];
        v.camera.held = 10.0;
        direct(&mut v, &project, &ls, 0.1);
        assert_eq!(v.camera.main, Some(2));
        // Moved by hand: auto mode waits for "follow" too.
        v.camera.paused = true;
        direct(&mut v, &project, &ls, 0.1);
        assert_eq!(v.camera.main, Some(2));
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
