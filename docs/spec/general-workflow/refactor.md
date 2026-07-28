# Refine v5 General Workflow Refactor Contract

## Status

Implementation-ready supplement for the Refine v5 general workflow design on
the `next` branch.

This document closes the deliberately open implementation choices in:

- [`model.md`](model.md);
- [`cli-surface.md`](cli-surface.md);
- [`browser-surface.md`](browser-surface.md);
- [`code.md`](code.md).

It is not a second architecture. The model invariants remain authoritative.
This document selects concrete codecs, compatibility rules, workflow semantics,
fixtures, checkpoints, and verification gates so implementation does not have
to rediscover them.

If implementation evidence requires a different contract, update the affected
specifications together before landing the conflicting behavior.

## Objective

Refactor Refine from its schema-2 fixed software-delivery implementation to the
schema-3 Product, Project, Job, and data-driven Workflow model without losing:

- durable Goal, Round, Feature, Note, Git, Governance, Quality, or log evidence;
- Node ownership and ordered-work behavior;
- Claim, Lease, Operation, Process, cancellation, and recovery truth;
- CLI, HTTP, browser, MCP, and Desktop capability parity;
- Git-backed `refine/state` synchronization;
- review boundaries or exact human approval.

The implementation may proceed autonomously through the checkpoints in this
document. A checkpoint is a verification boundary, not a request for routine
human approval.

## Decisions Closed Here

1. **Product is the attached context.** Current attached Project/App types,
   commands, routes, settings, and host services become Product types.
2. **Project is the former Feature.** Project membership and ordering remain
   stored on Jobs; Project rollups remain derived.
3. **Job is the aggregate root for executable work.** Project does not execute
   a Workflow.
4. **Schema 3 is a one-way cutover.** Schema 2 and schema 3 never write the same
   Product state concurrently.
5. **The v5 Workflow definition codec is version 1.** It supports acyclic
   graphs, deterministic conditional transitions, bounded collection fan-out,
   all-predecessor joins, and pinned subworkflows. Semantic cycles and
   `any`/quorum joins are rejected in definition version 1.
6. **Guards use a JSON abstract syntax tree.** v5 does not introduce a textual
   expression parser.
7. **Workflow semantic JSON uses a repository-defined canonical encoding.**
   Layout, timestamps, and editor state do not enter semantic digests.
8. **The legacy software-delivery behavior becomes a checked-in Workflow
   fixture.** Parity is tested against separate copies of the same input
   fixture; there is no production dual-write or shadow mutation.
9. **Migration preserves unknown legacy content.** Anything not mapped to a
   typed v5 field is retained in a `migration/v4-record.json` Attachment.
10. **tldraw is allowed for local development without a key.** Production
    remains explicitly unavailable without a configured valid key. Licensing
    is a release gate, not an implementation blocker on `next`.

## Baseline To Preserve

The refactor starts from these current code authorities:

| Concern | Current authority |
|---|---|
| Attached context and registry | [`src/model/project/mod.rs`](../../../src/model/project/mod.rs), [`project_registry`](../../../src/tools/product/project_registry/mod.rs) |
| Product schema migration | [`project_migration`](../../../src/tools/product/project_migration/mod.rs) |
| Feature aggregate and rollup | [`src/model/feature/mod.rs`](../../../src/model/feature/mod.rs) |
| Goal, Note, and Round records | [`src/model/goal/mod.rs`](../../../src/model/goal/mod.rs) |
| Allowed legacy operations | [`src/model/workflow/mod.rs`](../../../src/model/workflow/mod.rs) |
| Fixed workflow actions | [`src/workflow/behaviors/mod.rs`](../../../src/workflow/behaviors/mod.rs) |
| Scheduling and ordering | [`src/workflow/automation.rs`](../../../src/workflow/automation.rs), [`promotion.rs`](../../../src/workflow/promotion.rs) |
| Durable work-item mutation | [`work_items`](../../../src/tools/product/work_items/mod.rs) |
| End-to-end behavior | [`tests/full_workflow.rs`](../../../tests/full_workflow.rs), [`tests/multi_instance_sync.rs`](../../../tests/multi_instance_sync.rs) |
| Repository verification | [`xtask/src/main.rs`](../../../xtask/src/main.rs) |

Before deleting a current module, every behavior it owns must have either:

- a v5 shared capability and a parity test;
- an explicit migration-only reader;
- or an intentional removal recorded in the checkpoint evidence.

## Refactor Method

The source refactor is incremental; the durable cutover is atomic.

```text
characterize schema 2 behavior
        ↓
introduce v5 types and services behind shared contracts
        ↓
execute v5 only against schema-3 fixtures/staging roots
        ↓
compare legacy and v5 results on separate fixture copies
        ↓
prepare and validate migration candidate
        ↓
quiesce → atomically install schema 3
        ↓
remove schema-2 mutation paths
```

At no point may one live Product receive both Goal/Feature writes and
Job/Project writes.

## No Hard-Coded Procedure Rule

The refactor must not preserve the fixed workflow by relocating it behind new
names. In schema-3 code:

- the kernel may interpret Step kinds, guards, transitions, retries, joins,
  policies, and Fences;
- a registered capability may perform one bounded mechanism such as calling an
  agent, preparing a worktree, evaluating Quality, integrating Git, or running
  a Product build;
- only Workflow data may order those mechanisms into a domain procedure.

Forbidden implementations include:

- a `software_delivery.run`, `refine_workflow`, or equivalent action that
  performs several procedural stages internally;
- Rust, CLI, API, or browser branches on Workflow IDs such as
  `software-delivery`;
- branches on domain Step IDs such as `qa`, `ready-merge`, `build`, or
  `review` outside migration, compatibility projection, and fixture code;
- capability executors that choose or invoke the next capability;
- surfaces that manufacture transitions from display labels;
- copying `WorkflowBehavior` sequencing into the scheduler, interpreter, Job
  service, or action registry;
- a “temporary” schema-3 special case without a removal checkpoint and a
  failing generality test.

Allowed hard-coded kernel vocabulary is limited to:

```text
action
decision
human
timer
fan_out
join
subworkflow
terminal
```

Allowed fixed engine state describes machinery, not domain meaning:

```text
idle
queued
running
waiting
paused
interrupted
succeeded
failed
cancelled
```

Migration readers may recognize legacy Goal statuses in order to translate
them. Compatibility projections may render legacy labels for schema-2
Products. Neither exception may participate in schema-3 execution.

The built-in software-delivery Workflow must be replaceable with another valid
Workflow Version without recompiling Rust, changing a surface, or registering a
new general-workflow behavior. A non-software fixture exercises the same
compiler, scheduler, interpreter, services, and adapters.

## Nomenclature And Source Moves

The naming collision must be resolved before broad feature work:

| Current source/public concept | v5 source/public concept |
|---|---|
| `model::project` attached context | `model::product` |
| `ProjectConfig` | `ProductConfig` |
| `ProjectStatus` | `ProductStatus` |
| `ProjectSchemaStatus` | `ProductSchemaStatus` |
| `AppRegistry` | `ProductRegistry` |
| `RegisteredApp` | `RegisteredProduct` |
| `project_registry` | `product_registry` |
| `project_migration` | `product_migration` |
| `project_state` | `product_state` |
| `target_apps` | `target_products` |
| `ProcessOwner::TargetApp` | `ProcessOwner::TargetProduct` |
| `model::feature::Feature` | `model::project::Project` |
| `FeatureRollup` | `ProjectRollup` |
| `feature_id`, `feature_order` | `project_id`, `project_order` |
| `model::goal::Goal` | `model::job::Job` |
| `GoalRound` | `JobRevision` plus execution evidence |
| `GoalNote` | `notes/*` Attachment |

Temporary Rust re-exports and transport decoders may keep intermediate commits
compiling. They must be marked `compat_v4`, must not serialize old names, and
must be removed at the cleanup checkpoint.

## Common Codec Rules

All schema-3 durable records follow these rules:

- UTF-8 JSON with two-space pretty formatting and one trailing newline;
- `snake_case` field names and enum values;
- RFC 3339 UTC timestamps;
- object maps serialized in lexicographic key order;
- arrays retain semantic order;
- generated IDs are 26-character uppercase Crockford Base32 values;
- migrated IDs are preserved after uppercase normalization when they match
  `^[A-Z0-9][A-Z0-9_-]{2,63}$`;
- generated integer revisions begin at `1` and increase by exactly one;
- mutable aggregate writes require `expected_revision`;
- unknown top-level schema-3 fields are rejected;
- extensible domain data belongs under `properties`, `variables`, or named
  Attachments;
- schema-2 readers remain tolerant and preserve unknown legacy content during
  migration.

All persisted record types include `schema_version`. Record schema versions are
independent of the Product state schema version:

| Record | Initial schema version |
|---|---:|
| Product registry | 1 |
| Product config | 3 |
| Project | 1 |
| Workflow family | 1 |
| Workflow candidate | 1 |
| Workflow Version | 1 |
| Job | 1 |
| Job Revision | 1 |
| Job Execution | 1 |
| Artifact | 1 |
| Attachment set | 1 |

## Cross-Record Transaction Protocol

Atomic rename is sufficient only for one-file mutations. Reorder, transfer,
Project cancellation, Workflow activation, Job startup, transition settlement,
and migration installation touch multiple records and use a durable
write-ahead transaction:

```text
transactions/pending/<transaction-id>.json
```

The pending record contains:

- transaction ID, kind, actor, Product commit, and creation time;
- sorted lock keys;
- every target relative path;
- expected pre-image digest or expected absence;
- complete post-image bytes and digest;
- idempotent event IDs and event payloads;
- state: `prepared` or `applied`.

Protocol:

1. acquire Product coordination and aggregate locks in lexicographic key order;
2. recover any pending transaction affecting those keys;
3. load and verify every pre-image/revision;
4. compute and validate every post-image;
5. durably write and fsync the `prepared` transaction;
6. replace each target with its post-image using same-filesystem atomic rename;
7. read back and verify every post-image digest;
8. append the declared events idempotently by event ID;
9. durably mark the transaction `applied`;
10. remove the pending record;
11. request Git synchronization;
12. emit SSE only after service-level durable readback.

Every authoritative Product-state read first checks for pending transactions.
Recovery rolls a valid `prepared` transaction forward from its post-images; it
does not guess which partial writes to keep. A pre-image mismatch that is not
already the expected post-image is a hard conflict and preserves the journal
for diagnosis.

Git synchronization never begins while a pending transaction exists. Therefore
another Node receives either the pre-transaction commit or the complete
post-transaction commit, never an intentionally published partial transaction.

## Canonical Product Registry

Local runtime state uses `products.json`:

```json
{
  "schema_version": 1,
  "active_product": "refine",
  "products": {
    "refine": {
      "name": "refine",
      "path": "/home/buddy/projects/refine",
      "added_at": "2026-07-28T12:00:00Z",
      "last_used_at": "2026-07-28T12:00:00Z"
    }
  }
}
```

Product names are unique case-insensitively. Paths are stored as normalized
absolute paths. Selecting a Product changes local runtime authority only; it
does not mutate the Product's `refine/state`.

During migration:

1. read `apps.json` and `active_app`;
2. normalize and validate every path;
3. write `products.json` atomically;
4. read it back through `ProductRegistryService`;
5. retain the old file in the migration backup;
6. never write `apps.json` again.

## Canonical Product Config

The schema-3 `refine.json` shape is:

```json
{
  "schema_version": 3,
  "refine": {
    "version": "5.0.0"
  },
  "created_at": "2026-07-28T12:00:00Z",
  "updated_at": "2026-07-28T12:00:00Z",
  "settings": {},
  "active_workflow": {
    "id": "software-delivery",
    "version": 1
  }
}
```

`active_workflow` is nullable. Product lifecycle commands, provider settings,
Node configuration, Governance, Guidance, and Quality retain their existing
authoritative files unless their owning capability explicitly migrates them.
The general-workflow refactor does not collapse all Product configuration into
`refine.json`.

