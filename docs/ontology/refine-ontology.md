# Refine Ontology (As Implemented)

## Status

Code-derived reference, current at commit
`8e3cc6fc7ae6aae5bef8afb2fccd58f4a6959fb5` (2026-07-27).

This document describes the ontology that the current implementation actually
enforces. It is not a proposal. In particular,
[`ontology-spec.md`](../spec/ontology-spec.md) is a future design for ontology-driven
implementation and must not be read as the current Refine domain model.

The machine-readable companion is the
[`docs/ontology` OWL/RDF package](README.md).

## How To Read This Reference

Where the code has multiple representations, this document uses the following
order of authority:

1. durable records and the services that read and mutate them;
2. workflow and product rules that enforce relationships and transitions;
3. Rust model types;
4. projections, API payloads, and UI representations.

This ordering matters. Some public model structs lag the durable JSON shape.
For example, the durable Goal record has Git and revision fields not present in
[`model::goal::Goal`](../../src/model/goal/mod.rs), and the work-item service
stores Round evaluation fields flat while `GoalRound` still exposes older
nested `governance` and `quality` views. The service-owned JSON contract is the
implemented authority in those cases.

The scope is Refine's domain model: work, intent, ownership, workflow,
execution, evidence, and projections. Transport-only request/response types and
installer, release, and desktop-shell mechanics are excluded unless they define
a domain boundary.

## The Ontology In One Paragraph

A local **Refine Runtime** selects a **Target App** from an app registry. The
target app's Git repository owns a **Project State** containing **Governance**,
**Guidance**, **Quality Settings**, **Reporters**, **Nodes**, **Features**,
**Goals**, **Todo Lists**, and retained evidence. A **Node** owns each Feature
and Goal. A Feature groups Goals and may order some of them. A Goal is the
smallest schedulable work item; it contains Notes and an ordered history of
Rounds. The current Round supplies the implementation request and accumulates
implementation, Governance, Quality, Git-integration, failure, and log evidence.
The **Workflow Engine** reserves a Goal with a Claim, pins an Execution to one
Goal/Node/Round/revision, consumes an agent-capacity Lease, and realizes work
through Operations, Managed Processes, an isolated Git Worktree, and an agent
session. Automation ends at **Review**; a human acceptance operation verifies
the integrated candidate and moves the Goal to **Done**. Projections and
dashboards are rebuildable views, never the source of truth.

## Concept Map

```text
Refine Runtime
├── App Registry ──selects──> Target App (Git repository)
├── runtime authority
│   ├── Active Node selection
│   ├── Workflow Automation State
│   │   └── Workflow Claim ──pins──> Goal + Node + Round + revision
│   ├── Agent Capacity State
│   │   └── Lease ──owned by──> Claim or other agent work
│   ├── Operation Registry
│   ├── Managed Process Registry
│   └── Target App / agent session observations
└── Target App Project State (Git-backed)
    ├── Project Config
    ├── Governance
    │   ├── Product
    │   ├── Constitution
    │   └── Rules
    ├── Guidance
    ├── Quality Settings
    ├── Reporter Registry
    ├── Node Registry
    │   └── Node ──owns──> Feature and Goal
    ├── Feature ──groups by inverse reference──> Goal
    │   └── ordered membership ──constrains──> scheduling
    ├── Goal
    │   ├── Notes
    │   ├── Rounds
    │   │   └── request + pinned context + evaluation + integration evidence
    │   ├── Round log sidecar
    │   └── Git candidate identity
    ├── Reporter ──owns──> Todo Lists ──contain──> Todo Items
    ├── Chat Sessions ──attach to──> Goal | Feature | Standalone
    └── Activity

Derived only
└── Projection Snapshot
    ├── Goal summaries
    ├── Feature status and rollups
    ├── Activity and Git changes
    └── Dashboard and runtime projections
```

## Core Concepts

### Runtime, Target App, And Project State

