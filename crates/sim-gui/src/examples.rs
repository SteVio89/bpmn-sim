//! Example models built into the app, so it can be tried without any files (and in the
//! browser, where there is no folder to open).

use crate::workspace::Source;

pub struct Example {
    pub title: &'static str,
    pub description: &'static str,
    /// `(file name, text)`; the main model first.
    pub files: &'static [(&'static str, &'static str)],
    /// `(file name, text)` of the scenario that goes with it.
    pub scenario: (&'static str, &'static str),
}

/// `fixture!("dir/", "name")`: a test fixture as `(file name, text)`.
macro_rules! fixture {
    ($name:literal) => {
        fixture!("", $name)
    };
    ($dir:literal, $name:literal) => {
        (
            $name,
            include_str!(concat!("../../../tests/fixtures/", $dir, $name)),
        )
    };
}

pub const EXAMPLES: &[Example] = &[
    Example {
        title: "Support requests",
        description: "One pool with lanes, an exclusive gateway, resources with working hours and costs.",
        files: &[fixture!("v13-model.bpmn")],
        scenario: fixture!("v13-model.toml"),
    },
    Example {
        title: "Order with reminder and cancellation",
        description: "Event subprocesses: a timer reminds the customer, a later one cancels the order.",
        files: &[fixture!("event-subprocess.bpmn")],
        scenario: fixture!("event-subprocess.toml"),
    },
    Example {
        title: "Customer, operations and billing",
        description: "Several pools exchanging messages, call activities in other files, DMN decisions.",
        files: &[
            fixture!("multi-file/", "main.bpmn"),
            fixture!("multi-file/", "subprocess-a.bpmn"),
            fixture!("multi-file/", "subprocess-b.bpmn"),
            fixture!("multi-file/", "decision-limit.dmn"),
            fixture!("multi-file/", "decision-risk.dmn"),
        ],
        scenario: fixture!("multi-file/", "scenario.toml"),
    },
];

impl Example {
    pub fn sources(&self) -> Vec<Source> {
        self.files
            .iter()
            .map(|(n, t)| Source::Text {
                name: n.to_string(),
                text: t.to_string(),
            })
            .collect()
    }
}
