use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use sim_core::scenario::TimeUnit;
use sim_core::{Project, QId, ReplicationResult, Scenario};

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
        }
    }
}

impl App {
    /// A scenario that still has nothing configured can be replaced by a fresh skeleton.
    pub fn scenario_is_pristine(&self) -> bool {
        self.scenario.activities.is_empty() && self.scenario.gateways.is_empty()
    }

    pub fn regenerate_skeleton(&mut self) {
        self.scenario = Scenario::skeleton(&self.ws.project, None);
        self.scenario_path = None;
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
            CalendarSpec {
                days: ["mon", "tue", "wed", "thu", "fri"]
                    .map(String::from)
                    .to_vec(),
                hours: "09:00-17:00".into(),
            },
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

        // The mouse wheel zooms in.
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, 120.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Unknown,
            modifiers: egui::Modifiers::NONE,
        };
        frame_with(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(at(600.0, 400.0)), wheel],
        );
        for _ in 0..30 {
            frame_with(&mut app, &ctx, vec![]);
        }
        assert!(
            app.diagram.zoom > zoom * 1.05,
            "{zoom} -> {}",
            app.diagram.zoom
        );

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
        app.diagram.replay = true;
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
        for (name, fixture) in [
            ("v13", "v13-model.bpmn"),
            ("main", "multi-file/main.bpmn"),
            ("sub-b", "multi-file/subprocess-b.bpmn"),
            ("event-gateway", "event-gateway.bpmn"),
            ("task-types", "task-types.bpmn"),
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
                app.diagram.replay = true;
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
                // Complete tasks until a gateway asks, then show subprocess A.
                for _ in 0..6 {
                    let m = app.diagram.manual.as_mut().unwrap();
                    if let Some(sim_core::manual::Pending::Complete { token, .. }) = m
                        .game
                        .pending()
                        .into_iter()
                        .find(|p| matches!(p, sim_core::manual::Pending::Complete { .. }))
                    {
                        m.apply(sim_core::manual::Action::Complete { token });
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
        let mut app = App::default();
        app.ws.open(&fixtures().join(fixture));
        let text = std::fs::read_to_string(fixtures().join(scenario)).unwrap();
        app.scenario = Scenario::from_toml(&text).unwrap();
        app.scenario.simulation.replications = 1;
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
        app.diagram.replay = true;
        app.diagram.speed = Some(3600.0); // one simulated hour per second
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
                _ => None,
            });
            if let Some(a) = action {
                m.apply(a);
            }
        }
        let files: Vec<&str> = seen.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(
            files,
            [
                "main.bpmn",
                "subprocess-a.bpmn",
                "subprocess-b.bpmn",
                "main.bpmn"
            ]
        );
        // While inside a called file, "Back" leads to its caller's file.
        assert_eq!(seen[1].1, ["main.bpmn"]);
        assert_eq!(seen[2].1, ["main.bpmn"]);
        assert!(seen[3].1.is_empty());
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
