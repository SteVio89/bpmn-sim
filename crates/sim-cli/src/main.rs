use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use sim_core::{Project, ReplicationResult, Scenario, Severity, run_replications, validate};

const USAGE: &str = "usage:
  sim-cli check <model.bpmn | folder>...
  sim-cli run <model.bpmn | folder> --scenario <file.toml> [--seed N] [--json <out.json>]";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Returns `false` when the model has errors.
fn run() -> Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((cmd, rest)) if cmd == "check" && !rest.is_empty() => check(rest),
        Some((cmd, rest)) if cmd == "run" && !rest.is_empty() => simulate(rest),
        _ => bail!("{USAGE}"),
    }
}

fn load(paths: &[String]) -> Result<Project> {
    let mut project = Project::new();
    for p in paths {
        let path = Path::new(p);
        if path.is_dir() {
            project
                .add_dir(path)
                .with_context(|| format!("loading folder {p}"))?;
        } else {
            project.add_main(path)?;
        }
    }
    Ok(project)
}

fn check(paths: &[String]) -> Result<bool> {
    let project = load(paths)?;
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

fn simulate(args: &[String]) -> Result<bool> {
    let (mut model, mut scenario, mut seed, mut json) = (None, None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .with_context(|| format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--scenario" => scenario = Some(value("--scenario")?),
            "--seed" => seed = Some(value("--seed")?.parse::<u64>().context("--seed")?),
            "--json" => json = Some(value("--json")?),
            _ if model.is_none() => model = Some(a.clone()),
            _ => bail!("unexpected argument '{a}'\n{USAGE}"),
        }
    }
    let (Some(model), Some(scenario)) = (model, scenario) else {
        bail!("{USAGE}")
    };

    let project = load(&[model])?;
    let issues = validate(&project);
    for i in &issues {
        eprintln!("{i}");
    }
    if issues.iter().any(|i| i.severity == Severity::Error) {
        bail!("the model has errors");
    }
    let text = std::fs::read_to_string(&scenario).with_context(|| format!("reading {scenario}"))?;
    let mut sc = Scenario::from_toml(&text)?;
    if let Some(s) = seed {
        sc.simulation.seed = s;
    }
    let bound = sc.bind(&project)?;
    for w in &bound.warnings {
        eprintln!("warning: {w}");
    }
    let results = run_replications(&bound);
    for r in &results {
        print_result(r, bound.sim.time_unit.seconds(), bound.sim.time_unit.name());
    }
    if let Some(path) = json {
        std::fs::write(&path, serde_json::to_string_pretty(&results)?)
            .with_context(|| format!("writing {path}"))?;
    }
    Ok(results.iter().all(|r| r.stuck.is_empty()))
}

fn print_result(r: &ReplicationResult, unit: f64, unit_name: &str) {
    let t = |s: f64| s / unit;
    println!("== replication seed {} ==", r.seed);
    println!(
        "cases: {} started, {} completed; run length {:.2} {unit_name}",
        r.cases_started,
        r.cases_completed,
        t(r.end_time)
    );
    let c = &r.case_cycle_time;
    println!(
        "cycle time ({unit_name}): mean {:.2}  median {:.2}  p90 {:.2}  min {:.2}  max {:.2}",
        t(c.mean),
        t(c.median),
        t(c.p90),
        t(c.min),
        t(c.max)
    );
    println!("case cost: mean {:.2}", r.case_cost.mean);
    for (id, p) in &r.processes {
        println!(
            "  process {id}: {} instances, cycle time mean {:.2}",
            p.instances,
            t(p.cycle_time.mean)
        );
    }
    for (id, a) in &r.activities {
        let cancelled = if a.cancelled > 0 {
            format!(" ({} cancelled)", a.cancelled)
        } else {
            String::new()
        };
        println!(
            "  activity {id}: {} runs{cancelled}, wait {:.2}, processing {:.2}, cost {:.2}",
            a.runs,
            t(a.waiting.mean),
            t(a.processing.mean),
            a.total_cost
        );
    }
    for (name, p) in &r.resources {
        println!(
            "  resource {name}: utilisation {:.1}%, busy {:.2} {unit_name}, cost {:.2}",
            p.utilisation * 100.0,
            t(p.busy_time),
            p.cost
        );
    }
    for (id, n) in &r.end_events {
        println!("  end event {id}: {n}");
    }
    if !r.stuck.is_empty() {
        println!(
            "PROBLEM: {} token(s) stuck at the end of the run:",
            r.stuck.len()
        );
        for s in &r.stuck {
            let name = s
                .name
                .as_deref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default();
            println!(
                "  case {} in {}: {}{name} - {}",
                s.case, s.process, s.element, s.state
            );
        }
    }
}