| Concept | Implemented meaning | Identity and authority |
|---|---|---|
| **Refine Runtime** | One local daemon/control-plane instance and its port-scoped runtime directory. It owns local processes, operations, claims, capacity, caches, and app selection. Its pause control is keyed by port in durable app-support state so changing the runtime directory cannot resume automation. | A runtime bootstrap has an `instance_id`; its location is resolved by [`RuntimeRoot` and `RuntimePathLayout`](../../src/process/supervisor/runtime/mod.rs). |
| **Registered App** | A locally known target repository with a display name, path, add time, and optional last-used time. | Keyed in `AppRegistry.apps`; one path may be `active_app`. See [`model::project`](../../src/model/project/mod.rs) and [`FileProjectRegistryService`](../../src/tools/product/project_registry/mod.rs). |
| **Target App** | The Git repository whose product is being changed and whose build/health lifecycle Refine may control. | Identified operationally by repository path; there is no independent durable Target App ID in project state. Target-app observations are modeled in [`target_apps`](../../src/tools/host/target_apps/mod.rs). |
| **Project State** | Refine's durable model for one target app. It is a tree of plain JSON/JSONL records, mirrored to Git. | The live tree is `<git-common-dir>/refine-live-state`; the committed copy is `.refine/` on `refine/state` via `<git-common-dir>/refine-state-worktree`. See [`project_layout`](../../src/tools/host/project_layout.rs) and [`git_sync`](../../src/tools/host/git_sync/mod.rs). |
| **Project Config** | Schema/version metadata and project-level settings envelope in `refine.json`. | One per project-state tree; currently schema version 2. See [`ProjectConfig`](../../src/model/project/mod.rs) and [`project_migration`](../../src/tools/product/project_migration/mod.rs). |

There is no single durable `Project` entity with its own ID. "Project" in the
code is the conjunction of a registered target repository and its Refine state
tree. `ProjectStatus`, schema status, migration reports, and maintenance are
operational views over that conjunction.

### People And Intent

| Concept | Implemented meaning | Important semantics |
|---|---|---|
| **Reporter** | A named human/actor in `reporters.json`. Registry records have numeric `id`, `name`, and `created`. | Features, Goals, Rounds, and Todo Lists reference the **name**, not the registry ID. Rename/merge rewrites those name references. See [`reporters`](../../src/process/supervisor/config/reporters.rs) and [`reporter_codec`](../../src/process/supervisor/config/reporter_codec.rs). |
| **Assignee** | The actor expected to perform work. | Not a separate entity or foreign key. It is another Reporter-style name. A Goal's effective assignee is the latest valid Round's assignee, then legacy Goal assignee, then latest Round reporter. |
| **Governance** | Project-wide product intent and constraints. | `governance.json` contains `product`, `constitution`, `rules`, and derived `configured`. It is configured only when both Product and Constitution are non-empty. See [`governance`](../../src/process/supervisor/config/governance.rs) and [`governance_codec`](../../src/process/supervisor/config/governance_codec.rs). |
| **Rule** | A normalized prose constraint with `id`, `text`, timestamps, and `source`. | IDs are unique after normalization; rule text is bounded and whitespace-normalized. Rules are judged by the Governance agent, not mechanically executed. |
| **Guidance item** | Conditional agent instructions with `name`, `rule`, `instructions`, and `enabled`. | Enabled Guidance is pinned into Round agent context. Changed code requires the Goal Agent to report applicable indexes; the resulting decision is durable Round evidence. See [`guidance_codec`](../../src/process/supervisor/config/guidance_codec.rs) and [`WorkflowImplementation`](../../src/workflow/behaviors/mod.rs). |
| **Quality Settings** | Project-wide business requirements, instructions, plain-text tests, enablement, and timing. | Quality timing is `pre_merge` or `post_build` and is pinned per candidate Round so an in-flight candidate cannot change validation order when settings change. See [`quality::types`](../../src/tools/host/quality/types.rs) and [`WorkflowContext::quality_timing`](../../src/workflow/context.rs). |

The current code does not implement the durable **Architecture** object proposed
by `ontology-spec.md`. Current Governance is Product + Constitution + Rules.

### Work

#### Feature

A **Feature** is a durable container for a larger product outcome:

- identity and metadata: `id`, `name`, description, reporter, assignee,
  `node_id`, timestamps;
- it does **not** persist an embedded list of Goals;
- membership is the inverse of `Goal.feature_id`;
- order is the inverse of `Goal.feature_order`;
- its status, counts, next Goal, and blocking information are derived.

The basic Rust type is [`Feature`](../../src/model/feature/mod.rs), while the
durable writer in
[`service/features.rs`](../../src/tools/product/work_items/service/features.rs)
also persists `assignee`.

