# Validation

How the numbers of the simulator are checked (spec §8). `cargo test --workspace` runs V1–V12.

| # | Test (all in `crates/sim-core/tests/`) | What it checks |
|---|---|---|
| V1 | `m1_engine::v1_single_task_no_queue` | cycle time exactly 10 min, waiting 0, utilisation exactly 1/6 |
| V2 | `m1_engine::v2_overload_waiting_grows` | waiting grows steadily (k-th case waits k·5 min), utilisation > 99% |
| V3 | `m5_realism::v3_mm1_matches_theory_within_the_confidence_interval` | M/M/1, ρ = 0.8, 20 replications: theory inside the 95% CI (below) |
| V4 | `m1_engine::v4_xor_frequencies` | 0.7 / 0.3 split within 5σ over 20 000 cases |
| V5 | `m1_engine::v5_and_split_join_takes_the_longer_branch` | cycle time 20 min, not 30 |
| V6 | `m1_engine::v6_and_join_never_mixes_instances` | 500 overlapping cases: each completes exactly when its own slower branch does |
| V7 | `m3_collab::v7_*` | message: receiver continues only after it; early message buffered, or lost with buffering off |
| V8 | `m3_collab::v8_*` | signal thrown before the catch is active: receiver stuck, reported with location |
| V9 | `m4a_events::v9_*` | interrupting timer: task cancelled, resource released, boundary path taken |
| V10 | `m4b_multifile::v10_*` | call activity into a second file: same case, parent waits, costs summed |
| V10b | `m4b_multifile::v10b_*`, `discovery_*` | folder with reused ids, DMN files, qualified ids, shared pools by lane name |
| V10c | `m4b_multifile::v10c_*` | duplicate process ids: error naming both files |
| V11 | `m4a_events::v11_*` | link throw/catch behaves exactly like a sequence flow |
| V12 | `m1_engine::v12_*` and the `*_deterministic` tests | same seed, bit-identical results |
| V13 | by hand, see below | comparison with QBP / BIMP |
| M6 | `m6_dmn::*`, `dmn.rs` and `feel.rs` unit tests | decision table routes claims with the expected frequencies (auto 0.4, manual 0.1, reject 0.5 within 5σ), all hit policies, FEEL subset, no match / default flow / probability override |
| M6 | `m6_diagram::*`, GUI `diagram_*` tests | BPMNDI parsing, heat values equal the statistics, replay counts equal the overlapping log records and never exceed resource capacity; headless GUI: fit, drag pans, wheel zooms, replay advances |
| M6 | `m6_gateways::*` | inclusive split frequencies (5σ), OR join waits exactly for the chosen / still reachable branches, per instance; event-based gateway: first event wins, the others are cancelled |
| BPMN | `m17_conformance::*` | every file in `tests/fixtures/conformance` (BPMN 2.0.2 serializations: default namespace, other prefixes, QName references, root event definitions, any element order, vendor extensions, BPMNDI) parses, validates without issues and runs its skeleton scenario to completion; inline edge cases (BOM, character references, foreign elements, other root elements, unsupported elements, duplicate or missing ids, wrong namespaces) |

Further unit tests cover the t quantiles, confidence intervals and the calendar arithmetic
(`aggregate.rs`, `calendar.rs`).

## V3: M/M/1 queue

`tests/fixtures/v3-mm1.toml` on `tests/fixtures/v1-single-task.bpmn`: Poisson arrivals with mean
10 min, exponential service with mean 8 min, one resource, 40 000 cases per replication, the
first 4 000 completed cases left out, 20 replications (seeds 2024..2043).

    sim-cli run tests/fixtures/v1-single-task.bpmn --scenario tests/fixtures/v3-mm1.toml

| Quantity | Theory | Simulation (mean ± 95% CI half width) |
|---|---|---|
| waiting time Wq = ρ / (μ − λ) | 32.00 min | 31.96 ± 0.81 min |
| time in system W = 1 / (μ − λ) | 40.00 min | 39.97 ± 0.82 min |
| 90th percentile of W (= 40 · ln 10) | 92.1 min | 92.15 ± 2.23 min |
| utilisation ρ | 80.0 % | 79.98 ± 0.29 % |

