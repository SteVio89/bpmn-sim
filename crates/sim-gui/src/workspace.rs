//! The loaded model files, independent of the UI so it can be tested.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sim_core::Project;

#[derive(Default)]
pub struct Workspace {
    pub paths: Vec<PathBuf>,
    pub project: Arc<Project>,
    /// Error from the last (re)load; files before the failing one stay loaded.
    pub load_error: Option<String>,
}

impl Workspace {
    /// Open a folder (every `.bpmn` and `.dmn` file in it) or a main file together with the
    /// files it references (call activities, business rule tasks), replacing the current set.
    pub fn open(&mut self, path: &Path) {
        self.paths = if path.is_dir() {
            let mut v: Vec<PathBuf> = std::fs::read_dir(path)
                .map(|rd| {
                    rd.filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().is_some_and(|x| x == "bpmn" || x == "dmn"))
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        } else {
            sim_core::discover_files(path).unwrap_or_else(|_| vec![path.to_path_buf()])
        };
        self.reload();
    }

    pub fn add(&mut self, path: PathBuf) {
        if !self.paths.contains(&path) {
            self.paths.push(path);
        }
        self.reload();
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.paths.len() {
            self.paths.remove(index);
            self.reload();
        }
    }

    pub fn reload(&mut self) {
        let mut project = Project::new();
        self.load_error = None;
        for p in &self.paths {
            if let Err(e) = project.add_path(p) {
                self.load_error = Some(e.to_string());
                break;
            }
        }
        self.project = Arc::new(project);
    }
}
