use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use sim_core::scenario::TimeUnit;
use sim_core::{Project, QId, ReplicationResult, Scenario};

use crate::configure::{PilotRunner, Target};
use crate::diagram_panel::Mode;
use crate::job::Job;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Model,
    Scenario,
    Run,
    Results,
    Diagram,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ResultsTab {
    #[default]
    Summary,
    Cases,
    Activities,
    Resources,
    Problems,
    Replications,
}

pub struct RunResults {
    pub data: Vec<ReplicationResult>,
    pub project: Arc<Project>,
    pub unit: TimeUnit,
}

pub struct App {
    pub ws: Workspace,
    pub scenario: Scenario,
    pub scenario_path: Option<PathBuf>,
    pub status: String,
    pub tab: Tab,
    pub selected: Option<QId>,
    pub job: Option<Job>,
    pub run_error: Option<String>,
    pub results: Option<RunResults>,
    pub rep: usize,
    pub results_tab: ResultsTab,
    pub diagram: crate::diagram_panel::DiagramView,
    /// Elements whose parameters are generated placeholders nobody has looked at yet.
    pub unreviewed: BTreeSet<Target>,
    pub pilot: PilotRunner,
    /// What the last "Update from model" (or load) changed, shown in the scenario tab.
    pub update_note: Option<sim_core::needs::Update>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            ws: Workspace::default(),
            scenario: Scenario::default(),
            scenario_path: None,
            status: "Open a BPMN file or folder to begin.".into(),
            tab: Tab::Model,
            selected: None,
            job: None,
            run_error: None,
            results: None,
            rep: 0,
            results_tab: ResultsTab::default(),
            diagram: Default::default(),
            unreviewed: BTreeSet::new(),
            pilot: PilotRunner::default(),
            update_note: None,
        }
    }
}

impl App {
    /// A scenario that still has nothing configured can be replaced by a fresh skeleton.
    pub fn scenario_is_pristine(&self) -> bool {
        self.scenario.activities.is_empty() && self.scenario.gateways.is_empty()
    }

    /// Replace the scenario by placeholders for everything; all of them still to be checked.
    pub fn regenerate_skeleton(&mut self) {
        self.scenario = Scenario::skeleton(&self.ws.project, None);
        self.scenario_path = None;
        self.unreviewed.clear();
        self.mark_placeholders(sim_core::needs::Update {
            added: self.ws.project.nodes().map(|n| n.id.clone()).collect(),
            pools_added: self.scenario.resources.keys().cloned().collect(),
            arrival_added: true,
            orphaned: Vec::new(),
        });
    }

    /// Add placeholders for what the model has and the scenario lacks; keeps all edits.
    pub fn update_from_model(&mut self) -> sim_core::needs::Update {
        let u = self.scenario.update_from_model(&self.ws.project);
        self.mark_placeholders(u.clone());
        u
    }

    /// Remember the placeholders of `u` as still to be checked.
    fn mark_placeholders(&mut self, u: sim_core::needs::Update) {
        let p = &self.ws.project;
        for q in u.added {
            if p.node(&q)
                .is_some_and(|n| !sim_core::needs::needs(p, n).is_empty())
            {
                self.unreviewed.insert(Target::Node(q));
            }
        }
        self.unreviewed
            .extend(u.pools_added.into_iter().map(Target::Pool));
        if u.arrival_added {
            for a in &self.scenario.arrivals {
                for t in &a.triggers {
                    if let Some(n) = p.nodes().find(|n| n.id.to_string() == *t || n.id.id == *t) {
                        self.unreviewed.insert(Target::Node(n.id.clone()));
                    }
                }
            }
        }
    }

    /// Show `t` in the diagram with its editor open.
    pub fn focus(&mut self, t: Target) {
        self.tab = Tab::Diagram;
        self.diagram.manual = None;
        self.diagram.mode = Mode::Configure;
        self.diagram.focus(&self.ws.project, &t);
        self.diagram.target = Some(t);
    }

    pub fn start_run(&mut self, ctx: &egui::Context) {
        self.run_error = None;
        let ctx = ctx.clone();
        self.job = Some(crate::job::spawn(
            self.ws.project.clone(),
            self.scenario.clone(),
            move || ctx.request_repaint(),
        ));
    }