Feature deletion is aggregate deletion: it deletes all member Goal records and
then the Feature record. Feature transfer changes the Feature's Node owner and
all member Goals' owners. A Goal assigned to a Feature cannot be transferred
alone; transfer the aggregate instead.

#### Goal

A **Goal** is Refine's smallest schedulable and workflow-governed unit of work.
Its durable record contains:

- identity: `id`, `name`;
- scheduling: status, priority, Node owner;
- attribution: reporter and effective assignee;
- aggregation: optional `feature_id` and optional `feature_order`;
- Git candidate identity: branch, target branch, base commit, candidate commit;
- optimistic workflow revision;
- timestamps;
- Notes;
- ordered Rounds.

The typed core is [`model::goal::Goal`](../../src/model/goal/mod.rs). The
authoritative record shape is created by
[`create_authored_goal`](../../src/tools/product/work_items/service/goal_authoring/authoring.rs)
and mutated by [`FileWorkItemService`](../../src/tools/product/work_items/service.rs).

Priority is `low`, `medium`, or `high`. Within the same Node, claim selection
prefers higher-priority eligible Todo Goals before lower-priority eligible
Goals.

#### Note

A **Goal Note** is an embedded annotation with `id`, `author`, `body`,
`created`, and `updated`. Notes belong to exactly one Goal and have no workflow
state. They contribute to Goal search text.

#### Round

A **Round** is one implementation attempt/request in a Goal's ordered history.
It has no independent ID; its zero-based array index is its workflow identity,
and user-facing numbering is index + 1.

A Round contains four semantic groups:

1. **Request** — reporter, assignee, prompt, created/updated.
2. **Pinned context** — `agent_context`, Guidance decision, Quality timing, Git
   remote.
3. **Outcome evidence** — implementation report, Governance fields, Quality
   fields, failure fields.
4. **Integration evidence** — exact candidate/target commits, target branch,
   remote, push result, timestamp, and merge result.

Round construction is authoritative in
[`new_round_value`](../../src/tools/product/work_items/service/round_helpers.rs);
Round mutation is in
[`rounds_and_metadata.rs`](../../src/tools/product/work_items/service/rounds_and_metadata.rs).

The latest Round is the current implementation request. Earlier Rounds remain
history. A Goal with no Round may exist; the workflow creates a default Refine
Round before execution. Appending a Round while a Goal is in Review moves it
back to Todo, preserving the reviewed integration as history.

#### Todo List And Todo Item

A **Todo List** is a Reporter-owned personal list, separate from Feature/Goal
workflow. It has an ID, Reporter name, name, timestamps, and embedded Todo
Items. A **Todo Item** has an ID, text, done flag, and timestamps.

Todo Items are intentionally lightweight: they do not reference Goals,
Features, Nodes, Rounds, or workflow state. See
[`tools::product::todos`](../../src/tools/product/todos/mod.rs).

### Ownership And Distribution

#### Node

A **Node** is the durable ownership and distributed-execution unit. It has:

- ID and display name;
- enabled/archived state;
- Node-scoped project settings;
- SSH and remote checkout coordinates;
- target-app path and Refine port;
- optional observed health.

Nodes live in `nodes.json`; see [`model::node`](../../src/model/node/mod.rs) and
[`FileNodeRegistryService`](../../src/tools/product/nodes/mod.rs).

`node_id` on a Feature or Goal is ownership, not merely a display filter.
Automation may claim a Goal only when its owner matches the runtime's active
Node. The claim then pins that Node identity through execution and integration.
An archived Node cannot become active.

#### Cluster

**Cluster** is not a second durable set of machines. In the model,
`ClusterNode` is a type alias for `Node`, `ClusterHealth` is a type alias for
`NodeHealth`, and `Cluster` is a timestamped list view over Nodes. The cluster
service migrates legacy `cluster.json` data into the Node registry. See
[`model::cluster`](../../src/model/cluster/mod.rs) and
[`FileClusterService`](../../src/tools/host/cluster/mod.rs).

#### Feature Order

Feature order is the only implemented dependency relation:

- `feature_order = null` means unordered/independent within the Feature;
- an integer means ordered placement;
- earlier ordered Goals on the same Node block later Goals until the earlier
  Goal reaches Review, Done, or Cancelled;
