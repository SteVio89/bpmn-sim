# Semantic decisions

Kept up to date with the implementation (spec §5, §11). Each entry notes the milestone that introduced it.

## M0: model and validation

- **Qualified ids.** Every element is identified by `<file name>#<element id>` (`QId`). Ids are only unique inside one file.
- **Process ids are global.** Loading a second file that defines an already known process id fails with an error naming both files.
- **Namespace matching.** BPMN elements are matched by namespace URI, so `bpmn:`, `bpmn2:` and unprefixed documents all work. Only `zeebe:calledElement@processId` and `zeebe:calledDecision@decisionId` are read from vendor extensions; everything else is ignored.
- **Ignored elements.** Data objects, data stores, associations, annotations and groups are not flow nodes and are skipped silently.
- **Unsupported elements.** `complexGateway`, `transaction`, `adHocSubProcess` and unknown event definitions (e.g. conditional, compensation) are kept in the model as `Unsupported` and reported as warnings.
- **Lanes.** A node's lane is the innermost lane that lists it in `flowNodeRef`.
- **Dangling references** in sequence flows, boundary events (`attachedToRef`) and message flows are parse errors naming the file and element.
- **Reachability** (for "intermediate catch event can never be reached") starts from all start events, follows sequence flows and link throw → link catch jumps, and treats boundary events as reachable once their activity is. Message and signal arrival do not make an intermediate catch event reachable; it needs an incoming flow.
- **Exempt from the "no outgoing flow" check:** end events and link throw events.
- **Deferred.** BPMNDI shapes/edges (diagram view, M6) and scenario-dependent checks (gateway probabilities, M1) are not parsed yet. Discovery of referenced files from a single main file (M4) is not implemented; `Project::add_dir` loads every `*.bpmn` in a folder.

## M1: engine, scenario and statistics

- **Event order.** The queue is ordered by `(time, sequence number)`; the sequence number is the scheduling order, so events at the same time run in a fixed order. All maps are `BTreeMap`s. One `ChaCha8Rng` per replication; replication `i` uses `seed + i`.
- **First arrival at t = 0.** Every arrival generator fires its first case at time 0, then after each sampled inter-arrival time.
- **Case count.** `simulation.cases` is the total number of cases started across all generators and is required (`horizon` arrives with M5). The generator firing *after* the last case starts nothing; it only advances the clock. It therefore marks the end of the observation window, which makes V1's utilisation exactly 1/6.
- **Observation window** = time of the last event processed (last completion or that final generator firing, whichever is later). Utilisation = busy time ÷ (count × window). Resource statistics include warm-up cases; only case and activity statistics exclude them.
- **Warm-up.** The first `warm_up_cases` cases *to complete* are excluded from case and activity statistics. Cases that never complete are never counted in cycle-time statistics.
- **Triggers.** One process instance is created per distinct process among the triggers of an arrival; each trigger gets a token. A case completes when all its instances have no tokens left.
- **Tasks.** Waiting = start − arrival at the task; processing = sampled duration. The duration is sampled when the task starts. A resource freed by a finishing task goes to the head of its FIFO queue before the finished token moves on.
- **Resources.** Pool = `resource` override, else lane name, else participant name; a task with none of these needs no resource (unlimited). A named pool missing under `[resources]` is a scenario error. Every task of an instantiated process needs an `[activities]` entry with a duration.
- **Costs.** Activity cost = processing time × pool `cost_per_hour` ÷ 3600 + `fixed_cost`; case cost is the sum. Pool cost = busy time × rate.
- **XOR split.** Every outgoing flow needs a probability, and they must sum to 1 (±1e-9); the `default` flow gets no special treatment yet. Probability keys are flow ids local to the gateway's file, or qualified (`file#flow`). Single-outgoing XOR gateways need no entry; XOR joins pass every token.
- **AND join.** Tokens wait per instance and per incoming flow; the join fires when every incoming flow has a token and consumes one from each, so tokens of different instances can never mix. A node with several outgoing flows (task, event, gateway) sends a token down each of them, except XOR.
- **Implicit end.** A token leaving a non-end node with no outgoing flow is consumed silently (validation warns).
- **Stuck report.** Tokens alive when the queue runs empty are listed with case, process, element and state (`working`, `queued for resource 'X'`, `waiting at AND join`).
- **Percentiles** use linear interpolation between closest ranks (R type 7); standard deviation is the sample standard deviation.
- **Scenario strictness.** Unknown keys are errors (typos). Sections of later milestones (`calendars`, `events`, `boundary`, `messages`, `horizon`, resource `calendar`, `model.files`) are accepted and reported as ignored warnings. Elements the engine cannot run yet (messages, timers, subprocesses, call activities, boundary events, OR/event gateways, non-none end events) make binding fail instead of being silently skipped.