    fn poll_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        if !job.poll() {
            return;
        }
        match job.finished.take() {
            Some(Ok(data)) => {
                self.status = format!("Run finished: {} replication(s).", data.len());
                self.results = Some(RunResults {
                    data,
                    project: self.ws.project.clone(),
                    unit: self.scenario.simulation.time_unit,
                });
                self.rep = 0;
                self.tab = Tab::Results;
            }
            Some(Err(e)) => self.run_error = Some(e),
            None => {}
        }
        self.job = None;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// One frame of the whole UI (separate from `eframe::App::ui` so tests can drive it).
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.poll_job();
        self.pilot.tick(ui.ctx(), &self.ws.project, &self.scenario);
        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (tab, name) in [
                    (Tab::Model, "Model"),
                    (Tab::Scenario, "Scenario"),
                    (Tab::Run, "Run"),
                    (Tab::Results, "Results"),
                    (Tab::Diagram, "Diagram"),
                ] {
                    ui.selectable_value(&mut self.tab, tab, name);
                }
                ui.separator();
                if let Some(job) = &self.job {
                    let f = job.done as f32 / job.total.max(1) as f32;
                    ui.add(
                        egui::ProgressBar::new(f)
                            .desired_width(160.0)
                            .text(format!("{}/{}", job.done, job.total)),
                    );
                } else {
                    ui.label(&self.status);
                }
                // Theme switcher, right-aligned; the choice is remembered between starts.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut pref = ui.ctx().options(|o| o.theme_preference);
                    let before = pref;
                    for (p, name) in [
                        (egui::ThemePreference::Dark, "Dark"),
                        (egui::ThemePreference::Light, "Light"),
                        (egui::ThemePreference::System, "System"),
                    ] {
                        ui.selectable_value(&mut pref, p, name);
                    }
                    ui.label("Theme:");
                    if pref != before {
                        ui.ctx().set_theme(pref);
                    }
                });
            });
        });
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Model => crate::model_panel::show(self, ui),
            Tab::Scenario => crate::scenario_panel::show(self, ui),
            Tab::Run => crate::scenario_panel::show_run(self, ui),
            Tab::Results => crate::results_panel::show(self, ui),
            Tab::Diagram => crate::diagram_panel::show(self, ui),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use super::*;

    fn fixtures() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
    }

    /// Run `frames` UI frames headlessly.
    fn frames(app: &mut App, ctx: &egui::Context, n: usize) {
        for _ in 0..n {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.draw(ui));
            out.textures_delta.clear();
        }
    }

    #[test]
    fn open_run_and_draw_every_tab() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v5-and.bpmn"));
        assert!(app.ws.load_error.is_none());
        assert!(app.scenario_is_pristine());
        app.regenerate_skeleton();
        assert_eq!(app.scenario.activities.len(), 2);
        app.scenario.simulation.replications = 2;

        for tab in [Tab::Model, Tab::Scenario, Tab::Run, Tab::Results] {
            app.tab = tab;
            frames(&mut app, &ctx, 2);
        }

        app.start_run(&ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.results.is_none() {
            assert!(
                Instant::now() < deadline,
                "run did not finish: {:?}",
                app.run_error
            );
            assert!(app.run_error.is_none(), "{:?}", app.run_error);
            std::thread::sleep(Duration::from_millis(10));
            frames(&mut app, &ctx, 1);
        }
        assert_eq!(app.results.as_ref().unwrap().data.len(), 2);
        assert_eq!(app.tab, Tab::Results);
        for t in [
            ResultsTab::Summary,
            ResultsTab::Cases,
            ResultsTab::Activities,
            ResultsTab::Resources,
            ResultsTab::Problems,
        ] {
            app.results_tab = t;
            app.rep = 1;
            frames(&mut app, &ctx, 2);
        }
    }

    #[test]
    fn splits_that_follow_a_decision_output_draw_and_run() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("dmn-output/ticket.bpmn"));
        assert!(app.ws.load_error.is_none());
        app.regenerate_skeleton();
        let project = app.ws.project.clone();
        let gateways = ["Gateway_Queue", "Gateway_Priority", "Gateway_Checks"];
        for id in gateways {
            let g = &app.scenario.gateways[&format!("ticket.bpmn#{id}")];
            assert!(g.by_output(), "{id}: {g:?}");
        }
        let draw = |app: &mut App| {
            for _ in 0..2 {
                let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                    for id in gateways {
                        let n = project.node(&QId::new("ticket.bpmn", id)).unwrap();
                        crate::editors::node_editor(ui, &project, &mut app.scenario, n, true);
                    }
                });
                out.textures_delta.clear();
            }
        };
        draw(&mut app);
        // The business rule tasks draw their inputs and what the decision gives.
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                for id in ["Task_Triage", "Task_Checks"] {
                    let n = project.node(&QId::new("ticket.bpmn", id)).unwrap();
                    crate::editors::node_editor(ui, &project, &mut app.scenario, n, true);
                }
            });
            out.textures_delta.clear();
        }
        // A split that has gone back to shares draws its share editor.
        let queue = app
            .scenario
            .gateways
            .get_mut("ticket.bpmn#Gateway_Queue")
            .unwrap();
        queue.output = None;
        queue.values.clear();
        queue.probabilities = [
            ("Flow_front", 0.5),
            ("Flow_back", 0.25),
            ("Flow_expert", 0.25),
        ]
        .map(|(k, p)| (k.to_string(), p))
        .into();
        draw(&mut app);
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Configure;
        frames(&mut app, &ctx, 2);
        assert!(
            app.scenario.bind(&project).is_ok(),
            "{:?}",
            app.scenario.bind(&project).err()
        );
    }

    #[test]
    fn boundary_and_timer_sections_draw_and_run() {
        for fixture in [
            "v9-boundary-timer.bpmn",
            "timer-catch.bpmn",
            "or-upstream.bpmn",
            "or-split-join.bpmn",
        ] {
            let ctx = egui::Context::default();
            let mut app = App::default();
            app.ws.open(&fixtures().join(fixture));
            app.regenerate_skeleton();
            app.tab = Tab::Scenario;
            frames(&mut app, &ctx, 2);
            app.start_run(&ctx);
            let deadline = Instant::now() + Duration::from_secs(20);
            while app.results.is_none() {
                assert!(
                    Instant::now() < deadline && app.run_error.is_none(),
                    "{:?}",
                    app.run_error
                );
                std::thread::sleep(Duration::from_millis(10));
                frames(&mut app, &ctx, 1);
            }
            for t in [
                ResultsTab::Summary,
                ResultsTab::Activities,
                ResultsTab::Problems,
            ] {
                app.results_tab = t;
                frames(&mut app, &ctx, 2);
            }
        }
    }

    #[test]
    fn opening_a_main_file_loads_referenced_files() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("multi-file/main.bpmn"));
        assert!(app.ws.load_error.is_none(), "{:?}", app.ws.load_error);
        assert_eq!(app.ws.project.files.len(), 3);
        assert_eq!(app.ws.project.dmn.len(), 2);
        app.regenerate_skeleton();
        app.scenario.simulation.cases = Some(5);
        for tab in [Tab::Model, Tab::Scenario] {
            app.tab = tab;
            frames(&mut app, &ctx, 2);
        }
        app.start_run(&ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.results.is_none() {
            assert!(
                Instant::now() < deadline && app.run_error.is_none(),
                "{:?}",
                app.run_error
            );
            std::thread::sleep(Duration::from_millis(10));
            frames(&mut app, &ctx, 1);
        }
        for t in [
            ResultsTab::Summary,
            ResultsTab::Activities,
            ResultsTab::Problems,
        ] {
            app.results_tab = t;
            frames(&mut app, &ctx, 2);
        }
    }

    #[test]
    fn calendars_horizon_and_replications_draw_and_run() {
        use sim_core::calendar::CalendarSpec;
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v5-and.bpmn"));
        app.regenerate_skeleton();
        app.scenario.calendars.insert(
            "office".into(),
            CalendarSpec::uniform(&["mon", "tue", "wed", "thu", "fri"], "09:00-17:00"),
        );
        app.scenario.resources.get_mut("TeamA").unwrap().calendar = Some("office".into());
        app.scenario.simulation.replications = 3;
        app.scenario.simulation.cases = None;
        app.scenario.simulation.horizon = Some("30d".into());
        app.scenario.simulation.start = Some("2026-03-02T08:00".into());
        app.tab = Tab::Scenario;
        frames(&mut app, &ctx, 2);
        app.start_run(&ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.results.is_none() {
            assert!(
                Instant::now() < deadline && app.run_error.is_none(),
                "{:?}",
                app.run_error
            );
            std::thread::sleep(Duration::from_millis(10));
            frames(&mut app, &ctx, 1);
        }
        assert!(
            app.results
                .as_ref()
                .unwrap()
                .data
                .iter()
                .all(|r| r.horizon_reached)
        );
        for t in [
            ResultsTab::Summary,
            ResultsTab::Replications,
            ResultsTab::Problems,
        ] {
            app.results_tab = t;
            frames(&mut app, &ctx, 2);
        }
    }

    fn frame_with(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.draw(ui));
        out.textures_delta.clear();
    }

    #[test]
    fn diagram_draws_pans_zooms_and_replays() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v13-model.bpmn"));
        app.tab = Tab::Diagram;
        // Without results: the layout alone.
        for _ in 0..3 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert!(!app.diagram.fit, "the view was fitted to the diagram");
        let zoom = app.diagram.zoom;
        assert!(zoom > 0.1 && zoom < 5.0, "{zoom}");

        // Drag pans the view.
        let pan = app.diagram.pan;
        let at = |x: f32, y: f32| egui::pos2(x, y);
        let press = |p, pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame_with(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(at(600.0, 400.0)),
                press(at(600.0, 400.0), true),
            ],
        );
        for k in 1..=5 {
            frame_with(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(at(
                    600.0 + 20.0 * k as f32,
                    400.0,
                ))],
            );
        }
        frame_with(&mut app, &ctx, vec![press(at(700.0, 400.0), false)]);
        assert!(
            app.diagram.pan.x > pan.x + 50.0,
            "{:?} -> {:?}",
            pan,
            app.diagram.pan
        );

        // A two-finger swipe (wheel event) moves the view with the fingers, without zooming.
        let pan = app.diagram.pan;
        let wheel = |delta: egui::Vec2, modifiers: egui::Modifiers| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers,
        };
        frame_with(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(at(600.0, 400.0)),
                wheel(egui::vec2(-40.0, 30.0), egui::Modifiers::NONE),
            ],
        );
        for _ in 0..30 {
            frame_with(&mut app, &ctx, vec![]);
        }
        let moved = app.diagram.pan - pan;
        assert!(moved.x < -20.0 && moved.y > 15.0, "{moved:?}");
        assert_eq!(app.diagram.zoom, zoom, "swiping does not zoom");

        // Pinching apart zooms in around the pointer.
        frame_with(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(at(600.0, 400.0)),
                egui::Event::Zoom(1.5),
            ],
        );
        assert!(
            (app.diagram.zoom / zoom - 1.5).abs() < 1e-3,
            "{zoom} -> {}",
            app.diagram.zoom
        );
        // Ctrl + wheel zooms as well (the mouse users' pinch).
        let before = app.diagram.zoom;
        frame_with(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(at(600.0, 400.0)),
                wheel(egui::vec2(0.0, -120.0), egui::Modifiers::CTRL),
            ],
        );
        for _ in 0..30 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert!(app.diagram.zoom != before, "ctrl + wheel zooms");

        // With results: every heat metric and replay (also playing) draws.
        app.regenerate_skeleton();
        app.scenario.simulation.cases = Some(50);
        app.start_run(&ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.results.is_none() {
            assert!(
                Instant::now() < deadline && app.run_error.is_none(),
                "{:?}",
                app.run_error
            );
            std::thread::sleep(Duration::from_millis(10));
            frames(&mut app, &ctx, 1);
        }
        app.tab = Tab::Diagram;
        for m in [
            Some(sim_core::diagram::HeatMetric::Waiting),
            Some(sim_core::diagram::HeatMetric::Processing),
            Some(sim_core::diagram::HeatMetric::Runs),
            Some(sim_core::diagram::HeatMetric::Utilisation),
            None,
        ] {
            app.diagram.metric = m;
            frame_with(
                &mut app,
                &ctx,
                vec![egui::Event::PointerMoved(at(300.0, 300.0))],
            );
        }
        app.diagram.mode = Mode::Replay;
        app.diagram.playing = true;
        for _ in 0..10 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert!(app.diagram.time > 0.0, "replay advances while playing");
    }

    #[test]
    fn diagram_tab_without_layout_says_so() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v1-single-task.bpmn"));
        app.tab = Tab::Diagram;
        frames(&mut app, &ctx, 2);
        app.ws.open(&fixtures().join("multi-file/main.bpmn"));
        frames(&mut app, &ctx, 2);
    }

    /// Writes screenshots of the diagram view to `$SIM_GUI_SNAPSHOT_DIR` (run with
    /// `cargo test -p sim-gui -- --ignored screenshots`).
    #[test]
    #[ignore]
    fn screenshots() {
        let Some(dir) = std::env::var_os("SIM_GUI_SNAPSHOT_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        // Light theme: diagram, manual game highlights, validation colours.
        for (name, fixture, tab) in [
            (
                "light-diagram",
                "multi-file/subprocess-b.bpmn",
                Tab::Diagram,
            ),
            ("light-manual", "v13-model.bpmn", Tab::Diagram),
            ("light-model", "bad-model.bpmn", Tab::Model),
        ] {
            let ctx = egui::Context::default();
            ctx.set_theme(egui::ThemePreference::Light);
            let mut r = crate::snapshot::Renderer::new(1400, 800);
            let mut app = App::default();
            app.ws.open(&fixtures().join(fixture));
            app.tab = tab;
            if name == "light-manual" {
                let mut m = crate::manual_panel::ManualState::new(app.ws.project.clone());
                m.apply(sim_core::manual::Action::Start {
                    event: QId::new("v13-model.bpmn", "Start"),
                });
                let token = m.game.tokens()[0].id;
                m.apply(sim_core::manual::Action::Complete { token });
                app.diagram.manual = Some(m);
            }
            let mut out = None;
            for _ in 0..4 {
                out = Some(r.frame(&ctx, r.input(), |ui| app.draw(ui)));
            }
            r.save(&ctx, out.unwrap(), &dir.join(format!("{name}.png")));
        }
        {
            // Editors of splits that follow a decision output.
            let ctx = egui::Context::default();
            ctx.set_theme(egui::ThemePreference::Light);
            let mut r = crate::snapshot::Renderer::new(520, 560);
            let mut app = App::default();
            app.ws.open(&fixtures().join("dmn-output/ticket.bpmn"));
            app.regenerate_skeleton();
            let project = app.ws.project.clone();
            let mut out = None;
            for _ in 0..3 {
                out = Some(r.frame(&ctx, r.input(), |ui| {
                    for id in ["Gateway_Queue", "Gateway_Priority", "Gateway_Checks"] {
                        let n = project.node(&QId::new("ticket.bpmn", id)).unwrap();
                        ui.strong(n.label());
                        crate::editors::node_editor(ui, &project, &mut app.scenario, n, true);
                        ui.separator();
                    }
                }));
            }
            r.save(&ctx, out.unwrap(), &dir.join("editor-decision-output.png"));
            // Inputs of a decision with what it gives.
            let ctx = egui::Context::default();
            ctx.set_theme(egui::ThemePreference::Light);
            let mut r = crate::snapshot::Renderer::new(560, 900);
            let mut out = None;
            for _ in 0..3 {
                out = Some(r.frame(&ctx, r.input(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for id in ["Task_Triage", "Task_Checks"] {
                            let n = project.node(&QId::new("ticket.bpmn", id)).unwrap();
                            crate::editors::node_editor(ui, &project, &mut app.scenario, n, true);
                            ui.separator();
                        }
                    });
                }));
            }
            r.save(&ctx, out.unwrap(), &dir.join("editor-decision-inputs.png"));
        }
        {
            // Which characters the default fonts can draw.
            let ctx = egui::Context::default();
            let mut r = crate::snapshot::Renderer::new(500, 120);
            let mut out = None;
            for _ in 0..3 {
                out = Some(r.frame(&ctx, r.input(), |ui| {
                    ui.label(egui::RichText::new("dash — arrow → times × pm ± dots … x ✕ left ◀ right ▶ pause ⏸ dot ● tri ▲ sq ■ ⏵ ⏴ ⏩").size(18.0));
                }));
            }
            r.save(&ctx, out.unwrap(), &dir.join("glyphs.png"));
        }
        // Scenario and run tabs of a fresh skeleton and of a saved scenario.
        for (name, fixture, scenario, tab) in [
            ("scenario-main", "multi-file/main.bpmn", None, Tab::Scenario),
            (
                "scenario-v13",
                "v13-model.bpmn",
                Some("v13-model.toml"),
                Tab::Scenario,
            ),
            ("run-main", "multi-file/main.bpmn", None, Tab::Run),
        ] {
            let ctx = egui::Context::default();
            ctx.set_theme(egui::ThemePreference::Light);
            let mut r = crate::snapshot::Renderer::new(1400, 900);
            let mut app = App::default();
            app.ws.open(&fixtures().join(fixture));
            match scenario {
                Some(s) => {
                    let text = std::fs::read_to_string(fixtures().join(s)).unwrap();
                    app.scenario = Scenario::from_toml(&text).unwrap();
                }
                None => app.regenerate_skeleton(),
            }
            app.tab = tab;
            let project = app.ws.project.clone();
            let scenario = app.scenario.clone();
            app.pilot.finish(&ctx, &project, &scenario);
            let mut out = None;
            for _ in 0..4 {
                out = Some(r.frame(&ctx, r.input(), |ui| app.draw(ui)));
            }
            r.save(&ctx, out.unwrap(), &dir.join(format!("{name}.png")));
        }
        // Configure mode: badges, what needs input, an open editor, the pilot's traffic.
        for (name, fixture, scenario, open) in [
            (
                "configure-v13",
                "v13-model.bpmn",
                Some("v13-model.toml"),
                Some("Split"),
            ),
            (
                "configure-v13-task",
                "v13-model.bpmn",
                Some("v13-model.toml"),
                Some("Task_Intake"),
            ),
            ("configure-main", "multi-file/main.bpmn", None, None),
            (
                "configure-main-start",
                "multi-file/main.bpmn",
                None,
                Some("TimerStart"),
            ),
            (
                "configure-v13-dark",
                "v13-model.bpmn",
                Some("v13-model.toml"),
                Some("Task_Check"),
            ),
        ] {
            let ctx = egui::Context::default();
            ctx.set_theme(if name.ends_with("dark") {
                egui::ThemePreference::Dark
            } else {
                egui::ThemePreference::Light
            });
            let mut r = crate::snapshot::Renderer::new(1400, 800);
            let mut app = App::default();
            app.ws.open(&fixtures().join(fixture));
            match scenario {
                Some(s) => {
                    let text = std::fs::read_to_string(fixtures().join(s)).unwrap();
                    app.scenario = Scenario::from_toml(&text).unwrap();
                }
                None => app.regenerate_skeleton(),
            }
            app.tab = Tab::Diagram;
            app.diagram.mode = Mode::Configure;
            let project = app.ws.project.clone();
            let scenario = app.scenario.clone();
            app.pilot.finish(&ctx, &project, &scenario);
            for _ in 0..2 {
                r.frame(&ctx, r.input(), |ui| app.draw(ui));
            }
            if let Some(id) = open {
                let file = app.diagram.file.clone().unwrap();
                app.diagram.target = Some(Target::Node(QId::new(file, id)));
            }
            let mut out = None;
            for _ in 0..4 {
                out = Some(r.frame(&ctx, r.input(), |ui| app.draw(ui)));
            }
            r.save(&ctx, out.unwrap(), &dir.join(format!("{name}.png")));
        }
        for (name, fixture) in [
            ("v13", "v13-model.bpmn"),
            ("main", "multi-file/main.bpmn"),
            ("sub-b", "multi-file/subprocess-b.bpmn"),
            ("event-gateway", "event-gateway.bpmn"),
            ("task-types", "task-types.bpmn"),
            ("event-subprocess", "event-subprocess.bpmn"),
        ] {
            let ctx = egui::Context::default();
            let mut r = crate::snapshot::Renderer::new(1400, 800);
            let mut app = App::default();
            app.ws.open(&fixtures().join(fixture));
            app.tab = Tab::Diagram;
            let mut out = None;
            for _ in 0..4 {
                out = Some(r.frame(&ctx, r.input(), |ui| app.draw(ui)));
            }
            r.save(&ctx, out.unwrap(), &dir.join(format!("diagram-{name}.png")));
            if name == "v13" {
                // Replay: tokens of a few followed cases, then queues of all cases.
                let mut app = with_results("v13-model.bpmn", "v13-model.toml");
                app.tab = Tab::Diagram;
                app.diagram.mode = Mode::Replay;
                app.diagram.follow = true;
                app.diagram.case = 40;
                app.diagram.cases_shown = 6;
                let r0 = app.results.as_ref().unwrap().data[0].clone();
                // The busiest moment of cases 40..46.
                let cases: std::collections::BTreeSet<u64> = (40..46).collect();
                let travel = app.diagram.travel(&r0);
                app.diagram.time = r0
                    .trace
                    .iter()
                    .filter(|e| cases.contains(&e.case))
                    .map(|e| e.time + travel * 0.4)
                    .max_by_key(|t| {
                        sim_core::diagram::token_positions(&r0, &cases, *t, travel).len()
                    })
                    .unwrap_or(0.0);
                for _ in 0..3 {
                    r.frame(&ctx, r.input(), |ui| app.draw(ui));
                }
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("replay-tokens.png"));
                app.diagram.follow = false;
                app.diagram.time = (0..400)
                    .map(|k| r0.end_time * f64::from(k) / 400.0)
                    .max_by_key(|t| {
                        sim_core::diagram::snapshot(&r0, *t)
                            .values()
                            .map(|o| o.waiting * 10 + o.working)
                            .sum::<usize>()
                    })
                    .unwrap_or(0.0);
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("replay-queues.png"));
            }
            if name == "main" {
                // Manual game: the order is in subprocess A, waiting at its XOR split.
                let mut app = App::default();
                app.ws.open(&fixtures().join("multi-file/main.bpmn"));
                app.tab = Tab::Diagram;
                let mut m = crate::manual_panel::ManualState::new(app.ws.project.clone());
                m.game.stop_at_tasks = true;
                m.apply(sim_core::manual::Action::Start {
                    event: QId::new("main.bpmn", "TimerStart"),
                });
                app.diagram.manual = Some(m);
                for _ in 0..3 {
                    r.frame(&ctx, r.input(), |ui| app.draw(ui));
                }
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("manual-start.png"));
                // Complete tasks and go on from events until a gateway asks, then show subprocess A.
                for _ in 0..12 {
                    let m = app.diagram.manual.as_mut().unwrap();
                    let step = m.game.pending().into_iter().find_map(|p| match p {
                        sim_core::manual::Pending::Complete { token, .. } => {
                            Some(sim_core::manual::Action::Complete { token })
                        }
                        sim_core::manual::Pending::Continue { token, .. } => {
                            Some(sim_core::manual::Action::Continue { token })
                        }
                        _ => None,
                    });
                    match step {
                        Some(a) => m.apply(a),
                        None => break,
                    }
                }
                app.diagram.file = Some("subprocess-a.bpmn".into());
                app.diagram.fit = true;
                for _ in 0..3 {
                    r.frame(&ctx, r.input(), |ui| app.draw(ui));
                }
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("manual-sub-a.png"));
            }
            if name == "task-types" {
                // Close-up of the markers.
                app.diagram.zoom = 2.6;
                app.diagram.pan = egui::vec2(
                    10.0 - 100.0 * 2.6,
                    app.diagram.area.height() / 2.0 - 120.0 * 2.6,
                );
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("diagram-task-types-zoom.png"));
                app.diagram.pan.x -= 1250.0;
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("diagram-task-types-zoom2.png"));
                app.diagram.pan.x -= 1250.0;
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("diagram-task-types-zoom3.png"));
                app.diagram.pan.x -= 1250.0;
                let out = r.frame(&ctx, r.input(), |ui| app.draw(ui));
                r.save(&ctx, out, &dir.join("diagram-task-types-zoom4.png"));
            }
        }
        // Replay: the camera follows Ops into the called process, the customer on the right;
        // the heat map colours the call activities by what happens inside them.
        for (name, follow_ops) in [("replay-camera", true), ("heat-scopes", false)] {
            let ctx = egui::Context::default();
            ctx.set_theme(egui::ThemePreference::Light);
            let mut r = crate::snapshot::Renderer::new(1400, 800);
            let mut app = with_results("multi-file/main.bpmn", "multi-file/scenario.toml");
            app.tab = Tab::Diagram;
            let res = app.results.as_ref().unwrap().data[0].clone();
            if follow_ops {
                app.diagram.mode = Mode::Replay;
                app.diagram.follow = true;
                let ops = res
                    .instances
                    .iter()
                    .find(|i| i.case == 0 && i.parent.is_none() && i.process.id == "Process_Ops")
                    .unwrap()
                    .id;
                app.diagram.camera.promote(ops);
                let inside = res
                    .records
                    .iter()
                    .find(|x| x.case == 0 && x.activity.file == "subprocess-a.bpmn")
                    .unwrap();
                // Played up to there, so cams open as the other pools get busy.
                let to = (inside.start + inside.end) / 2.0;
                for step in 0..=60 {
                    app.diagram.time = to * f64::from(step) / 60.0;
                    r.frame(&ctx, r.input(), |ui| app.draw(ui));
                }
            } else {
                app.diagram.metric = Some(sim_core::diagram::HeatMetric::Waiting);
            }
            let mut out = None;
            for _ in 0..30 {
                out = Some(r.frame(&ctx, r.input(), |ui| app.draw(ui)));
            }
            r.save(&ctx, out.unwrap(), &dir.join(format!("{name}.png")));
        }
        // Halfway through the slide into the called diagram.
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::Light);
        let mut r = crate::snapshot::Renderer::new(1400, 800);
        let mut app = with_results("multi-file/main.bpmn", "multi-file/scenario.toml");
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Replay;
        app.diagram.follow = true;
        app.diagram.camera.set_auto();
        let mut out = None;
        for step in 0..=2000 {
            if app
                .diagram
                .camera
                .cut()
                .is_some_and(|c| c.progress() >= 0.5)
            {
                break;
            }
            if app.diagram.camera.cut().is_none() && !app.diagram.camera.holds_clock() {
                app.diagram.time = f64::from(step) * 6.0;
            }
            out = Some(r.frame(&ctx, r.input(), |ui| app.draw(ui)));
        }
        r.save(&ctx, out.unwrap(), &dir.join("replay-slide.png"));
    }

    #[test]
    fn clicking_a_call_activity_opens_the_called_diagram() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("multi-file/main.bpmn"));
        app.tab = Tab::Diagram;
        for _ in 0..3 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.diagram.file.as_deref(), Some("main.bpmn"));
        let click = |app: &mut App, p: egui::Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame_with(app, &ctx, vec![egui::Event::PointerMoved(p)]);
            frame_with(app, &ctx, vec![button(true)]);
            frame_with(app, &ctx, vec![button(false)]);
            frame_with(app, &ctx, vec![]);
        };
        let b = app.ws.project.files["main.bpmn"].diagram.shapes[&QId::new("main.bpmn", "Call_A")];
        let p = app
            .diagram
            .to_screen(b.x + b.width / 2.0, b.y + b.height / 2.0);
        click(&mut app, p);
        assert_eq!(app.diagram.file.as_deref(), Some("subprocess-a.bpmn"));
        assert_eq!(app.diagram.history, ["main.bpmn"]);
        assert!(
            !app.diagram.fit,
            "the called diagram was fitted to the view"
        );

        // Clicking a plain task does nothing.
        let t = app.ws.project.files["subprocess-a.bpmn"].diagram.shapes
            [&QId::new("subprocess-a.bpmn", "Task_Check")];
        let p = app.diagram.to_screen(t.x + 10.0, t.y + 10.0);
        click(&mut app, p);
        assert_eq!(app.diagram.file.as_deref(), Some("subprocess-a.bpmn"));
    }

    /// Load a model and its scenario and put a finished run into the app (no worker thread).
    fn with_results(fixture: &str, scenario: &str) -> App {
        with_results_edited(fixture, scenario, |_| {})
    }

    /// Like `with_results`, with the scenario changed by `edit` before the run.
    fn with_results_edited(fixture: &str, scenario: &str, edit: impl FnOnce(&mut Scenario)) -> App {
        let mut app = App::default();
        app.ws.open(&fixtures().join(fixture));
        let text = std::fs::read_to_string(fixtures().join(scenario)).unwrap();
        app.scenario = Scenario::from_toml(&text).unwrap();
        app.scenario.simulation.replications = 1;
        edit(&mut app.scenario);
        let bound = app.scenario.bind(&app.ws.project).unwrap();
        let data = sim_core::run_replications(&bound);
        app.results = Some(RunResults {
            data,
            project: app.ws.project.clone(),
            unit: app.scenario.simulation.time_unit,
        });
        app
    }

    #[test]
    fn replay_runs_in_real_time_and_follows_cases() {
        let ctx = egui::Context::default();
        let mut app = with_results("v13-model.bpmn", "v13-model.toml");
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Replay;
        app.diagram.speed = Some(3600.0); // one simulated hour per second
        app.diagram.skip_quiet = false;
        app.diagram.follow = false;
        frame_with(&mut app, &ctx, vec![]);
        app.diagram.playing = true;
        for _ in 0..60 {
            frame_with(&mut app, &ctx, vec![]);
        }
        // Headless frames are 1/60 s apart: about one simulated hour has passed, not the whole run.
        let t = app.diagram.time;
        assert!((3000.0..=4000.0).contains(&t), "{t}");

        // Following case 5: playback is limited to that case and stops after it.
        let r = app.results.as_ref().unwrap().data[0].clone();
        let c = &r.cases[5];
        app.diagram.follow = true;
        app.diagram.case = 5;
        app.diagram.time = c.arrival;
        app.diagram.speed = Some(86_400.0);
        app.diagram.playing = true;
        // The case lasts about half an hour; at one day per second (plus 0.6 s for the last
        // token to travel) 1.5 s of playback are more than enough.
        for _ in 0..90 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert!(!app.diagram.playing, "stopped at the end of the case");
        let end = c.completion.unwrap() + app.diagram.travel(&r);
        assert!(
            (app.diagram.time - end).abs() < 1e-6,
            "{} vs {end}",
            app.diagram.time
        );
    }

    #[test]
    fn replay_fast_forwards_through_quiet_times() {
        // The first case arrives Monday 00:00; the clerks start at 09:00.
        let office = |s: &mut Scenario| {
            s.simulation.cases = Some(3);
            s.simulation.warm_up_cases = 0;
            s.calendars
                .insert("office".into(), crate::editors::office_hours());
            for r in s.resources.values_mut() {
                r.calendar = Some("office".into());
            }
        };
        let ctx = egui::Context::default();
        let r = with_results_edited("v13-model.bpmn", "v13-model.toml", office)
            .results
            .unwrap()
            .data[0]
            .clone();
        let opens = r.records[0].start;
        assert_eq!(opens, 9.0 * 3600.0);
        for skip in [true, false] {
            let mut app = with_results_edited("v13-model.bpmn", "v13-model.toml", office);
            app.tab = Tab::Diagram;
            app.diagram.mode = Mode::Replay;
            app.diagram.speed = Some(60.0); // one simulated minute per second
            app.diagram.skip_quiet = skip;
            frame_with(&mut app, &ctx, vec![]);
            app.diagram.playing = true;
            // 0.6 s for the token to reach the task, then about 1 s through the night.
            for _ in 0..110 {
                frame_with(&mut app, &ctx, vec![]);
            }
            let t = app.diagram.time;
            if skip {
                // Normal speed again half a second (30 simulated seconds) before 09:00.
                assert!((opens - 60.0..opens).contains(&t), "{t}");
                assert!(app.diagram.ff.is_none());
                for _ in 0..60 {
                    frame_with(&mut app, &ctx, vec![]);
                }
                // On past 09:00 (the 4-minute task is quiet at this speed too).
                assert!(app.diagram.time > opens, "{}", app.diagram.time);
            } else {
                assert!(t < 120.0, "{t}");
            }
        }
    }

    #[test]
    fn replay_camera_follows_into_called_files_and_shows_the_other_pools() {
        use crate::camera;
        let ctx = egui::Context::default();
        let mut app = with_results("multi-file/main.bpmn", "multi-file/scenario.toml");
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Replay;
        app.diagram.follow = true;
        frame_with(&mut app, &ctx, vec![]);
        let r = app.results.as_ref().unwrap().data[0].clone();
        let project = app.ws.project.clone();
        let top = |process: &str| {
            r.instances
                .iter()
                .find(|i| i.case == 0 && i.parent.is_none() && i.process.id == process)
                .unwrap()
                .id
        };
        let (customer, ops) = (top("Process_Customer"), top("Process_Ops"));
        // By default the main view follows the pool of the first start event.
        assert_eq!(app.diagram.camera.main_root(&r, 0), Some(customer));
        assert_eq!(camera::name_of(&project, &r, customer), "Customer");

        // Following Ops while it works in the called process: the camera switches files.
        app.diagram.camera.promote(ops);
        let inside = r
            .records
            .iter()
            .find(|x| x.case == 0 && x.activity.file == "subprocess-a.bpmn")
            .unwrap();
        let t = (inside.start + inside.end) / 2.0;
        app.diagram.time = t;
        // Playing: a beat to see the token at the call activity, then a slide into the called
        // diagram (from the right); the clock waits for both.
        app.diagram.playing = true;
        frame_with(&mut app, &ctx, vec![]);
        assert_eq!(app.diagram.file.as_deref(), Some("main.bpmn"));
        let held = app.diagram.time;
        let mut frames = 0;
        while app.diagram.file.as_deref() == Some("main.bpmn") && frames < 300 {
            frame_with(&mut app, &ctx, vec![]);
            frames += 1;
        }
        assert!(frames > 3, "switched after {frames} frames");
        assert_eq!(app.diagram.file.as_deref(), Some("subprocess-a.bpmn"));
        assert_eq!(app.diagram.history, ["main.bpmn"]);
        let cut = app.diagram.camera.cut().expect("slides in");
        assert_eq!((cut.file.as_str(), cut.dir), ("main.bpmn", 1.0));
        while app.diagram.camera.cut().is_some() && frames < 600 {
            assert_eq!(app.diagram.time, held, "the clock waits");
            frame_with(&mut app, &ctx, vec![]);
            frames += 1;
        }
        assert!(app.diagram.camera.cut().is_none());
        frame_with(&mut app, &ctx, vec![]);
        assert!(app.diagram.time > held, "and goes on afterwards");
        app.diagram.playing = false;
        app.diagram.time = t;
        // The customer waits in main.bpmn, out of sight: an action cam or a chip.
        let v = &mut app.diagram;
        let tokens = sim_core::diagram::token_positions(&r, &v.followed(), t, v.travel(&r));
        let busy_for = v.speed_of(&r) * camera::BUSY_SECONDS;
        let lineages = camera::lineages(&project, &r, 0, &tokens, t, busy_for);
        assert!(lineages.iter().any(|l| l.root == ops));
        let side = camera::side(v, &project, &r, lineages, 0.0);
        let shown: Vec<u64> = side
            .cams
            .iter()
            .map(|c| c.lineage.root)
            .chain(side.chips.iter().map(|l| l.root))
            .collect();
        assert!(shown.contains(&customer), "{shown:?}");
        assert!(!shown.contains(&ops), "the main view shows it");

        // Moved by hand: the view stays, even when the followed token is elsewhere.
        app.diagram.camera.paused = true;
        app.diagram.camera.promote(customer);
        app.diagram.camera.paused = true;
        frame_with(&mut app, &ctx, vec![]);
        assert_eq!(app.diagram.file.as_deref(), Some("subprocess-a.bpmn"));
        // Following the customer again brings the view back to its pool.
        app.diagram.camera.paused = false;
        for _ in 0..120 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert_eq!(app.diagram.file.as_deref(), Some("main.bpmn"));
        assert!(app.diagram.history.is_empty());
        // Playing on draws the side column and keeps working to the end of the case.
        app.diagram.playing = true;
        for _ in 0..120 {
            frame_with(&mut app, &ctx, vec![]);
        }
    }

    #[test]
    fn replay_action_cams_stay_while_their_instance_waits() {
        use crate::camera;
        let ctx = egui::Context::default();
        let mut app = with_results("multi-file/main.bpmn", "multi-file/scenario.toml");
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Replay;
        app.diagram.follow = true;
        frame_with(&mut app, &ctx, vec![]);
        let r = app.results.as_ref().unwrap().data[0].clone();
        let case: Vec<_> = r.instances.iter().filter(|i| i.case == 0).collect();
        let start = case.iter().map(|i| i.start).fold(f64::INFINITY, f64::min);
        let end = case.iter().filter_map(|i| i.end).fold(start, f64::max);
        let main = app.diagram.camera.main_root(&r, 0).unwrap();
        let (followed, travel) = (app.diagram.followed(), app.diagram.travel(&r));
        let alive = |t: f64| -> Vec<u64> {
            sim_core::diagram::token_positions(&r, &followed, t, travel)
                .iter()
                .map(|tok| camera::root_of(&r, tok.instance))
                .collect()
        };
        let mut before: Vec<u64> = Vec::new();
        let mut opened = 0;
        for step in 0..=200 {
            let t = start + (end - start) * f64::from(step) / 200.0;
            app.diagram.time = t;
            frame_with(&mut app, &ctx, vec![]);
            let now = app.diagram.camera.cam_roots();
            assert!(!now.contains(&main), "the main view's instance gets no cam");
            // A cam stays (in its slot) until its instance has no tokens any more.
            let living = alive(t);
            let kept: Vec<u64> = before
                .iter()
                .copied()
                .filter(|c| living.contains(c))
                .collect();
            let stayed: Vec<u64> = now.iter().copied().filter(|c| kept.contains(c)).collect();
            assert_eq!(stayed, kept, "at {t}: {before:?} -> {now:?}");
            opened += now.iter().filter(|c| !before.contains(c)).count();
            before = now;
        }
        assert!(opened > 0, "no action cam opened");
    }

    #[test]
    fn replay_auto_focus_follows_the_pool_with_the_most_going_on_and_cams_show_the_rest() {
        use crate::camera;
        let ctx = egui::Context::default();
        let mut app = with_results("multi-file/main.bpmn", "multi-file/scenario.toml");
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Replay;
        app.diagram.follow = true;
        frame_with(&mut app, &ctx, vec![]);
        app.diagram.camera.set_auto();
        let r = app.results.as_ref().unwrap().data[0].clone();
        let case: Vec<_> = r.instances.iter().filter(|i| i.case == 0).collect();
        let start = case.iter().map(|i| i.start).fold(f64::INFINITY, f64::min);
        let end = case.iter().filter_map(|i| i.end).fold(start, f64::max);
        let at = |step: u32| start + (end - start) * f64::from(step) / 400.0;
        let mut followed = std::collections::BTreeSet::new();
        let mut files = std::collections::BTreeSet::new();
        let (mut cams_seen, mut handed_over) = (false, 0);
        let mut before = (app.diagram.camera.main, app.diagram.camera.cam_roots());
        for step in 0..=400 {
            app.diagram.time = at(step);
            frame_with(&mut app, &ctx, vec![]);
            let cam = &app.diagram.camera;
            assert!(cam.auto);
            let (main, cams) = (cam.main, cam.cam_roots());
            assert!(
                main.is_none_or(|m| !cams.contains(&m)),
                "the focus gets no cam"
            );
            cams_seen |= !cams.is_empty();
            // Focus moved to a pool with a cam: the pool focused so far takes that cam.
            if let ((Some(old), old_cams), Some(new)) = (&before, main)
                && *old != new
                && let Some(i) = old_cams.iter().position(|c| *c == new)
                && i < cams.len()
            {
                assert_eq!(cams[i], *old, "step {step}: {old_cams:?} -> {cams:?}");
                handed_over += 1;
            }
            followed.extend(main);
            files.extend(app.diagram.file.clone());
            before = (main, cams);
        }
        // It cut between pools and followed them into the called diagrams; the others had cams.
        assert!(followed.len() > 1, "{followed:?}");
        assert!(files.len() > 1, "{files:?}");
        assert!(cams_seen, "no action cams in auto focus");
        assert!(handed_over > 0, "no hand-over seen");

        // Clicking a cam follows that pool and ends auto focus; the focused one takes the cam.
        app.diagram.camera = camera::Camera::default();
        app.diagram.camera.set_auto();
        let mut step = 0;
        while app.diagram.camera.cam_roots().is_empty() && step < 400 {
            app.diagram.time = at(step);
            frame_with(&mut app, &ctx, vec![]);
            step += 1;
        }
        let focus = app.diagram.camera.main_root(&r, 0).unwrap();
        let clicked = app.diagram.camera.cam_roots()[0];
        camera::follow(&mut app.diagram, &r, clicked);
        assert!(!app.diagram.camera.auto);
        assert_eq!(app.diagram.camera.main, Some(clicked));
        assert_eq!(app.diagram.camera.cam_roots()[0], focus);
    }

    #[test]
    fn pool_editor_defines_and_edits_working_hours() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v13-model.bpmn"));
        app.regenerate_skeleton();
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Configure;
        let cals = &mut app.scenario.calendars;
        assert_eq!(crate::editors::add_calendar(cals), "office");
        assert_eq!(crate::editors::add_calendar(cals), "office 2");
        assert_eq!(cals["office"], crate::editors::office_hours());
        cals.get_mut("office 2").unwrap().periods[0].hours = "17:00-09:00".into();
        // Other hours at the weekend.
        let weekend = sim_core::calendar::PeriodSpec {
            days: vec!["sat".into(), "sun".into()],
            hours: "08:00-24:00".into(),
        };
        cals.get_mut("office").unwrap().periods.push(weekend);
        // Shared, invalid and undefined working hours all draw in the pool's editor.
        for (clerks, experts) in [
            ("office", "office"),
            ("office 2", "office"),
            ("night", "office"),
        ] {
            app.scenario.resources.get_mut("Clerks").unwrap().calendar = Some(clerks.into());
            app.scenario.resources.get_mut("Experts").unwrap().calendar = Some(experts.into());
            app.focus(Target::Pool("Clerks".into()));
            frame_with(&mut app, &ctx, vec![]);
            frame_with(&mut app, &ctx, vec![]);
            assert_eq!(app.diagram.target, Some(Target::Pool("Clerks".into())));
        }
        app.tab = Tab::Scenario;
        frame_with(&mut app, &ctx, vec![]);
    }

    #[test]
    fn arrival_editor_picks_and_edits_working_hours() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v13-model.bpmn"));
        app.regenerate_skeleton();
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Configure;
        let office = crate::editors::add_calendar(&mut app.scenario.calendars);
        app.scenario.resources.get_mut("Clerks").unwrap().calendar = Some(office.clone());
        let start = Target::Node(QId::new("v13-model.bpmn", "Start"));
        // Shared with a pool, and undefined: both draw in the start event's editor.
        for c in [office.as_str(), "night"] {
            app.scenario.arrivals[0].calendar = Some(c.into());
            app.focus(start.clone());
            frame_with(&mut app, &ctx, vec![]);
            frame_with(&mut app, &ctx, vec![]);
            assert_eq!(app.diagram.target, Some(start.clone()));
        }
        app.tab = Tab::Scenario;
        frame_with(&mut app, &ctx, vec![]);
        // The quick check and the run take the arrivals' working hours.
        app.scenario.arrivals[0].calendar = Some(office);
        let project = app.ws.project.clone();
        let bound = app.scenario.bind(&project).expect("binds");
        assert!(bound.arrivals[0].calendar.is_some());
    }

    #[test]
    fn configure_mode_opens_editors_by_clicking() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v13-model.bpmn"));
        app.regenerate_skeleton();
        app.tab = Tab::Diagram;
        app.diagram.mode = Mode::Configure;
        for _ in 0..3 {
            frame_with(&mut app, &ctx, vec![]);
        }
        let q = |id: &str| QId::new("v13-model.bpmn", id);
        // A fresh skeleton is all placeholders: tasks, the split, the start event, the lanes.
        for t in [
            Target::Node(q("Task_Intake")),
            Target::Node(q("Split")),
            Target::Node(q("Start")),
            Target::Pool("Clerks".into()),
        ] {
            assert!(app.unreviewed.contains(&t), "{t:?}");
        }
        let shapes = app.ws.project.files["v13-model.bpmn"].diagram.clone();
        let b = shapes.shapes[&q("Task_Intake")];
        let p = app
            .diagram
            .to_screen(b.x + b.width / 2.0, b.y + b.height / 2.0);
        click_at(&mut app, &ctx, p);
        assert_eq!(app.diagram.target, Some(Target::Node(q("Task_Intake"))));
        // Escape closes the editor.
        let esc = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame_with(&mut app, &ctx, vec![esc]);
        assert_eq!(app.diagram.target, None);
        // The name band of a lane stands for its resource pool.
        let lane = shapes.shapes[&q("Lane_Clerks")];
        let p = app
            .diagram
            .to_screen(lane.x + 5.0, lane.y + lane.height / 2.0);
        click_at(&mut app, &ctx, p);
        assert_eq!(app.diagram.target, Some(Target::Pool("Clerks".into())));
        // A click on empty space closes it.
        let lane = shapes.shapes[&q("Lane_Experts")];
        let p = app
            .diagram
            .to_screen(lane.x + lane.width - 15.0, lane.y + lane.height - 15.0);
        click_at(&mut app, &ctx, p);
        assert_eq!(app.diagram.target, None);

        // Jumping to an element from elsewhere opens its editor in the diagram.
        app.tab = Tab::Scenario;
        app.focus(Target::Node(q("Split")));
        frame_with(&mut app, &ctx, vec![]);
        assert_eq!(app.tab, Tab::Diagram);
        assert_eq!(app.diagram.target, Some(Target::Node(q("Split"))));

        // Updating from the model marks only the new placeholders.
        app.unreviewed.clear();
        app.scenario.activities.remove("v13-model.bpmn#Task_Close");
        let u = app.update_from_model();
        assert_eq!(u.added, [q("Task_Close")]);
        assert_eq!(
            app.unreviewed.iter().collect::<Vec<_>>(),
            [&Target::Node(q("Task_Close"))]
        );
    }

    fn click_at(app: &mut App, ctx: &egui::Context, p: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame_with(app, ctx, vec![egui::Event::PointerMoved(p)]);
        frame_with(app, ctx, vec![button(true)]);
        frame_with(app, ctx, vec![button(false)]);
        frame_with(app, ctx, vec![]);
    }

    #[test]
    fn manual_simulation_is_played_by_clicking() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v13-model.bpmn"));
        app.tab = Tab::Diagram;
        app.diagram.manual = Some(crate::manual_panel::ManualState::new(
            app.ws.project.clone(),
        ));
        for _ in 0..3 {
            frame_with(&mut app, &ctx, vec![]);
        }
        let shapes = app.ws.project.files["v13-model.bpmn"].diagram.clone();
        let center = |app: &App, id: &str| {
            let b = shapes.shapes[&QId::new("v13-model.bpmn", id)];
            app.diagram
                .to_screen(b.x + b.width / 2.0, b.y + b.height / 2.0)
        };
        let at = |app: &App| -> Vec<String> {
            app.diagram
                .manual
                .as_ref()
                .unwrap()
                .game
                .tokens()
                .iter()
                .map(|t| t.node.id.clone())
                .collect()
        };
        // Click the start event: the token moves to the first task and waits there.
        let p = center(&app, "Start");
        click_at(&mut app, &ctx, p);
        assert_eq!(at(&app), ["Task_Intake"]);
        // Click the task to complete it: the token goes on to the XOR split.
        let p = center(&app, "Task_Intake");
        click_at(&mut app, &ctx, p);
        assert_eq!(at(&app), ["Split"]);
        // Click the middle of the "simple" flow, then complete the remaining tasks.
        let edge = &shapes.edges[&QId::new("v13-model.bpmn", "Flow_simple")];
        let (a, b) = (edge[1], edge[2]);
        let p = app.diagram.to_screen((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        click_at(&mut app, &ctx, p);
        assert_eq!(at(&app), ["Task_Simple"]);
        for task in ["Task_Simple", "Task_Close"] {
            let p = center(&app, task);
            click_at(&mut app, &ctx, p);
        }
        let game = &app.diagram.manual.as_ref().unwrap().game;
        assert!(game.tokens().is_empty(), "{:?}", game.tokens());
        assert_eq!(game.finished().len(), 1);
    }

    #[test]
    fn manual_simulation_follows_tokens_into_called_files_and_back() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("multi-file/main.bpmn"));
        app.tab = Tab::Diagram;
        app.diagram.manual = Some(crate::manual_panel::ManualState::new(
            app.ws.project.clone(),
        ));
        frame_with(&mut app, &ctx, vec![]);
        let m = app.diagram.manual.as_mut().unwrap();
        m.apply(sim_core::manual::Action::Start {
            event: QId::new("main.bpmn", "TimerStart"),
        });
        let mut seen = vec![(
            app.diagram.file.clone().unwrap(),
            app.diagram.history.clone(),
        )];
        for _ in 0..60 {
            frame_with(&mut app, &ctx, vec![]);
            let here = (
                app.diagram.file.clone().unwrap(),
                app.diagram.history.clone(),
            );
            if seen.last() != Some(&here) {
                seen.push(here);
            }
            // Act on the token in focus, as a player following the case would.
            let m = app.diagram.manual.as_mut().unwrap();
            let Some(f) = m.game.focus() else { break };
            let action = m.game.pending().into_iter().find_map(|p| match p {
                sim_core::manual::Pending::Complete { token, task } if task == f => {
                    Some(sim_core::manual::Action::Complete { token })
                }
                sim_core::manual::Pending::Choose {
                    token,
                    gateway,
                    flows,
                    ..
                } if gateway == f => Some(sim_core::manual::Action::Choose {
                    token,
                    flows: vec![flows[0].clone()],
                }),
                sim_core::manual::Pending::Continue { token, at } if at == f => {
                    Some(sim_core::manual::Action::Continue { token })
                }
                _ => None,
            });
            if let Some(a) = action {
                m.apply(a);
            }
        }
        let files: Vec<&str> = seen.iter().map(|(f, _)| f.as_str()).collect();
        // Into A, back to the main file (signal and message events wait there), into B, back.
        assert_eq!(
            files,
            [
                "main.bpmn",
                "subprocess-a.bpmn",
                "main.bpmn",
                "subprocess-b.bpmn",
                "main.bpmn"
            ]
        );
        // While inside a called file, "Back" leads to its caller's file.
        assert_eq!(seen[1].1, ["main.bpmn"]);
        assert!(seen[2].1.is_empty());
        assert_eq!(seen[3].1, ["main.bpmn"]);
        assert!(seen[4].1.is_empty());
    }

    #[test]
    fn folder_open_remove_and_errors() {
        let mut app = App::default();
        app.ws.open(&fixtures().join("multi-file"));
        assert_eq!(app.ws.project.files.len(), 4);
        assert_eq!(app.ws.project.dmn.len(), 3);
        let unrelated = app
            .ws
            .paths
            .iter()
            .position(|p| p.ends_with("unrelated.bpmn"))
            .unwrap();
        app.ws.remove(unrelated);
        assert_eq!(app.ws.project.files.len(), 3);
        // Two files defining the same process id: loading stops with an error naming both.
        app.ws.open(&fixtures().join("dup-process"));
        let e = app.ws.load_error.clone().unwrap();
        assert!(e.contains("Process_SubA"), "{e}");
        let ctx = egui::Context::default();
        app.tab = Tab::Model;
        frames(&mut app, &ctx, 2);
    }

    #[test]
    fn run_with_invalid_scenario_reports_error() {
        let ctx = egui::Context::default();
        let mut app = App::default();
        app.ws.open(&fixtures().join("v5-and.bpmn"));
        app.start_run(&ctx); // pristine empty scenario: cannot bind
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.run_error.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
            frames(&mut app, &ctx, 1);
        }
        assert!(app.results.is_none());
    }
}
