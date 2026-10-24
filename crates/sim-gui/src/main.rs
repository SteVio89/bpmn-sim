// No console window next to the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod camera;
mod configure;
mod decision_inputs;
mod diagram_icons;
mod diagram_panel;
mod editors;
mod job;
mod manual_panel;
mod model_panel;
mod results_panel;
mod scenario_panel;
#[cfg(test)]
mod snapshot;
mod update;
mod widgets;
mod workspace;

fn main() -> eframe::Result {
    eframe::run_native(
        "BPMN Simulator",
        eframe::NativeOptions::default(),
        Box::new(|cc| {
            let mut app = app::App::default();
            app.updater.check(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