- only one ordered Goal in a Feature/Node may be in an automated state at once;
- a Failed earlier Goal therefore blocks later ordered work until recovery or
  cancellation.

There is no general Goal dependency graph, cross-Feature dependency, or typed
prerequisite edge. `FeatureGoalPlacement::After` is an authoring operation that
computes integer order; the referenced prerequisite is not retained as an edge.
See [`feature_claim_eligible`](../../src/workflow/policy.rs) and
[`goal_authoring/placement.rs`](../../src/tools/product/work_items/service/goal_authoring/placement.rs).

### Workflow Authority

#### Goal Status

`GoalStatus` is the public work lifecycle:

```text
backlog
todo
in-progress
qa
ready-merge
build
review
done
failed
cancelled
```

The status is durable on the Goal. Its enum and operation rules are in
[`model::workflow`](../../src/model/workflow/mod.rs). Status is not inferred from
processes, claims, browser state, or Git.

The normal automated paths are:

```text
Pre-merge Quality:
backlog -> todo -> in-progress -> qa -> ready-merge -> build -> review

Post-build Quality:
backlog -> todo -> in-progress -> ready-merge -> build -> qa -> review

Human acceptance:
review -> done
```

- Backlog becomes Todo manually or by age-based promotion.
- Todo creates the isolated branch/worktree and enters In Progress.
- In Progress runs the Goal Agent, commits/no-op anchors the candidate, records
  Guidance and Governance evidence, then chooses the path pinned by Quality
  timing.
- QA evaluates the exact candidate.
- Ready Merge integrates and optionally publishes the exact candidate, then
  records `RoundIntegration`.
- Build runs the configured target-app build.
- Review is an automation stop and human boundary.
- Approval verifies the candidate is still integrated (and published when
  recorded as pushed) before moving Review to Done.

Automation may move any active automated stage to Failed. Failed is recoverable
by recording a new Round and separately moving it back to Todo, or by an
explicit stage-retry route; appending the Round alone does not requeue a Failed
Goal. Failed is not included in `is_terminal_status`, although Feature rollup
treats Failed as final when deciding whether all Feature Goals have ended.
Cancelled and Done are the terminal statuses used by generic terminal checks.

User transitions are deliberately much narrower than the enum:

```text
backlog <-> todo
failed -> todo
cancelled -> todo
done -> review
```

Submitting a new Round is the normal Review-decline model and preserves the
history needed for Failed recovery. For a Failed Goal, a supported transition
from Failed to Todo or a stage retry is still required after the Round is
recorded.
Explicit cancellation moves any non-Done Goal to Cancelled through shared
process/workflow settlement when active work exists.

#### Workflow Claim

A **Workflow Claim** is runtime authority to work one Goal. It records:

- `claim_id` and `goal_id`;
- pinned Node, provider, and target-app identities;
- optional `execution_id`, Round index, and Goal revision;
- a monotonic `decision_version`;
- timestamps and claim state.

Claim states are `claimed`, `running`, `completed`, `failed`, `cancelled`, and
`interrupted`. Claims live in `workflow-automation-state.json` and are distinct
from Goal status. See [`WorkflowClaim`](../../src/workflow/mod.rs).

At most one active Claim is selected for a Goal. Starting a Claim creates an
Execution ID and acquires capacity. Before consequential work, execution fences
bind the Claim, Execution, Goal, Node, Round, Goal revision, and decision
version. These fences prevent stale work from integrating or settling a newer
Round.

#### Execution

An **Execution** is a correlation identity, not a standalone Rust entity. Its ID
appears on the Workflow Claim, workflow context, process metadata, operation
requests, logs, cancellation records, and integration fences. One execution
runs one claimed Goal attempt from its pinned current status and Round.

#### Capacity Lease

An **Agent Capacity Lease** is a runtime reservation with owner, role, Node,
provider, target app, holder PID, and acquisition time. Workflow leases use
`owner_id = workflow:<claim_id>`. Capacity policy applies global, per-Node,
per-provider, and per-target-app limits. Dead holder PIDs are pruned. See
[`workflow::capacity`](../../src/workflow/capacity.rs).

### Execution And Interaction

#### Operation

An **Operation** is a durable runtime envelope for a cancellable background
action. It has ID, owner string, state, request, progress, result, and optional
error. States are Pending, Running, Cancelling, Succeeded, Failed, Cancelled,
and Interrupted.

