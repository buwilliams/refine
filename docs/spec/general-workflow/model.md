# Refine v5 General Workflow Model

## Status

Design specification and implementation target for Refine v5. This document is
not a description of the current v4 model.

This is one of five coordinated specifications:

- [`model.md`](model.md) defines the domain model and invariants.
- [`cli-surface.md`](cli-surface.md) defines the command-line contract.
- [`browser-surface.md`](browser-surface.md) defines the browser/desktop UX.
- [`code.md`](code.md) defines the implementation architecture and rollout.
- [`refactor.md`](refactor.md) closes codecs, migration, parity, and checkpoint
  details required for implementation.

Where these documents disagree, the model and authority boundaries in this
document win; surface ergonomics do not override shared semantics.

## Decision

Refine v5 generalizes Refine from a fixed software-delivery workflow into a
data-driven workflow engine centered on **Jobs**:

```text
Agent or user ──designs──> Domain Workflow

User, agent, or system ──creates──> Job
                                      │
                                      └──moves through──> Domain Workflow
```

A Job is the durable identity of one unit of work. A versioned Domain Workflow
defines the states, actions, transitions, prompts, schemas, policies, and
completion conditions that may advance it.

The engine remains deterministic. Agents may propose workflows, outcomes,
property patches, attachments, and amendments, but shared capabilities validate
and apply those proposals. An agent response is never mutation authority by
itself.

## Nomenclature

The v5 product vocabulary is:

| v4 term | v5 term | Meaning |
|---|---|---|
| Target App / attached Project | Target Product / Product | The Git-backed product Refine is attached to and operates on |
| Registered App | Registered Product | One locally registered Target Product |
| App Registry | Product Registry | The local registry and active-product selection |
| Feature | Project | A durable outcome or initiative that groups and orders Jobs |
| Goal | Job | The central durable unit of work |
| Goal Round | Job Revision | An immutable revision of the requested work |
| Goal status | Workflow state projection | Domain state derived from the active Workflow Version and Job Execution |
| Workflow behavior | Step executor | A registered implementation of a workflow action |
| Goal Agent | Job Agent | An agent invocation attached to a Job step attempt |

The entity noun is **Product**; **Target Product** is its role while Refine is
attached to it. Lowercase prose may say “the targeted product.” Source
identifiers should converge on `target_product` for active-context host
capabilities and `product` for durable entity contracts, rather than
retaining older Project-based identifiers.

**Project** is reserved for the former Feature concept. It must not also mean
the attached Product in a v5 public contract.

“Revision” is always qualified:

- **Job Revision** changes the requested work;
- **Workflow Version** changes reusable workflow behavior;
- **Run Plan Revision** changes the future plan of one active Job Execution;
- **Artifact revision** is represented by a new immutable Artifact, not an
  in-place content mutation.

## Model Overview

```text
Refine Runtime
├── Product Registry
│   └── Registered Product ──locates──> Product
├── runtime authority
│   ├── active Product and Node selection
│   ├── execution Claims and Fences
│   ├── capacity Leases
│   ├── Operations and Managed Processes
│   └── Agent Sessions
└── Product durable state
    ├── Domain Workflows
    │   └── immutable Workflow Versions
    │       ├── Step Definitions
    │       ├── Transition Definitions
    │       ├── policies and schemas
    │       └── named Attachments
    ├── Projects
    │   ├── domain properties
    │   ├── named Attachments
    │   └── ordered Job membership
    ├── Jobs
    │   ├── Job Revisions
    │   ├── domain properties
    │   ├── named Attachments
    │   └── Job Executions
    │       ├── Run Plan Revisions
    │       ├── Step Activations
    │       │   └── Step Attempts
    │       ├── runtime variables
    │       └── evidence and events
    ├── Artifacts
    ├── Nodes
    └── product policy and configuration
```

## Product

A **Product** is the durable work context Refine operates on. It is identified
operationally by a Git repository path and registered name. A locally known
Product is a Registered Product; the selected one is the Target Product.

The Target Product owns:

- Refine product state on `refine/state`;
- Domain Workflows and Workflow Versions;
- Projects and their Job membership;
- Jobs, Job Revisions, and durable execution evidence;
- product-scoped Artifacts and Attachment bindings;
- Nodes, settings, guidance, governance, quality policy, and other retained
  product configuration;
