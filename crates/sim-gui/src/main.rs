// No console window next to the app on Windows (release builds).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod camera;
mod configure;
mod decision_inputs;
mod diagram_icons;
mod diagram_panel;
mod editors;
mod examples;
mod job;
mod manual_panel;
mod model_panel;
mod platform;
mod results_panel;
mod scenario_panel;
#[cfg(test)]
mod snapshot;
#[cfg(not(target_arch = "wasm32"))]
mod update;
mod widgets;
mod workspace;

/// Where the desktop app can be downloaded (linked from the browser version).
#[cfg(target_arch = "wasm32")]
const RELEASES_URL: &str = "https://github.com/SteVio89/bpmn-sim/releases/latest";

#[cfg(not(target_arch = "wasm32"))]
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

/// In the browser: run on the canvas of `index.html` and drop the loading text once started.
#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .expect("no document");
        let canvas = document
            .get_element_by_id("app")
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("no canvas with id \"app\"");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|_cc| Ok(Box::new(app::App::default()))),
            )
            .await;
        if let Some(loading) = document.get_element_by_id("loading") {
            match started {
                Ok(()) => loading.remove(),
                Err(e) => loading.set_text_content(Some(&format!(
                    "BPMN Sim could not start in this browser: {e:?}"
                ))),
            }
        }
    });
}