Operations own cancellation and recovery boundaries around imports, Quality,
Ready Merge, chat turns, and similar work. Their request payloads carry domain
correlation such as Goal, Round, Claim, and Execution IDs. See
[`operations`](../../src/process/supervisor/operations/mod.rs).

#### Managed Process

A **Managed Process** is Refine's observable OS-execution record. It has an ID,
owner kind, PID, state, label/details, output paths, resource limits, start
time, and exit code. Owner kinds are Daemon, Runner, Target App, Agent, Quality,
Import, Maintenance, and User Helper.

Relationships to Goal, Feature, session, Claim, Execution, Round, worker, and
Operation are carried in structured process metadata rather than typed foreign
keys. See [`process::subprocess`](../../src/process/subprocess/mod.rs).

Process state is evidence about execution, not authority for Goal status.
Stopping Goal work therefore uses shared settlement that coordinates process
exit, Operation state, Workflow Claim, capacity Lease, and durable Goal outcome.

#### Agent Session

An **Agent Session** is a PTY-backed interactive view over a managed Agent
process. Its snapshot exposes session/process IDs, profile, provider, cwd,
optional Goal/worktree, attention, transcript size, and liveness. A workflow
Goal Agent exists only while a live workflow-owned session is correlated to the
Goal. See [`process::agent_sessions`](../../src/process/agent_sessions.rs).

#### Chat Session

A **Chat Session** is a durable conversation attached to a Goal, Feature,
Standalone context, or legacy Supervisor context. It records provider/session
identity, transcript events, queued messages, importable artifacts, closure and
interruption state, and an optional standalone worktree.

Standalone chat may produce a worktree and later submit it as a Goal. A chat
session is not itself a Round or Workflow Claim. See
[`tools::product::chat`](../../src/tools/product/chat/mod.rs).

### Evidence

| Evidence concept | Belongs to | Meaning |
|---|---|---|
| **Implementation report** | Round | Goal Agent's completion output and timestamp. |
| **Guidance decision** | Round | Which enabled Guidance items were applied or skipped for the observed change. |
| **Governance evaluation** | Round | Rule/meta-rule/Product/Constitution states, message, details, checked time, and proposed rule actions. |
| **Quality evaluation** | Round | State, message, structured details/test results, and checked time for the exact candidate. |
| **Failure evidence** | Round | Failure category, message, and time for the attempt that failed. |
| **Round Integration** | Round | Exact candidate commit, target branch/commit, remote, push result, integration time, and merge result. |
| **Round Log Entry** | Goal + Round index | Append-only workflow/Git/agent/Quality/build/state evidence in the Goal's `logs.jsonl` sidecar. |
| **Activity Entry** | Local project observability, optionally Goal | General activity record with severity, category, actor, details, and actions. |
| **Git Change projection** | Derived Goal join | Recent commit joined to a Goal by branch/subject; useful for display, not authoritative linkage. |

Round logs are physically separate from `goal.json`. `show_goal_detail` joins
them into each Round at read time and adds latest-log convenience fields.
Global Activity uses `logs/activity.jsonl`, which the Git synchronizer classifies
as runtime-only. Round logs live under `goals/` and are included in Git-backed
project state.

## Cardinalities And References

| Source | Relation | Target |
|---|---|---|
| Runtime | knows `0..*`, selects `0..1` | Registered App |
| Registered App | locates `1` | Target App repository |
| Target App | owns `1` | Project State tree |
| Project State | contains `1` registry | Nodes |
| Project State | contains `0..*` | Features, Goals, Reporters, Todo Lists, Chat Sessions |
| Node | owns `0..*` by `node_id` | Features and Goals |
| Feature | groups `0..*` by inverse `feature_id` | Goals |
| Goal | embeds `0..*` | Notes |
| Goal | embeds ordered `0..*` | Rounds |
| Goal | has `0..1` | Feature |
| Goal Round | has `0..*` by Round index | Round Log Entries |
| Goal Round | has `0..1` each | pinned context, implementation, Governance, Quality, failure, integration evidence |
| Reporter name | owns `0..*` | Todo Lists |
| Todo List | embeds `0..*` | Todo Items |
| Workflow Automation State | contains `0..*` historical/current | Claims |
| Workflow Claim | refers to `1` | Goal |
| Workflow Claim | pins `0..1` current | Round and Execution |
| Capacity State | contains `0..*` | Leases |
| Operation | correlates by metadata to `0..*` | Managed Processes and domain objects |
| Managed Process | correlates by metadata to `0..1` each | Goal, Feature, Claim, Execution, Round, session, Operation |

