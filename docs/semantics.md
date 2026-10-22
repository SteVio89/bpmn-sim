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