The automated test asserts that the theoretical values lie inside the confidence intervals
(a 95% interval misses the truth in 5% of independent experiments; the seeds are fixed, so the
test is deterministic and passes).

## Performance

Release build, one core: 10 000 cases of the multi-file fixture (3 pools, 2 called files,
about 40 elements) take about 0.4 s including process start; the 20 × 40 000 case M/M/1
experiment takes about 1.3 s. This meets the target of §11 (10 000 cases of a 50 element model in
a few seconds).

## V13: cross-check with QBP / BIMP (to be done by hand)

**Status: not yet done.** QBP and BIMP are not available in the environment where the simulator
was built, so only the simulator side of the comparison exists. Do the following:

1. Model: `tests/fixtures/v13-model.bpmn`, one pool "Support" with the lanes Clerks and Experts:
   Intake (Clerks) → XOR "Complex?" → simple (60 %): Handle simple request (Clerks) / complex
   (40 %): AND split → Technical check (Experts) ∥ Document review (Clerks) → AND join → XOR join →
   Close request (Experts) → End.
2. Parameters: `tests/fixtures/v13-model.toml`.

   | | |
   |---|---|
   | arrivals | exponential, mean 12 min |
   | Clerks | 2 resources, 20 per hour |
   | Experts | 1 resource, 50 per hour |
   | Intake | triangular 2 / 4 / 9 min |
   | Handle simple request | normal, mean 10 min, σ 2 min |
   | Technical check | exponential, mean 6 min |
   | Document review | uniform 5–15 min |
   | Close request | fixed 3 min |

   Enter the same in QBP / BIMP (5 000 cases, 30 replications, the first 500 cases as warm-up) and
   record mean cycle time, mean waiting time per task and utilisation of both roles.
3. Simulator: `sim-cli run tests/fixtures/v13-model.bpmn --scenario tests/fixtures/v13-model.toml`.

Simulator results (30 replications, mean ± 95% CI half width):

| Quantity | This simulator | QBP / BIMP |
|---|---|---|
| mean cycle time | 26.97 ± 0.20 min | _to be filled in_ |
| cycle time P90 | 42.97 ± 0.48 min | _to be filled in_ |
| waiting, Intake | 3.01 ± 0.07 min | _to be filled in_ |
| waiting, Handle simple request | 3.08 ± 0.07 min | _to be filled in_ |
| waiting, Technical check | 2.31 ± 0.09 min | _to be filled in_ |
| waiting, Document review | 3.07 ± 0.09 min | _to be filled in_ |
| waiting, Close request | 2.17 ± 0.07 min | _to be filled in_ |
| utilisation Clerks | 62.73 ± 0.35 % | _to be filled in_ |
| utilisation Experts | 45.13 ± 0.39 % | _to be filled in_ |

Independent plausibility check that needs no other tool: the mean work per request is
Clerks 5 + 0.6·10 + 0.4·10 = 15 min and Experts 0.4·6 + 3 = 5.4 min, so with an arrival every
12 min the expected utilisations are 15 / (2 · 12) = 62.5 % and 5.4 / 12 = 45 %, which the
simulation reproduces within its confidence intervals.

Differences to expect when comparing with QBP / BIMP: how a tool treats the warm-up period,
whether durations are cut off at 0, and the exact definition of utilisation (this simulator:
busy time ÷ (resources × observation window); the window ends when the last event of the run is
processed, see `docs/semantics.md`).

## Looking at the GUI without a display

`crates/sim-gui/src/snapshot.rs` renders egui frames on the CPU (no GPU, no window) into PNG files.
The ignored test `screenshots` writes the diagram view of several fixtures:

    SIM_GUI_SNAPSHOT_DIR=/some/dir cargo test -p sim-gui -- --ignored screenshots

This is how the diagram drawing was checked during development; it is no replacement for trying
the real window (fonts and anti-aliasing differ slightly).