Most relationships are string references rather than typed database foreign
keys. Referential integrity is enforced at mutation/workflow boundaries, not by
a database.

## Authority And Persistence

### Git-Backed Project Authority

The live project-state tree is a working projection of the `.refine/` tree
committed on branch `refine/state`. Git sync includes every non-transient file
except runtime-only top-level paths (`run`, `runtime`, `logs`,
`support-bundles`, `provider-bin`) and `manage-app.log`.

Key authoritative records:

| Path under project state | Authority |
|---|---|
| `refine.json` | Project schema and version |
| `nodes.json` | Node registry and Node-scoped settings |
| `governance.json` | Product, Constitution, Rules |
| `guidance.json` | Guidance items |
| `quality/settings.json` | Quality requirements, tests, enablement, timing |
| `reporters.json` | Reporter registry |
| `todo-lists.json` | Reporter-owned Todo Lists and Items |
| `features/<ID[0:2]>/<ID[2:]>/feature.json` | Feature record |
| `goals/<ID[0:2]>/<ID[2:]>/goal.json` | Goal, Notes, Rounds, Git candidate identity |
| `goals/<ID[0:2]>/<ID[2:]>/logs.jsonl` | Round-scoped Goal log evidence |
| `chat/sessions/<session-id>.json` | Chat session record |

Goal and Feature writes carry `workflow_revision` and use coordinated atomic
replacement. The projection cache must never be used as mutation authority.

### Runtime Authority

Runtime records are local operational truth and are not part of synchronized
project state:

| Runtime record | Authority |
|---|---|
| `apps.json` | Local Registered App selection |
| `active-node.json` | Local active Node selection when a runtime root is supplied |
| `workflow-automation-state.json` | Claim history, active claims, policy snapshot |
| `agent-capacity-state.json` | Live capacity leases |
| `process-control.json` | Workflow admission pause |
| `operations/*.json` | Background Operation state and recovery |
| `processes/*.json` and `process-identities/*.json` | Managed process observation and OS identity |
| `target-app-state.json` | Last target-app lifecycle/health observation |
| cache `projection-snapshot.json` | Rebuildable read model |

The same service can fall back to placing `active-node.json` in the Refine
directory when no separate active root is supplied; production callers should
pass the runtime root when selection is meant to remain local.

### Derived Views

[`ProjectionSnapshot`](../../src/tools/product/project_state/types.rs) derives:

- Goal and Feature summary maps;
- inverse Feature membership and order;
- Feature rollups and status;
- Activity joined from global activity and Round logs;
- recent Git changes joined heuristically to Goals;
- status counts and Reporter/Assignee statistics;
- dashboard attention;
- runtime display fields.

Its cache is accepted only when source fingerprints still match. Malformed,
incomplete, or old-version snapshots are cache misses. A projection can
disappear without losing domain truth.

## Derived Semantics Worth Preserving

### Feature Status Is Not Durable

Feature status is a Goal-status rollup:

1. if all Goals are Done, Failed, or Cancelled, Feature status is Done;
2. otherwise any active Goal makes it In Progress;
3. otherwise any Failed Goal makes it Failed;
4. otherwise all-Cancelled makes it Cancelled;
5. otherwise any Todo makes it Todo;
6. otherwise it is Backlog.

Therefore Feature `Done` means all member work is final, not that every Goal
succeeded. The individual counts preserve the distinction.

### Reporter And Assignee Are Denormalized Names

The Reporter registry has numeric IDs, but work records retain names. This makes
records readable and sync-friendly, but means rename/merge is a cross-record
rewrite. There is no referential guarantee that every historical name still has
a Reporter registry row.

### Latest Round Supplies Current Assignment

Goal summaries derive:

- Reporter from Goal reporter, then first Round reporter, then legacy Goal
  assignee;
- Assignee from latest Round assignee, then legacy Goal assignee, then latest
  Round reporter.

Assignment can therefore change by Round while the Goal's original Reporter
remains stable.

### Status, Claim, Operation, And Process Are Separate State Machines

