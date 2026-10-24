//! The loaded model files, independent of the UI so it can be tested.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sim_core::Project;

/// One model file: on disk (re-read on every reload), or given as text (an upload in the
/// browser, a built-in example).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Path(PathBuf),
    Text { name: String, text: String },
}

impl Source {
    /// What the file list shows.
    pub fn label(&self) -> String {
        match self {
            Source::Path(p) => p.display().to_string(),
            Source::Text { name, .. } => name.clone(),
        }
    }

    fn add_to(&self, project: &mut Project) -> Result<(), sim_core::ParseError> {
        match self {
            Source::Path(p) => project.add_path(p),
            Source::Text { name, text } if name.ends_with(".dmn") => {
                project.add_dmn_source(name, text)
            }
            Source::Text { name, text } => project.add_source(name, text),
        }
    }
}

#[derive(Default)]
pub struct Workspace {
    pub files: Vec<Source>,
    pub project: Arc<Project>,
    /// Error from the last (re)load; files before the failing one stay loaded.
    pub load_error: Option<String>,
}

impl Workspace {
    /// Open a folder (every `.bpmn` and `.dmn` file in it) or a main file together with the
    /// files it references (call activities, business rule tasks), replacing the current set.
    pub fn open(&mut self, path: &Path) {
        let paths = if path.is_dir() {
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
        self.files = paths.into_iter().map(Source::Path).collect();
        self.reload();
    }

    /// Open the given files, replacing the current set. BPMN files come first in the given
    /// order, then DMN files, like a folder.
    pub fn open_sources(&mut self, files: Vec<Source>) {
        let (dmn, bpmn): (Vec<_>, Vec<_>) =
            files.into_iter().partition(|f| f.label().ends_with(".dmn"));
        self.files = bpmn.into_iter().chain(dmn).collect();
        self.reload();
    }

    pub fn add(&mut self, source: Source) {
        if !self.files.contains(&source) {
            self.files.push(source);
        }
        self.reload();
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.files.len() {
            self.files.remove(index);
            self.reload();
        }
    }

    pub fn reload(&mut self) {
        let mut project = Project::new();
        self.load_error = None;
        for f in &self.files {
            if let Err(e) = f.add_to(&mut project) {
                self.load_error = Some(e.to_string());
                break;
            }
        }
        self.project = Arc::new(project);
    }
}