## Canonical Project Record

`projects/<id[0..2]>/<id[2..]>/project.json`:

```json
{
  "schema_version": 1,
  "id": "PRJ123",
  "revision": 3,
  "type": "software-initiative",
  "name": "Account recovery",
  "description": "Deliver self-service recovery.",
  "reporter": "Buddy",
  "assignee": null,
  "node_id": "default",
  "properties_schema": null,
  "properties": {},
  "archived_at": null,
  "created_at": "2026-07-28T12:00:00Z",
  "updated_at": "2026-07-28T12:00:00Z"
}
```

Project rules:

- Job membership is authoritative on `Job.project_id`;
- ordering is authoritative on `Job.project_order`;
- ordered values are positive integers and unique within a Project;
- a reorder compacts ordered members to `1..n` in one fenced transaction;
- unordered members have `project_order: null` and sort after ordered members;
- Project rollup is rebuilt from Jobs and is never written to `project.json`;
- archive hides the Project from default lists but does not mutate member Jobs;
- cancel is a system-owned cascade that cancels each eligible non-terminal
  member Job and retains every Job and execution record;
- delete is not a v5 public operation; migration preserves any legacy delete
  tombstone it encounters as evidence;
- transfer moves the Project and every member Job to one Node or fails without
  partial ownership changes; any active Claim/Attempt makes the aggregate
  temporarily non-transferable.

Project properties are optional in definition version 1. When
`properties_schema` is set, it contains the exact Artifact ID and SHA-256
digest used for every patch. When it is null, `properties` must be empty. This
prevents an ungoverned global property bag.

Project creation uses the active Workflow Version's `project_schema` by default.
An explicitly supplied schema Artifact must already be trusted and available.
Projects retain their pinned schema digest when the active Workflow changes.

`ProjectRollup` contains:

```text
job_count
succeeded_count
failed_count
cancelled_count
active_count
waiting_count
blocked_count
complete
attention
next_job_id
```

`complete` is true when the Project has at least one Job and every member Job is
`succeeded`, `failed`, or `cancelled`. `attention` is true when any member is
`failed` or `interrupted`. `next_job_id` is the first ordered non-terminal Job
whose predecessors are terminal and non-failed. Rollup computation uses one
coherent Product projection.

## Artifact And Attachment Codec

Artifact metadata:

```json
{
  "schema_version": 1,
  "id": "01K1ARTIFACT00000000000000",
  "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  "media_type": "text/markdown",
  "byte_size": 120,
  "storage": {
    "kind": "git",
    "path": "artifacts/01/01K1ARTIFACT00000000000000/content"
  },
  "schema_id": null,
  "trust": "trusted_project_content",
  "created_by": "Buddy",
  "created_at": "2026-07-28T12:00:00Z"
}
```

One Attachment-set file exists per subject:

```json
{
  "schema_version": 1,
  "subject": {
    "kind": "job",
    "id": "JOB123"
  },
  "revision": 4,
  "bindings": {
    "request": {
      "artifact_id": "01K1ARTIFACT00000000000000",
      "artifact_sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
      "bound_by": "Buddy",
      "bound_at": "2026-07-28T12:00:00Z",
      "supersedes": null
    }
  }
}
```

Allowed `subject.kind` values are:

```text
product
project
workflow_candidate
workflow_version
step
job
job_revision
job_execution
step_activation
step_attempt
```

Attachment mutation locks the subject Attachment set, verifies its revision,
verifies the Artifact digest and availability, and writes the new set
atomically. Immutable subjects reject replacement and removal.

Artifact content is immutable. A changed payload always creates a new Artifact.

## Workflow Definition Version 1

### Family

```json
{
  "schema_version": 1,
  "id": "software-delivery",
  "revision": 2,
  "name": "Software Delivery",
  "description": "Implement and review a software change.",
  "job_type": "software-change",
  "active_version": 1,
  "created_at": "2026-07-28T12:00:00Z",
  "updated_at": "2026-07-28T12:00:00Z"
}
```

### Candidate

A candidate contains mutable authoring state:

```json
{
  "schema_version": 1,
  "id": "01K1WORKFLOWCANDIDATE000000",
  "revision": 7,
  "base": {
    "workflow_id": "software-delivery",
    "version": null
  },
  "definition": {},
  "layout": {},
  "proposer": "Buddy",
  "source_product_commit": "0123456789abcdef0123456789abcdef01234567",
  "created_at": "2026-07-28T12:00:00Z",
  "updated_at": "2026-07-28T12:00:00Z"
}
```

`definition` must decode as the semantic Workflow Version source below.
`layout` is not semantic. Candidate commands require both expected `revision`
and expected semantic digest.

### Version Source

```json
{
  "definition_version": 1,
  "workflow_id": "incident-response",
  "name": "Incident Response",
  "description": "Diagnose, mitigate, verify, and close an incident.",
  "job_type": "incident",
  "project_type": "incident-program",
  "project_schema": null,
  "job_schema": {
    "artifact_id": "01K1JOBSCHEMA000000000000000",
    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "start_step": "diagnose",
  "steps": {},
  "transitions": [],
  "policies": {
    "default_retry_limit": 0,
    "default_timeout_seconds": 3600,
    "failure": "fail_execution",
    "cancellation": "cancel_execution"
  }
}
```

The published version adds:

```text
version
semantic_digest
compiled_at
published_at
published_by
validation_artifact_id
```

Published versions are immutable.

### Semantic Digest

The compiler first resolves defaults and mutable references, then constructs a
semantic object containing:

- definition version, Workflow ID, name, description, Job and Project types;
- exact Job and optional Project schema Artifact IDs and SHA-256 digests;
- normalized Steps, Transitions, guards, policies, action versions, Agent
  profile versions, and Attachment digests;
- pinned subworkflow IDs, versions, and semantic digests.

Normalization:

- Step maps and all object keys sort lexicographically;
- Transitions sort by `from`, numeric `priority`, then `id`;
- set-like lists sort and deduplicate;
- order-bearing lists remain ordered;
- omitted optional fields materialize as their typed default or `null`;
- semantic definitions forbid floating-point JSON numbers.

