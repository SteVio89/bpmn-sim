# BPMN 2.0.2 rule catalogue

The normative rules of **OMG BPMN 2.0.2 (formal/13-12-09)** that matter for bpmn-sim. This official
document is the only source. Every rule cites the printed page; the PDF page is that number + 30.
The status of each rule in bpmn-sim is in `docs/conformance.md`.

Scope: clauses 2, 7–10 and 13, read in full.
Out of scope: Choreography (11), Diagram Interchange beyond drawing (12), the BPEL mapping (14) and
exchange formats (15).

bpmn-sim's goal (BPMN §2.2.7): it does not have to support every element, but every element it runs
SHALL behave as specified, and elements it does not run are reported. Simulation parameters are
outside BPMN (§7.2) and so are a legitimate extension.

**IDs:** `R-<section>-<nn>`. They are stable; new rules get new numbers.

**Tags:**
- `STRUCT`: well-formedness, checked when the model is validated;
- `EXEC`: behaviour at runtime;
- `ATTR`: an attribute and its default;
- `CONF`: a conformance clause;
- `VISUAL`: notation.

The rule texts paraphrase the spec but keep its MUST / SHALL / MAY.

## 2 Conformance

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-2.1-01 | 1 | CONF | Conformance to a type can be claimed only if all of its compliance points are met; partial implementations may only say they are *based on* BPMN 2.0.2. |
| R-2.2.7-01 | 9 | CONF | Optional features are NOT REQUIRED, but a feature that is implemented SHALL be implemented as specified. |
| R-2.2.3-01 | 8 | VISUAL | A tool that displays diagrams SHALL use the specified shapes and markers. It MAY add markers and colours, but SHALL NOT change a shape. |
| R-2.3-01 | 10 | CONF | Process Execution Conformance MUST interpret the Clause 13 operational semantics and the activity lifecycle. Non-operational elements (§13.1) MAY be ignored. |
| R-2.3.2-01 | 10 | CONF | An implementation MUST import Process diagrams, including their definitional Collaboration. |

## 7 Overview, connection rules

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-7.2-01 | 20 | CONF | Simulation, monitoring and deployment are out of BPMN's scope. |
| R-7.2.1-01 | 25 | EXEC | A token created by a Start Event MUST eventually be consumed at an End Event, which MAY be implicit (a node without outgoing flow). |
| R-7.2.1-02 | 25 | EXEC | Tokens do not traverse Message Flows. A Message is passed instead. |
| R-7.3-01 | 34 | EXEC | Uncontrolled flow: several unconditional outgoing flows of an activity or event fork, and several incoming flows merge without synchronisation. |
| R-7.3-02 | 36 | EXEC | Cycles built from sequence flows are allowed. |
| R-7.6.1-01 | 40 | STRUCT | A Start Event is never the target of a sequence flow, and an End Event is never its source (Table 7.3). |
| R-7.6.1-02 | 40 | STRUCT | A sequence flow MUST NOT cross the boundary of a Sub-Process or a Pool. |
| R-7.6.1-03 | 40 | STRUCT | Artifacts, data, lanes and pools are never connected by sequence flow. |
| R-7.6.2-01 | 41 | STRUCT | Message flow (Table 7.4): message start and catch events are targets only; message throw and end events are sources only; gateways, lanes and data are never connected. |
| R-7.7-01 | 42 | STRUCT | A mustUnderstand extension MUST be understood. Other extensions MAY be ignored. |

