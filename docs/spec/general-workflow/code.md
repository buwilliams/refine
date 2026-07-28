# Refine v5 General Workflow Implementation Roadmap

## Status

Implementation architecture and ordered roadmap for Refine v5. This document
translates the contracts in:

- [`model.md`](model.md);
- [`cli-surface.md`](cli-surface.md);
- [`browser-surface.md`](browser-surface.md).

It is a design for the `next` branch, not a claim about current `main`.

Current baseline:

- product version `4.1.0`;
- Product state schema version `2`;
- hard-coded `GoalStatus` and `WorkflowBehavior` implementations;
- Goal/Round persistence in `tools/product/work_items`;
- runtime Claim/Lease/Operation/Process infrastructure;
- App Registry and Target App lifecycle services;
- static vanilla-JS browser shell with Idiomorph, SSE, xterm, and hash routing.

## Implementation Goal

Replace the hard-coded software-delivery state machine with a deterministic
interpreter of immutable Workflow Versions while retaining Refine's proven
infrastructure:

- Rust shared capabilities;
- flat inspectable files;
- Git-backed `refine/state`;
- Node ownership and fleet synchronization;
- Claims, capacity Leases, and execution Fences;
- supervised Operations, Processes, and Agent sessions;
- Git worktrees and exact commit evidence;
- daemon-routed CLI/API/browser adapters;
- SSE reconciliation and Idiomorph-preserving UI updates.

The v5 architecture must run the current software-delivery lifecycle as data and
also run at least one non-software workflow without adding a Rust behavior.

## Architectural Boundaries

```text
Surfaces
CLI · HTTP/API · Browser/Desktop · MCP · Agent
                         │
                         ▼
Shared product capabilities
Product · Project · Workflow · Job · Artifact/Attachment · Intervention
                         │
                         ▼
Workflow kernel
Compiler · Scheduler · Interpreter · Guards · Settlement · Fencing
                         │
                         ▼
Registered actions
Agent call · Human signal · Product/Git · Build/Test · Subworkflow
                         │
                         ▼
Host/process substrate
Node · Lease · Operation · Managed Process · PTY · Git · Files
```

Rules:

1. Surfaces never implement workflow meaning.
2. Workflow definitions configure registered actions; they do not embed host
   code.
3. The compiler owns executable-definition validity.
4. The interpreter owns activation and transition semantics.
5. Product services own durable aggregate mutation.
6. The process substrate owns OS observation and cancellation.
7. Git sync owns durable publication, not runtime locking.

## Proposed Source Organization

### Model

```text
src/model/
  product/
    mod.rs                    # ProductRegistry, RegisteredProduct, Target Product status
  project/
    mod.rs                    # Project aggregate, properties, Job rollup
  job/
    mod.rs                    # Job and projections
    revision.rs               # JobRevision
    execution.rs              # JobExecution, activation, attempt
    state.rs                  # fixed kernel execution states
  workflow_definition/
    mod.rs                    # Workflow family and version
    candidate.rs              # candidate/provenance lifecycle
    step.rs                   # StepDefinition and action binding
    transition.rs             # transitions and guard source
    policy.rs                 # retry/timeout/intervention/resource policies
    compiled.rs               # normalized executable representation
  artifact/
    mod.rs                    # Artifact metadata
    attachment.rs             # named subject bindings
  event/
    mod.rs                    # durable Job/Workflow event vocabulary
```

`model::workflow::GoalStatus` is removed after migration. Fixed kernel enums
describe execution machinery, not domain workflow:

```text
WorkflowVersionState
JobEngineState
ActivationState
AttemptState
ClaimState
OperationState
```

### Product Capabilities

```text
src/tools/product/
  product_registry/           # renamed local registry and vocabulary
  product_migration/          # Product state schema 2 → 3 / v4 → v5
  projects/
    mod.rs
    store.rs
    authoring.rs
    membership.rs
    properties.rs
    queries.rs
  workflows/
    mod.rs
    store.rs
    candidates.rs
    compiler_adapter.rs
    publication.rs
    analysis.rs
    commands.rs               # typed candidate edit command batches
    queries.rs
  jobs/
    mod.rs
    store.rs
    authoring.rs
    revisions.rs
    properties.rs
    executions.rs
    actions.rs
    interventions.rs
    queries.rs
  artifacts/
    mod.rs
    store.rs
    attachments.rs
    content.rs
    integrity.rs
  product_state/
    store/
      projects.rs
      jobs.rs
      workflows.rs
      artifacts.rs
    projections/
      jobs.rs
      workflows.rs
```

