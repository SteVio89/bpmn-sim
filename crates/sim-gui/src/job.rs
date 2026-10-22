//! Background simulation run so the UI stays responsive.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use sim_core::{Project, ReplicationResult, Scenario, run_replications_with};

pub enum Message {
    Progress(u32, u32),
    Finished(Result<Vec<ReplicationResult>, String>),
}

pub struct Job {
    rx: Receiver<Message>,
    pub done: u32,
    pub total: u32,
    pub finished: Option<Result<Vec<ReplicationResult>, String>>,
}

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