## 8 Core structure

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-8.2.1-01 | 50 | ATTR | `definitions@expressionLanguage` defaults to XPath. `FormalExpression@language` overrides it. |
| R-8.2.2-01 | 51 | CONF | Imports of XSD, WSDL and BPMN MUST be supported. |
| R-8.3.1-01 | 54 | STRUCT | ids are xsd:ID (unique per document) and REQUIRED when the element is referenced. |
| R-8.3.5-01 | 62 | STRUCT | An event definition is either inline or a root element referenced by `eventDefinitionRef`. |
| R-8.4.1-01 | 66 | EXEC | Artifacts (groups, annotations, associations) do not affect the flow. |
| R-8.4.2-01 | 72 | EXEC | Key-based correlation: the first send or receive initialises the CorrelationKey, and later messages MUST match it. |
| R-8.4.2-02 | 76 | EXEC | Context-based correlation: CorrelationSubscription bindings over process data, updated when the data changes. |
| R-8.4.3-01 | 79 | EXEC | An error catcher without an errorCode catches every error. With a code it catches only matching errors. |
| R-8.4.3-02 | 79 | STRUCT | An error end or throw event in an executable process MUST supply an errorCode. |
| R-8.4.4-01 | 80 | EXEC | An escalation catcher without an escalationCode catches every escalation. With a code it catches only matching ones. |
| R-8.4.4-02 | 80 | STRUCT | An escalation throw in an executable process MUST supply an escalationCode. |
| R-8.4.6-01 | 82 | EXEC | A plain Expression (natural language) is not executable. A FormalExpression is. |
| R-8.4.9-01 | 88 | EXEC | Gateways do no work and have zero effect on cost and time. |
| R-8.4.9-02 | 88 | STRUCT | `gatewayDirection`: Converging MUST have ≤1 outgoing flow, Diverging MUST have ≤1 incoming, Mixed has several of both, Unspecified (the default) is unconstrained. |
| R-8.4.13-01 | 96 | STRUCT | A sequence flow has exactly one source and one target, each an event, activity or gateway. An event sub-process is never a source or target. |
| R-8.4.13-02 | 96 | EXEC | A token is placed on a conditional flow only if its condition is true. |
| R-8.4.13-03 | 96 | STRUCT | A conditional flow leaving an activity requires at least one other outgoing flow. |
| R-8.4.13-04 | 96 | STRUCT | Outgoing flows of Parallel and Event-Based gateways MUST NOT have conditions. |
| R-8.4.13-05 | 97 | STRUCT | A default flow is allowed only from exclusive, inclusive or complex gateways and from activities. |
| R-8.4.13-06 | 97 | EXEC | The default flow is taken only if every other outgoing condition is false. A condition on the default flow is ignored. |
| R-8.4.13-07 | 98 | ATTR | `outgoing` is an ordered collection, which gives the evaluation order. |
| R-8.4.13-08 | 97 | ATTR | `isImmediate`: when absent it means true for executable processes, and it MUST NOT be false there. |
| R-8.4.12-01 | 93 | EXEC | Resources are abstract and have no capacity, cost or availability. |
| R-8.5-01 | 101 | EXEC | Interface, Operation and EndPoint define services. |

## 9 Collaboration

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-9.3-01 | 111 | STRUCT | A Pool MAY have no Process (black box). Message flows then attach to its boundary. |
| R-9.4-01 | 120 | STRUCT | A Message Flow MUST connect two separate Pools and MUST NOT connect two objects in the same Pool. |
| R-9.4-02 | 121 | ATTR | `messageRef` on a message flow is optional. |
| R-9.3.1-01 | 114 | ATTR | ParticipantMultiplicity (minimum 0, maximum 1) makes a pool multi-instance. |
| R-9.1-01 | 109 | ATTR | `Collaboration@isClosed` = false: unmodelled messages MAY occur. |
| R-9.5-01 | 123 | EXEC | Conversations group message flows and have no flow semantics. |