- product lifecycle capabilities such as inspect, build, test, start, stop, and
  health where the domain supplies them.

The rename does not remove the existing Git boundary. Durable product state
remains outside the primary worktree in the Git-owned live projection and
isolated `refine/state` worktree. Runtime claims, processes, operations, leases,
and caches remain local runtime authority.

## Project

A **Project** is a durable outcome, initiative, or body of work within a Target
Product. It replaces the current Feature entity and groups Jobs without
becoming a second workflow engine.

```json
{
  "id": "PRJ123",
  "name": "Account recovery",
  "description": "Deliver self-service recovery across the product.",
  "reporter": "Buddy",
  "assignee": null,
  "node_id": "default",
  "properties_revision": 3,
  "properties": {
    "release": "2026.09",
    "risk": "medium"
  },
  "created_at": "2026-07-28T...",
  "updated_at": "2026-07-28T..."
}
```

A Project owns display metadata, domain-specific properties, named
Attachments, and ordering policy for its member Jobs. Membership is represented
by `Job.project_id`; `Job.project_order` provides stable ordering when the
Project requires it. A Job belongs to at most one Project in v5. Jobs may also
exist without a Project.

Project rollups are projections from member Jobs. A Project does not directly
claim steps, run a Workflow, or acquire execution authority. Transferring a
Project between Nodes transfers all member Jobs after refusing active fenced
work, using the same reviewable aggregate operation as the current Feature
transfer.

Cancelling a Project is a system-owned cascade over eligible non-terminal Jobs.
It retains the Project, Jobs, Job Revisions, executions, and evidence.

## Domain Workflow

### Workflow

A **Workflow** is the stable identity of a domain-specific process.

```json
{
  "id": "software-delivery",
  "name": "Software Delivery",
  "description": "Analyze, implement, verify, integrate, and review a change.",
  "job_type": "software-change",
  "active_version": 7,
  "created_at": "2026-07-28T...",
  "updated_at": "2026-07-28T..."
}
```

A Workflow is a family. It is not directly executable and does not contain
mutable live state.

### Workflow Version

A **Workflow Version** is an immutable, compiled, executable definition. It
contains or pins:

- workflow schema version;
- compatible Job type and property schema;
- optional Project type and property schema;
- start step;
- Step Definitions;
- Transition Definitions and guards;
- exact action/capability identifiers;
- exact Agent profile and named Attachment references;
- retry, timeout, capacity, intervention, and failure policies;
- completion and cancellation outcomes;
- content digest, author/proposer, validation evidence, and promotion status.

Lifecycle:

```text
draft candidate → validated candidate → approved version → active
                                                    └──→ retired
```

Only an approved Workflow Version may start new Job Executions. “Active” means
the default for new Jobs; more than one approved version may remain executable
for already-pinned Jobs.

Publishing a version resolves every mutable reference. An active execution
never uses “latest prompt,” “current profile,” or an unversioned capability
alias.

### Step Definition

A **Step Definition** is one semantic node in a Workflow Version.

Required fields:

- stable step ID within the Workflow Version;
- name and description;
- step kind;
- action binding or wait behavior;
- input bindings;
- output contract;
- transitions;
- retry, timeout, capacity, and intervention policy;
- named Attachment bindings.

Initial step kinds:

| Kind | Meaning |
|---|---|
| `action` | Invoke a registered capability, including `agent.call` |
| `decision` | Evaluate deterministic guards without external effects |
| `human` | Wait for an authorized human decision or structured input |
| `timer` | Wait until a durable deadline |
| `fan_out` | Create activations over a bounded collection |
| `join` | Wait for a declared set of branch outcomes |
| `subworkflow` | Start a pinned Workflow Version as a child execution |
| `terminal` | Set the Job Execution outcome |

The kernel owns these control-flow meanings. Domain behavior lives in
registered actions and workflow data.

### Transition Definition

A **Transition Definition** connects one step outcome to another step.

```json
{
  "from": "diagnose",
  "outcome": "diagnosis_complete",
  "guard": {
    "op": "gte",
    "left": {
      "path": "attempt.output.confidence_percent"
    },
    "right": {
      "value": 80
    }
  },
  "to": "plan",
  "job_patch": [
    {
      "from": "attempt.output.severity",
      "to": "job.properties.severity"
    }
  ]
}
```