Each capability has one authoritative service contract. CLI, HTTP, browser,
MCP, migration, and workflow actions delegate to it.

### Workflow Kernel

```text
src/workflow/
  mod.rs
  compiler/
    mod.rs
    normalize.rs
    graph.rs
    guards.rs
    schemas.rs
    capabilities.rs
    diagnostics.rs
  runtime/
    mod.rs
    scheduler.rs
    interpreter.rs
    activation.rs
    attempt.rs
    transition.rs
    settlement.rs
    recovery.rs
  actions/
    mod.rs
    registry.rs
    agent_call.rs
    human.rs
    timer.rs
    subworkflow.rs
    product.rs
  context/
    mod.rs
    bindings.rs
    attachments.rs
    prompt.rs
  fence.rs
  capacity.rs
  state.rs
  state_persistence.rs
```

Current files are reused or split rather than wrapped by caller-specific v5
variants.

### Target Product Host Capability

```text
src/tools/host/target_apps/      → src/tools/host/target_products/
FileTargetAppService             → FileTargetProductService
TargetAppSnapshot                → TargetProductSnapshot
TargetAppOperation               → TargetProductOperation
ProcessOwner::TargetApp          → ProcessOwner::TargetProduct
```

Wire/API input aliases may read old names during migration. Durable v5 output,
logs, activity, CLI help, and browser text use only Target Product vocabulary.

## Core Service Contracts

### Workflow Service

```rust
trait WorkflowService {
    fn list(&self) -> Result<Vec<WorkflowSummary>>;
    fn show(&self, selector: WorkflowSelector) -> Result<WorkflowDetail>;
    fn create_candidate(&self, input: WorkflowCandidateInput)
        -> Result<WorkflowCandidate>;
    fn apply_commands(&self, request: WorkflowCommandBatch)
        -> Result<WorkflowCandidate>;
    fn validate(&self, candidate_id: &str, expected_digest: &str)
        -> Result<WorkflowValidation>;
    fn publish(&self, request: PublishWorkflowRequest)
        -> Result<WorkflowVersion>;
    fn activate(&self, selector: WorkflowVersionSelector)
        -> Result<Workflow>;
}
```

Publication is compare-and-swap on candidate revision and digest.

### Project Service

```rust
trait ProjectService {
    fn create(&self, request: CreateProjectRequest) -> Result<ProjectDetail>;
    fn list(&self, filter: ProjectFilter) -> Result<ProjectPage>;
    fn show(&self, project_id: &str) -> Result<ProjectDetail>;
    fn patch(&self, request: PatchProjectRequest) -> Result<ProjectDetail>;
    fn set_membership(&self, request: SetProjectMembershipRequest)
        -> Result<ProjectMembershipReceipt>;
    fn reorder_job(&self, request: ReorderProjectJobRequest)
        -> Result<ProjectDetail>;
    fn transfer(&self, request: TransferProjectRequest)
        -> Result<ProjectTransferReceipt>;
}
```

Membership is authoritative on `Job.project_id` and `Job.project_order`.
Project detail and progress are durable readback plus a projection over member
Jobs, not an independently mutable workflow status.

### Job Service

```rust
trait JobService {
    fn create(&self, request: CreateJobRequest) -> Result<JobDetail>;
    fn list(&self, filter: JobFilter) -> Result<JobPage>;
    fn show(&self, job_id: &str) -> Result<JobDetail>;
    fn patch(&self, request: PatchJobRequest) -> Result<JobDetail>;
    fn revise(&self, request: ReviseJobRequest) -> Result<JobDetail>;
    fn start(&self, request: StartJobRequest) -> Result<JobExecution>;
    fn available_actions(&self, job_id: &str) -> Result<Vec<AvailableAction>>;
    fn act(&self, request: JobActionRequest) -> Result<JobDetail>;
    fn intervene(&self, request: InterventionRequest) -> Result<Intervention>;
    fn cancel(&self, request: CancelJobRequest) -> Result<CancellationReceipt>;
}
```

### Artifact Service

```rust
trait ArtifactService {
    fn put(&self, request: PutArtifactRequest) -> Result<Artifact>;
    fn open(&self, artifact_id: &str) -> Result<ArtifactContent>;
    fn verify(&self, artifact_id: &str) -> Result<ArtifactIntegrity>;
    fn list_attachments(&self, subject: SubjectRef, prefix: Option<&str>)
        -> Result<Vec<Attachment>>;
    fn bind(&self, request: BindAttachmentRequest) -> Result<Attachment>;
    fn unbind(&self, request: UnbindAttachmentRequest) -> Result<AttachmentHistory>;
}
```