## 10.2–10.3 Process and activities

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-10.2-01 | 146 | ATTR | A non-executable process may have informal conditions, so tools need a fallback. |
| R-10.3-01 | 150 | ATTR | `isForCompensation` = false. A true value means the activity runs only through compensation, never in normal flow and never as an implicit start. |
| R-10.3-02 | 150 | ATTR | `startQuantity` = 1 (≥1): the number of tokens needed before the activity starts. |
| R-10.3-03 | 151 | ATTR | `completionQuantity` = 1 (≥1; the XSD spells it `completetionQuantity`): the number of tokens produced on completion. |
| R-10.3-04 | 151 | EXEC | An activity without incoming flow MUST be instantiated with its process or sub-process, except compensation activities and event sub-processes. |
| R-10.3-05 | 151 | EXEC | An activity without outgoing flow ends its path. The scope completes when no path is active. |
| R-10.3.3-01 | 159 | EXEC | Send Task: the message is sent down all outgoing message flows, then the task completes. |
| R-10.3.3-02 | 159 | EXEC | Receive Task: waits for its message and completes when it arrives. |
| R-10.3.3-03 | 160 | ATTR | `Receive Task@instantiate` = false. It MAY be true only without incoming flow, and the task then starts the process. |
| R-10.3.3-04 | 163 | EXEC | A Script Task without a script behaves as an Abstract Task. User and Manual tasks are done by performers. |
| R-10.3.5-01 | 172 | EXEC | Start and End Events are optional in a Sub-Process. Without a start event, every node without incoming flow starts. |
| R-10.3.5-02 | 174 | STRUCT | An event sub-process MUST NOT have incoming or outgoing sequence flows, and MUST have exactly one start event with a trigger. |
| R-10.3.5-03 | 174 | EXEC | An event sub-process starts a new instance every time its trigger occurs while the parent is active. |
| R-10.3.5-04 | 178 | EXEC | Transaction: Success, Cancel (cancellation and compensation, then the Cancel boundary event) or Hazard (error boundary, no compensation). |
| R-10.3.5-05 | 178 | STRUCT | A Cancel boundary event attaches only to a Transaction. A Cancel end event is used only inside one. |
| R-10.3.5-06 | 180 | EXEC | Ad-hoc sub-process: `ordering` (Parallel by default), `completionCondition`, `cancelRemainingInstances` = true. It contains no start or end events. |
| R-10.3.6-01 | 182 | EXEC | A Call Activity passes control to the called process with sub-process semantics. |
| R-10.3.6-02 | 184 | EXEC | Errors and escalations propagate from the called element to the Call Activity's boundary. |
| R-10.3.6-03 | 185 | ATTR | `calledElement` is [0..1]. Without it the call cannot run. |
| R-10.3.6-04 | 185 | EXEC | `calledElement` MAY reference a GlobalTask (user, manual, script or business rule). |
| R-10.3.6-05 | 184 | STRUCT | A Call Activity's ioSpecification MUST match the callee's. |
| R-10.3.8-01 | 190 | EXEC | Standard loop: `testBefore` = false (do-while), `loopMaximum` is the cap, the loop repeats while `loopCondition` holds. |
| R-10.3.8-02 | 191 | ATTR | Multi-instance: `isSequential` = false. The count comes from `loopCardinality` or `loopDataInputRef`, one of which MUST be given. |
| R-10.3.8-03 | 192 | EXEC | Multi-instance `completionCondition`: when it is true, the remaining instances are cancelled and the activity completes. |
| R-10.3.8-04 | 192 | ATTR | Multi-instance `behavior` = All. None, One and Complex throw implicit events that can be caught on the boundary. |
| R-10.3.8-05 | 191 | EXEC | Multi-instance data: `loopDataInput` item → `inputDataItem`, `outputDataItem` → `loopDataOutput`. Runtime counters exist (`loopCounter`, `numberOfInstances`…). |

## 10.4 Data

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-10.4-01 | 206 | EXEC | A Data Object lives with its parent instance and is visible lexically. Properties are scoped to their owner. |
| R-10.4-02 | 220 | EXEC | Data associations carry no tokens. Only an unavailable source blocks an activity's start. |
| R-10.4-03 | 224 | EXEC | InputSets are tried in order and the first available one starts the activity. Otherwise the activity waits. On completion the chosen OutputSet's associations run. |
| R-10.4-04 | 225 | EXEC | Data association steps: the transformation replaces the target, then the assignments run, otherwise the single source is copied. |
| R-10.4-05 | 212 | STRUCT | An ioSpecification only appears on tasks and callable elements and has ≥1 InputSet and ≥1 OutputSet. Every DataInput and DataOutput belongs to ≥1 set. |
| R-10.4-06 | 225 | EXEC | XPath helper functions such as getDataObject and getActivityInstanceAttribute return an empty node-set on error. |