Canonical bytes are compact UTF-8 JSON with no insignificant whitespace.
Integers use base-10 without leading zeroes. Strings use JSON escaping only for
control characters, quotation marks, and reverse solidus; Unicode is not
otherwise normalized. The semantic digest is lowercase hex SHA-256 over those
bytes.

Candidate metadata, publication metadata, validation summaries, `layout`,
camera, selection, timestamps, and the digest field itself are excluded.

### Step Definition

Steps are a map keyed by stable step ID:

```json
{
  "diagnose": {
    "label": "Diagnose",
    "kind": "action",
    "action": {
      "capability": "agent.call",
      "version": 1
    },
    "attachments": {
      "prompt": "prompt"
    },
    "input_bindings": {},
    "output_schema": {
      "artifact_id": "01K1OUTPUTSCHEMA000000000000",
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    },
    "retry": {
      "limit": 1,
      "backoff_seconds": 0
    },
    "timeout_seconds": 3600,
    "fan_out": null,
    "join": null,
    "display_state": "diagnosis"
  }
}
```

Allowed `kind` values:

| Kind | Meaning |
|---|---|
| `action` | Execute one registered capability |
| `decision` | Evaluate outgoing guards without an external effect |
| `human` | Wait for one allowed structured signal |
| `timer` | Wait until a pinned timestamp or duration |
| `fan_out` | Materialize a bounded collection into keyed branches |
| `join` | Wait for all declared predecessor branches |
| `subworkflow` | Run a pinned Workflow Version as a nested execution context |
| `terminal` | Set the domain and engine outcome |

Rules:

- `action` requires `action`;
- `decision`, `fan_out`, `join`, `subworkflow`, and `terminal` forbid
  `action`;
- `human` declares allowed signal IDs and optional input schemas;
- `fan_out` declares a typed array input, a stable item-key selector, and
  `max_items` in `1..=256`;
- `join` in definition version 1 is always `all`;
- `subworkflow` pins an approved Workflow Version and declares input/output
  bindings;
- `terminal` declares one engine outcome: `succeeded`, `failed`, or
  `cancelled`;
- `display_state` is a projection label and does not affect scheduling;
- a step may not supply a shell command, executable path, environment map, or
  network endpoint.

A fan-out configuration is:

```json
{
  "items": {
    "path": "execution.variables.targets"
  },
  "item_key_pointer": "/id",
  "max_items": 64
}
```

`item_key_pointer` is an RFC 6901 JSON Pointer relative to each item. It must
resolve to a unique string or signed integer. A null pointer uses the materialized
array index as the stable key.

An all-join configuration is:

```json
{
  "mode": "all",
  "predecessor_transition_ids": [
    "fanout-to-process"
  ],
  "aggregate": "object_by_branch_key"
}
```

The aggregate is validated by the join Step's output schema.

A subworkflow configuration is:

```json
{
  "workflow": {
    "id": "diagnostic-check",
    "version": 2,
    "semantic_digest": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "input_bindings": {},
  "output_schema": {
    "artifact_id": "01K1SUBWORKFLOWSCHEMA0000000",
    "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  }
}
```

### Transition Definition

```json
{
  "id": "diagnose-to-mitigate",
  "from": "diagnose",
  "to": "mitigate",
  "outcome": "diagnosed",
  "priority": 100,
  "guard": {
    "op": "eq",
    "left": {
      "path": "attempt.output.severity"
    },
    "right": {
      "value": "high"
    }
  }
}
```

Transition IDs are unique. Priority is an integer; lower values are evaluated
first. For a non-fan-out step, exactly one transition must match the settled
outcome and guard values. Zero or multiple matches fail settlement without
starting another activation.

For a `fan_out` step, the engine materializes the complete input array before
creating work. Each item receives a unique stable branch key and exactly one
matching outgoing transition. The branch key and item are available to input
bindings and guards. A join activation is created once every materialized
branch has produced a successful token.

Definition version 1 rejects:

- graph cycles;
- `any` joins;
- dynamic step creation;
- transitions into the start step;
- joins with an undeclared predecessor;
- fan-out branches that cannot reach a common join or terminal step;
- fan-out without a statically bounded `max_items`;
- unreachable steps;
- non-terminal paths with no outgoing transition.

Retries create Step Attempts under the same Step Activation. They do not
traverse a transition or create graph cycles.

A `subworkflow` Step creates a nested execution context under its parent Step
Activation. The nested context has its own Run Plan, Activations, Attempts, and
Fence suffix, but not a second Job identity. It inherits only explicitly bound
inputs and returns only schema-valid declared outputs. Parent cancellation
cascades to it. Recursive subworkflow references are rejected in definition
version 1.

## Guard AST Version 1

The compiler accepts these operators:

```text
always
all
any
not
exists
eq
ne
lt
lte
gt
gte
in
```

Boolean forms:

```json
{
  "op": "all",
  "args": [
    {
      "op": "exists",
      "value": {
        "path": "job.properties.risk"
      }
    },
    {
      "op": "eq",
      "left": {
        "path": "outcome.id"
      },
      "right": {
        "value": "approved"
      }
    }
  ]
}
```

Operands are exactly one of:

```json
{
  "path": "job.properties.risk"
}
```

```json
{
  "value": "high"
}
```

Readable path roots are:

```text
job.id
job.type
job.priority
job.project_id
job.properties.*
revision.inputs.*
execution.variables.*
attempt.output.*
outcome.id
signal.input.*
fanout.item
fanout.key
```

Semantics:

- `always` is true;
- `all([])` is true;
- `any([])` is false;
- `exists` is true only for a present path, including a present `null`;
- a missing operand makes comparison operators false;
- `eq` and `ne` require identical JSON types;
- ordered comparison supports strings and signed 64-bit integers only;
- `in` requires a scalar left operand and a literal array right operand;
- strings compare by exact UTF-8 value;
- guards cannot access time, environment, files, secrets, Artifacts, or the
  network;