The service validates reserved names, subject mutability, provenance, trust,
storage availability, digest, and supersession.

### Capability Registry

```rust
trait WorkflowActionExecutor: Send + Sync {
    fn definition(&self) -> CapabilityDefinition;
    fn execute(&self, context: ActionContext, input: Value)
        -> Result<ActionOutcome>;
    fn cancel(&self, context: CancellationContext)
        -> Result<CancellationReceipt>;
    fn recover(&self, context: RecoveryContext)
        -> Result<RecoveryOutcome>;
}
```

The registry is assembled at runtime from built-in and explicitly installed
capabilities. Workflow files cannot dynamically load a native library or shell
entrypoint.

## Persistence And Locking

### Product State Schema

v5 renames `CURRENT_PROJECT_SCHEMA_VERSION` to
`CURRENT_PRODUCT_SCHEMA_VERSION` and increments it from `2` to `3`. The Refine
product version and Product state schema version are intentionally independent.

The migration introduces:

```text
workflows/
projects/
jobs/
artifacts/
attachments/
```

and removes new writes to:

```text
goals/
features/                       # migrated deterministically to projects/
```

### Aggregate Locks

Mutation order:

1. acquire Product/workflow coordination lock where cross-aggregate;
2. acquire exact aggregate lock;
3. load authoritative state;
4. verify expected revision/digest/Fence;
5. validate schema and invariants;
6. write replacement files durably;
7. append event/evidence records;
8. release locks;
9. request debounced Git sync;
10. emit SSE notification after durable readback.

No surface reports success before durable readback.

### Canonical JSON And Digests

Workflow Version digests cover semantic executable content:

- normalized Workflow metadata;
- Job schema;
- Step and Transition Definitions;
- guards and policies;
- exact capability versions;
- exact Agent profile versions;
- referenced instruction, prompt, schema, and example Artifact digests.

Excluded:

- tldraw coordinates and camera;
- editor selection/session state;
- display-only color/layout;
- derived validation summaries;
- timestamps not affecting behavior.

Canonical serialization sorts maps and applies one normalized representation
before hashing.

### Artifacts

Artifact IDs are stable generated IDs; `sha256` is the integrity/content
identity. Deduplication may reuse content while retaining distinct provenance.

Storage providers:

1. inline JSON/small text;
2. Git-backed Artifact content;
3. managed local content-addressed payload;
4. Target Product Git object/path pinned to commit;
5. external content pinned by digest.

The Artifact service chooses storage from media type, size, Product policy, and
requested durability. Workflow executable Attachments must be synchronizable
and available on every eligible Node.

## Workflow Compiler

### Input

The compiler accepts only a Workflow candidate with:

- candidate revision and digest;
- source Workflow/base version;
- Job schema;
- Step/Transition graph;
- Attachment bindings;
- requested capability/profile references;
- Product policy context.

### Passes

1. **Normalize** identifiers, maps, defaults, and attachment selectors.
2. **Resolve** exact Artifact, capability, Agent profile, and child Workflow
   versions.
3. **Validate schemas** and step input/output mappings.
4. **Compile guards** into a bounded typed expression AST.
5. **Build graph** and validate start, reachability, terminal paths, cycles,
   fan-out, joins, and subworkflow recursion.
6. **Check effects** against grants, Node eligibility, idempotency, recovery,
   and human-gate policy.
7. **Check data flow** for required values and illegal writes across scopes.
8. **Check migration** impact on Jobs pinned to an earlier schema/version.
9. **Emit diagnostics** with stable codes and entity locations.
10. **Emit compiled representation** and semantic digest.

The compiler is pure after references are resolved. The same inputs produce the
same compiled output and diagnostics.

### Guard Language

The first guard language supports:

- JSON scalar access through declared paths;
- equality/ordering;
- boolean operations;
- null/presence checks;
- bounded collection `any`/`all` where schemas permit;
- deterministic string/number/date predicates.

It excludes:

- arbitrary scripting;
- I/O;
- time except explicit engine-supplied timestamp values;
- randomness;
- model calls;
- mutation.

## Workflow Runtime

### Scheduler

The scheduler replaces fixed Goal-status eligibility with ready Step
Activations.

For each evaluation pass:

1. reconcile orphaned running Claims/Attempts;
2. read workflow admission pause;
3. load ready Activations owned by the active Node;
4. sort by explicit workflow priority, Job priority, creation time, and stable
   ID;
5. enforce Workflow/Node/provider/Product/capability capacity policy;
6. create or recover one Claim per Activation;
7. acquire capacity Lease;
8. create Step Attempt and execution Fence;
9. dispatch to the registered executor;
10. replenish capacity as attempts settle.

The current thread-scoped concurrent executor and capacity code can be retained
behind the generalized activation contract.

### Interpreter

Pseudo-flow:

```text
claim activation
→ pin Fence and attempt
→ resolve step inputs and Attachments
→ invoke action / enter wait state
→ receive ActionOutcome
→ validate output schema
→ persist output Artifacts/Attachments
→ select deterministic transition(s)
→ validate Job and execution-variable patches
→ revalidate Fence
→ atomically settle attempt + activation
→ apply patches
→ create next activation(s) or terminal outcome
→ append evidence/events
→ settle Claim/Lease/Operation/Process
→ durable readback + sync + SSE
```

An action cannot ask the interpreter to jump to an arbitrary step. It emits one
of its declared outcomes.

### Exactly-Once Boundary

The engine promises durable at-most-one settlement for a fenced attempt. It
does not pretend external effects are exactly once.

Every effectful capability declares:

- idempotency-key support;
- probe/recovery behavior;
- whether replay is safe;
- compensation if available;
- evidence needed before settlement.

The attempt ID is the default idempotency key.

### Wait Steps

Human, timer, and external-signal steps persist `waiting` state without holding
a worker thread, Lease, or process.

A signal includes expected activation/decision version. Delivery is idempotent.
After validation, it resumes normal transition settlement.

### Fan-Out And Join

Fan-out materializes a bounded collection before creating branches. Each branch
receives a stable key. Join declares:

- branch set;
- all/any/quorum policy;
- failure and cancellation behavior;
- output aggregation schema.

Dynamic unbounded spawning is not permitted in v5.

### Subworkflows

A subworkflow Step pins an approved Workflow Version and creates a child Job
Execution or child execution context according to its definition. Parent-child
correlation and cancellation are explicit. Recursive cycles require a compiler
depth bound and are disabled initially.

## Agent Call Executor

`agent.call` reuses:

- installed provider adapters;
- exact final environment/argv preflight;
- secure prompt-file fallback;
- Managed Process and PTY session infrastructure;
- prompt redaction and transcript handling;
- cancellation and recovery;
- Job Agent attachment.

Execution:

1. load the pinned Step and prompt Attachment;
2. resolve explicit context selectors;
3. classify trusted instruction versus untrusted input content;
4. render the prompt with a structured envelope;
5. append output schema and allowed outcomes;
6. select provider/profile and assemble exact launch environment;
7. create managed Agent session/process;
8. parse structured completion;
9. validate outcome, Job patch proposal, Artifact outputs, and evidence;
10. return `ActionOutcome` to the interpreter.

Malformed output is an attempt failure or configured repair/retry path. The
executor never heuristically scrapes prose into an authoritative transition.

## Product Analysis And Workflow Drafting

`WorkflowAnalysisService` is a long-running Operation:

1. pin Target Product path, Git revision, Product schema, and base Workflow;
2. create a Workflow Designer agent session;
3. expose bounded Product inspection capabilities;
4. collect proposed Job schema, graph, actions, prompts, schemas, and references;
5. store proposal content as Artifacts and named Attachments;
6. create a Workflow candidate;
7. compile and validate;
8. attach analysis/validation reports;
9. return candidate ID and next actions.

The operation may be resumed or retried. Publication is never part of analysis.

## Job Mutation And Settlement

### Creation

Job creation:

- resolves active or explicit approved Workflow Version;
- validates Job type and properties;
- creates Revision 1;
- stores `request` and supplied Attachments;
- writes Job aggregate;
- optionally requests execution startup as a separately reported result.

### Revision

Creating a Job Revision:

- settles or rejects conflicting active execution according to policy;
- snapshots immutable revision inputs and Attachments;
- validates Workflow compatibility;
- advances `current_revision_id`;
- never deletes prior execution evidence.

### Property Patch

Property patches use a bounded JSON Patch subset:

```text
add
replace
remove
test
```

Paths are restricted to schema-authorized Job properties or explicit execution
variables. A transition may map outputs to paths but cannot patch generic Job
identity, Workflow binding, Node ownership, or engine state.