Transition evaluation is deterministic:

1. validate the step output;
2. select transitions matching the declared outcome;
3. evaluate guards in stable priority and transition-ID order;
4. require exactly one matching transition unless the step explicitly fans out;
5. validate any durable Job property patch;
6. atomically settle the attempt and create the next activation(s).

Guards are a bounded expression language over typed data. They cannot execute
shell commands, invoke agents, call the network, or mutate state.

### Agent Call

An agent call is the registered action `agent.call`, not a separate control-flow
system. Its Step Definition pins:

- Agent profile/version;
- prompt Attachment name;
- selected Workflow, Job, Revision, execution, and prior-output context;
- capability grants;
- time, token, and cost budgets where supported;
- output schema Attachment;
- allowed outcomes;
- retry and interruption policy.

The engine assembles the prompt from explicit bindings. Attaching content does
not automatically place it in an agent context.

### Workflow Draft And Analysis

If a Target Product has no active Workflow, a user or agent may request product
analysis. The result is a **Workflow candidate**, never an active workflow.

The analysis operation:

1. inspects the Target Product and existing Refine configuration;
2. proposes a Job type/schema;
3. proposes workflow steps, transitions, and required capabilities;
4. creates prompt/schema/reference Artifacts and Attachment bindings;
5. produces a draft graph and analysis report;
6. validates the candidate;
7. stops for review or explicit policy-controlled publication.

Analysis may improve an existing Workflow by producing a new candidate version.
It never edits an approved version in place.

## Job Aggregate

### Job

A **Job** is the aggregate root and durable identity of one unit of work.

```json
{
  "id": "JOB123",
  "type": "software-change",
  "name": "Add account recovery",
  "priority": "high",
  "reporter": "Buddy",
  "assignee": null,
  "node_id": "default",
  "project_id": "PRJ123",
  "project_order": 20,
  "workflow": {
    "id": "software-delivery",
    "version": 7
  },
  "current_revision_id": "JOB123-R2",
  "active_execution_id": "EXEC456",
  "engine_state": "running",
  "workflow_state": ["implementation"],
  "properties_revision": 12,
  "properties": {
    "repository": "example/service",
    "target_branch": "main",
    "risk": "medium"
  },
  "created_at": "2026-07-28T...",
  "updated_at": "2026-07-28T..."
}
```

Generic Job fields are intentionally small:

- identity, type, and display metadata;
- priority and ownership;
- optional Project membership and stable order;
- Workflow binding;
- current Job Revision and execution references;
- engine/workflow state projections;
- schema-bound domain properties;
- timestamps and optimistic revisions.

There is no `JobNote` entity. Notes are named Attachments.

### Job Revision

A **Job Revision** is an immutable version of the requested work.

It contains:

- ID and monotonically increasing sequence within the Job;
- author/reporter and optional assignee;
- creation time and reason;
- the exact Workflow Version intended for the next execution;
- immutable revision input values;
- named Attachment bindings such as `request`, `change-summary`, and
  `inputs/*`;
- provenance linking it to the prior Job Revision.

A changed request creates a Job Revision. A provider failure, step retry, or
restarted process does not.

The current v4 Goal Round mixes revised intent and execution evidence. v5 splits
that representation:

```text
v4 Goal Round request       → Job Revision
v4 implementation attempt  → Job Execution + Step Attempts
v4 evaluation fields       → Attempt outputs and Evidence Attachments
v4 Round logs              → Job/Execution event and log Attachments
```

### Job Execution

A **Job Execution** runs one Job Revision through one pinned Workflow Version.

It records:

- execution ID;
- Job and Job Revision IDs;
- Workflow ID and exact version/digest;
- current Run Plan Revision;
- engine state and terminal outcome;
- active Workflow state projection;
- Node owner and scheduling metadata;
- creation, start, update, settlement, and interruption times.

A Job may have many historical executions but at most one mutable active
execution unless a Workflow Version explicitly models coordinated child
executions. Starting a new execution never destroys prior execution evidence.

### Run Plan Revision

A **Run Plan Revision** is the effective future workflow graph for one Job
Execution. Revision 1 is materialized from the pinned Workflow Version.

An approved intervention may create a later Run Plan Revision. It may:

- add or replace future steps;
- alter future transitions or bindings;
- skip an unstarted optional step;
- add a recovery branch;
- bind new Attachments.

