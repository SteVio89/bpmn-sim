//! Opening and saving files: file dialogs on the desktop, uploads and downloads in the
//! browser. Browser dialogs are asynchronous, so picked files arrive in an inbox that the app
//! empties every frame; on the desktop they arrive there right away.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use eframe::egui;

/// What the user picked files for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    OpenModel,
    AddModel,
    Scenario,
}

/// A picked file: a path on the desktop, the contents in the browser (and for files dropped
/// there).
#[derive(Clone, Debug)]
pub enum Picked {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Path(PathBuf),
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    Text { name: String, text: String },
}

impl Picked {
    pub fn name(&self) -> String {
        match self {
            Picked::Path(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Picked::Text { name, .. } => name.clone(),
        }
    }
}

pub struct Filter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

#[cfg(not(target_arch = "wasm32"))]
pub const BPMN: Filter = Filter {
    name: "BPMN",
    extensions: &["bpmn"],
};
pub const BPMN_DMN: Filter = Filter {
    name: "BPMN, DMN",
    extensions: &["bpmn", "dmn"],
};
pub const TOML: Filter = Filter {
    name: "TOML",
    extensions: &["toml"],
};

type Inbox = Rc<RefCell<Vec<(Purpose, Vec<Picked>)>>>;

#[derive(Default)]
pub struct Files {
    inbox: Inbox,
}

impl Files {
    /// Ask for one file (or several) and deliver them to the inbox.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn pick(&self, ctx: &egui::Context, purpose: Purpose, filter: &Filter, multiple: bool) {
        let dialog = rfd::FileDialog::new().add_filter(filter.name, filter.extensions);
        let picked: Vec<PathBuf> = if multiple {
            dialog.pick_files().unwrap_or_default()
        } else {
            dialog.pick_file().into_iter().collect()
        };
        if !picked.is_empty() {
            let picked = picked.into_iter().map(Picked::Path).collect();
            self.inbox.borrow_mut().push((purpose, picked));
            ctx.request_repaint();
        }
    }

    /// Ask for one file (or several) and deliver them to the inbox.
    #[cfg(target_arch = "wasm32")]
    pub fn pick(&self, ctx: &egui::Context, purpose: Purpose, filter: &Filter, multiple: bool) {
        let dialog = rfd::AsyncFileDialog::new().add_filter(filter.name, filter.extensions);
        let inbox = self.inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let handles = if multiple {
                dialog.pick_files().await.unwrap_or_default()
            } else {
                dialog.pick_file().await.into_iter().collect()
            };
            let mut picked = Vec::new();
            for h in handles {
                let text = String::from_utf8_lossy(&h.read().await).into_owned();
                picked.push(Picked::Text {
                    name: h.file_name(),
                    text,
                });
            }
            if !picked.is_empty() {
                inbox.borrow_mut().push((purpose, picked));
                ctx.request_repaint();
            }
        });
    }

    /// Files dropped onto the window: models (and their DMN files) open as a new set, a
    /// scenario file is loaded.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn take_dropped(&self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let picked = dropped.iter().map(|f| Picked::Path(f.path().to_path_buf()));
            sort_dropped(&mut self.inbox.borrow_mut(), picked.collect());
        }
    }

    /// Files dropped onto the window: models (and their DMN files) open as a new set, a
    /// scenario file is loaded.
    #[cfg(target_arch = "wasm32")]
    pub fn take_dropped(&self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        let inbox = self.inbox.clone();
        let ctx = ctx.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut picked = Vec::new();
            for f in &dropped {
                if let Ok(bytes) = f.bytes_async().await {
                    picked.push(Picked::Text {
                        name: f.path().to_string_lossy().into_owned(),
                        text: String::from_utf8_lossy(&bytes).into_owned(),
                    });
                }
            }
            sort_dropped(&mut inbox.borrow_mut(), picked);
            ctx.request_repaint();
        });
    }

    pub fn take(&self) -> Vec<(Purpose, Vec<Picked>)> {
        std::mem::take(&mut self.inbox.borrow_mut())
    }
}

fn sort_dropped(inbox: &mut Vec<(Purpose, Vec<Picked>)>, dropped: Vec<Picked>) {
    let (mut models, mut scenarios) = (Vec::new(), Vec::new());
    for p in dropped {
        let name = p.name();
        if name.ends_with(".bpmn") || name.ends_with(".dmn") {
            models.push(p);
        } else if name.ends_with(".toml") {
            scenarios.push(p);
        }
    }
    if !models.is_empty() {
        inbox.push((Purpose::OpenModel, models));
    }
    // Several dropped scenarios: the last one wins, like loading them one after another.
    if let Some(s) = scenarios.pop() {
        inbox.push((Purpose::Scenario, vec![s]));
    }
}

/// Save text under `name`: on the desktop the user chooses where, in the browser it is
/// downloaded. Returns where it went (just `name` in the browser), or `None` if cancelled.
#[cfg(not(target_arch = "wasm32"))]
pub fn save(
    name: &str,
    filter: Option<&Filter>,
    content: impl FnOnce() -> Result<String, String>,
) -> Option<Result<PathBuf, String>> {
    let mut dialog = rfd::FileDialog::new().set_file_name(name);
    if let Some(f) = filter {
        dialog = dialog.add_filter(f.name, f.extensions);
    }
    let path = dialog.save_file()?;
    Some(
        content()
            .and_then(|c| std::fs::write(&path, c).map_err(|e| e.to_string()))
            .map(|()| path),
    )
}

/// Save text under `name`: on the desktop the user chooses where, in the browser it is
/// downloaded. Returns where it went (just `name` in the browser), or `None` if cancelled.
#[cfg(target_arch = "wasm32")]
pub fn save(
    name: &str,
    _filter: Option<&Filter>,
    content: impl FnOnce() -> Result<String, String>,
) -> Option<Result<PathBuf, String>> {
    Some(
        content()
            .and_then(|c| download(name, &c).map_err(|e| format!("{e:?}")))
            .map(|()| PathBuf::from(name)),
    )
}

#[cfg(target_arch = "wasm32")]
fn download(name: &str, content: &str) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(content));
    let blob = web_sys::Blob::new_with_str_sequence(&parts)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a")?.dyn_into()?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    // The URL is not revoked: the download may not have started yet, and it is freed with the
    // page anyway.
    Ok(())
}