### Terminal Settlement

Terminal Step settlement records:

- terminal outcome ID from the Workflow Version;
- fixed engine outcome (`succeeded`, `failed`, or `cancelled`);
- final Job property revision;
- terminal evidence Attachments;
- completion timestamp;
- Claim/Lease/Operation/Process settlement receipt.

Domain completion does not have to mean software “Done.”

## Intervention And Run Amendment

An amendment is a candidate patch over the future Run Plan:

1. pin execution, current Run Plan Revision, and completed activation set;
2. apply typed workflow commands to the remaining graph;
3. compile against pinned completed outputs and current capabilities;
4. reject changes that reinterpret settled work;
5. produce semantic diff and validation;
6. require configured approval;
7. atomically append a Run Plan Revision and update future activations.

An agent may propose an amendment. Only the Job service applies it.

Successful amendments may later seed a Workflow candidate, but promotion to a
reusable version remains separate.

## Built-In Software Delivery Workflow

The current fixed behavior becomes an initial built-in Workflow candidate:

```text
Backlog/promotion
→ Todo/worktree preparation
→ Implementation agent
→ Governance
→ pre-merge Quality or Ready Merge
→ integration
→ Build
→ post-build Quality when configured
→ human Review
→ Done
```

Implementation-specific logic moves into registered actions:

```text
product.git.prepare-worktree
agent.call
governance.evaluate
quality.evaluate
product.git.integrate
product.build
product.review-verify
```

The Workflow Version controls ordering and transitions. Action executors retain
their focused Git/Quality/Governance invariants.

Parity tests replay the current workflow matrix through both the legacy
behavior table and compiled candidate during development. Only the interpreter
remains after migration.

## Feature To Project Migration

Feature becomes the v5 Project aggregate. This is a durable domain rename, not
a workflow-kernel state:

- preserve valid Feature IDs as Project IDs;
- preserve name, description/request, reporter, assignee, Node owner, and
  timestamps;
- migrate Feature Attachments or embedded descriptive content to named Project
  Attachments;
- map Goal membership to `Job.project_id`;
- map Feature ordering to `Job.project_order`;
- rebuild Project rollups from member Jobs;
- preserve aggregate transfer and review behavior;
- report malformed membership, duplicate order, or dangling references for
  review.

Projects group Jobs but do not own Step Activations, Claims, or an independently
mutable workflow status. This mapping avoids both a generic dependency graph
and a parent-Job special case.

## Product Registry And Target Product Rename

Rename through all layers:

- Rust public types and fields;
- durable local registry codec;
- API routes/response keys;
- CLI help and arguments;
- browser labels/test IDs where not externally fixed;
- Process owner kind;
- activity/log categories;
- settings and generated instructions;
- intent/spec documentation.

Migration reads `apps.json`/`active_app` and writes the selected v5 registry
shape as local runtime `products.json`/`active_product` atomically. This
registry does not enter Target Product `refine/state`. A compatibility decoder
accepts the v4 fields exactly once; new state is not dual-written.

Path helpers retain existing Git layout behavior even as parameter names change
from `target_root` to `product_root`.

## HTTP, SSE, CLI, MCP, And Browser Adapters

### HTTP Modules

```text
src/surfaces/web_server/
  product_routes.rs
  product_routes/
    registry.rs
    lifecycle.rs
    migration.rs
  project_routes.rs
  project_routes/
    authoring.rs
    membership.rs
  workflow_routes.rs
  workflow_routes/
    candidates.rs
    versions.rs
    analysis.rs
    commands.rs
  job_routes.rs
  job_routes/
    authoring.rs
    executions.rs
    actions.rs
    interventions.rs
  artifact_routes.rs
```

Routes normalize aliases at the transport boundary and delegate immediately.

### SSE

Add typed events:

```text
product_updated
project_updated
workflow_candidate_changed
workflow_validation_changed
workflow_version_published
workflow_activation_changed
job_changed
job_execution_changed
step_activation_changed
step_attempt_changed
attachment_changed
operation_progress
```

Events are hints containing IDs/revisions, not full mutation authority. Browser
reconnect reads authoritative endpoints.

### CLI

Replace action/dispatch modules:

```text
actions/goals.rs       → actions/jobs.rs
dispatch/goals.rs      → dispatch/jobs.rs
actions/features.rs    → actions/projects.rs
dispatch/features.rs   → dispatch/projects.rs
attached Project commands → Product commands
actions/workflow.rs    → expanded Workflow commands
dispatch/workflow.rs   → shared daemon routes
```