It may not rewrite completed Step Activations or reinterpret settled evidence.
Each activation pins the plan revision and Step Definition digest under which it
was created.

### Step Activation

A **Step Activation** is one occurrence of a Step Definition becoming runnable.
This is distinct from the definition because loops and fan-out can activate the
same step more than once.

Kernel states:

```text
blocked → ready → claimed → running → waiting → succeeded
                           ├───────────────→ failed
                           ├───────────────→ cancelled
                           └───────────────→ interrupted
```

`waiting` applies to human input, timers, external signals, and suspended agent
interaction. Domain-specific state names do not replace these kernel states.

### Step Attempt

A **Step Attempt** is one attempt to execute an activation.

It pins:

- activation, Job, execution, plan, and Step Definition identities;
- Claim, Node, provider, capability, Operation, Process, and session identities
  where applicable;
- resolved input bindings and Attachment digests;
- output value and Attachment bindings;
- outcome, Evidence, and failure information;
- timestamps and decision version.

Retries append attempts. They never overwrite a failed or interrupted attempt.

## Domain Properties And Data Scopes

Refine v5 uses four explicit data scopes:

| Scope | Mutability | Purpose |
|---|---|---|
| Job properties | Durable and revision-checked | Queryable current domain state |
| Job Revision inputs | Immutable | Requested outcome and input values |
| Job Execution variables | Durable for one execution | Just-in-time values passed across steps |
| Step input/output | Immutable per attempt | Resolved arguments and structured result |

Workflow bindings reference these scopes explicitly:

```text
$job.properties.severity
$revision.inputs.requested_outcome
$execution.variables.suspected_service
$steps.diagnose.latest.output.root_cause
```

An action cannot directly mutate Job properties. It returns a structured output
and optional patch proposal. The transition owns promotion from output or
execution variables into durable Job properties.

Every property patch requires:

- expected Job property revision;
- the Workflow Version and transition authorizing it;
- schema validation;
- actor/attempt provenance;
- atomic application with transition settlement.

The engine rejects undeclared fields unless the Job schema explicitly permits
an extension namespace.

## Artifact And Attachment

### Artifact

An **Artifact** is immutable stored content:

- content ID and digest;
- media type and byte size;
- storage kind and locator;
- creator and creation time;
- optional schema identifier;
- confidentiality/trust metadata;
- integrity and availability status.

Supported storage kinds may include:

- inline JSON or small text;
- Git-backed product-state content;
- managed local/object content addressed by digest;
- Target Product path plus pinned Git commit;
- external URI plus recorded digest where the content can be fetched.

Workflow prompts, schemas, and instructions must use content Refine can pin by
digest. An unversioned external URI is not sufficient for an approved Workflow
Version.

### Attachment

An **Attachment** is a named relationship from a subject to an Artifact:

```text
Subject + Attachment name → immutable Artifact
```

Subjects include:

- Product;
- Project;
- Workflow Version;
- Step Definition;
- Job;
- Job Revision;
- Job Execution;
- Step Activation or Attempt.

The key is unique on `(subject_type, subject_id, name)`.

Names are logical keys, not storage paths or display filenames. Initial reserved
namespaces:

| Subject | Names |
|---|---|
| Product | `instructions`, `references/*`, `policies/*` |
| Project | `request`, `notes/*`, `references/*`, `outputs/*`, `evidence/*` |
| Workflow Version | `instructions`, `job-schema`, `references/*`, `policies/*` |
| Step Definition | `prompt`, `input-schema`, `output-schema`, `examples/*`, `instructions/*` |
| Job | `request`, `notes/*`, `inputs/*`, `references/*`, `outputs/*`, `evidence/*` |
| Job Revision | `request`, `change-summary`, `inputs/*`, `references/*` |
| Step Attempt | `inputs/*`, `outputs/*`, `evidence/*`, `logs/*` |

The name implies how a shared capability looks up content; it does not by itself
grant trust. The Attachment also records provenance and trust classification.
Only authorized actors may bind reserved instruction/schema names.

Replacing mutable subject content creates a new Attachment binding that
supersedes the old binding. History remains addressable. Immutable subjects such
as Workflow Versions, Job Revisions, and settled Step Attempts never replace a
binding in place.

### Context Assembly

Attachment availability is not prompt inclusion. An agent Step Definition
declares exact selectors and ordering:

```text
kernel instructions
→ Workflow instruction Attachments
→ Step `prompt` Attachment
→ structured Job/Revision/execution values
→ selected untrusted Job Attachments
→ output contract
```

Untrusted inputs remain visibly delimited and cannot acquire instruction
authority through their filename or Attachment name.

## Capability Registry

A **Capability Definition** describes a trusted action available to workflows:

- stable capability ID and version;
- input and output schemas;
- required grants;
- side-effect class;
- idempotency contract;
- cancellation and recovery behavior;
- eligible Node/runtime constraints;
- executor implementation reference.

Examples:

```text
agent.call
product.inspect
product.git.commit
product.git.integrate
product.build
product.test
attachment.read
attachment.write
human.request
workflow.invoke
```

Workflow data may configure registered capabilities. It cannot define arbitrary
host code or elevate its own grants.

## Intervention And Improvement

An **Intervention** is durable input from a user or agent associated with a Job,
execution, activation, or attempt. Kinds include:

- observe;
- advise;
- answer;
- retry;
- pause/resume;
- cancel;
- propose Run Amendment;
- propose Workflow Version.

Advice is an Attachment or structured value. It has no authority until a Step
Definition or approved amendment consumes it.

Two improvement paths remain distinct:

```text
Improve one execution:
Intervention → Run Amendment → Run Plan Revision

Improve future Jobs:
Intervention/evidence → Workflow candidate → validation → approval → Workflow Version
```

## State And Authority

A Job exposes three different forms of state:

| State | Authority |
|---|---|
| Engine state | Fixed kernel state machine |
| Workflow state | Active Step Activations under the pinned plan |
| Domain state | Schema-bound Job properties |

For a linear workflow, `workflow_state` usually contains one step ID. For
parallel execution it may contain several. A single display status is a
projection and is never sufficient to reconstruct execution authority.

Claims, Operations, Managed Processes, and Agent Sessions remain separate state
machines. Their observations do not silently overwrite Job state.

## Node, Claim, Lease, And Fence

The existing Refine scale architecture is retained and generalized:

- a Node durably owns a Job;
- a Claim reserves a ready Step Activation, not an entire hard-coded Goal
  lifecycle;
- a capacity Lease admits an action by Node, provider, Target Product,
  capability, and resource class;
- an execution Fence pins Job, Job Revision, Job Execution, Run Plan Revision,
  Step Activation, Step Attempt, Node, and decision version;
- an Operation owns cancellable/recoverable background execution;
- a Managed Process provides OS-level observation;
- an Agent Session is an interactive attachment to an agent Step Attempt.

Consequential effects validate the Fence immediately before acting and again
before settling.

Git synchronization remains the durable distribution mechanism. A Job has one
Node owner for mutation at a time. Claims and leases are runtime authority and
are not treated as Git-based distributed locks.

## Persistence Shape

The local runtime root contains the Product Registry and active selection:

```text
products.json                     # Registered Products and active_product
```

This replaces the v4 local `apps.json` vocabulary and is not synchronized as
Target Product state.

The exact product-state codec may evolve during implementation, but v5 should
converge on this logical layout:

```text
refine.json
nodes.json

workflows/
  <workflow-id>/
    workflow.json                 # stable family metadata
    versions/
      <version>/
        workflow.json             # immutable compiled definition
        layout.json               # non-semantic editor layout

projects/
  <id-prefix>/<id-suffix>/
    project.json                  # aggregate metadata, properties, and rollup

jobs/
  <id-prefix>/<id-suffix>/
    job.json                      # current aggregate and projections
    revisions/
      <revision-id>.json
    executions/
      <execution-id>/
        execution.json
        plan-revisions.jsonl
        activations.jsonl
        attempts.jsonl
        events.jsonl

artifacts/
  <digest-prefix>/<digest>/
    artifact.json
    content                       # when stored in product state

attachments/
  <subject-kind>/<subject-id>.json
```

Large or external Artifact payloads may live outside the state branch, but
their metadata, digest, provenance, and availability remain durable. Workflow
instruction and schema Artifacts should be small, inspectable, and Git-backed.

Current-state files remain authoritative for mutation. JSONL evidence is
append-only. Projection snapshots and browser canvas state remain rebuildable
or explicitly non-semantic.

## Cardinalities

