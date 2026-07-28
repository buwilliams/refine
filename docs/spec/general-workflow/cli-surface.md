# Refine v5 General Workflow CLI Surface

## Status

Design specification for the v5 CLI. It depends on the entities and authority
rules in [`model.md`](model.md) and the implementation sequencing in
[`code.md`](code.md). Exact codecs, compatibility behavior, and checkpoint
gates are defined in [`refactor.md`](refactor.md).

## Decision

The CLI remains Refine's most stable, scriptable, agent-friendly surface. It is
a thin adapter to daemon-routed shared capabilities; it does not parse workflow
semantics or write product-state files directly.

The primary journey becomes:

```text
Select or load Target Product
→ load, select, or design Domain Workflow
→ create and run Jobs
```

Example:

```sh
refine product attach .
refine workflow status
refine workflow analyze --name software-delivery --wait
refine workflow validate WFCANDIDATE1
refine workflow publish WFCANDIDATE1 --activate
refine job create --name "Add account recovery" \
  --request-file request.md \
  --start
```

## Surface Principles

- Every mutation routes through the daemon and one shared service.
- Every command supports machine-usable JSON; JSON is the canonical contract.
- Human output summarizes the same response rather than inventing extra state.
- Long-running actions use the shared Operation capability.
- Definitions are addressed by immutable version; drafts use candidate IDs.
- Commands expose general workflow operations rather than hard-coded domain
  states such as QA, Ready Merge, or Review.
- `refine commands` continues to emit the complete Clap tree.
- `docs/spec/cli-reference.md` remains generated from the binary.
- `refine next` recommends the next valid operation from durable state.

## Nomenclature Changes

| v4 CLI | v5 CLI |
|---|---|
| `refine goal ...` | `refine job ...` |
| `refine goal round ...` | `refine job revise ...` |
| `refine goal note* ...` | `refine job attachment ...` under `notes/*` |
| `refine feature ...` | `refine project ...` |
| `refine project attach/switch/...` | `refine product attach/switch/...` |
| Goal status arguments | Workflow state/action selectors |
| target app wording | Target Product wording |
| `--target-app-*` flags | `--target-product-*` flags |
| Goal Agent profile | Job Agent profile |

The v5 public command catalog does not list `goal`, `feature`, or `target-app`.
Removed commands fail with a short migration message naming the replacement.
Internal wire aliases may be accepted only at the schema/API migration
boundary; new output never emits old terminology.

## Object Selectors

Stable selector syntax:

```text
Workflow family:       software-delivery
Workflow version:      software-delivery@7
Workflow candidate:    WFCANDIDATE1
Product:                refine
Project:                PRJ123
Job:                   JOB123
Job revision:          JOB123@2
Job execution:         EXEC456
Step activation:       ACT789
Step attempt:          ATTEMPT12
Attachment subject:    job:JOB123
                       product:refine
                       project:PRJ123
                       job-revision:JOB123-R2
                       workflow:software-delivery@7
                       step:software-delivery@7:implement
                       attempt:ATTEMPT12
```

Commands accept exact IDs in JSON and human forms. Ambiguous names produce a
conflict with matching IDs; the CLI never guesses.

## `refine product`

The existing attached-work Project command becomes Product. Product commands
select and manage the Git-backed context Refine operates on.

### `refine product status`

Reports:

- active Registered Product and Target Product path;
- schema compatibility and migration state;
- active Node;
- active Workflow and version;
- workflow readiness: `missing`, `draft`, `ready`, or `invalid`;
- Job counts and active executions;
- Target Product lifecycle/health observation where configured.

### `refine product attach <path>`

Registers and selects a local Git Target Product. The response includes
`workflow_readiness` and the exact recommended next command.

Attaching does not automatically publish an agent-generated workflow.

### `refine product switch <name>`

Switches to an existing Registered Product, reconciles product state, and
reports its active Workflow state.

### `refine product register`, `clone`, `remove`, `detach`

Retain current semantics with Target Product vocabulary. Removing a registration
does not delete the product or its Refine state.

### `refine product migrate`

Reports and applies the v4-to-v5 semantic migration. Default mode is inspection:

```sh
refine product migrate
refine product migrate --prepare
refine product migrate --apply <migration-candidate-id>
```

`--prepare` creates a reviewable migration candidate and backup. `--apply`
requires the exact candidate ID and unchanged source digest.

### `refine product sync` and `doctor`

Remain stable. Doctor adds Workflow, Job, Artifact availability, Attachment
integrity, and execution-fence diagnostics.

### Target Product lifecycle

Existing target-app lifecycle capability moves under Product:

```text
refine product inspect
refine product start
refine product stop
refine product build
refine product test
refine product health
```

These commands are available only when the active Domain Workflow or product
configuration binds the corresponding capabilities. A non-software product may
not expose build or start.

## `refine project`

Project is the v5 name for the former Feature aggregate. It groups and orders
Jobs within the active Product.

### `refine project create`

```sh
refine project create \
  --name "Account recovery" \
  --description-file project.md \
  --property release=2026.09
```

Options include `--reporter`, `--assignee`, `--node`, `--properties`,
`--property`, and named `--attachment`. Creation does not create or start a Job
unless an explicit future workflow capability does so.

### Project inspection and mutation

```text
refine project list [--node <id>] [--property <path=value>] [--attention]
refine project show <project-id> [--attachments] [--jobs]
refine project edit <project-id> [metadata/property patch options]
refine project transfer <project-id> --node <node-id>
refine project cancel <project-id>
refine project archive <project-id>
```

`list` includes projected member-Job counts and rollup state. `transfer` is a
fenced aggregate operation over the Project and all member Jobs and refuses
active claimed work.
`cancel` settles eligible member Jobs through shared Job cancellation.
Archiving a Project does not delete its Jobs or Attachments.

### Project membership and ordering

```text
refine project add-job <project-id> <job-id> [--order <integer>]
refine project remove-job <project-id> <job-id>
refine project reorder-job <project-id> <job-id> --before <job-id>
refine project reorder-job <project-id> <job-id> --after <job-id>
```

These commands update the Job's `project_id` and `project_order` through one
shared membership capability. Moving a Job from another Project reports the
source and destination and requires an unchanged Job revision.

### Project Attachments

```text
refine project attachment list <project-id> [--prefix <prefix>]
refine project attachment show <project-id> <name>
refine project attachment put <project-id> <name> --file <path>
refine project attachment remove <project-id> <name>
```

There is no Project Note entity. Notes use the same `notes/*` naming convention
as Job and Workflow content.

## `refine workflow`

### `refine workflow status`

Shows workflow readiness for the active Target Product:

```json
{
  "product": "refine",
  "readiness": "missing",
  "active_workflow": null,
  "candidates": [],
  "next": {
    "command": "refine workflow analyze --wait",
    "reason": "The Target Product has no active Domain Workflow."
  }
}
```

### `refine workflow list`

Lists Workflow families, active versions, approved versions, candidates, Job
counts, and last-updated time.

Flags:

```text
--candidate
--approved
--retired
--json
```

### `refine workflow show <selector>`

Shows one Workflow, version, or candidate:

- metadata and digest;
- Job type/schema summary;
- ordered Step Definitions and transitions;
- capability and Agent bindings;
- named Attachments;
- validation and promotion evidence;
- active/pinned Job counts.

Views:

```text
--graph        machine-readable nodes and edges
--steps        step-oriented human summary
--attachments  named Attachment inventory
--source       canonical definition JSON
```

### `refine workflow analyze`

Starts an agent operation that analyzes the active Target Product and produces a
Workflow candidate.

```sh
refine workflow analyze \
  --name incident-response \
  --instructions-file workflow-intent.md \
  --wait
```

Options:

```text
--name <workflow-name>
--base <workflow@version>
--instructions <text>
--instructions-file <path|->
--agent-profile <profile@version>
--provider <provider>
--wait
--no-wait
```

The operation returns:

- candidate ID;
- source Target Product and Git revision;
- analysis report Attachment;
- proposed Workflow and Job schema;
- prompt/schema/reference Attachments;
- validation result and issues;
- warnings and required human decisions.

It never activates the candidate.

### `refine workflow create`

Imports a declarative candidate without invoking an agent:

```sh
refine workflow create --file workflow.json
refine workflow create --stdin
```

The input becomes a candidate and passes through the same compiler and
validation service as agent output.

### `refine workflow validate <candidate>`

Compiles and validates a candidate against:

- workflow schema;
- graph reachability and terminal paths;
- step and transition IDs;
- guard syntax and type checks;
- resolved capabilities and grants;
- exact Artifact/Attachment availability and digests;
- Agent input/output contracts;
- cycle rejection, bounded fan-out, and join definitions;
- configured approval and intervention policy.

Validation is read-only and repeatable for an unchanged candidate digest.

### `refine workflow diff <candidate> [--against <workflow@version>]`

Shows semantic rather than file-order differences:

- added, removed, and changed steps;
- transition and guard changes;
- capability/grant changes;
- prompt/schema/reference Attachment digest changes;
- Job schema changes;
- migration impact on existing Jobs.

### `refine workflow publish <candidate>`

Publishes an immutable approved Workflow Version after validation:

```sh
refine workflow publish WFCANDIDATE1
refine workflow publish WFCANDIDATE1 --activate
```

The request pins the candidate digest. `--activate` selects the published
version as the default for new Jobs but does not migrate existing Jobs.

### `refine workflow activate <workflow@version>`

Selects the approved default version. It reports Jobs pinned to other versions
and never silently migrates them.

### `refine workflow retire <workflow@version>`

Prevents new Jobs from selecting the version. Existing pinned executions remain
readable and recoverable.

### Workflow Attachments

```text
refine workflow attachment list <workflow@version> [--step <step-id>]
refine workflow attachment show <workflow@version> <name> [--step <step-id>]
refine workflow attachment put <candidate> <name> --file <path> [--step <step-id>]
refine workflow attachment remove <candidate> <name> [--step <step-id>]
```

Approved Workflow Versions are immutable, so `put` and `remove` accept only a
candidate. To change an approved prompt, first derive a candidate:

```sh
refine workflow derive software-delivery@7
refine workflow attachment put WFCANDIDATE2 prompt \
  --step implement \
  --file prompts/implement.md
```

### `refine workflow edit <selector>`

Opens the browser Workflow Editor for a candidate or derives a candidate from an
approved version:

```sh
refine workflow edit WFCANDIDATE1
refine workflow edit software-delivery@7 --derive
```

The CLI prints the local URL even when browser launch is unavailable.

### `refine workflow pause` and `resume`

Retain current global admission-gate semantics. Pause prevents new activations
from being claimed; active attempts continue unless separately stopped.

## `refine job`

### `refine job create`

Creates a Job bound to an exact Workflow Version:

```sh
refine job create \
  --name "Resolve payment incident" \
  --project PRJ123 \
  --workflow incident-response@3 \
  --request-file incident.md \
  --properties properties.json \
  --attachment inputs/logs=service.log \
  --start
```

Options:

```text
--name <text>
--project <project-id>
--type <job-type>
--workflow <workflow@version>
--request <text>
--request-file <path|->
--properties <json-file|->
--property <path=value>
--attachment <name=path>
--reporter <name>
--assignee <name>
--priority <value>
--node <node-id>
--start
```

When `--workflow` is omitted, the active Workflow Version is used only if it is
unambiguous and compatible with the requested Job type.

`--start` is atomic with creation only in intent: if execution startup fails,
the Job remains durably created and the response reports the failed start
operation separately.

### `refine job list`

Filters are general:

```text
--workflow <id[@version]>
--project <project-id>
--engine-state <state>
--workflow-state <step-id>
--type <job-type>
--property <path=value>
--node <node-id>
--reporter <name>
--assignee <name>
--priority <value>
--attention
--limit <n>
```

Domain Workflow state IDs are strings from Workflow Versions; the CLI has no
compiled enum of possible values.

### `refine job show <job-id>`

Returns:

- core Job, optional Project membership, and domain properties;
- current Job Revision;
- Workflow binding and state;
- named Attachments;
- active and historical executions;
- active activations/attempts, claims, processes, and Agent sessions;
- available user actions;
- evidence and recent events.

Views:

```text
--revision <sequence>
--execution <execution-id>
--attachments
--events
--attempts
```

### `refine job edit <job-id>`

Edits generic metadata or submits a schema-valid domain property patch:

```sh
refine job edit JOB123 --priority high
refine job edit JOB123 --project PRJ123
refine job edit JOB123 --properties-patch patch.json \
  --expected-revision 12
```

Workflow-owned fields and active execution state cannot be edited through this
command.

### `refine job revise <job-id>`

Creates an immutable Job Revision:

```sh
refine job revise JOB123 \
  --request-file revised-request.md \
  --reason "Review requested a narrower rollout" \
  --start
```

The service validates whether the active execution must settle, whether the
Workflow Version remains compatible, and whether a new execution may start.

### Job Attachments

```text
refine job attachment list <job-id> [--revision <n>] [--prefix <prefix>]
refine job attachment show <job-id> <name> [--revision <n>]
refine job attachment put <job-id> <name> --file <path>
refine job attachment remove <job-id> <name>
```

Examples:

```sh
refine job attachment put JOB123 notes/decision --file decision.md
refine job attachment put JOB123 inputs/customer-report --file report.pdf
refine job attachment list JOB123 --prefix notes/
```

There are no Job Note commands.

### `refine job start <job-id>`

Starts a Job Execution pinned to the current Job Revision and its exact approved
Workflow Version.

Options allow explicit selection when needed:

```text
--revision <sequence>
--workflow <workflow@version>
--node <node-id>
--wait
--no-wait
```

