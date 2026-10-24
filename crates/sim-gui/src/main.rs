// No console window next to the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
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
mod widgets;
mod workspace;

fn main() -> eframe::Result {
    eframe::run_native(
        "BPMN Simulator",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(app::App::default()))),
    )
}
