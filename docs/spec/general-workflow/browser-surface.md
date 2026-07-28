# Refine v5 General Workflow Browser Surface

## Status

Design specification for the v5 browser and desktop experience. It depends on
[`model.md`](model.md) and is implemented according to [`code.md`](code.md).
The CLI equivalent is [`cli-surface.md`](cli-surface.md); concrete refactor and
local-editor gates are in [`refactor.md`](refactor.md).

## Decision

Refine v5 retains the current browser architecture:

- static application shell served by the daemon;
- vanilla JavaScript for the primary product surface;
- hash routing;
- `#main`, banners, toolbar dock, and Guide panel;
- authoritative HTTP reads for initial/reconnect reconciliation;
- SSE as the only live notification transport;
- Idiomorph `renderInto` refreshes that preserve focus, selection, scroll, and
  bound DOM identity;
- desktop as packaging around the same browser surface.

The Workflow Editor is a bounded exception. It uses tldraw in a lazy-loaded
React island mounted only on Workflow candidate/editor routes. The rest of
Refine does not migrate to React.

tldraw currently provides a React component, custom shapes, an editor API, and
document/session snapshots. Refine uses those capabilities through an isolated
adapter; it does not adopt the tldraw document as Workflow authority. See the
[official installation](https://tldraw.dev/installation),
[shape](https://tldraw.dev/docs/shapes), and
[persistence](https://tldraw.dev/docs/persistence) contracts.

## Product Journey

The top-level journey becomes:

```text
1. Select / Load Target Product
              ↓
2. Load / Design Domain Workflow
              ↓
3. Create / Run / Supervise Jobs
```

Refine must always make the current stage obvious.

### Stage 1: Target Product

When no Target Product is selected, the main screen shows:

- registered products;
- attach local product;
- clone and register;
- recent product history;
- product schema/migration notices;
- daemon, Node, and system controls that remain valid without a product.

The copy says “Target Product,” never “app” or “application” when referring to
the attached work context.

### Stage 2: Domain Workflow

After Product selection, Refine reads Workflow readiness:

| Readiness | Primary action |
|---|---|
| `missing` | Analyze Product or create/import Workflow |
| `draft` | Continue editing/reviewing candidate |
| `invalid` | Open validation issues |
| `ready` | Continue to Jobs |

For a missing Workflow, the primary experience is:

```text
No Domain Workflow

Refine needs a workflow that describes how Jobs move through this product.

[Analyze Product with Agent]  [Import Workflow]  [Start Blank]
```

“Analyze Product” starts an observable Operation. The browser shows the attached
Job/Agent session, analysis progress, discovered product capabilities, proposed
Job schema, and candidate Workflow graph. Agent output remains a candidate.

### Stage 3: Jobs

When an active Workflow Version exists, Dashboard and Jobs become the primary
work surfaces. Creating a Job binds it to an exact approved version.

## Application Shell

The existing shell remains:

```text
Topbar
├── brand + active Node
├── primary navigation
├── Target Product / Workflow context
├── Reporter / actor context
├── agent and process status
└── primary create action

Banners
Main route surface
Toolbar dock
Guide panel
```

The topbar context control shows two independently meaningful statuses:

```text
Target Product: refine
Workflow: software-delivery @ 7
```

If the Product has no Workflow, the second status is a visible warning/action:

```text
Workflow: Not configured
```

The target-product lifecycle indicator remains available when the product
defines lifecycle capabilities.

## Primary Navigation

The v5 primary navigation is:

```text
Dashboard
Projects
Jobs
Workflow
Changes
Logs
```

Changes is shown only where the Target Product or Workflow exposes meaningful
version/change evidence. Logs remain the cross-domain operational record.

The primary create action becomes:

```text
+ New Job
```

Its menu includes:

- New Job;
- New Project;
- import Jobs;
- analyze/improve Workflow;
- attach/register Target Product;
- domain-specific creation commands exposed by the active Workflow.

Project is the renamed Feature aggregate and is a first-class navigation
concept. It groups and orders Jobs but does not replace the Job as the unit that
moves through a Workflow.

## Routes

Proposed hash routes:

```text
#/                              Dashboard
#/products                      Target Product selector
#/product                       Active Target Product overview
#/product/<tab>                 Product policy/settings

#/projects                      Project list
#/projects/new                  New Project
#/projects/<project-id>         Project detail

#/workflows                     Workflow families and versions
#/workflows/new                 Start blank/import/analyze
#/workflows/<workflow-id>       Workflow family detail
#/workflows/<id>/versions/<n>   Approved read-only version
#/workflow-candidates/<id>      Candidate review
#/workflow-candidates/<id>/edit Workflow Editor

#/jobs                          Job list
#/jobs/new                      New Job
#/jobs/<job-id>                 Job detail modal
#/jobs/<job-id>/executions/<id> Execution inspection

#/changes                       Domain/Git changes
#/logs                          Activity and execution logs
#/node/<tab>                    Node/runtime management
```

Cold deep links first resolve active Target Product and schema state. A route
for a different registered Product never silently switches context.

## Dashboard

Dashboard is stage-aware.

### No Target Product

Show Product selection and system health, not empty Job cards.

### Target Product Without Workflow

Show:

- Product identity and Git revision;
- Product analysis readiness;
- any Workflow candidates and operations;
- primary “Analyze Product” action;
- relevant Guide content;
- Product health and diagnostics.

### Ready Workflow

Show:

- active Workflow and version;
- dynamic workflow-state summary;
- active/waiting/failed Jobs;
- pending human actions;
- active Agent/Node capacity;
- recent Attachments/evidence and changes;
- workflow improvement suggestions;
- recent operations and failures.

The workflow summary is generated from the active Workflow Version and Job
projection. No browser constant lists domain states.

## Projects Surface

The current Features surface becomes Projects. A Project is the durable
initiative that groups Jobs within the active Product.

### Project List

The list shows Project identity, ownership, Node, configured properties, member
Job counts, projected progress/attention, and last update. Filters cover Node,
reporter, assignee, property values, attention, and archived state.

Project rollups are computed from member Jobs; the UI does not persist a second
workflow status on the Project.

### New And Edit Project

The form supports generic metadata, schema-bound Project properties, named
Attachments, ownership, and Node assignment. There is no Project Note model;
`notes/*` Attachments render through the shared Attachment component.

### Project Detail

Project detail contains:

```text
Header
├── Project identity, ownership, and projected rollup
├── edit, transfer, cancel, and archive actions
└── attention indicators

Overview
├── description/request Attachment
├── domain properties
└── Attachments

Jobs
├── ordered member Jobs
├── add, remove, and reorder controls
└── create Job in Project
```

Membership operations update `Job.project_id` and `Job.project_order` through
the shared service. Drag-and-drop reordering has keyboard and explicit
before/after alternatives.

## Jobs Surface

### Job List

The current Goals list becomes Jobs and retains its useful patterns:

- URL-backed filters;
- sortable/paginated dense table;
- attention and active-state indicators;
- bulk selection with explicit scope;
- preserved input focus during SSE refresh;
- detail modal over the underlying list/dashboard context.

Columns are composed from:

- generic Job fields;
- optional Project membership and ordering;
- active Workflow state;
- configured display properties from the Job schema;
- active execution/agent status;
- attention and evidence indicators.

Filters:

- Workflow and version;
- Project;
- Job type;
- engine state;
- Workflow step/state;
- Node;
- Reporter and assignee;
- priority;
- schema-declared indexed properties;
- attention;
- attachment prefix/presence where useful.

Bulk actions come from shared kernel operations plus Workflow-declared bulk-safe
actions. The UI does not infer safety from labels.

### New Job

The form is generated from:

- generic Job fields;
- active Workflow Version;
- Job property/input schema;
- required named Attachments;
- Workflow-provided instructions and examples.

The form supports:

- text/Markdown request;
- file and multimodal Attachments;
- structured domain properties;
- Reporter/assignee/Node;
- optional Project;
- create only or create and start;
- validation without persistence.

### Job Detail

Job detail remains a modal over the current route when practical. It contains:

```text
Header
├── Job identity and Workflow binding
├── engine state and active Workflow state
├── permitted actions
└── active agent/process indicators

Overview
├── domain properties
├── current request
├── Attachments
└── ownership and metadata

Revision timeline
├── immutable Job Revisions
└── change reason and request Attachments

Execution
├── active graph position
├── Step Activations and Attempts
├── inputs, outputs, evidence, and logs
└── Claim/Lease/Operation/Process correlation

Intervention
├── answer waiting step
├── add advice Attachment
├── retry/pause/resume/cancel
└── propose Run Amendment
```

The browser renders available domain actions from the API. It does not contain
hard-coded Approve, QA, Merge, or Review transitions. A software-delivery
Workflow may expose those actions.

### Attachments

Every attachable subject uses one shared component:

- exact logical name;
- content/media type;
- subject and scope;
- trust/provenance;
- digest and availability;
- preview/download;
- supersession history;
- “used by” references;
- add/replace/remove controls where mutable.

Key Attachment names get domain-specific presentation:

- `request` renders as the primary Job request;
- `notes/*` renders as a chronological notes stream;
- `prompt` renders in the Step prompt editor;
- `input-schema` and `output-schema` render as schema panels;
- `evidence/*` renders in evidence sections.

These are views over Attachments, not separate Note, Prompt, or Evidence-file
stores.

## Workflow Surfaces

### Workflow List

Shows:

- Workflow families;
- active and approved versions;
- candidates and validation state;
- pinned Job counts;
- last analysis/promotion;
- compatibility and migration warnings.

### Approved Workflow Detail

Read-only semantic view:

- graph;
- steps and transitions;
- Job schema;
- capability/grant inventory;
- Agent profiles;
- Attachment inventory and digests;
- validation/promotion evidence;
- Jobs pinned to the version;
- “Derive candidate” action.

### Candidate Review

Shows:

- source Product/Git revision and analysis operation;
- semantic diff from the base version;
- validation errors and warnings;
- unresolved capabilities or Attachments;
- affected Jobs and migration impact;
- open in editor;
- validate;
- discard;
- publish;
- publish and activate.

Publishing always sends candidate ID, digest, and expected revision.

## Workflow Editor

### Purpose

The Workflow Editor edits candidates. It does not edit approved versions or
active executions.

It must support both people and agents:

- an agent can generate or revise a candidate;
- a person can inspect and manipulate the graph;
- changes remain structured and reviewable;
- the compiler continuously explains invalid or unreachable behavior.

### Layout

Desktop:

```text
┌──────────────── Workflow candidate header ────────────────┐
│ Name · base version · validation · Analyze · Save · Publish│
├─────────────┬────────────────────────────┬─────────────────┤
│ Step palette│                            │ Inspector       │
│             │       tldraw canvas        │ - step config   │
│ Agent       │                            │ - action        │
│ Action      │                            │ - prompt        │
│ Decision    │                            │ - schemas       │
│ Human       │                            │ - transitions   │
│ Timer       │                            │ - policies      │
│ Fan-out     │                            │ - attachments   │
│ Join        │                            │                 │
├─────────────┴────────────────────────────┴─────────────────┤
│ Validation / semantic diff / agent analysis drawer         │
└────────────────────────────────────────────────────────────┘
```

On narrow screens the inspector and validation panel become drawers. The
editor remains usable through a non-canvas ordered Step list.

### Semantic Shapes

Initial custom shapes:

| Shape | Semantic entity |
|---|---|
| Start | Workflow start reference |
| Agent | `action` step bound to `agent.call` |
| Capability | `action` step bound to another capability |
| Decision | deterministic decision step |
| Human | human signal/input step |
| Timer | durable timer step |
| Fan-out | bounded branch creation |
| Join | branch join |
| Subworkflow | pinned child Workflow Version |
| Terminal | execution outcome |

Arrows represent Transition Definitions. Arrow labels show outcome and guard
summary.

Shape IDs contain or map deterministically to Step IDs. Layout coordinates,
camera, selection, colors, and collapsed inspector state are not semantic.

### Authority Boundary

The authoritative candidate is canonical Workflow JSON in the shared Workflow
service.

tldraw stores:

- visual shape records;
- bindings/arrows used to render semantic edges;
- coordinates and grouping;
- editor-local document layout;
- per-user session state such as camera and selection.

Semantic editing flows through typed commands:

```text
AddStep
UpdateStep
RemoveStep
AddTransition
UpdateTransition
RemoveTransition
SetStartStep
BindAttachment
UnbindAttachment
UpdateJobSchema
UpdatePolicy
```

The editor adapter translates a valid gesture into one command batch with:

- candidate ID;
- expected candidate revision and digest;
- idempotency key;
- semantic command(s);
- optional layout patch.

The server validates and returns the resulting canonical graph. The canvas then
reconciles to that graph.

Moving a shape changes only layout. Connecting two shapes proposes a semantic
Transition. Deleting a Step is rejected or requires explicit handling when
incoming/outgoing transitions, active validation issues, or referenced
Attachments would be orphaned.

tldraw local persistence is not used as Workflow authority. Document layout may
be persisted to `layout.json`; session camera/selection may stay browser-local.
The official snapshot APIs are an implementation mechanism, not the domain
model.

### Bounded React Island

The tldraw SDK currently requires React. Refine contains it as one compiled,
lazy-loaded feature bundle:

```text
Vanilla Refine shell
└── #workflow-editor-host
    └── React root
        └── tldraw + editor inspector
```

Rules:

1. React is not loaded outside Workflow Editor routes.
2. The island calls the same `/api/workflows/...` routes as CLI/API adapters.
3. It cannot import Rust-generated business rules into browser-only code.
4. It exposes `mountWorkflowEditor(host, context)` and
   `unmountWorkflowEditor(host)`.
5. Route exit explicitly unmounts the React root and disposes tldraw listeners.
6. The bundle and its dependency lock are reproducible and checked into the
   normal Refine build/release process.
7. No tldraw multiplayer/sync service is required for v5.

### tldraw Licensing Boundary

Refine's MIT license does not relicense the embedded tldraw SDK. Under tldraw's
current source-available terms, an open-source project may include the SDK, but
production use still requires an applicable trial, commercial, or hobby
license key, including for downstream deployments.

The browser must accept the public domain-bound key from Refine runtime/release
configuration, never Target Product state. Missing or invalid production
configuration renders an explicit unavailable-editor state while retaining the
non-canvas Step list and CLI/API editing paths. A permissively licensed editor
alternative remains a valid implementation choice if Refine cannot impose that
downstream key obligation.

The release must preserve the upstream license, disclose any license-mode
telemetry, and verify localhost, remote-browser, desktop, offline, and
downstream distribution behavior. See the official
[license](https://tldraw.dev/community/license) and
[license-key](https://tldraw.dev/sdk-features/license-key) documentation.

### Idiomorph Contract

The editor host uses:

```html
<div id="workflow-editor-host"
     data-morph-preserve="1"
     data-testid="workflow-editor"></div>
```

Idiomorph must never reconcile descendants owned by React/tldraw.

- Route entry creates the stable host and mounts once.
- Refreshes morph the surrounding vanilla shell, not the editor subtree.
- Route exit calls the explicit release/unmount hook before removing the host.
- A new candidate ID or base revision unmounts and remounts deliberately.
- Preserved nodes must not survive into a non-editor route.

This follows the existing toolbar/xterm ownership lesson: DOM preservation is a
lifecycle contract, not an assumption that compatible-looking elements are safe
to reuse.

### Concurrent Change And SSE

The editor pins candidate revision and digest.

Relevant SSE events:

```text
workflow_candidate_changed
workflow_validation_changed
workflow_version_published
workflow_activation_changed
workflow_analysis_progress
operation_progress
```

If a candidate changes elsewhere:

- clean editor: fetch and reconcile canonical graph;
- dirty editor: show a sticky stale-revision banner and stop autosave;
- user may reload, inspect semantic diff, or rebase local command batches;
- no remote event overwrites unsaved changes.

Outside the editor, SSE events trigger normal `renderInto` refreshes. SSE is a
notification transport; reconnect performs authoritative HTTP reconciliation.

### Validation UX

Validation runs incrementally but publishes only server-authoritative results.
Issues identify:

- step/transition;
- severity;
- violated invariant;
- suggested repair;
- whether publication is blocked.

The editor shows:

- unreachable steps;
- missing terminal paths;
- ambiguous transitions;
- invalid guards;
- unresolved capabilities;
- missing prompt/schema Attachments;
- unsafe/unbounded loops or fan-out;
- incompatible Job schema changes;
- unavailable Nodes/providers;
- missing human gates required by policy.

The user can ask an agent to repair selected issues. The repair is another
candidate command batch.

### Agent-Assisted Design

Agent actions:

- Analyze Target Product;
- draft initial Workflow;
- explain a step;
- propose prompts/schemas;
- add recovery behavior;
- simplify the graph;
- assess a semantic diff;
- improve from Job execution evidence.

The agent operates on the candidate revision visible to the user. Its response
produces a reviewable command batch and Attachments. It never mutates the canvas
or publishes a version through hidden client state.

## Dynamic Workflow Visualization

The current fixed status-card renderer is replaced with a shared renderer over
Workflow projection data:

```json
{
  "workflow_id": "incident-response",
  "version": 3,
  "states": [
    {
      "id": "diagnose",
      "label": "Diagnosis",
      "kind": "agent",
      "count": 4,
      "agent_managed": true,
      "attention": 1
    }
  ],
  "edges": []
}
```

Dashboard/Job-list summaries remain lightweight cards or a compact graph. They
do not mount tldraw. The same semantic projection supplies labels, filters, and
links.

## Toolbar And Agent Interaction

The existing toolbar remains the interaction dock:

- Job Agent tabs attach to active agent Step Attempts;
- Workflow Designer agent tabs attach to analysis/candidate operations;
- Files can preview Artifact content or Target Product paths;
- System shows operations and failures;
- Terminal remains process-backed;
- Todo remains independent unless later generalized.

Tabs display Job, execution, activation, and attempt identity so multiple agent
calls cannot appear to be one session.

Switching Agent/Files/System views retains the existing renderer ownership and
Idiomorph release contract.

## Settings And Product UX

Product settings rename Application/Target App labels to Target Product.

Settings separate:

- Node/runtime settings;
- Target Product lifecycle and health;
- active Domain Workflow;
- Workflow/Job schema and policies;
- Agents/providers;
- Artifact storage;
- governance/guidance/quality capabilities where installed;
- migration and release management.

Workflow-defined Product lifecycle actions appear from capability bindings; the
browser does not assume every Target Product is software.

## API Contracts Used By Browser

Route groups, with exact shapes defined by shared Rust types:

```text
GET  /api/product/status
GET  /api/products

GET  /api/projects
POST /api/projects
GET  /api/projects/<id>
POST /api/projects/<id>/jobs
DELETE /api/projects/<id>/jobs/<job-id>
POST /api/projects/<id>/jobs/reorder

GET  /api/workflows
GET  /api/workflows/<id>
GET  /api/workflow-versions/<id>/<version>
POST /api/workflow-analysis
GET  /api/workflow-candidates/<id>
POST /api/workflow-candidates/<id>/commands
POST /api/workflow-candidates/<id>/validate
POST /api/workflow-candidates/<id>/publish
POST /api/workflows/<id>/activate

GET  /api/jobs
POST /api/jobs
GET  /api/jobs/<id>
PATCH /api/jobs/<id>
POST /api/jobs/<id>/revisions
POST /api/jobs/<id>/executions
GET  /api/jobs/<id>/actions
POST /api/jobs/<id>/actions/<action>
POST /api/jobs/<id>/interventions

GET  /api/attachments?subject=<selector>
POST /api/attachments
GET  /api/artifacts/<id>

GET  /api/sse
```

Browser handlers remain adapters. Validation, publication, action availability,
property patches, attachment naming, and execution settlement belong to shared
services.

## Loading, Empty, And Failure States

Every main route distinguishes:

- loading;
- no Target Product;
- incompatible Product schema;
- missing Workflow;
- candidate pending;
- invalid Workflow;
- no Jobs;
- degraded/unavailable Artifact;
- stale candidate;
- lost live updates/reconnecting;
- operation running/failed/cancelled;
- permission or capability unavailable.

No state collapses into an empty table or silent disabled button.

## Accessibility

- Every canvas operation has a Step-list/inspector equivalent.
- Workflow graph nodes and edges have an accessible textual outline.
- Keyboard users can add, configure, connect, reorder, and remove steps.
- Color is never the only indication of step kind, validation, or state.
- Agent activity and validation updates use bounded live regions.
- Attachment previews retain download/open alternatives.
- Focus is restored after Idiomorph refresh and after editor drawers close.

## Browser Verification

Static/DOM tests:

- route parsing and stage gating;
- no-product/no-workflow/ready-workflow transitions;
- Project membership, ordering, and rollup rendering;
- dynamic Workflow status rendering;
- Job filters and detail modal preservation;
- Attachment naming and views;
- Idiomorph focus/selection preservation;
- editor host preserve/release behavior;
- no old Target App/Goal/Round labels in v5 routes.

Workflow Editor unit tests:

- semantic graph to shapes and shapes to command batches;
- layout-only versus semantic changes;
- candidate revision/digest conflicts;
- custom shape migrations;
- mount/unmount disposal;
- validation issue mapping;
- accessible Step-list parity.

Real-browser tests:

- Analyze Product → candidate → editor → validate → publish → activate;
- create/start Job and observe SSE state movement;
- edit prompt Attachment and verify semantic diff;
- concurrent candidate update while local edits are dirty;
- direct editor route reload;
- editor → Jobs → editor lifecycle without retained React/tldraw DOM;
- Agent/Files/Workflow Editor transitions preserve only their owned state;
- keyboard-only workflow authoring path.

Playwright/Chromium absence is reported as skipped, not passed. Browser tests use
the same built editor bundle shipped by Refine.

## Browser Acceptance

- A first-time user understands the Product → Workflow → Job progression.
- Projects visibly group and order Jobs without acquiring workflow state.
- A missing Workflow leads directly to agent analysis, import, or blank design.
- The entire non-editor UI remains within the existing static/vanilla shell.
- tldraw is lazy, bounded, explicitly mounted/unmounted, and non-authoritative.
- Approved Workflow Versions are read-only.
- Candidate edits are typed, revision-checked, and semantically validated.
- Workflow summaries and Job actions contain no fixed software-delivery state
  assumptions.
- Attachments provide prompts, notes, schemas, inputs, outputs, and evidence
  through one shared component.
- SSE and Idiomorph retain their existing truth and interaction-preservation
  contracts.