- paths must type-check against the pinned Job, output, or signal schema;
- unknown operators and unknown paths are compile errors.

Floating-point comparison is intentionally unsupported in definition version 1.

## JSON Schema Profile

Job, Project, Step input/output, and human-signal schemas use a strict subset of
JSON Schema Draft 2020-12.

Supported validation keywords:

```text
$schema
$id
$defs
$ref
type
properties
required
additionalProperties
items
minItems
maxItems
enum
const
oneOf
allOf
minimum
maximum
minLength
maxLength
pattern
```

Supported annotations:

```text
title
description
default
examples
```

Rules:

- remote `$ref` is forbidden;
- `$ref` may address only the same Artifact's `$defs`;
- root Job and Project property schemas have type `object`;
- root `additionalProperties` must be `false`;
- unsupported validation keywords are compiler errors;
- annotations never alter validation;
- values are never coerced;
- `default` affects form presentation only and is not written implicitly;
- the exact schema Artifact digest is pinned before validation or execution.

## Action Outcome Contract

Every executor returns:

```json
{
  "outcome_id": "diagnosed",
  "output": {},
  "job_properties_patch": [],
  "execution_variables_patch": [],
  "artifacts": [],
  "evidence_attachment_names": [],
  "message": "Diagnosis completed."
}
```

Patch entries use the RFC 6902 shapes for `add`, `replace`, `remove`, and
`test`, but paths are relative to the owning `properties` or `variables`
object:

```json
{
  "op": "replace",
  "path": "/risk",
  "value": "high"
}
```

Before accepting it, the interpreter:

1. verifies the execution Fence;
2. validates `outcome_id` against the Step Definition;
3. validates `output` against the pinned output schema;
4. validates each bounded JSON Patch operation;
5. stores Artifact content and metadata;
6. binds evidence to the Step Attempt;
7. writes the settled Attempt;
8. evaluates transitions;
9. atomically writes successor Activations and aggregate projections.

An executor never mutates Job or Workflow files directly.

Each registered capability declares an effect class (`none`, `local`,
`product_git`, `process`, or `external`), an idempotency strategy, cancellation
support, recovery support, required grants, and capacity dimensions. The
compiler rejects an effectful capability without either an idempotency key
contract or an observe-before-replay recovery contract.

## Registered Capability Set

The first complete implementation registers:

```text
agent.call@1
human.wait@1
timer.wait@1
product.git.prepare_worktree@1
product.git.commit_candidate@1
product.git.publish_candidate@1
product.git.integrate@1
product.build@1
product.test@1
governance.evaluate@1
quality.evaluate@1
```

Decision, fan-out, join, terminal, and subworkflow control are kernel
operations, not dynamically registered capabilities.

Capabilities may resolve named Product configuration. A Workflow Version may
select a capability and structured input, but it cannot embed an arbitrary
command. Existing configured build/test commands remain behind
`product.build@1`, `product.test@1`, or `quality.evaluate@1`.

## Job And Execution Records

### Job

```json
{
  "schema_version": 1,
  "id": "JOB123",
  "revision": 12,
  "type": "software-change",
  "name": "Add account recovery",
  "priority": "high",
  "reporter": "Buddy",
  "assignee": null,
  "node_id": "default",
  "project_id": "PRJ123",
  "project_order": 1,
  "workflow": {
    "id": "software-delivery",
    "version": 1,
    "semantic_digest": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "current_job_revision_id": "01K1JOBREVISION000000000000",
  "active_execution_id": "01K1EXECUTION0000000000000",
  "engine_state": "running",
  "workflow_state": [
    "implementation"
  ],
  "properties_revision": 3,
  "properties": {},
  "created_at": "2026-07-28T12:00:00Z",
  "updated_at": "2026-07-28T12:00:00Z"
}
```

Allowed engine states:

```text
idle
queued
running
waiting
paused
interrupted
succeeded
failed
cancelled
```

Job priority is exactly `low`, `medium`, or `high` in schema 3. New priority
values require a later Job record schema. Scheduler order is high, medium, low,
then creation time and stable ID after the Workflow/Project constraints.

### Job Revision

```json
{
  "schema_version": 1,
  "id": "01K1JOBREVISION000000000000",
  "job_id": "JOB123",
  "sequence": 2,
  "author": "Buddy",
  "assignee": null,
  "reason": "Narrow the rollout.",
  "workflow": {
    "id": "software-delivery",
    "version": 1,
    "semantic_digest": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "inputs": {},
  "prior_revision_id": "01K1JOBREVISION000000000001",
  "created_at": "2026-07-28T12:00:00Z"
}
```

The request is a named Attachment on the Job Revision, not an inline field.

### Job Execution

```json
{
  "schema_version": 1,
  "id": "01K1EXECUTION0000000000000",
  "job_id": "JOB123",
  "job_revision_id": "01K1JOBREVISION000000000000",
  "workflow": {
    "id": "software-delivery",
    "version": 1,
    "semantic_digest": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  },
  "plan_revision": 1,
  "engine_state": "running",
  "terminal_outcome": null,
  "node_id": "default",
  "created_at": "2026-07-28T12:00:00Z",
  "started_at": "2026-07-28T12:00:01Z",
  "updated_at": "2026-07-28T12:00:01Z",
  "settled_at": null
}
```

Activation states:

```text
ready
claimed
running
waiting
succeeded
failed
cancelled
skipped
```

Attempt states:

```text
running
succeeded
failed
cancelled
interrupted
```

An interrupted Attempt may be recovered only through its registered
capability's recovery contract. Otherwise it settles failed with explicit
recovery evidence and may be retried according to policy.

## Scheduler And Settlement

Each scheduler pass:

1. reads one authoritative Product snapshot;
2. reconciles orphaned Claims, Leases, Operations, Processes, and Attempts;
3. finds ready Activations owned by the active Node;
4. removes Activations blocked by earlier ordered Project Jobs;
5. sorts by Workflow priority, Job priority, Project order, creation time, and
   stable ID;