Regenerate CLI reference and catalog from Clap after each command change.

### MCP And Agent Tools

MCP mirrors typed Product, Project, Workflow, Job, Artifact, and action
capabilities. It does not expose direct file mutation. Agent-facing discovery
returns schemas, available actions, and exact selectors.

## Browser Implementation

### Vanilla Shell

Retain current files/patterns:

- `static/index.html` shell;
- `common.js` state/API/SSE;
- `router.js` route mapping;
- `dom-morph.js` `renderInto`/`bindOnce`;
- feature-specific JS modules;
- CSS split by feature;
- static DOM and real-browser tests.

Add:

```text
static/js/features/products.js
static/js/features/projects.js
static/js/features/workflows.js
static/js/features/jobs-list.js
static/js/features/jobs-detail.js
static/js/features/jobs-new.js
static/js/features/attachments.js
static/js/workflow-projection.js
static/css/workflows.css
static/css/jobs.css
```

Existing Goal modules migrate to Jobs, Feature modules migrate to Projects, and
attached-context Project modules migrate to Products after v5 parity.

### tldraw Editor Island

Source:

```text
src/surfaces/web/workflow_editor/
  package.json
  package-lock.json
  vite.config.ts
  src/
    index.tsx
    bridge.ts
    editor.tsx
    shapes/
    inspector/
    commands/
    validation/
    accessibility/
```

Generated assets:

```text
src/surfaces/web/static/generated/workflow-editor.js
src/surfaces/web/static/generated/workflow-editor.css
```

The repository pins exact React and tldraw versions. tldraw does not follow
semantic versioning, so upgrades require explicit snapshot/shape migration and
browser verification.

An xtask command:

```text
cargo run --manifest-path xtask/Cargo.toml -- workflow-editor-build
cargo run --manifest-path xtask/Cargo.toml -- workflow-editor-check
```

builds and verifies generated assets. Normal Rust release builds consume the
committed generated bundle so Cargo users are not required to install Node.

The island bridge exports:

```ts
mountWorkflowEditor(host, context): WorkflowEditorHandle
WorkflowEditorHandle.update(snapshot)
WorkflowEditorHandle.focusStep(stepId)
WorkflowEditorHandle.dispose()
```

It owns every descendant of its mount host. Vanilla code owns the host and
route lifecycle.

### tldraw License Gate

Refine being MIT-licensed does not relicense the tldraw SDK. Current tldraw SDK
terms allow inclusion in an open-source codebase, but production use still
requires an applicable trial, commercial, or hobby license key, including for
downstream production use. Before v5 release:

1. choose current tldraw with a documented downstream key obligation, obtain
   explicit suitable terms, or select a permissively licensed editor;
2. pin and preserve the upstream SDK license;
3. define how downstream users supply a public domain-bound key;
4. expose configuration without writing keys to Target Product state;
5. disclose any trial/hobby telemetry behavior;
6. test localhost, remote HTTP, remote HTTPS, desktop, and offline modes;
7. provide a clear unavailable-editor state when production licensing is not
   configured.

Development on `next` may proceed locally without treating that as production
licensing approval. An older permissively licensed tldraw release is not an
automatic escape hatch; it requires a separate maintenance, compatibility, and
security decision.

## Migration Architecture

### Prepare

`product migrate --prepare`:

1. quiesces new workflow admission;
2. reads schema-2 durable state and current Git revision;
3. creates a backup;
4. creates built-in software-delivery Workflow candidate;
5. maps Goals/Rounds/Notes/evidence into candidate v5 records;
6. maps Features to Projects and Goal membership to Jobs;
7. validates every Project, Job, and Attachment;
8. produces counts, warnings, and unresolved decisions;
9. writes no live v5 state.

### Apply

`product migrate --apply <candidate>`:

1. verifies unchanged source digest and no active v4 Claims/Processes;
2. acquires Product-wide migration coordination;
3. writes v5 state into a staging directory;
4. reads it back through v5 services;
5. atomically swaps live state;
6. increments Product state schema;
7. commits/syncs `refine/state`;
8. rebuilds projections;
9. emits migration, Product, and Project events.

Partial replacement is a failure. The backup and migration candidate remain for
recovery.

### No Dual Runtime

There is no mode in which v4 Goal workflow and v5 Job workflow both mutate the
same Product. Before cutover, v5 services may read fixtures or a migration
staging tree. After cutover, v4 mutation routes reject the schema with migration
guidance.

