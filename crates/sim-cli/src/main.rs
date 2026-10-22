use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use sim_core::{Project, Severity, validate};

const USAGE: &str = "usage: sim-cli check <model.bpmn | folder>...";

fn main() -> ExitCode {
    match run() {
        Ok(ok) => {
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Returns `false` when validation found errors.
fn run() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((cmd, paths)) if cmd == "check" && !paths.is_empty() => check(paths),
        _ => bail!("{USAGE}"),
    }
}

fn check(paths: &[String]) -> Result<bool> {
    let mut project = Project::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            project
                .add_dir(path)
                .with_context(|| format!("loading folder {p}"))?;
        } else {
            project.add_path(path)?;
        }
    }
    let issues = validate(&project);
    for i in &issues {
        println!("{i}");
    }
    let errors = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    println!(
        "{} file(s), {} element(s), {} error(s), {} warning(s)",
        project.files.len(),
        project.nodes().count(),
        errors,
        issues.len() - errors
    );
    Ok(errors == 0)
}