### `refine job actions <job-id>`

Lists currently permitted domain and kernel actions with input schemas:

```json
{
  "job_id": "JOB123",
  "actions": [
    {
      "id": "approve",
      "kind": "workflow_signal",
      "label": "Approve",
      "input_schema": null
    },
    {
      "id": "request_changes",
      "kind": "workflow_signal",
      "label": "Request changes",
      "input_schema": "artifact:sha256:..."
    }
  ]
}
```

### `refine job act <job-id> <action-id>`

Submits a domain-specific human/manual action:

```sh
refine job act JOB123 approve
refine job act JOB123 request_changes --input-file feedback.json
```

The engine resolves the action from the current Workflow Version. CLI code does
not implement `approve`, `decline`, or other domain meanings.

### `refine job retry <job-id>`

Retries a failed/interrupted activation:

```text
--activation <activation-id>
--attempt <attempt-id>
--from-step <step-id>
--reason <text>
```

The engine reports when the Workflow requires a Job Revision or Run Amendment
instead of a retry.

### `refine job intervene <job-id>`

Adds advice, answers a waiting step, or proposes an amendment:

```sh
refine job intervene JOB123 \
  --activation ACT789 \
  --advice-file guidance.md

refine job intervene JOB123 \
  --amendment-file recovery-branch.json \
  --reason "The external service is unavailable"
```

Applying a Run Amendment is a separate validated action:

```sh
refine job amendment validate JOB123 AMEND1
refine job amendment apply JOB123 AMEND1
```

### `refine job pause`, `resume`, and `cancel`

These are kernel operations:

- pause prevents new activations for one Job Execution;
- resume reopens admission;
- cancel settles active attempts and the execution according to shared process
  control;
- none directly manufactures a domain-specific success outcome.

### Execution Inspection

```text
refine job execution list <job-id>
refine job execution show <execution-id>
refine job activation list <execution-id>
refine job attempt show <attempt-id>
refine job events <job-id> [--follow]
```

`--follow` uses the daemon event stream and reconciles durable state after
reconnection.

## `refine agent`

Agent commands retain their role but adopt Job vocabulary:

```text
refine agent open
refine agent open --profile job <job-id>
refine agent open --profile job <job-id> --activation <activation-id>
refine agent open --profile workflow-designer <candidate-id>
```

The Job profile attaches only to the live workflow-owned Agent session for the
selected attempt. It does not start a second agent when an attachment is
expected.

Workflow designer sessions may propose candidate edits but cannot publish an
approved Workflow Version directly.

## `refine next`

`refine next` becomes the state-aware entry point for the three-stage UX:

```text
No Target Product
→ recommend `refine product attach`

Product with no active Workflow
→ recommend `refine workflow analyze` or `create`

Workflow candidate awaiting review
→ recommend `validate`, `diff`, `edit`, or `publish`

Ready Product with no Jobs
→ recommend `refine job create`

Active/blocked Jobs
→ recommend exact Job actions or inspection commands
```

The recommendation includes machine-readable reasons and does not perform the
action.

## Output And Errors

Every JSON response includes:

```json
{
  "ok": true,
  "kind": "workflow_candidate",
  "product": "refine",
  "data": {},
  "warnings": [],
  "next": []
}
```

Errors use stable categories:

```text
invalid_input
not_found
conflict
stale_revision
workflow_missing
workflow_invalid
schema_mismatch
capability_unavailable
attachment_unavailable
operation_failed
partial_failure
```

Long-running commands return an Operation ID. `--wait` waits through the shared
operation endpoint; it does not implement private polling or process discovery.

## Generated Reference And Tests

Implementation must:

- define the command tree in Clap action types;
- keep dispatch as daemon transport;
- regenerate `docs/spec/cli-reference.md` using the existing xtask;
- update `refine commands` catalog tests;
- test human and JSON output from the same typed response;
- test no-product, no-workflow, candidate, active, stale-version, and migration
  states;
- prove browser/API/CLI parity for workflow publication, Job mutation,
  Attachment mutation, intervention, cancellation, and retry;
- verify old terminology is absent from the public v5 command catalog;
- preserve exact local/daemon error semantics.

## CLI Acceptance

- A fresh user can attach a Target Product, generate/review a Workflow, and run
  a Job without editing state files.
- An agent can discover the complete command tree and operate it using JSON.
- No CLI command embeds a domain workflow state.
- Workflow and Job Attachment operations use the shared Artifact service.
- Domain-specific user decisions flow through `job actions` and `job act`.
- Active definitions and executions are always addressed by immutable version.
- Long-running analysis and execution remain observable and recoverable through
  Operations.