6. checks global, Node, provider, Product, capability, and resource capacity;
7. creates or recovers a Claim;
8. acquires a capacity Lease;
9. creates an Attempt and Fence;
10. dispatches the registered capability.

Project ordering preserves current Feature semantics:

- only the first non-terminal ordered Job in a Project is eligible;
- failed Jobs block later ordered Jobs;
- cancelled and succeeded Jobs do not block later ordered Jobs;
- unordered Jobs do not participate in Project-order blocking;
- Project and Job Node ownership must agree before claim admission.

All consequential effects validate the Fence immediately before the effect and
again before settlement.

## Built-In Software-Delivery Workflow

The checked-in fixture is:

```text
tests/fixtures/general-workflow/workflows/software-delivery-v1.json
```

It contains these semantic steps:

| Step | Kind/capability | Display state |
|---|---|---|
| `backlog` | `human` signal `promote` or admission policy | `backlog` |
| `prepare` | `product.git.prepare_worktree@1` | `todo` |
| `implement` | `agent.call@1` | `in-progress` |
| `commit` | `product.git.commit_candidate@1` | `in-progress` |
| `governance` | `governance.evaluate@1` | `in-progress` |
| `publish` | `product.git.publish_candidate@1` | `in-progress` |
| `quality-pre` | `quality.evaluate@1` | `qa` |
| `integrate` | `product.git.integrate@1` | `ready-merge` |
| `build` | `product.build@1` | `build` |
| `quality-post` | `quality.evaluate@1` | `qa` |
| `review` | `human` signals `approve`, `request_changes` | `review` |
| `done` | `terminal/succeeded` | `done` |

The Job Revision pins `quality_timing` as `pre_merge` or `post_build`.
The built-in Job schema also defines nullable `branch_name`, `target_branch`,
`base_commit`, and `candidate_commit` string properties. These replace the
schema-2 top-level Goal Git fields. The selected `quality_timing` lives in
immutable Job Revision `inputs`, not mutable Job properties.
Transitions are:

```text
backlog --promote--> prepare
prepare --prepared--> implement
implement --completed--> commit
commit --committed-or-clean--> governance
governance --passed--> publish

publish --published [quality_timing == pre_merge]--> quality-pre
quality-pre --passed--> integrate

publish --published [quality_timing == post_build]--> integrate
integrate --integrated--> build
build --passed-or-skipped [quality_timing == post_build]--> quality-post
build --passed-or-skipped [quality_timing == pre_merge]--> review
quality-post --passed--> review

review --approve--> done
```

`request_changes` is a Job-service operation:

1. settle the waiting review execution as `cancelled` with domain outcome
   `revision_requested`;
2. create a new Job Revision;
3. preserve the prior execution and evidence;
4. optionally start the new revision at `backlog` or `prepare` according to the
   request.

Any capability failure settles the current Attempt and execution as `failed`.
Retry reopens the failed Activation when policy allows it. Cancellation settles
the Attempt, Claim, Lease, Operation, Process, and execution as one retained
transaction.

The built-in Workflow must preserve this parity matrix:

| Scenario | Required result |
|---|---|
| Pre-merge Quality | candidate Quality before integration; Build then Review |
| Post-build Quality | integrate, Build, integrated-target Quality, then Review |
| Build not configured | explicit skipped evidence, never implied success |
| No candidate changes | clean no-op commit evidence and continued workflow |
| Governance failure | failed execution with Governance evidence |
| Quality failure | failed execution at the exact Quality activation |
| Merge conflict | failed integration Attempt; candidate/worktree retained |
| Build failure | failed Build Attempt with Process evidence |
| Agent interruption | interrupted/failed Attempt, recoverable or retryable |
| Cancellation race | fenced effect refusal and complete process settlement |
| Retry | same Revision and Workflow Version; new Attempt |
| Revised request | new Job Revision and new execution |
| Review approval | terminal success without a second Git integration |
| Ordered Project failure | later ordered Jobs remain ineligible |
| Project cancellation | eligible member Jobs cancel; terminal evidence remains |

## Migration Contract

### Fixture Set

Before implementing the migrator, add:

```text
tests/fixtures/general-workflow/
  schema2/
    minimal/
    complete-software-delivery/
    pre-merge-review/
    post-build-review/
    failed-agent/
    failed-quality/
    cancelled/
    ordered-feature/
    unordered-feature/
    malformed-dangling-feature/
    interrupted-automation/
  schema3/
    expected/
  workflows/
    software-delivery-v1.json
    incident-response-v1.json
```

Fixture timestamps and IDs are fixed. Fixtures contain no secrets, absolute
machine paths, or live runtime state.

### Mapping

| Schema 2 | Schema 3 |
|---|---|
| App registry entry | Registered Product |
| Target App | Target Product |
| Feature | Project |
| Feature Goal membership | `Job.project_id` |
| `feature_order` | `Job.project_order` |
| Goal | Job |
| Goal Round request | Job Revision plus `request` Attachment |
| Goal Note | Job `notes/<note-id>` Attachment |
| implementation report | Attempt `outputs/implementation-report` Attachment |
| Governance fields | Governance Attempt output/evidence |
| Quality fields | Quality Attempt output/evidence |
| integration fields | Integration Attempt output/evidence |
| Round log entry | execution event with original timestamp/category/details |
| branch/base/candidate commit | built-in Job properties and Git evidence |
| unknown legacy field | `migration/v4-record.json` Attachment |

Every Goal produces at least one Job Revision. A Goal with no Round receives a
synthetic Revision 1 with:

```json
{
  "reason": "schema-2 migration without a Goal Round",
  "inputs": {
    "migration_missing_request": true
  }
}
```

Status mapping for the latest Round:

| Goal status | Job/execution result |
|---|---|
| `backlog` | Job `idle`, Workflow projection `backlog`, no active execution |
| `todo` | execution `queued`, ready `prepare` Activation |
| `in-progress` | execution `interrupted`, failed/interrupted current Activation at `implement`, `commit`, `governance`, or `publish` inferred from evidence |
| `qa` | execution `interrupted`, current Activation `quality-pre` or `quality-post` from pinned timing/integration evidence |
| `ready-merge` | execution `interrupted`, current Activation `integrate` |
| `build` | execution `interrupted`, current Activation `build` |
| `review` | execution `waiting`, waiting `review` Activation |
| `done` | execution `succeeded`, terminal `done` Activation |
| `failed` | execution `failed`, last stage inferred from evidence or a synthetic migration failure Attempt |
| `cancelled` | execution `cancelled`, cancellation evidence preserved |

