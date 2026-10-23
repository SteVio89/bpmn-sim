//! Manual token game in the diagram view: the controls above the diagram and what the diagram
//! highlights and reacts to.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText, Ui};
use sim_core::manual::{Action, Game, Pending};
use sim_core::{Project, QId};

pub const ACCENT: Color32 = Color32::from_rgb(90, 210, 130);

pub struct ManualState {
    pub project: Arc<Project>,
    pub game: Game,
    undo: Vec<Game>,
    /// Flows ticked so far at inclusive splits, per waiting token.
    pub selection: BTreeMap<u64, BTreeSet<QId>>,
    pub error: Option<String>,
    /// Set after a step: the diagram may have to follow the tokens into another file.
    pub moved: bool,
}

/// What the diagram highlights and lets the player click.
#[derive(Default)]
pub struct Marks {
    pub nodes: BTreeSet<QId>,
    pub flows: BTreeSet<QId>,
    pub selected: BTreeSet<QId>,
}

impl ManualState {
    pub fn new(project: Arc<Project>) -> Self {
        Self {
            game: Game::new(project.clone()),
            project,
            undo: Vec::new(),
            selection: BTreeMap::new(),
            error: None,
            moved: false,
        }
    }

    fn label(&self, id: &QId) -> String {
        let name = self
            .project
            .node(id)
            .map_or_else(|| id.id.clone(), |n| n.label().to_string());
        if self.project.files.len() > 1 {
            format!("{name} ({})", id.file)
        } else {
            name
        }
    }

    fn flow_label(&self, f: &QId) -> String {
        match self.project.flow(f) {
            Some(fl) => fl.name.clone().unwrap_or_else(|| {
                format!(
                    "to {}",
                    self.project
                        .node(&fl.target)
                        .map_or(fl.target.id.as_str(), |n| n.label())
                )
            }),
            None => f.id.clone(),
        }
    }

