//! Background simulation run so the UI stays responsive. On the desktop it runs on a worker
//! thread; in the browser, which has none here, a slice of every frame runs replications.

use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, Sender, channel};

#[cfg(not(target_arch = "wasm32"))]
use sim_core::run_replications_with;
use sim_core::{Project, ReplicationResult, Scenario};

#[cfg(not(target_arch = "wasm32"))]
pub enum Message {
    Progress(u32, u32),
    Finished(Result<Vec<ReplicationResult>, String>),
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Job {
    rx: Receiver<Message>,
    pub done: u32,
    pub total: u32,
    pub finished: Option<Result<Vec<ReplicationResult>, String>>,
}

#[cfg(not(target_arch = "wasm32"))]
/// Start a run on a worker thread. `wake` is called after every message (e.g. to repaint the UI).
pub fn spawn(project: Arc<Project>, scenario: Scenario, wake: impl Fn() + Send + 'static) -> Job {
    let (tx, rx) = channel();
    let total = scenario.simulation.replications;
    std::thread::spawn(move || run(&project, &scenario, &tx, &wake));
    Job {
        rx,
        done: 0,
        total,
        finished: None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run(project: &Project, scenario: &Scenario, tx: &Sender<Message>, wake: &dyn Fn()) {
    let send = |m| {
        let _ = tx.send(m);
        wake();
    };
    match scenario.bind(project) {
        Err(e) => send(Message::Finished(Err(e.to_string()))),
        Ok(bound) => {
            let results = run_replications_with(&bound, |d, t| send(Message::Progress(d, t)));
            send(Message::Finished(Ok(results)));
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Job {
    /// Drain pending messages; returns true once the run has finished.
    pub fn poll(&mut self) -> bool {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Message::Progress(d, t) => (self.done, self.total) = (d, t),
                Message::Finished(r) => self.finished = Some(r),
            }
        }
        self.finished.is_some()
    }
}

#[cfg(target_arch = "wasm32")]
pub struct Job {
    project: Arc<Project>,
    scenario: Scenario,
    results: Vec<ReplicationResult>,
    wake: Box<dyn Fn()>,
    pub done: u32,
    pub total: u32,
    pub finished: Option<Result<Vec<ReplicationResult>, String>>,
}

/// Start a run; `poll` advances it. `wake` is called while there is more to do.
#[cfg(target_arch = "wasm32")]
pub fn spawn(project: Arc<Project>, scenario: Scenario, wake: impl Fn() + 'static) -> Job {
    let total = scenario.simulation.replications;
    wake();
    Job {
        project,
        scenario,
        results: Vec::new(),
        wake: Box::new(wake),
        done: 0,
        total,
        finished: None,
    }
}

#[cfg(target_arch = "wasm32")]
impl Job {
    /// Run replications for a slice of this frame; returns true once the run has finished.
    pub fn poll(&mut self) -> bool {
        /// Long enough to get through small runs quickly, short enough to keep the page usable.
        const SLICE: web_time::Duration = web_time::Duration::from_millis(30);
        if self.finished.is_some() {
            return true;
        }
        let bound = match self.scenario.bind(&self.project) {
            Ok(b) => b,
            Err(e) => {
                self.finished = Some(Err(e.to_string()));
                return true;
            }
        };
        let start = web_time::Instant::now();
        while self.done < self.total && start.elapsed() < SLICE {
            self.results
                .push(sim_core::run_replication(&bound, self.done));
            self.done += 1;
        }
        if self.done < self.total {
            (self.wake)();
            return false;
        }
        self.finished = Some(Ok(std::mem::take(&mut self.results)));
        true
    }
}