## 10.5 Events

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-10.5.1-01 | 233 | EXEC | A Signal is broadcast within and across processes, pools and diagrams. |
| R-10.5.1-02 | 233 | EXEC | An Error propagates to the innermost enclosing scope that can catch it, and the thrower is suspended. An Escalation propagates the same way but the thrower continues. |
| R-10.5.1-03 | 234 | EXEC | A trigger that nothing catches is unresolved; its behaviour is unspecified. |
| R-10.5.1-04 | 234 | EXEC | Termination ends every activity immediately, including all multi-instance instances, without compensation or event handling. |
| R-10.5.1-05 | 236 | EXEC | A catch event with several definitions is Multiple (any one trigger). With `parallelMultiple` = true all triggers are needed. A throw event with several definitions throws all of them. |
| R-10.5.2-01 | 238 | STRUCT | Start events are optional. If a level has End Events it MUST have a Start Event, and the other way round. |
| R-10.5.2-02 | 238 | EXEC | Implicit start: every node without incoming flow gets a token, except compensation activities, link catch events and event sub-processes. |
| R-10.5.2-03 | 239 | EXEC | Each start occurrence creates an instance, with one token per outgoing flow. |
| R-10.5.2-04 | 239 | EXEC | When a process with several None Start Events is called, only one of them fires. |
| R-10.5.2-05 | 240 | STRUCT | Allowed triggers: at top level None, Message, Timer, Conditional, Signal, Multiple and Parallel Multiple; in an embedded sub-process None only; in an event sub-process anything but None (Error is always interrupting). |
| R-10.5.2-06 | 241 | ATTR | `isInterrupting` = true. It only matters for event sub-process starts. |
| R-10.5.2-07 | 243 | STRUCT | A start event has no incoming flow (unless it sits on a sub-process border), MUST have an outgoing flow, and its outgoing flows have no conditions. |
| R-10.5.2-08 | 240 | EXEC | A Conditional start fires when its condition becomes true, edge-triggered. |
| R-10.5.2-09 | 240 | EXEC | A Message or Signal start creates an instance when the message or signal arrives. |
| R-10.5.2-10 | 240 | EXEC | A Timer start fires at a date or on a cycle. |
| R-10.5.3-01 | 245 | EXEC | An End Event consumes tokens. The process completes only when every token is consumed. |
| R-10.5.3-02 | 248 | STRUCT | An end event MUST have an incoming flow and MUST NOT have an outgoing flow (except on a sub-process border). |
| R-10.5.3-03 | 246 | EXEC | A Message end event sends one message per outgoing message flow. |
| R-10.5.3-04 | 246 | EXEC | An Error end event terminates all active threads of its sub-process and is caught by the nearest catcher with a matching code or no code. |
| R-10.5.3-05 | 247 | EXEC | With an Escalation end event, the other threads continue. |
| R-10.5.3-06 | 247 | EXEC | A Compensation end event compensates its activityRef, or every visible completed activity in reverse order. |
| R-10.5.3-07 | 248 | STRUCT | An end event with several outgoing message flows MUST be Multiple. |
| R-10.5.4-01 | 248 | EXEC | An intermediate throw event passes immediately. A catch event waits for its trigger. |
| R-10.5.4-02 | 249 | STRUCT | Error and Cancel are never used in normal flow. None and Link are never used on a boundary. |
| R-10.5.4-03 | 254 | STRUCT | An intermediate event in normal flow MUST have incoming and outgoing flow. A link throw needs no outgoing flow. |
| R-10.5.4-04 | 255 | STRUCT | Link: an event is never both source and target. A source needs a target with the same name. Several sources may share one target, but one source has only one target. Links stay on one process level. |
| R-10.5.4-05 | 253 | STRUCT | `cancelActivity` (Table 10.92): Error and Cancel are always interrupting, Compensation not applicable, the rest either way. |
| R-10.5.4-06 | 252 | ATTR | `BoundaryEvent@cancelActivity` = true. |
| R-10.5.4-07 | 254 | EXEC | A boundary event with several outgoing flows forks. |
| R-10.5.4-08 | 256 | STRUCT | A message intermediate event has one incoming OR one outgoing message flow, never both. |
| R-10.5.5-01 | 273 | STRUCT | A timer has exactly one of timeDate, timeCycle and timeDuration, in ISO-8601. |
| R-10.5.5-02 | 273 | EXEC | A timer fires at the date, after the duration, or on the cycle. |
| R-10.5.5-03 | 273 | EXEC | A `timeCycle` on a non-interrupting boundary event or event sub-process start fires repeatedly. |
| R-10.5.5-04 | 268 | STRUCT | In an executable process, `messageRef` (and `operationRef`) MUST be given. |
| R-10.5.5-05 | 262 | ATTR | Compensate definition: `activityRef`, `waitForCompletion` = true. |
| R-10.5.5-06 | 263 | EXEC | A Conditional event fires when its condition becomes true. |
| R-10.5.6-01 | 276 | EXEC | Interrupting: the activity is terminated and one token goes down the exception flow. |
| R-10.5.6-02 | 277 | EXEC | Non-interrupting: the activity continues and a parallel token goes down the boundary flow. |
| R-10.5.6-03 | 278 | STRUCT | At most one interrupting handler (event sub-process or boundary event) per event declaration, and then no non-interrupting ones for it. |
| R-10.5.6-04 | 278 | EXEC | Non-interrupting handlers of the same declaration may run in parallel. |
| R-10.5.6-05 | 279 | EXEC | When an inline event sub-process and a boundary event handle the same event, the inline one runs first. If it re-throws, the boundary event fires; otherwise the event is absorbed. |