Earlier Rounds become earlier Job Revisions and historical executions. The
migrator never fabricates successful evidence. When a stage cannot be inferred,
it creates a migration evidence Attachment and marks the execution
`interrupted` or `failed` for explicit recovery.

### Prepare

`refine product migrate --prepare`:

1. acquires Product-wide migration coordination;
2. blocks new schema-2 workflow admission;
3. snapshots the Product commit and schema-2 source digests;
4. records active Claims, Operations, and Processes;
5. writes a timestamped backup;
6. writes schema-3 state to an isolated staging root;
7. reads every staged record through v5 services;
8. compiles the built-in Workflow;
9. rebuilds all projections;
10. compares source and destination entity/evidence counts;
11. writes a candidate report with warnings and blockers;
12. leaves live state unchanged.

Prepare is idempotent for the same source digest.

### Apply

Apply requires:

- unchanged Product commit and source digest;
- zero active schema-2 Claims;
- zero active workflow-owned Processes;
- no unresolved migration blockers;
- successful staged durable readback;
- successful schema-3 projection rebuild.

It then atomically swaps staged state, writes Product schema version `3`,
commits/synchronizes `refine/state`, and reads the installed state through the
daemon. Failure retains the schema-2 live tree, backup, staging tree, and report.

There is no automatic rollback after other Nodes have accepted schema 3.
Rollback instructions restore the backup only while the migration commit has
not been distributed.

## Compatibility Window

During implementation:

- schema-2 Products continue using legacy services;
- schema-3 fixtures and migrated Products use v5 services;
- one Product selects exactly one service generation from its schema version;
- shared surfaces may contain temporary schema-aware routing;
- schema-2 and schema-3 records are never mixed in one state tree.

At the public cutover checkpoint:

- `refine product` replaces attached-context `refine project`;
- `refine project` replaces `refine feature`;
- `refine job` replaces `refine goal`;
- old commands return migration guidance;
- old request fields decode only inside schema-2 services and the Product
  migration reader, never on schema-3 routes;
- all successful responses emit Product, Project, Job, and Job Revision terms.

The final cleanup removes legacy mutation code only after migration fixtures,
parity tests, and a copied real Product pass.

## Workflow Editor Development Contract

Local development may use tldraw without a license key under tldraw's
development behavior.

Rules:

- `REFINE_WORKFLOW_EDITOR=tldraw` enables the local editor island;
- `TLDRAW_LICENSE_KEY` is read from runtime/release environment only;
- no key is written to Product state, logs, Operations, or browser persistence;
- localhost/development without a key displays
  `Development-only tldraw editor`;
- a production-origin request without a configured key renders the accessible
  Step-list editor and an explicit unavailable-canvas message;
- the semantic Step-list editor remains complete enough to author, validate,
  publish, and activate a Workflow without tldraw;
- release documentation must resolve the production licensing gate before v5
  is declared release-ready.

This lets the editor implementation proceed without silently treating local
development as production authorization.

## Implementation Checkpoints

### R0: Characterization

Deliver:

- schema-2 fixture set;
- machine-readable legacy operation/transition matrix;
- captured full-workflow pre-merge and post-build evidence;
- current CLI/API/browser contract snapshots;
- baseline test results with unrelated failures identified.

Gate:

```sh
cargo test --lib --bins -- --test-threads=1
cargo test --test full_workflow -- --ignored --test-threads=1
```

### R1: Vocabulary And Codecs

Deliver:

- Product/Project/Job module moves;
- schema-3 structs and strict codecs;
- temporary `compat_v4` re-exports;
- canonical JSON and digest helpers;
- registry rename and readback tests.

Gate:

- schema-2 fixtures still decode;
- schema-3 round trips are byte-stable;
- no schema-3 serializer emits App, Feature, Goal, or Round fields.

### R2: Artifact And Definition Plane

Deliver:

- Artifact and Attachment services;
- Workflow family/candidate/version stores;
- JSON guard AST;
- compiler diagnostics;
- publication and activation;
- checked-in built-in and non-software Workflow fixtures.

Gate:

- semantic digest golden tests;
- graph/guard/schema rejection matrix;
- immutable publication tests;
- candidate revision/digest conflict tests.
- no compiler or store branch recognizes the built-in Workflow ID.

### R3: Project, Job, And Runtime Plane

Deliver:

- Project service and rollups;
- Job/Revision/Execution stores;
- scheduler, Activations, Attempts, joins, settlement, and recovery;
- capability registry and core deterministic actions;
- generalized Claims, Leases, and Fences.

Gate:

- synthetic workflows cover action, decision, human wait, timer, fan-out,
  all-join, retry, failure, cancellation, and recovery;
- Project ordering and ownership tests pass;
- durable readback precedes success responses.
- capability executors cannot settle transitions or dispatch another
  capability.

### R4: Software-Delivery Parity

Deliver:

- registered Git, Agent, Governance, Quality, integration, Build, and Review
  capabilities;
- built-in Workflow execution;
- parity harness using separate schema-2 and schema-3 fixture copies.

Gate:

- every row in the built-in parity matrix passes;
- resulting Git commits, Quality timing, evidence, and human Review behavior
  match;
- no `GoalStatus` dispatch is used by schema-3 execution.
- software-delivery ordering exists only in its Workflow fixture.

### R5: Migration And Cutover

Deliver:

- prepare/apply migration;
- reports, backups, stale-candidate refusal, and recovery instructions;
- schema-aware service selection;
- projection rebuild.

Gate:

- every migration fixture matches its expected schema-3 tree;
- repeated prepare is idempotent;
- apply refuses active runtime work;
- a copied real Product migrates and synchronizes across Nodes.

