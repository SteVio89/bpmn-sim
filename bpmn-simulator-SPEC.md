# BPMN Process Simulator — Project Specification

A desktop application, written in Rust with egui, that runs discrete-event simulations of BPMN 2.0
process models and reports key performance indicators (cycle time, waiting time, resource
utilisation, cost). It is comparable to tools like the QBP Simulator / BIMP, but it also supports
**collaboration diagrams** (several pools exchanging messages), which those tools handle only partially.

This document is the starting point for a new project. It contains no project-specific process
models. Create all test models from scratch, as described in the "Test fixtures" section.

---

## 1. Goals and non-goals

### Goals
- Load one or more BPMN 2.0 XML files, for example a main process plus the files its call activities point to.
- Let the user set simulation parameters (arrivals, durations, probabilities, resources, costs), and save and load them as a scenario file.
- Simulate many process instances quickly and reproducibly: same seed, same result.
- Report KPIs that can be trusted. **Getting the numbers right comes before adding features.**
- Report model problems found during simulation, such as instances that never finish (deadlocks, lost messages).
- Export results (CSV/JSON) and a simulated event log (CSV, optionally XES) for process mining tools.

### Non-goals (at least for now)
- Executing real work (no job workers, no forms, no engine API).
- Full FEEL or full DMN support; only a small subset (see §5.9).
- Editing BPMN diagrams. The tool only reads them.
- Exact BPMN execution semantics for every element; anything left out must be listed in §5.

---

## 2. Technology

- **Language:** Rust (stable, edition 2021 or newer).
- **GUI:** `eframe` / `egui`, `egui_plot` for charts, `rfd` for file dialogs.
- **XML:** `roxmltree` (read-only DOM is enough).
- **Randomness:** `rand`, `rand_distr`, and a seedable reproducible generator (e.g. `rand_chacha`).
- **Configuration:** `serde` + `toml` (scenario files), `serde_json` (result export).
- **Errors:** `thiserror` in the library, `anyhow` in the binaries.
- **Testing:** built-in `cargo test`; optionally `proptest` for property tests and `insta` for snapshot tests.
- Use `cargo add` to get current crate versions. Do not hard-code versions from memory.

### Workspace layout
```
bpmn-sim/
├── Cargo.toml            # workspace
├── crates/
│   ├── sim-core/         # library: model, parser, engine, statistics — NO UI dependencies
│   ├── sim-cli/          # headless runner: `sim-cli run model.bpmn --scenario s.toml --seed 1`
│   └── sim-gui/          # eframe/egui desktop app using sim-core
├── tests/fixtures/       # small BPMN files + scenarios used by tests
└── docs/semantics.md     # the semantic decisions of §5, kept up to date
```
`sim-core` must work fully without the GUI. All validation tests run against `sim-core` or `sim-cli`.

---

## 3. Input: BPMN files

### 3.1 Parsing
- Support BPMN 2.0 XML with the usual prefixes (`bpmn:`, `bpmn2:`, or none). Match elements by **namespace URI**, not by prefix.
- Read the following:
  - `definitions`, `collaboration`, `participant` (pool), `process`, `laneSet`/`lane` (with `flowNodeRef`), `messageFlow`
  - flow nodes and `sequenceFlow` (`sourceRef`, `targetRef`, `name`); the `default` attribute on gateways
  - event definitions: none, timer, message, signal, link, error, escalation, terminate
  - `boundaryEvent` (`attachedToRef`, `cancelActivity`)
  - `callActivity`: the called process ID comes from the BPMN `calledElement` attribute **or** from the Camunda 8 extension `zeebe:calledElement processId="…"` (namespace `http://camunda.org/schema/zeebe/1.0`)
  - `signal`, `message` root elements and their references (`signalRef`, `messageRef`)
  - BPMNDI shapes and edges (bounds and waypoints), used only for the optional diagram view
- **Ignore and don't fail on:** vendor extensions (`zeebe:*`, `camunda:*`, `modeler:*`), documentation, data objects, data stores, data associations, text annotations, groups.
- Parse errors must name the file and the element ID.

### 3.2 Models made of several files (required from M4; design for it from M0)
A model usually consists of **several files in one folder**: a main collaboration, one file per
called subprocess (each may contain its own collaboration with extra pools and message flows),
and DMN files referenced by business rule tasks.