## Implementation Phases

### Phase 0: Specification And Fixtures

- approve these four specs;
- capture representative current Products as migration fixtures;
- encode the current workflow transition/evidence matrix;
- select a non-software demonstration workflow;
- encode the deterministic Feature-to-Project migration;
- record tldraw license decision owner and release gate.

Exit: fixtures and behavioral matrices are committed; no runtime behavior
changes.

### Phase 1: Vocabulary And Shared Types

- introduce v5 model modules;
- rename Target App types behind compatibility decoders;
- define Product, Project, Artifact, Attachment, Workflow, Job, execution,
  activation, and attempt types;
- define typed API contracts and errors;
- add canonical JSON/digest utilities.

Exit: types serialize deterministically and old Product fixtures still load
through migration readers.

### Phase 2: Artifact And Workflow Definition Plane

- implement Artifact/Attachment service;
- implement Workflow candidate store and typed edit commands;
- implement compiler and validation diagnostics;
- implement publication/activation;
- build the declarative software-delivery candidate;
- add CLI/API/MCP definition operations.

Exit: Workflow candidates can be created, edited, validated, published, and
read without running Jobs.

### Phase 3: Project, Job, And Execution Plane

- implement Project aggregate, membership, ordering, transfer, and rollups;
- implement Job/Revision store;
- implement execution/activation/attempt records;
- generalize Claims, capacity, and Fences;
- implement interpreter, guards, waits, settlement, and recovery;
- implement action registry and core actions;
- implement agent call executor.

Exit: a synthetic Workflow can execute deterministic and agent steps with
retries, cancellation, evidence, and recovery.

### Phase 4: Software-Delivery Parity

- register Git, Governance, Quality, Build, and Review capabilities;
- run current lifecycle as compiled Workflow data;
- port cancellation, Ready Merge, approval, prompt transport, and evidence
  tests;
- prove Node/fleet behavior and Git synchronization.

Exit: all current Refine workflow scenarios pass through the interpreter with
no hard-coded state dispatch.

### Phase 5: Migration And Cutover

- implement schema-2-to-3 prepare/apply;
- migrate fixture Products and convert Features to Projects;
- remove v4 writes and behavior dispatch;
- rebuild Project rollups and Product projections from Jobs;
- emit exact migration reports and recovery guidance.

Exit: a copied real Product migrates, runs, syncs, and recovers entirely through
v5 services.

### Phase 6: CLI And Vanilla Browser

- implement v5 command tree and generated reference;
- implement Product → Workflow → Job onboarding and Project grouping;
- replace Goal UI with dynamic Job UI;
- implement Attachments and dynamic Workflow summaries;
- add SSE events and reconciliation;
- update Desktop packaging and Guide.

Exit: users and agents can complete the full journey without direct file edits.

### Phase 7: Workflow Editor

- add pinned React/tldraw island source and build tooling;
- implement semantic shapes and typed command bridge;
- implement inspector, Attachments, validation, diff, and agent repair;
- implement explicit Idiomorph mount/release lifecycle;
- implement license-key configuration and release gate;
- add accessible list editor.

Exit: a candidate can be designed, validated, published, and activated from the
browser without canvas state becoming semantic authority.

### Phase 8: Generality And Scale Proof

- execute one non-software Workflow end-to-end;
- run multi-Node distribution with ready Activations;
- test Product sync conflicts and Project/Job owner transfer;
- test intervention/Run Amendment;
- test Workflow improvement from prior Job evidence;
- benchmark large Job sets and graphs.

Exit: generality and scale are demonstrated rather than inferred from the
software-delivery case.

### Phase 9: v5 Release

- remove transitional public vocabulary;
- update intent, ontology, API, CLI reference, Guide, and migration runbooks;
- complete Rust, integration, browser, migration, sync, install, and release
  gates;
- confirm tldraw licensing and notices;
- publish v5 migration and rollback instructions.

## Verification Matrix

### Model And Persistence

- serde round trips and unknown-field policy;
- canonical digest stability;
- immutable version/revision/attempt enforcement;
- Attachment name uniqueness and reserved-name authorization;
- Artifact digest and availability;
- property schema validation and optimistic revision;
- corrupt/partial record recovery;
- projection rebuild from durable truth.

### Compiler

- unreachable/missing terminal steps;
- ambiguous and no-match transitions;
- guard type errors;
- unbounded cycle/fan-out;
- join mismatch;
- unresolved capability/profile/Artifact;
- unsafe effects and missing human gate;
- layout changes do not alter semantic digest;
- prompt/schema Artifact changes do alter digest.

