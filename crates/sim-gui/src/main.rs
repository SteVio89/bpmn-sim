mod app;
mod job;
mod model_panel;
mod results_panel;
mod scenario_panel;
mod widgets;
mod workspace;

fn main() -> eframe::Result {
    eframe::run_native(
        "BPMN Simulator",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(app::App::default()))),
    )
}