- **Project = folder or file set.** The user opens either a **folder** or one main file. If a main file is opened, the tool scans its folder (not recursively) for `*.bpmn` and `*.dmn`, and loads every file that a call activity or business rule task references, directly or indirectly. In the GUI, loaded files are listed and can be added or removed by hand.
- **Main file:** the file that contains the arrival triggers. It is chosen in the scenario (`[model] main = "main.bpmn"`) or in the GUI.
- **IDs are only unique within one file.** Different files often reuse the same IDs (e.g. `Flow_1`, `Task_Analyse`, `Lane_1`). Internally every element is therefore identified by a **qualified ID** `"<file name>#<element id>"`, e.g. `"subprocess-a.bpmn#Task_Analyse"`.
- **Process IDs must be unique across all loaded files**, because call activities resolve them globally. Duplicates are an error that names both files.
- **Resolving call activities:** `calledElement` / `zeebe:calledElement processId` → the process with that ID in any loaded file. Not found → error (or warning, if the user chooses "treat as plain task with duration").
- **Collaborations in called files:** when a called process takes part in a collaboration in its own file (e.g. message flows to a black-box pool defined there), those message flows apply to the child instance. Message flows never cross file boundaries; parent and child talk only through the call activity.
- **Resources across files:** resource pools are global and matched **by name** (§5.6). A lane "Team A" in a subprocess file and a pool or lane "Team A" in the main file use the same resource pool. The GUI shows which lanes and pools from which files map to each resource pool.
- **DMN files:** business rule tasks name their decision via `zeebe:calledDecision decisionId` (Camunda 8) or a vendor-neutral attribute. Decision IDs are resolved across all loaded `.dmn` files; duplicates are an error. Until DMN evaluation exists (M6), a found decision is only shown as information; the gateway after it uses probabilities.
- **Black-box pools with the same name in different files** are separate participants (one per file). They don't need resources.

