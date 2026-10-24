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
timers, signals, link events, inclusive and event-based gateways, conditional flows, DMN
decision tables behind business rule tasks (gateways can follow their outputs), and gateways
that ask what other pools of the same case have already done. `docs/semantics.md` lists where it deviates from
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
   are listed there. The scenario starts with placeholders for everything the model needs.
2. **Diagram, Configure:** click a task, gateway, start event, timer, lane or pool to set its
   duration, branching, arrivals or resources and working hours in an editor next to it.
   Placeholders still to check are outlined, "Next" leads through them, and a quick check in
   the background shows the load per resource pool and the cycle time after every change.
3. **Scenario:** the same parameters as tables, plus working hours, decision inputs and
   messages. "Update from model" adds what a changed model needs and keeps your values.
   Scenarios are saved as TOML files.
4. **Run:** choose how many cases (or how much simulated time) and replications, and start
   the simulation.
5. **Results:** summary, cases, activities, resources, problems, and confidence intervals
   across replications; export to JSON, CSV and XES.
6. **Diagram:** the model with a heat map (waiting time, utilisation, ...), an animated
   token replay of the simulated cases, and the manual token game.

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
calendar = "office"            # optional: cases arriving outside these hours start at the next opening

[calendars.office]
days = ["mon", "tue", "wed", "thu", "fri"]
hours = "09:00-17:00"

[resources.Clerks]
count = 2
cost_per_hour = 20.0

[activities.Task_Intake]
duration = { dist = "triangular", min = "2m", mode = "4m", max = "9m" }

[gateways.Split]
probabilities = { Flow_simple = 0.6, Flow_complex = 0.4 }
```

Tasks use the resource pool named like their lane (or pool). The desktop app fills in
placeholders for a loaded model and keeps them up to date when the model changes.

A gateway after a business rule task can follow the decision's result instead of shares, also
when the flows have no conditions. A table with several outputs stores each one as
`<resultVariable>.<output>`; name the one that decides the gateway:

```toml
[decisions."order-triage".inputs]             # how the decision's inputs are drawn
category = { choice = { standard = 0.7, express = 0.3 } }

[gateways.Gateway_Route]
output = "triage.route"                       # resultVariable "triage", output "route"
values = { Flow_ship = "ship", Flow_hold = ["hold", "review"] }
```

Pools that start together and ask each other about progress ("is it ready yet?")
can let a two-way XOR split ask what has happened in the case instead of drawing it, start
later than the case, and run a call activity whose process is in no loaded file as a task:

```toml
[[arrivals]]
inter_arrival = { dist = "fixed", value = "30d" }
triggers = ["Start_Build", "Start_Ship"]      # one start event in each pool

[events.Start_Ship]                           # this pool begins up to 10 days later
delay = { dist = "uniform", min = "0d", max = "10d" }

[gateways.Gateway_Ready]                      # "Built yet?"
if_completed = "End_Built"                    # once this element completed in the case...
then = "Flow_yes"                             # ...take this flow, until then the other one

[activities.Call_Build]                       # call activity whose process is not loaded
duration = { dist = "uniform", min = "5d", max = "20d" }
```

## Documentation

- `docs/semantics.md`: how every BPMN element and scenario option behaves in the simulation
  and in the manual token game.
- `docs/validation.md`: how the numbers are checked.
- `bpmn-simulator-SPEC.md`: the original specification.

## Pre-built binaries

Releases on GitHub contain binaries for Linux, macOS and Windows. They are not signed: on
macOS, open `BPMN Sim.app` with right-click > Open the first time (or run
`xattr -dr com.apple.quarantine "BPMN Sim.app"`); on Windows, confirm the SmartScreen warning.

## Licence

Copyright (C) 2026 Stefan Viol

This program is free software: you can redistribute it and/or modify it under the terms of
the GNU Affero General Public License as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version. See `LICENSE`.

Third-party licences (Rust crates and the fonts embedded in the desktop app) are listed in
`third-party/`.