    pub fn apply(&mut self, action: Action) {
        let before = self.game.clone();
        match self.game.apply(action) {
            Ok(()) => {
                self.undo.push(before);
                self.selection.clear();
                self.error = None;
                self.moved = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    pub fn marks(&self) -> Marks {
        let mut m = Marks::default();
        m.nodes.extend(self.game.start_events());
        for p in self.game.pending() {
            match p {
                Pending::Choose { token, flows, .. } => {
                    m.flows.extend(flows);
                    m.selected
                        .extend(self.selection.get(&token).into_iter().flatten().cloned());
                }
                Pending::Complete { task, .. } => {
                    m.nodes.insert(task);
                }
                Pending::Fire { events, .. } => m.nodes.extend(events),
                Pending::Continue { at, .. } => {
                    m.nodes.insert(at);
                }
            }
        }
        m
    }

    /// React to a click on a highlighted element; returns whether it was used.
    pub fn click(&mut self, node: Option<&QId>, flow: Option<&QId>) -> bool {
        let pending = self.game.pending();
        if let Some(f) = flow {
            for p in &pending {
                if let Pending::Choose {
                    token,
                    flows,
                    multiple,
                    ..
                } = p
                    && flows.contains(f)
                {
                    if *multiple {
                        let sel = self.selection.entry(*token).or_default();
                        if !sel.remove(f) {
                            sel.insert(f.clone());
                        }
                    } else {
                        self.apply(Action::Choose {
                            token: *token,
                            flows: vec![f.clone()],
                        });
                    }
                    return true;
                }
            }
        }
        let Some(n) = node else { return false };
        if self.game.start_events().contains(n) {
            self.apply(Action::Start { event: n.clone() });
            return true;
        }
        for p in &pending {
            match p {
                Pending::Complete { token, task } if task == n => {
                    self.apply(Action::Complete { token: *token });
                    return true;
                }
                Pending::Fire { token, events, .. } if events.contains(n) => {
                    self.apply(Action::Fire {
                        token: *token,
                        event: n.clone(),
                    });
                    return true;
                }
                Pending::Continue { token, at } if at == n => {
                    self.apply(Action::Continue { token: *token });
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// The controls above the diagram.
    pub fn controls(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.game.stop_at_tasks, "stop at tasks");
            ui.checkbox(
                &mut self.game.stop_at_events,
                "stop at message / signal events",
            );
            if ui
                .add_enabled(!self.undo.is_empty(), egui::Button::new("Undo"))
                .clicked()
                && let Some(g) = self.undo.pop()
            {
                self.game = g;
                self.selection.clear();
                self.error = None;
                self.moved = true;
            }
            if ui.button("Reset").clicked() {
                let (tasks, events) = (self.game.stop_at_tasks, self.game.stop_at_events);
                *self = ManualState::new(self.project.clone());
                self.game.stop_at_tasks = tasks;
                self.game.stop_at_events = events;
            }
            ui.separator();
            ui.label("Start:");
            for s in self.game.start_events() {
                if ui
                    .button(self.label(&s))
                    .on_hover_text("start a token here")
                    .clicked()
                {
                    self.apply(Action::Start { event: s });
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, e);
        }
        let pending = self.game.pending();
        if pending.is_empty() {
            let msg = if self.game.tokens().is_empty() {
                if self.game.finished().is_empty() {
                    "Click a start event (highlighted) to begin."
                } else {
                    "All tokens have finished."
                }
            } else {
                "No decision needed: the remaining tokens wait for messages, signals or other tokens."
            };
            ui.label(msg);
        }
        egui::ScrollArea::vertical()
            .max_height(110.0)
            .id_salt("manual-pending")
            .show(ui, |ui| {
                for p in pending {
                    ui.horizontal_wrapped(|ui| match p {
                        Pending::Choose {
                            token,
                            gateway,
                            flows,
                            multiple,
                        } => {
                            ui.label(
                                RichText::new(format!("{}:", self.label(&gateway))).color(ACCENT),
                            );
                            if multiple {
                                for f in &flows {
                                    let mut on =
                                        self.selection.get(&token).is_some_and(|s| s.contains(f));
                                    if ui.checkbox(&mut on, self.flow_label(f)).changed() {
                                        let sel = self.selection.entry(token).or_default();
                                        if on {
                                            sel.insert(f.clone());
                                        } else {
                                            sel.remove(f);
                                        }
                                    }
                                }
                                let chosen: Vec<QId> = flows
                                    .iter()
                                    .filter(|f| {
                                        self.selection.get(&token).is_some_and(|s| s.contains(*f))
                                    })
                                    .cloned()
                                    .collect();
                                if ui
                                    .add_enabled(!chosen.is_empty(), egui::Button::new("Go"))
                                    .clicked()
                                {
                                    self.apply(Action::Choose {
                                        token,
                                        flows: chosen,
                                    });
                                }
                            } else {
                                for f in flows {
                                    if ui.button(self.flow_label(&f)).clicked() {
                                        self.apply(Action::Choose {
                                            token,
                                            flows: vec![f],
                                        });
                                    }
                                }
                            }
                        }
                        Pending::Complete { token, task } => {
                            if ui
                                .button(format!("Complete {}", self.label(&task)))
                                .clicked()
                            {
                                self.apply(Action::Complete { token });
                            }
                        }
                        Pending::Fire { token, at, events } => {
                            ui.label(
                                RichText::new(format!("at {}:", self.label(&at))).color(ACCENT),
                            );
                            for e in events {
                                if ui.button(format!("{} happens", self.label(&e))).clicked() {
                                    self.apply(Action::Fire { token, event: e });
                                }
                            }
                        }
                        Pending::Continue { token, at } => {
                            let sending = matches!(
                                self.project.node(&at).map(|n| &n.kind),
                                Some(
                                    sim_core::NodeKind::IntermediateThrowEvent
                                        | sim_core::NodeKind::EndEvent
                                )
                            );
                            let what = if sending { "Send from" } else { "Go on from" };
                            if ui.button(format!("{what} {}", self.label(&at))).clicked() {
                                self.apply(Action::Continue { token });
                            }
                        }
                    });
                }
            });
        egui::CollapsingHeader::new(format!("Log ({})", self.game.log.len()))
            .id_salt("manual-log")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &self.game.log {
                            ui.label(l);
                        }
                    });
            });
    }
}