### Runtime

- linear, branch, loop, wait, timer, fan-out/join, and subworkflow;
- retry versus new Job Revision;
- concurrent Claims and capacity;
- stale Fence rejection;
- interruption and daemon restart;
- idempotent signal and settlement replay;
- cancellation races and process cleanup;
- unavailable Node/provider/capability;
- Git sync and owner handoff.

### Agent

- exact prompt/context ordering;
- untrusted Attachment delimiting;
- output schema and allowed outcome validation;
- malformed output repair/failure;
- secure prompt transport and environment sizing;
- PTY attachment and transcript isolation;
- cancellation/recovery.

### Surfaces

- shared response/error parity across CLI/API/browser/MCP;
- dynamic state/action discovery;
- generated CLI docs;
- no direct surface writes;
- SSE reconnect reconciliation;
- Idiomorph focus and editor lifecycle;
- tldraw semantic/layout separation;
- detached, missing Workflow, draft, invalid, and ready onboarding.

### Migration

- every v4 Goal/Note/Round/evidence field accounted for;
- status mapping;
- branch/commit/integration preservation;
- Feature-to-Project metadata, membership, ordering, and rollup mapping;
- idempotent prepare;
- stale candidate refusal;
- backup and rollback;
- no active v4 execution at cutover;
- post-migration sync from multiple Nodes.

### Performance

Measure:

- compiler latency by graph size;
- scheduler pass by ready activation count;
- Job list/projection rebuild by Job count;
- Artifact lookup and context assembly;
- SSE event/reconciliation pressure;
- editor bundle size and lazy-load time;
- tldraw canvas interaction by step/edge count;
- multi-Node sync conflict rate.

No agent or browser benchmark substitutes for deterministic service timing.

## Removal Plan

After migration parity:

- remove `GoalStatus` and fixed status validators;
- remove `WorkflowBehavior` status dispatch;
- remove Goal/Round mutation services;
- remove Goal-specific CLI/API/browser modules;
- remove fixed workflow visualization constants;
- remove Target App public vocabulary;
- remove Job Note representation;
- remove caller-specific prompt-template selection where Workflow Attachments
  now bind prompts;
- retain reusable Git/Quality/Governance/process implementations as registered
  actions.

Removal happens only after repository-wide reference checks and migration
fixtures prove retained evidence.

## Risks And Mitigations

| Risk | Mitigation |
|---|---|
| Stringly typed workflow data | Compiler, schemas, normalized IDs, typed compiled form |
| Agent-authored unsafe workflow | Candidate boundary, capability grants, validation, approval |
| Mutable active meaning | Immutable Workflow Versions and Run Plan Revisions |
| Global JSON property bag | Explicit schemas, scopes, patches, provenance |
| Prompt injection through Attachments | Trust metadata, explicit selectors, delimited context |
| External effect replay | Idempotency contracts, probes, Fences, evidence |
| Git coordination latency | Node ownership; runtime Claims/Leases; Git for durable sync only |
| Canvas becomes authority | Typed command service; semantic JSON/digest excludes layout |
| React spreads through UI | One lazy island with explicit bridge and build gate |
| tldraw license blocks distribution | Explicit production-license release gate and unavailable state |
| Migration loses current evidence | Candidate report, fixtures, backups, durable readback |
| Overgeneralization delays value | Software parity first, one non-software proof second |

## v5 Completion Criteria

Refine v5 is complete only when:

1. Target App has become Target Product across public surfaces and durable v5
   output.
2. Feature has become Project with membership, ordering, ownership, and rollups
   preserved.
3. Goal/Round have become Job/Job Revision with preserved migration evidence.
4. Prompt templates, notes, schemas, inputs, outputs, and evidence use named
   Attachments over immutable Artifacts.
5. The software-delivery lifecycle is an approved Workflow Version interpreted
   by the general kernel.
6. A non-software Workflow runs without new Rust behavior code.
7. Agents can analyze a Target Product and produce a validated reviewable
   candidate.
8. Users can complete Product → Workflow → Job from CLI and browser and manage
   Project groupings.
9. Workflow editing uses typed semantic commands; tldraw layout is not
   authority.
10. Node, Git sync, Claim, Lease, Fence, Operation, Process, and Agent-session
   recovery remain proven.
11. Migration, documentation, licensing, performance, and release gates pass
    with exact evidence.