**Placement of event definitions** (Table 10.93, rebuilt from the rules in §10.5.2–10.5.5, because the
table is only a graphic). TL = top level, ESP = event sub-process, int / non = interrupting /
non-interrupting. An embedded sub-process allows only a None start.

| Trigger | Start TL | ESP int | ESP non | Catch | Boundary int | Boundary non | Throw | End |
|---|---|---|---|---|---|---|---|---|
| None | ✓ | – | – | – | – | – | ✓ | ✓ |
| Message | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Timer | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | – |
| Error | – | ✓ | – | – | ✓ | – | – | ✓ |
| Escalation | – | ✓ | ✓ | – | ✓ | ✓ | ✓ | ✓ |
| Cancel | – | – | – | – | ✓ (Transaction) | – | – | ✓ (in Transaction) |
| Compensation | – | ✓ | – | – | ✓ | – | ✓ | ✓ |
| Conditional | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | – |
| Link | – | – | – | ✓ | – | – | ✓ | – |
| Signal | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Terminate | – | – | – | – | – | – | – | ✓ |
| Multiple | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Parallel Multiple | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | – | – |

## 10.6 Gateways

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-10.6.1-01 | 288 | STRUCT | A gateway MUST have several incoming or several outgoing flows. |
| R-10.6.1-02 | 288 | EXEC | A gateway without incoming flow in a level without a start event fires when the level is instantiated. |
| R-10.6.2-01 | 290 | EXEC | Exclusive split: exactly one path, the first true condition in order, otherwise the default, otherwise a runtime exception. |
| R-10.6.2-02 | 290 | EXEC | An exclusive merge passes every token through. |
| R-10.6.3-01 | 291 | EXEC | Inclusive split: every true path, otherwise the default, otherwise a runtime exception. |
| R-10.6.4-01 | 292 | EXEC | A parallel gateway forks to every outgoing flow and its join waits for every incoming flow. |
| R-10.6.5-01 | 294 | EXEC | Complex gateway: `activationCondition`, `waitingForStart`/reset. |
| R-10.6.6-01 | 297 | STRUCT | An event-based gateway has ≥2 outgoing flows and none of them has a condition. |
| R-10.6.6-02 | 297 | STRUCT | Event-based gateway targets are intermediate catch events (message, signal, timer, conditional) or receive tasks. |
| R-10.6.6-03 | 297 | STRUCT | An event-based gateway does not mix message events and receive tasks. Its receive tasks have no boundary events, and its targets have no other incoming flows. |
| R-10.6.6-04 | 298 | EXEC | At an event-based gateway the first event to occur wins and the other branches are withdrawn. |
| R-10.6.6-05 | 298 | ATTR | `instantiate` = false. A true value starts the process; such a gateway has no incoming flow. |
| R-10.6.6-06 | 298 | ATTR | `eventGatewayType` = Exclusive. Parallel is allowed only with `instantiate`. |

## 10.7–10.9 Compensation, lanes, process instances

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-10.7-01 | 301 | EXEC | Only completed activities are compensated, through a boundary handler or a compensation event sub-process, in reverse order. Compensating anything else is a no-op (presumed abort). |
| R-10.8-01 | 305 | EXEC | Lanes have no execution semantics. |
| R-10.9-01 | 309 | ATTR | `Process@isClosed` and `SequenceFlow@isImmediate`. |

## 13 Execution semantics

