# BPMN Simulator

A desktop application and command-line tool for simulating BPMN 2.0 process models, written
in Rust.

It reads BPMN files as they come out of common modellers (bpmn.io, Camunda Modeler, also
Camunda 8 models) and does three things with them:

- **Simulation.** Runs many cases of a model as a discrete-event simulation with durations,
  arrival rates, branching probabilities, resources, calendars and costs. It reports cycle
  times, waiting times, resource utilisation and costs, with confidence intervals across
  replications, and exports the results and a simulated event log (CSV, XES).
- **Model checking.** Reports where cases get stuck: tokens waiting forever at joins or
  events, messages nobody receives, signals sent before anyone listens.
- **Manual token game.** Plays one case through the diagram by hand. You start it, decide at
  every gateway, complete tasks and fire events, and watch the token move, including into
  called subprocesses in other files. Useful for finding gaps in a model and for explaining
  one.

It supports collaborations (several pools exchanging messages), call activities across files,
embedded subprocesses and event subprocesses, multi-instance and loop activities, boundary
events (including errors and escalations thrown inside subprocesses and called processes),
timers, signals, link events, inclusive and event-based gateways, conditional flows, and DMN
decision tables behind business rule tasks. `docs/semantics.md` lists where it deviates from
BPMN 2.0.2 and which elements it does not run.

**Status:** early, version 0.1. The simulation results are checked against queueing theory
and hand-calculated cases (see `docs/validation.md`), but the tool has so far been used on a
limited number of real models. Bug reports with a (shareable) BPMN file are welcome.

## Building

You need Rust 1.95 or newer ([rustup.rs](https://rustup.rs)).

```sh
git clone <this repository>
cd bpmn-sim
cargo build --release
```

This builds two programs into `target/release/`:

- `sim-gui`: the desktop app
- `sim-cli`: the command-line tool

On Linux the desktop app needs the usual desktop libraries at runtime (X11 or Wayland,
`libxkbcommon`); they are present on any normal desktop installation.

Run the tests with `cargo test`.

## Running the desktop app

```sh
cargo run --release -p sim-gui
```

1. **Model:** open a BPMN file (files it calls through call activities and the DMN files it
   uses are loaded from the same folder automatically) or a whole folder. Validation problems
   are listed there.
2. **Scenario:** set the simulation parameters, or generate a skeleton with placeholders for
   every task and gateway and adjust it. Scenarios are saved as TOML files.
3. **Run:** choose the number of cases and replications and start the simulation.
4. **Results:** summary, cases, activities, resources, problems, and confidence intervals
   across replications; export to JSON, CSV and XES.
5. **Diagram:** the model with a heat map (waiting time, utilisation, ...), an animated
   token replay of the simulated cases, and the manual token game ("manual simulation").

To try it, open `tests/fixtures/v13-model.bpmn` and load `tests/fixtures/v13-model.toml`
as the scenario, or open `tests/fixtures/multi-file/main.bpmn` for a model with several pools
and called subprocesses, or `tests/fixtures/event-subprocess.bpmn` with
`tests/fixtures/event-subprocess.toml` for event subprocesses.

## Running the command-line tool

```sh
# Check a model (and the files it references)
cargo run --release -p sim-cli -- check tests/fixtures/multi-file/main.bpmn

# Simulate it with a scenario and export results
cargo run --release -p sim-cli -- run tests/fixtures/v13-model.bpmn \
    --scenario tests/fixtures/v13-model.toml \
    --summary-csv summary.csv --event-log log.csv --xes log.xes
```

`sim-cli run` prints the results per replication (and with confidence intervals if there are
several) and exits with status 1 if tokens got stuck.

## Scenario files

A scenario is a TOML file next to the model. Elements are referenced by their BPMN id
(`"file.bpmn#id"` when the same id exists in several files):

```toml
[simulation]
seed = 1
replications = 10
cases = 1000
time_unit = "minutes"

[[arrivals]]
inter_arrival = { dist = "exponential", mean = "12m" }
triggers = ["Start"]

[resources.Clerks]
count = 2
cost_per_hour = 20.0

[activities.Task_Intake]
duration = { dist = "triangular", min = "2m", mode = "4m", max = "9m" }

[gateways.Split]
probabilities = { Flow_simple = 0.6, Flow_complex = 0.4 }
```

Tasks use the resource pool named like their lane (or pool). The desktop app can generate a
complete skeleton for a loaded model.

## Documentation

- `docs/semantics.md`: how every BPMN element and scenario option behaves in the simulation
  and in the manual token game.
- `docs/validation.md`: how the numbers are checked.
- `bpmn-simulator-SPEC.md`: the original specification.

## Pre-built binaries

Releases on GitHub contain binaries for Linux, macOS and Windows. They are not signed: on
macOS, open the app with right-click > Open the first time (or run
`xattr -d com.apple.quarantine sim-gui`); on Windows, confirm the SmartScreen warning.

## Licence

Copyright (C) 2026 Stefan Viol

This program is free software: you can redistribute it and/or modify it under the terms of
the GNU Affero General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version. See `LICENSE`.

Third-party licences (Rust crates and the fonts embedded in the desktop app) are listed in
`third-party/`.