These facts may temporarily differ without contradiction:

- Goal status says where the work is in the product workflow;
- Claim state says whether automation owns or has settled a scheduling claim;
- Operation state says whether one cancellable background capability is active;
- Process state says what the OS child is observed doing;
- session state says whether an interactive attachment is available.

Settlement and recovery code exists to reconcile them. No one of the latter
four may silently overwrite durable Goal meaning.

### Git Is Evidence And Realization

The Goal identifies the current branch/base/candidate. The Round records exact
integration evidence. The target repository holds the realized source change.
None alone is sufficient:

- a candidate commit without Round integration is not Ready Merge success;
- Round integration without current Git ancestry fails human approval;
- a merged commit does not by itself mutate Goal status;
- projection joins from recent commits are display conveniences.

## Negative Space: Concepts The Code Does Not Have

The absence of these concepts from Refine's runtime model is part of the
current ontology:

- no runtime-maintained or operationally authoritative ontology graph; the
  descriptive [`docs/ontology`](README.md) OWL/RDF layer does not
  replace Refine's JSON, JSONL, Git, or runtime services;
- no durable Architecture document;
- no generic persistent `WorkItem` supertype—Feature and Goal are separate
  records despite `WorkItemService` naming;
- no Project ID distinct from repository path/state tree;
- no independent Round ID or Round status;
- no general dependency graph beyond Feature integer order;
- no Assignee entity distinct from Reporter-style names;
- no durable Execution object apart from correlated IDs and evidence;
- no Cluster node type distinct from Node;
- no Feature-owned Goal list;
- no process-derived Goal status;
- no projection-derived mutation authority;
- no automatic transition from Review to Done.

## Known Representation Drift

These are not speculative design concerns; they are visible differences inside
the current code and should be considered when changing the model:

1. `model::goal::Goal` omits durable `target_branch`, `base_commit`,
   `candidate_commit`, `workflow_revision`, and effective assignee.
2. `model::feature::Feature` omits the durable assignee field.
3. `GoalRound` exposes nested `governance` and `quality` options, while the
   current work-item service creates and updates flattened evaluation fields.
4. Several domain objects—Governance, Guidance, Reporter, and raw Round
   evaluation—are normalized `serde_json::Value` rather than fully typed Rust
   models.
5. Process and Operation relationships use metadata/string conventions rather
   than typed references.

Until those representations converge, code that needs authoritative current
meaning should use the shared product/config/workflow services, not deserialize
the older public structs and write them back wholesale.

## Primary Code Index

| Concern | Primary code |
|---|---|
| Core model types | [`src/model`](../../src/model/mod.rs) |
| Goal/Feature mutation and invariants | [`tools/product/work_items`](../../src/tools/product/work_items/mod.rs) |
| Durable record projection | [`tools/product/project_state`](../../src/tools/product/project_state/mod.rs) |
| Workflow states and operation rules | [`model/workflow`](../../src/model/workflow/mod.rs) |
| Workflow claims and scheduling | [`workflow`](../../src/workflow/mod.rs) |
| Status behaviors | [`workflow/behaviors`](../../src/workflow/behaviors/mod.rs) |
| Execution fences/context | [`workflow/context`](../../src/workflow/context.rs) and [`workflow/ready_merge`](../../src/workflow/ready_merge.rs) |
| Human Review approval | [`tools/product/merging`](../../src/tools/product/merging/mod.rs) |
| Node and Cluster ownership | [`tools/product/nodes`](../../src/tools/product/nodes/mod.rs) and [`tools/host/cluster`](../../src/tools/host/cluster/mod.rs) |
| Governance, Guidance, Reporters, settings | [`process/supervisor/config`](../../src/process/supervisor/config/mod.rs) |
| Quality | [`tools/host/quality`](../../src/tools/host/quality/mod.rs) |
| Process substrate | [`process/subprocess`](../../src/process/subprocess/mod.rs) |
| Operations | [`process/supervisor/operations`](../../src/process/supervisor/operations/mod.rs) |
| Agent sessions | [`process/agent_sessions`](../../src/process/agent_sessions.rs) |
| Project-state layout and sync | [`tools/host/project_layout`](../../src/tools/host/project_layout.rs) and [`tools/host/git_sync`](../../src/tools/host/git_sync/mod.rs) |