| ID | p. | Tag | Rule |
|---|---|---|---|
| R-13.1-01 | 425 | CONF | Non-operational elements MAY be ignored: Manual Task, Abstract Task, DataState, IORules, Ad-Hoc, Physical items, while-executing sets, isClosed, isImmediate. |
| R-13.2-01 | 426 | STRUCT | A global process MUST NOT have an empty start event or any activity or gateway without incoming flow (event gateways excepted). |
| R-13.2-02 | 426 | EXEC | A process can also be instantiated by an event-based gateway or a receive task. With a parallel event-based gateway the other events are awaited. |
| R-13.2-03 | 426 | EXEC | An instance completes iff no token remains and no activity is active. |
| R-13.3.1-01 | 427 | EXEC | Mixed conditional and unconditional outgoing flows of an activity act as AND plus OR (Fig. 13.1). |
| R-13.3.1-02 | 427 | EXEC | Moving a token along a sequence flow takes no defined time. |
| R-13.3.2-01 | 429 | EXEC | Withdrawn: tasks after an exclusive event-based gateway are withdrawn when another branch wins. |
| R-13.3.2-02 | 429 | EXEC | Completing: the activity waits for running non-interrupting event sub-processes, and no new ones start. |
| R-13.3.2-03 | 429 | EXEC | Interrupting: nested activities and non-interrupting event sub-processes are terminated. |
| R-13.3.2-04 | 429 | EXEC | When no OutputSet is available, or an IORule is violated, a runtime exception is raised. |
| R-13.3.3-01 | 430 | EXEC | A Service Task fault is an interrupting error and the task fails. |
| R-13.3.4-01 | 430 | EXEC | A Sub-Process instantiates its unique none start event, or, without start events, every free node. |
| R-13.3.4-02 | 430 | STRUCT | A Sub-Process MUST NOT have non-none start events (apart from its event sub-processes). |
| R-13.3.4-03 | 431 | EXEC | A Sub-Process may be entered through inner start events that are targets of outside flows; each such start creates an instance. |
| R-13.3.4-04 | 431 | EXEC | A Terminate end event inside a Sub-Process terminates it abnormally. |
| R-13.3.4-05 | 431 | EXEC | A called process's non-none start events are ignored when it is called. |
| R-13.3.6-01 | 432 | EXEC | A loop runs while `loopCondition` holds, tested before or after each run, up to `loopMaximum`. |
| R-13.3.7-01 | 432 | EXEC | Multi-instance: the count is evaluated once, and instances run sequentially or in parallel. |
| R-13.4.1-01 | 434 | EXEC | A parallel join fires when every incoming flow has a token and consumes one per flow. Excess tokens stay. |
| R-13.4.2-01 | 434 | EXEC | Exclusive gateway: every token activates it. The first true condition in order wins, then the default, otherwise an exception. |
| R-13.4.3-01 | 436 | EXEC | Inclusive join: it fires when ≥1 incoming flow has a token and every token that can reach an empty incoming flow can also reach a filled one. It consumes one token per filled flow. |
| R-13.4.4-01 | 437 | EXEC | Event-based gateway: the first event wins and it raises no exception. A parallel instantiating gateway accepts only message triggers with the same correlation. |
| R-13.4.5-01 | 437 | EXEC | Complex gateway: `activationCount`/`activationExpression`, waiting for start and for reset. |
| R-13.5.2-01 | 440 | EXEC | An intermediate catch event waits from the moment its token arrives, and the occurrence is consumed. |
| R-13.5.3-01 | 440 | EXEC | Boundary event: the occurrence is consumed. With `cancelActivity` the activity is cancelled (all multi-instance instances); otherwise it continues (never with errors). |
| R-13.5.4-01 | 440 | EXEC | An interrupting event sub-process cancels its parent (for multi-instance, only the affected instance). A non-interrupting one runs in parallel. |
| R-13.5.4-02 | 441 | EXEC | An event sub-process is enabled only while its parent runs. Only one interrupting handler can start per parent activation; after that, no further handlers start. |
| R-13.5.5-01 | 441 | EXEC | Compensation: handlers, triggering, `waitForCompletion`, snapshot data, default reverse order. |
| R-13.5.6-01 | 443 | EXEC | Terminate at process level ends this instance only; other instances are unaffected. |
| R-13.5.6-02 | 443 | EXEC | A process (or sub-process) completes iff all its start nodes were visited and no token remains. |
| R-13.5.6-03 | 443 | EXEC | A Cancel end event aborts the transaction and leaves through the cancel boundary event. |


## Errata and contradictions in the spec

These are places where the 2.0.2 text contradicts itself. They are not open decisions: the reading on
the right is the one used in this catalogue.

