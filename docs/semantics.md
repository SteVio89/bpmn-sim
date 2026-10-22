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

## M2: GUI and export

- **GUI scope.** `sim-gui` only calls `sim-core` (bind, run, export) and displays results. A run executes on a worker thread; the scenario is bound on that thread and bind errors come back as a message. Progress is per finished replication.
- **Opening files.** Opening one file loads only that file (automatic discovery of referenced files comes with M4); opening a folder loads every `*.bpmn` in it. Opening a model while the scenario is still empty generates a skeleton.
- **Skeleton.** One `[activities]` entry per task (10 min fixed), one `[gateways]` entry per XOR split (equal probabilities), one resource pool (count 1) per lane / pool name used by a task, and one arrival every hour triggering all none/timer start events of the main file (default: first file). Keys are qualified ids.
- **Scenario files** are written back with durations in the largest exact unit (`7200` s becomes `"2h"`). Comments in a hand-written file are not preserved.
- **Exports.** JSON is the full result list. The summary CSV is a tidy table (replication, scope, id, name, metric, value); the event log CSV has one row per finished task. All times in exports are seconds.

## M3: collaboration

- **Instances of a case.** An arrival creates *all* instances of its triggers first and only then runs them, in trigger order. (Otherwise a case could look complete before a later pool's instance existed.) Processes without a trigger get an instance only when a message start event receives a message.
- **Active processes.** Binding follows message flows from the triggered processes (and from replying black boxes) to message start events and treats the receiving processes as part of the run: their tasks need `[activities]` entries and their elements are checked for support. Message start events, signal events and message events cannot be arrival triggers.
- **Messages** (§5.3). A message goes through the message flow within the sender's case and takes no time. At the target:
  1. a token is waiting at the target element: the longest-waiting one continues (`delivered`);
  2. the target is a message start event: a new instance is created in the case (`started_instance`); if the case is already complete the message is `lost`;
  3. otherwise it is stored if `[messages] buffer_unconsumed = true` (the default; `buffered`) or dropped (`lost`).
  A buffered message is taken by the next token that reaches that element in the same case. Buffered messages never picked up are reported as `undelivered`. Messages are matched per receiving *element*, not per message name.
- **Which elements send/receive.** Send tasks (after their duration), message throw events and message end events send; every message flow starting at them is used. Receive tasks, message catch events and message start events receive. A message flow from any other element, or to a pool that has a process, is a scenario error.
- **Receive task.** It only waits for the message: no resource, no duration, no `[activities]` entry.
- **Black boxes** (participants without a process). Messages to them are logged (`black_box`). With `[messages.black_box_reply] <participant> = <distribution>` they answer after the sampled delay along every message flow that leaves them; the reply is delivered like any message. Without a configured reply the receivers wait forever and show up as stuck.
- **Signals** (§5.4). Signal throw/end events broadcast by signal *name*. Only tokens waiting at a matching signal catch event at that moment receive it (all of them, in case/instance/token order); nothing is stored. `simulation.signal_scope = "global"` reaches all cases instead of the sender's. Every broadcast is logged with its receiver count, so missed signals are visible. Signal start events and signal boundary events come with M4.
- **Reports.** The result lists all messages with outcomes, the lost-message count, undelivered buffered messages and all signals; the GUI problems tab shows them.
- **Validation** now also warns about receive/catch message elements without an incoming message flow and send/throw message elements without an outgoing one.

## M4a: link/timer events, terminate, subprocesses, boundary events (single file)

- **Subscriptions.** A token that waits (catch event, receive task) or an activity with boundary events holds *subscriptions* to messages, signals or timers. Completing or cancelling the token deactivates them; a stale timer firing later does nothing.
- **Link events.** A link throw event moves its token immediately to the link catch event with the same name in the same process (V11); a catch event reached through a normal flow just passes. A throw without a matching catch is a binding error.
- **Timer catch events** need `[events.<id>] delay = <distribution>`. Message/signal catch events and receive tasks may also get an `[events]` entry (a trigger from outside the model): whichever happens first, message/signal or timer, continues the token.
- **Terminate end event.** At the top level of a process it cancels every other token of the instance (running tasks release their resource, queued requests are withdrawn, subscriptions end). Inside an embedded subprocess it cancels only the other tokens of that subprocess, which then completes normally. (Deviation from the spec's "ends the whole process instance" for the subprocess case; this is the BPMN definition.) Error and escalation end events behave like none end events.
- **Cancelled tasks.** They are logged (`cancelled = true`) with the time they actually worked; the resource is released at once and charged only for that time (no fixed cost). Activity statistics report `runs` (completed) and `cancelled` separately; waiting/processing summaries cover completed runs only.
- **Embedded subprocess.** A token arriving at the subprocess starts a scope: one token at each none start event inside. The subprocess completes when its scope has no tokens left; the waiting token then leaves along the outgoing flow. It needs at least one none start event. Tokens created inside (AND/OR splits) stay in the scope.
- **Boundary events** (on tasks, receive tasks and embedded subprocesses) are armed when the token *arrives* at the activity, so time spent queueing for a resource counts towards a timer. Interrupting (`cancelActivity` true): the activity (task run or whole subprocess scope) is cancelled and the token continues on the boundary flow. Non-interrupting: an additional token starts on the boundary flow in the same scope; a timer fires once, message/signal boundaries stay armed.
  - **Timer:** `[boundary.<id>] delay = ...` (optional `probability`, default 1).
  - **Error / escalation:** `probability` and `delay`: with that probability the event fires `delay` after activation, if the activity is still active.
  - **Message / signal:** no parameters. A message to a boundary event is never buffered: if its activity is not active, the message is lost.
- **Signal start events** create an instance in the sender's case for every broadcast of that name (recorded as `started` in the signal log). Processes with such a start event take part in the run when an active process throws the signal.
- **Not yet:** call activities (M4b), conditional/compensation/multiple event definitions, boundary events on call activities.

## M4b: call activities and several files

- **Discovery.** Opening a main file (`Project::open_main`, `sim-cli`, GUI "Open file…") loads that file plus, directly or indirectly, every `*.bpmn` in its folder that defines a process named by a call activity (`calledElement` or `zeebe:calledElement`) and every `*.dmn` that defines a decision named by a business rule task (`zeebe:calledDecision`). The folder is not searched recursively; unreadable unrelated files are ignored. If two files in the folder define a referenced process (or decision), opening fails naming both. References nothing defines are left to validation (error for processes, warning for decisions). "Open folder…" loads every `.bpmn` and `.dmn` in it.
- **Qualified ids** (§3.2/§4) were already the basis of the model; scenario keys may be bare if unique across the loaded files, otherwise `"file.bpmn#id"`. `model.main` must name a loaded BPMN file and every entry of `model.files` must be loaded; neither changes which file arrivals use (triggers are resolved by id).
- **Call activity.** The called process is found globally by process id. Its none start events (top level) each get a token in a new *child instance* of the same case; the call activity completes when the child instance completes. Costs of the child belong to the case. Resource pools are global and matched by name, so parent and child compete for the same pool. Called processes must have a none start event; direct or indirect recursion is a binding error. A call activity can carry boundary events: an interrupting one cancels the child instance (and everything it called).
- **Message groups.** Messages are matched within a *group*: the top-level instances of a case form one group; every call activity starts a new group for the child instance and for whatever the child's messages start (message start events). This is how a called file's own collaboration (e.g. its black-box pool and message flows) applies to the child instance only, and how two calls of the same process in one case do not mix their messages. Signals still use the case (or all cases) as scope.
- **Instance statistics.** The result lists, per process, the number of completed instances and their cycle time (`processes`), excluding warm-up cases.
- **DMN.** Decision tables are not evaluated yet; found decisions are only shown (GUI model tree). The gateway after a business rule task still uses probabilities.
- **Not implemented.** The option "treat an unresolvable call activity as a plain task with a duration" (§3.2) is not offered; unresolved calls stay an error.