| Source | Relation | Target |
|---|---|---|
| Product | contains `0..*` | Projects, Workflows, and Jobs |
| Project | groups `0..*` | Jobs |
| Job | belongs to `0..1` | Project |
| Workflow | has `1..*` | Workflow Versions |
| Workflow | selects `0..1` active | Workflow Version |
| Workflow Version | contains `1..*` | Step Definitions |
| Step Definition | contains `0..*` | Transition Definitions |
| Workflow Version or Step | has `0..*` | Attachments |
| Job | pins `1` | Workflow Version |
| Job | has `1..*` | Job Revisions |
| Job | has `0..1` active, `0..*` historical | Job Executions |
| Job Execution | pins `1` | Job Revision and Workflow Version |
| Job Execution | has `1..*` | Run Plan Revisions |
| Job Execution | has `0..*` | Step Activations |
| Step Activation | has `0..*` | Step Attempts |
| Any attachable subject | has `0..*` named | Attachments |
| Attachment | refers to `1` | Artifact |
| Ready Step Activation | has `0..1` active | Claim |

## Migration From v4

The v5 migration is semantic and must be agent-assisted where product policy is
ambiguous.

Deterministic migration:

- rename App Registry fields and API vocabulary to Product Registry;
- convert each Feature to a Project while preserving IDs where valid;
- convert Feature membership and ordering to `Job.project_id` and
  `Job.project_order`;
- create the built-in `software-delivery` Workflow candidate corresponding to
  the current hard-coded lifecycle;
- convert each Goal to a Job;
- convert Goal Notes to `notes/<note-id>` Attachments;
- convert each Goal Round request into a Job Revision and `request` Attachment;
- convert Round outcome/evaluation/integration fields into execution,
  attempt, output, and evidence records;
- preserve Goal IDs as Job IDs where valid;
- preserve Node ownership, priority, Project association metadata, branches,
  commits, timestamps, and logs;
- map current status to the corresponding active/terminal step in the built-in
  workflow.

Review-required migration:

- infer Job property schemas from untyped durable fields;
- resolve malformed Feature membership or ordering that cannot be converted to
  a Project deterministically;
- resolve malformed or partially settled Rounds;
- validate prompt and settings conversion into Workflow/Step Attachments.

Migration is one-way at schema v5. It creates a backup and a candidate report
before replacing live state. There is no ongoing dual-write between Goal/Round
and Job/Revision stores.

## Invariants

1. Every executable Job pins an exact approved Workflow Version.
2. Workflow Versions, Job Revisions, Artifacts, and settled Step Attempts are
   immutable.
3. A Job has at most one mutable active execution.
4. A Step Activation pins the plan and Step Definition under which it was
   created.
5. Exactly one deterministic transition settles an action outcome unless the
   step explicitly fans out.
6. Agents return proposals; shared services validate and apply mutations.
7. Durable Job property patches are schema-valid, revision-checked, fenced, and
   provenance-bearing.
8. Attachment names are unique per subject and never imply trust by
   themselves.
9. Prompt/context assembly selects Attachments explicitly.
10. Claims, leases, operations, processes, and sessions cannot silently derive
    or overwrite Job meaning.
11. Canvas layout is never workflow semantic authority.
12. Surfaces are thin adapters over shared capabilities.

## Non-Goals For v5

- RDF/OWL as runtime authority;
- arbitrary code embedded in Workflow definitions;
- Git as a high-frequency lock or message queue;
- transparent mutation of active Workflow Versions;
- automatic publication of agent-drafted workflows without validation and
  configured approval;
- a universal database-backed SaaS control plane;
- implicit inclusion of every Job Attachment in every agent prompt;
- preserving v4 Goal/Round APIs indefinitely.

## Model Acceptance

The model is ready for implementation when:

- the built-in software-delivery workflow can represent the complete v4 Goal
  lifecycle without special status code;
- one non-software workflow can be represented without adding a Rust workflow
  behavior;
- Job Revision, execution, activation, and attempt histories are unambiguous;
- prompt templates, notes, schemas, inputs, and outputs all use Artifact plus
  named Attachment;
- property scopes and mutation authority are explicit;
- intervention can revise future execution without rewriting completed history;
- Node/Claim/Lease/Fence semantics remain independently recoverable;
- CLI, browser, API, and agent surfaces can expose the same shared contracts.