- §2.3 cites "14.2.2" for the execution semantics; they are in Clause 13.
- The XSD spells the attribute `completetionQuantity`; the text and the modellers use `completionQuantity`. Accept both.
- §10.3.5 (p.174) leaves Timer out of the event sub-process triggers, while §10.5.2 (p.241) allows it. Timer is allowed.
- Table 2.2 lists `eventGatewayType` for the inclusive gateway. That is an editorial error.
- Tables 2.2/2.3 footnotes allow conditions only out of gateways; §8.4.13 also allows them from activities. Follow §8.4.13.
- The Complex gateway text says tokens may be produced on reset and also that they MUST NOT be. Clause 13 (§13.4.5) governs.
- The `compensable` attribute of §10.7.1/§13.5.5 does not exist in the XSD.
- `isExecutable` has no default: §10.9 says an absent value is false in one sentence and true in the next.
- `waitForCompletion` defaults to true in Table 10.94, but the XSD gives no default. True is used.

## Open decisions

These are places where the spec is ambiguous or contradicts itself, and bpmn-sim has to pick a
reading. Each one stays **open** until it is decided; the decision is then written here and the rows
that reference it are updated.

| ID | Question | What the spec says, and today's behaviour | Rules | Decision |
|---|---|---|---|---|
| D-1 | Top-level process without start events / free nodes in a scope with start events | §10.5.2 starts every node without incoming flow. §13.2 forbids such nodes in a global process. | R-10.3-04, R-13.2-01 | **Decided.** (A) A top-level process without any start event is allowed (§10.5.2): an arrival can name the process, and every node without incoming flow starts; validation warns, citing §13.2. (B) A scope that has start events and also nodes without incoming flow is malformed (§13.2/§13.3.4): validation error naming both rules, and those nodes never run. |
| D-2 | Several None Start Events in a called process or sub-process | §10.5.2 says only one fires and leaves which one open. §13.3.4 assumes a unique one. Today all of them fire. | R-10.5.2-04, R-13.3.4-01 | **Decided.** (a) An embedded sub-process with several None start events is malformed (§13.3.4): validation error, not run. (b) For a called process with several None start events, the scenario names the one a call activity uses (`[calls.<call activity>] start = "<start event>"`); without it, binding fails and lists the candidates. Exactly one fires. |
| D-3 | What a "runtime exception" is in the simulator | Covers no path at an XOR, OR or complex gateway, a missing OutputSet and an IORule violation. Today the token stays stuck (incident). | R-10.6.2-01, R-10.6.3-01, R-13.4.2-01 | **Decided.** A runtime exception is an *incident*. The instance halts at the element with its tokens kept, and the case is reported as "failed: runtime exception" together with the rule ID, separately from deadlocks. It never counts as completed. It is not a BPMN error, so error boundary events and error event sub-processes do not catch it. |
| D-4 | Uncaught error or escalation | Unspecified. Terminating the instance is mentioned only as common. Today an error leaves its token stuck and an escalation does nothing. | R-10.5.1-03 | **Decided.** (a) An uncaught error is an incident, as in D-3: the whole instance halts, running tasks are cancelled, and the case is reported as "failed: uncaught error '<code>'". (b) An uncaught escalation has no effect and the thrower continues (escalation is non-critical), but the run report lists it. |
| D-5 | Escalation boundary default | The text says it does not interrupt by default. The XSD default for `cancelActivity` is true. Proposal: follow the XSD (as today). | R-10.5.4-06 | open |
| D-6 | Compensating a loop or multi-instance activity with a boundary handler | Once per instance (§10.7.2) or once (§13.5.5)? | R-13.5.5-01 | open |
| D-7 | startQuantity/completionQuantity > 1 | The spec doesn't say whether the tokens must come from different flows, or whether N means per flow or in total. | R-10.3-02, R-10.3-03 | open |
| D-8 | Ad-hoc order in a simulation | No performer exists. The scenario could give a random choice, a sequence, or a probability per activity. | R-10.3.5-06 | open |
| D-9 | Transaction protocol verification | Proposal: instant success, with an optional cancel or hazard probability. | R-10.3.5-04 | open |
| D-10 | Terminate inside a sub-process | "Abnormally terminated" with no word on the parent. Today the parent continues (as in Camunda). | R-13.3.4-04 | **Decided.** The parent continues. The sub-process's other tokens are cancelled (for multi-instance, only that instance), the sub-process ends, and its token leaves along the outgoing flows. This is the common reading (Camunda; Silver, *BPMN Method and Style*) of a point the spec leaves open. |
| D-11 | How often conditional events are evaluated | Proposal: after every data change, edge-triggered. | R-10.5.5-06 | open |
| D-12 | Data-driven vs. scenario-driven, per element | Proposal: data-driven if there is a formal expression in a supported language, unless the scenario has an entry. | R-10.4-01 | open |