### R6: Surfaces

Deliver:

- Product, Project, Workflow, Job, and Attachment CLI/API/MCP adapters;
- generated CLI reference;
- Product → Workflow → Job browser onboarding;
- Project and dynamic Job surfaces;
- SSE event rename and authoritative reconnect reads.

Gate:

- shared response/error parity;
- no direct surface persistence;
- no old public vocabulary in v5 success output;
- browser no-Product, no-Workflow, candidate, ready, and failure states pass.

### R7: Workflow Editor

Deliver:

- accessible semantic editor;
- local tldraw island and typed bridge;
- candidate diff, validation, Attachment editing, and agent repair;
- explicit mount, update, unmount, and Idiomorph release lifecycle.

Gate:

- all canvas operations have non-canvas equivalents;
- layout-only changes do not change semantic digest;
- production without a key has an explicit usable fallback;
- browser absence is reported as skipped, not passed.

### R8: Cleanup And Generality

Deliver:

- removal of compatibility re-exports and schema-2 mutation routes;
- incident-response demonstration Workflow;
- performance and multi-Node evidence;
- updated ontology, intent, Guide, runbooks, and release documentation.

Gate:

- repository-wide old-symbol audit;
- one non-software Workflow completes without new Rust behavior;
- release gates pass except the explicitly deferred tldraw production license
  decision.

## Required Test Assets

Add focused suites:

```text
src/model/general_workflow/tests/
src/tools/product/product_migration/tests/
src/tools/product/projects/tests/
src/tools/product/jobs/tests/
src/tools/product/workflows/tests/
src/tools/product/artifacts/tests/
src/workflow/compiler/tests/
src/workflow/runtime/tests/
tests/general_workflow_parity.rs
tests/general_workflow_migration.rs
tests/general_workflow_api.rs
tests/general_workflow_cli.rs
tests/web_general_workflow.test.js
tests/web_workflow_editor.test.js
```

Tests must assert durable files and service readback, not only response status.
Failure-path tests retain staging state, candidates, worktrees, Attempts, and
evidence for diagnosis.

Add a procedure-independence test that:

1. registers instrumented fake capabilities;
2. executes the software-delivery fixture and records capability order;
3. edits only Workflow data to reorder or omit eligible fake steps;
4. proves runtime order follows the new definition without a rebuild;
5. executes `incident-response-v1.json` through the same services;
6. fails if schema-3 runtime or surface modules contain a built-in Workflow ID
   or legacy domain-status dispatch.

The source audit permits old status tokens only in schema-2 compatibility,
migration, fixtures, tests, and user-facing migration messages.

## Verification Commands

Focused checks run at each checkpoint. Before a checkpoint is considered
complete:

```sh
cargo fmt --all -- --check
cargo test --lib --bins -- --test-threads=1
cargo test --doc
cargo run --manifest-path xtask/Cargo.toml -- check-static-assets
cargo run --manifest-path xtask/Cargo.toml -- cli-reference
git diff --check
```

Before migration cutover and final cleanup:

```sh
cargo run --manifest-path xtask/Cargo.toml -- test-all
cargo run --manifest-path xtask/Cargo.toml -- test-full-workflow
cargo run --manifest-path xtask/Cargo.toml -- test-multi-instance-sync
```

Browser checks require real Playwright/Chromium execution. Missing browser
tooling is a reported skip and does not satisfy the browser gate.

## Performance Gates

Performance measurements exclude provider, Git, build, test, and network time.
They run on the same host, release build, warm filesystem cache, fixed fixtures,
and at least 20 measured iterations after 5 warm-up iterations.

Required budgets:

| Operation | Fixture | Budget |
|---|---|---:|
| Compile and digest | 500 Steps / 1,500 Transitions | p95 below 500 ms |
| Scheduler evaluation | 10,000 Jobs / 2,000 ready Activations | p95 below 250 ms |
| Job list projection | 10,000 Jobs | p95 below 250 ms |
| Project rollup rebuild | 1,000 Projects / 10,000 Jobs | p95 below 250 ms |
| Guard evaluation | 100,000 typed guard evaluations | p95 batch below 100 ms |
| In-process transition settlement | no external effect | p95 below 10 ms |

The non-editor browser route must not request the React/tldraw JavaScript or CSS
bundles. The editor bundle size and load time are recorded but do not block
local development; production budgets are fixed with the licensing/release
decision.

Any missed budget blocks the associated checkpoint until profiling evidence
identifies and fixes the bottleneck or the specification is deliberately
revised.

Each checkpoint report records:

- exact commands;
- pass, fail, or skip;
- baseline versus introduced failures;
- durable fixture/result paths;
- local commit;
- source and migration digests where applicable.

## Refactor Invariants

1. A surface never writes Product state directly.
2. An agent response is a proposal, never mutation authority.
3. Every executable Job pins an approved immutable Workflow Version.
4. Every external effect is capability-registered, fenced, observable, and
   recoverable or explicitly non-recoverable.
5. Every Job property patch is schema-valid and revision-checked.
6. Project membership has one authority: `Job.project_id`.
7. Project rollup is derived.
8. Attachment availability never implies prompt inclusion or trust.
9. Schema-2 and schema-3 mutation never target the same Product.
10. Migration never discards an unknown legacy field.
11. tldraw layout and browser state never enter Workflow semantic authority.
12. Human Review remains human-owned.
13. Domain procedure exists only in Workflow data; Rust contains mechanisms,
    validation, and deterministic interpretation.

## Implementation-Ready Definition

The refactor is sufficiently specified to begin when:

- the four parent specifications and this document are internally consistent;
- the baseline code authorities above still exist or their replacements are
  identified;
- schema-2 characterization fixtures are created before destructive cleanup;
- tldraw is treated as local-development-only until production configuration is
  supplied.

No additional architecture decision is required before R0. Routine field-level
adjustments discovered from code should preserve these contracts and be
recorded in checkpoint evidence. Stop for user direction only when evidence
requires changing a model invariant, discarding durable data, weakening a human
gate, or expanding production licensing obligations.