### 3.3 Model validation (before simulating)
Report the following as warnings or errors, each with element ID and name:
- an element that isn't an event but has no incoming flow; an element that isn't an end event but has no outgoing flow (link events are exempt)
- intermediate catch events that can never be reached
- link throw events without a matching link catch event (same name) in the same process
- call activities whose called process ID can't be found in any loaded file
- an XOR or inclusive split without a probability for each outgoing flow (or with probabilities that don't add up to 1), once the scenario is loaded
- elements in the model that the simulator doesn't support yet

---

## 4. Input: scenario file (simulation parameters)

TOML, kept separate from the BPMN files. Elements are referenced by **BPMN element ID**. A `label` field is optional and only there for readability. The GUI can generate a skeleton with every element of the loaded model.

**Qualified IDs (§3.2):** a bare ID like `Task_Build` is allowed only if it is unique across all loaded files. Otherwise use the qualified form `"file.bpmn#Task_Build"`, as a quoted TOML key. If a bare ID is ambiguous, the scenario fails to load with an error listing the matching files. The generated skeleton always uses qualified IDs.

```toml
[model]
main = "main.bpmn"           # file with the arrival triggers
files = ["main.bpmn", "subprocess-a.bpmn", "subprocess-b.bpmn", "decision-x.dmn"]  # optional; otherwise found automatically (§3.2)

[activities."subprocess-a.bpmn#Task_Analyse"]   # qualified ID, because the ID also exists in another file
duration = { dist = "fixed", value = "30m" }

[simulation]
seed = 42
replications = 10            # independent runs with seeds seed, seed+1, ...
cases = 200                  # number of cases to start per replication (or use `horizon`)
# horizon = "365d"
warm_up_cases = 20           # leave the first N completed cases out of the statistics
time_unit = "minutes"        # unit used in reports

[[arrivals]]                 # one generator creates new cases
label = "New quarter"
inter_arrival = { dist = "fixed", value = "90d" }
triggers = ["StartEvent_A", "StartEvent_B"]   # start events that fire together for each new case

[resources.DevOps]           # resource pools, matched to lanes by name (see §5.6)
count = 2
cost_per_hour = 80.0
calendar = "office"          # optional

[calendars.office]
days = ["mon", "tue", "wed", "thu", "fri"]
hours = "09:00-17:00"

[activities.Task_Build]
duration = { dist = "triangular", min = "30m", mode = "45m", max = "2h" }
fixed_cost = 0.0
# resource = "DevOps"        # optional override of the lane-based mapping

[gateways.Gateway_TestsOk]   # XOR: probabilities per outgoing sequence flow ID
probabilities = { Flow_pass = 0.85, Flow_fail = 0.15 }

[events.Catch_Approval]      # catch events whose trigger comes from outside the model
delay = { dist = "exponential", mean = "2d" }

[boundary.Boundary_Timeout]  # boundary events (see §5.5)
probability = 0.1
delay = { dist = "uniform", min = "1h", max = "4h" }

[messages]
buffer_unconsumed = true     # see §5.3
black_box_reply = {}         # optional: participant ID -> reply delay, for black-box pools
```

**Supported distributions:** `fixed`, `uniform`, `normal` (cut off at 0), `exponential`, `triangular`, `lognormal`.

**Durations** are strings with a unit: `s`, `m`, `h`, `d`, `w`. Internally all time is held as `f64` seconds.

---

## 5. Simulation semantics (decide carefully, document them in `docs/semantics.md`, test every rule)

The engine is a **discrete-event simulation**:
- a priority queue of future events, ordered by `(time, sequence_number)`, so events at the same time are processed in a fixed, reproducible order
- tokens move through the process graph
- activities claim resources and wait in a first-in-first-out queue when no resource is free

### 5.1 Cases and instances
- A **case** is one end-to-end run, e.g. "one quarterly release". An arrival creates a case and fires every start event listed in `triggers`, which creates one **process instance** per participant involved.
- Every process instance belongs to exactly one case. Messages and signals are matched **within the case** (see §5.3 and §5.4).
- A case is complete when all of its process instances have completed. **Case cycle time** = completion time − arrival time.

### 5.2 Flow elements
| Element | Behaviour |
|---|---|
| None/timer start event | Fired by the arrival generator (via `triggers`) |
| Message start event | Creates a new process instance in the same case when a message arrives (§5.3) |
| Signal start event | Creates an instance when a matching signal is broadcast within the case |
| Task (all types: user, manual, service, send, receive, script, business rule) | Claims 1 unit of its resource, takes the sampled duration, releases the resource. Waiting time and processing time are recorded separately. |
| Send task | Like a task; when it completes, it sends a message along its outgoing message flow |
| Receive task / message catch event | Waits for a message (§5.3) |
| Message throw / message end event | Sends a message along its outgoing message flow; takes no time |
| Timer catch event | Waits for a sampled delay (`[events]`) |
| Signal throw / signal end event | Broadcasts the signal (§5.4) |
| Signal catch event | Waits for a matching signal (§5.4) |
| Link throw → link catch | Immediate jump to the link catch event with the same name in the same process |
| None intermediate event | Passes the token through immediately |
| End event | Consumes the token; the instance completes when it has no tokens left |
| Terminate end event | Ends the whole process instance right away and cancels all its tokens and activities |
| XOR split | Picks exactly one outgoing flow by probability |
| XOR join | Passes on every incoming token |
| AND split | One token on every outgoing flow |
| AND join | Waits until a token from the **same instance** has arrived on every incoming flow |
| Inclusive (OR) split | Each outgoing flow is taken with its own probability; if none is selected, the default flow (or, failing that, the flow with the highest probability) is taken |
| Inclusive (OR) join | Waits for all tokens of the same instance that can still reach it. This is hard to get right: implement it after AND, and build the reachability check explicitly. |
| Event-based gateway | Starts all following catch events; the first one to fire wins and the others are cancelled |
| Embedded subprocess | Is expanded inline; its scope can be cancelled by an interrupting boundary event |
| Call activity | Starts a child instance of the called process (none start event) in the same case; the call activity completes when the child completes |

### 5.3 Messages
- A message flow goes from a send task, throw event or end event to a catch event, receive task, message start event, or a black-box pool.
- **Delivery:** the message goes to the instance of the target process **in the same case**.
  - The target is a message start event: create a new instance.
  - The target instance is currently waiting at the target element: it continues.
  - The target instance isn't waiting: if `buffer_unconsumed = true`, the message is stored and used as soon as the instance reaches the catch element; otherwise it is dropped and logged as a **lost message**.
- **Messages to black-box pools** (participants without a process) are only logged. With `black_box_reply`, a reply can optionally be modelled as a delayed message.
- Messages take no time to travel, unless a delay is configured.

### 5.4 Signals
- **Scope:** broadcast to all instances **in the same case** by default. The option `signal_scope = "global"` broadcasts to all cases instead.
- **Signals are not stored.** Only elements that are waiting at that moment receive them: catch events, active boundary events, signal start events. This matches BPMN; a missed signal can leave an instance waiting forever, and that must show up in the "stuck instances" report.

### 5.5 Boundary events
- **Timer boundary event:** fires after the sampled delay if the activity is still running.
- **Message/signal boundary event:** fires when a matching message or signal arrives while the activity is running.
- **Error and escalation boundary events, and boundary events whose trigger comes from outside the model:** fire with the configured `probability`, at a sampled time within the activity.
- **Interrupting** (`cancelActivity = true`): cancels the activity (and its subprocess scope), releases resources, and continues on the boundary flow.
- **Non-interrupting:** additionally starts a token on the boundary flow.

### 5.6 Resources
- **Assignment:** each task uses the resource pool named like its **lane**. If the process has no lanes, it uses the pool named like its **participant** (pool name). A per-task `resource` setting overrides this.
- **Queueing:** first-in-first-out per resource pool.
- **Calendars:** work only happens during calendar hours. A task that is still running at closing time pauses and continues at the next opening. Document this choice.
- **Events and gateways** never claim resources.
- **Utilisation** = busy time ÷ available time (calendar time × count), per pool.

### 5.7 Costs
- Activity cost = processing time × the resource's hourly rate + the activity's `fixed_cost`.
- Case cost = sum of the costs of all its activities, including those of child instances.

### 5.8 Stopping and problem detection
- The run stops when all cases are complete, or when the event queue is empty, or at the horizon.
- **Stuck instances:** an instance that still has tokens when the run ends is reported together with *where* each token is waiting (element ID and name). This turns the simulator into a model checker, so this report must always be shown.

### 5.9 Decisions (DMN) — later milestone
- **MVP:** business rule tasks behave like ordinary tasks. The gateway after them uses probabilities.
- **Later:** evaluate DMN 1.3 decision tables with the hit policies UNIQUE, FIRST, ANY and COLLECT. The FEEL subset needed: literals, `-`, comparisons (`<`, `<=`, `>`, `>=`, `=`), ranges `[a..b]`, lists `"a","b"`, `not(...)`. Input values are sampled from distributions in the scenario; gateway conditions select a flow from the decision output.

---

## 6. Output and statistics

For each replication, and aggregated across replications (mean, standard deviation, 95% confidence interval):
- **Cases:** number started and completed; cycle time (mean, median, P90, min, max); histogram
- **Per process:** instance cycle time
- **Per activity:** number of runs; waiting time; processing time; cost
- **Per resource pool:** utilisation; busy time; cost
- **Per end event:** how many tokens ended there (outcome distribution)
- **Problems:** stuck instances, lost messages, gateway probabilities that don't match the model, unsupported elements
- **Export:** results as JSON and CSV; event log as CSV with case ID, process instance ID, activity ID/name, resource, start time, end time; optionally XES

---

## 7. GUI (eframe/egui)

1. **Model panel:** open a folder or a main file (`rfd`); a list of loaded files (main file marked, referenced files found automatically, add or remove by hand); tree of files → participants → processes → elements, with the call activity → called process links shown; mapping of lanes/pools to resource pools; validation messages, clickable to select the element.
2. **Scenario panel:** editable parameters per element (distribution picker with fields, probability sliders that sum to 1 per gateway, resource table, arrivals); load and save TOML; "generate skeleton".
3. **Run panel:** seed, replications, number of cases; a run button; progress bar. The simulation runs in a background thread so the UI stays responsive.
4. **Results panel:** tabs for summary, cases, activities, resources, problems; charts with `egui_plot` (cycle time histogram, utilisation bars, waiting time per activity); export buttons.
5. **Diagram view (stretch goal):** draw pools, lanes, shapes and edges from the BPMNDI coordinates with the egui painter (pan and zoom); a heat-map overlay by waiting time or utilisation; optionally animated token replay from the event log.

---

## 8. Validation (required, not optional)

The simulator is only useful if its numbers are right. Each test below must pass before a milestone counts as done:

| # | Test model (fixture) | Expected result |
|---|---|---|
| V1 | Start → one task with fixed 10 min → end, 1 resource, arrivals every 60 min | cycle time exactly 10 min, waiting 0, utilisation exactly 1/6 |
| V2 | Same as V1 but arrivals every 5 min (overload) | waiting time grows steadily; utilisation → 1 |
| V3 | **M/M/1 queue:** exponential arrivals (rate λ) and service (rate μ), 1 resource, ρ = λ/μ = 0.8, many cases, warm-up | mean waiting time ≈ Wq = ρ / (μ − λ), mean time in system ≈ W = 1 / (μ − λ), each within the 95% CI over the replications |
| V4 | XOR split 0.7 / 0.3 | observed frequencies within the statistical tolerance |
| V5 | AND split → two tasks of 10 and 20 min → AND join | cycle time = 20 min (not 30) |
| V6 | Two instances interleaved at one AND join | a join never mixes tokens from different instances |
| V7 | Two pools, a message flow from a send task to a catch event | the receiver continues only after the message; with buffering off and the message sent early, it is reported as lost |
| V8 | Signal thrown before the catch is active | the catch instance is reported as stuck |
| V9 | Interrupting timer boundary event on a task | the task is cancelled and its resource released; the boundary path is taken |
| V10 | Call activity into a second file | the child runs in the same case; the parent waits for it; costs are summed |
| V11 | Link throw/catch | behaves exactly like a sequence flow |
| V12 | Same seed twice | identical results, bit for bit |
| V10b | Folder with a main file + 2 subprocess files that **reuse the same element IDs** (e.g. both contain `Task_Analyse`, `Flow_1`, `Lane_1`), each subprocess with its own black-box pool and message flows, + 1 DMN file | the tool finds all files automatically; a bare ambiguous ID in the scenario gives a clear error; qualified IDs work; statistics keep both `Task_Analyse` apart; message flows inside each subprocess file work; lanes with the same name in different files share one resource pool |
| V10c | Two files defining the same **process ID** | loading fails with an error that names both files |
| V13 | **Cross-check with QBP/BIMP:** a simple single-pool model (tasks, XOR, AND, 2 lanes) with identical parameters in both tools | mean cycle time and utilisation agree within the confidence intervals; document the comparison in `docs/validation.md` |

Put V1–V12 in automated tests (`cargo test`) with fixed seeds and statistical tolerances. V13 is documented by hand.

---

## 9. Milestones

| Milestone | Content | Done when |
|---|---|---|
| **M0** | Workspace, CI-ready `cargo test`/`clippy`/`fmt`, BPMN parser, model validation. **Internal model built on qualified IDs from the start** (file + element ID, §3.2), even if only one file is loaded | parses the fixtures; validation reports the right warnings |
| **M1 (MVP engine)** | Event queue; tasks, none start/end, XOR, AND; resources without calendars; distributions; one arrival generator; statistics; `sim-cli` | V1–V6, V12 pass |
| **M2 (GUI MVP)** | eframe app: load model, edit and save scenario, run in background, results tables + charts, export | a whole run can be done in the GUI without the terminal |
| **M3 (collaboration)** | Cases across pools, message flows, message start/catch, receive/send tasks, black-box pools, lost-message and stuck-instance reports | V7, V8 pass |
| **M4 (more elements + several files)** | Link events, timer/signal events, boundary events (interrupting/non-interrupting), terminate end event, embedded subprocess, call activity across files; loading a folder, automatic discovery of referenced files, qualified IDs in the scenario, collaborations inside called files, global resource pools matched by name (§3.2) | V9–V11, V10b, V10c pass |
| **M5 (realism)** | Calendars, costs, warm-up, replications with confidence intervals, event log export | V3 passes with CI; V13 documented |
| **M6 (optional)** | Inclusive gateway, event-based gateway, DMN subset, diagram view with heat map, token replay | each has its own test |

---

## 10. Test fixtures

- Write small BPMN XML files by hand (or with a modeller) in `tests/fixtures/`, one per test from §8, each with a matching `.toml` scenario.
- Also include **one larger collaboration fixture as a folder** (`tests/fixtures/multi-file/`) that uses everything together:
  - **main file:** 3 pools; timer and message start events; messages in both directions; a signal used between two instances of the same pool; an interrupting signal boundary event; two call activities
  - **two subprocess files** that **reuse the same element IDs** as each other. Each has its own collaboration with a black-box pool and message flows to it, lanes (one lane named like a pool in the main file), an XOR loop with link events, and an AND block. One also has business rule tasks.
  - **one or two DMN files** referenced by those business rule tasks via `zeebe:calledDecision`
  - a data store with a data association (must be ignored), and Camunda 8 extension elements (must be ignored, except `zeebe:calledElement` and `zeebe:calledDecision`)

---

## 11. Working rules for the implementation

- **Get the semantics right before adding features.** Every rule in §5 gets a test before or together with its implementation.
- **Make every semantic decision visible:** when the spec leaves something open, choose the simplest correct behaviour, write it into `docs/semantics.md`, and mention it in the commit message.
- **Determinism:** no `HashMap` iteration order inside the engine logic (use `BTreeMap`/`IndexMap`, or sort); all randomness comes from the seeded generator held by each replication.
- **Performance target:** 10,000 cases of a model with about 50 elements in under a few seconds in release mode. Don't optimise before M5.
- **Error handling:** no `unwrap()` in `sim-core` outside of tests; errors carry the element ID.
- Keep the GUI thin: it only calls `sim-core` APIs and displays the results.
