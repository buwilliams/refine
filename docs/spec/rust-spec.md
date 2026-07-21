# Rust Architecture Spec

## Summary

Define a Rust-native Refine architecture with three user-facing surfaces:
**web**, **desktop**, and **CLI**. All surfaces talk to one local
supervisor daemon that owns host integration, process lifecycle, project
attachment, target-app operations, agent execution, workflow automation, and
persisted .refine state. Real work is centralized in `workflow` and `tools` so
Desktop, browser, and CLI expose the same product capabilities through
different presentation and transport layers.

The Rust direction is a product architecture for a native local Refine
platform. The current Python implementation remains useful as the behavioral
baseline during the port, but the target architecture should not require host
Python or `uv` for Refine itself.

## Goals

- Make Refine installable as a native desktop application on macOS and Windows.
- Preserve a first-class web surface for local and remote browser use.
- Preserve a first-class CLI surface for scripting, debugging, and automation.
- Establish one local supervisor daemon as the process and state authority.
- Model Refine around explicit system capabilities instead of UI-specific
  actions.
- Remove host Python and `uv` as required Refine runtime dependencies.
- Keep target-app and provider dependencies explicit, discoverable, and
  task-scoped.
- Make installation, update, diagnostics, and recovery native OS behaviors.
- Keep business logic shared across web, desktop, and CLI surfaces.
- Preserve complete feature parity across web, desktop, and CLI by routing
  product behavior through shared `workflow` and `tools` modules.
- Support an incremental port that can be validated workflow by workflow.

## Non-Goals

- Preserve old Python package or module boundaries in Rust.
- Recreate the current web app as a separate product from Desktop.
- Let Tauri own Refine backend logic or long-running workflow execution.
- Hide all target-app dependencies. Git, Node, Docker, browsers, provider auth,
  and language toolchains may still be necessary for specific workflows.
- Duplicate workflow logic in the web UI, desktop UI, and CLI.
- Require a local browser for the desktop surface.

## Product Surfaces

Refine has three surfaces over one local system. They should have complete
feature parity. A surface may choose a different interaction style, but missing
capabilities should be treated as implementation goals unless explicitly
documented as product decisions. This is the main reason `workflow` and
`tools` exist: workflow semantics, state mutation, process lifecycle, provider
execution, and storage orchestration are implemented once and exposed through
every surface.

### Desktop

Desktop is the default download-and-use surface for macOS and Windows.

Responsibilities:

- Install, update, launch, and stop the local Refine daemon.
- Render the shared Refine UI in a native webview pointed at the local Refine
  web server.
- Provide native window, tray, menu, notification, and deep-link integration.
- Guide first-run setup: target app, provider, dependency checks, and auth.
- Surface daemon status, logs, diagnostics, and recovery actions.
- Broker only narrow native commands. Desktop must not start target apps,
  agent processes, workers, or rebuilds itself.

Tauri is the preferred desktop shell. Tauri commands should be a thin native
bridge for OS shell integration, daemon bootstrap/status checks, window
control, tray/menu actions, notifications, and deep links. The preferred
desktop design is for the Tauri webview to point at the daemon's local web
server; Tauri should not become a parallel backend.

### Web

Web is the browser surface served by the daemon's local web server.

Responsibilities:

- Provide the full Refine product UI.
- Work locally inside Desktop's webview and in an external browser.
- Support remote or headless installs where Desktop is not present.
- Use the same HTTP and server-sent-event APIs as Desktop.

The web surface should remain deployable as static assets served by the daemon.
Business logic belongs in Rust workflow/tools services, not in frontend-only code. The
Rust-owned web UI asset tree lives under `src/surfaces/web/static/`, so native
Refine is self-contained and does not depend on Python-owned static files.

### CLI

CLI is the operator and automation surface.

Responsibilities:

- Expose model-oriented command groups that match Rust model vocabulary.
- Put actions under the model they operate on rather than copying the old Python
  CLI groupings.
- Emit human-readable output and structured JSON for automation.
- Call the same daemon APIs and workflow/tools operations as the UI surfaces.

The CLI may also run limited bootstrap commands when the daemon is not yet
installed, but normal operation should go through the daemon.

The Rust CLI is an intentional improvement over the Python CLI, not a
compatibility shell. Product commands should be grouped by Rust model nouns,
with verbs expressed as actions on those models. Temporary Python-compatible
aliases may exist during migration, but they should delegate into the
model-oriented command surface and should not define the architecture.

Representative model-oriented CLI groups:

- `refine project`: attachment, registry, schema, migration, sync, and active
  app state. Actions include `status`, `attach`, `switch`, `detach`,
  `register`, `remove`, `migrate`, `sync`, and `doctor`.
- `refine goal`: executable Goal records and Goal-owned rounds, notes, quality,
  governance, implementation, review, merge, and deletion. Actions include
  `create`, `list`, `show`, `edit`, `note`, `round`, `start`, `cancel`,
  `retry`, `verify`, `merge`, `undo`, `delete`, `assign-feature`, and
  `remove-feature`.
- `refine feature`: Feature metadata, ordered Goal membership, derived status,
  workflow movement, import-backed creation, cancellation, and deletion.
  Actions include `create`, `list`, `show`, `edit`, `add-goal`, `remove-goal`,
  `reorder-goal`, `move`, `cancel`, `delete`, and `import`.
- `refine workflow`: controls for the always-on workflow engine. Public
  actions include `pause` and `resume`; workflow state movement is automatic
  while the daemon is running.
- `refine node`: local node identity, active-node selection, ownership, and
  transfer. Actions include `list`, `show`, `create`, `activate`, `archive`,
  `rename`, `settings`, and `transfer`.
- `refine cluster`: cluster operations over nodes, project-state sync,
  maintenance, and bounded remote operations. Actions include `list`, `show`,
  `add-node`, `edit-node`, `enable-node`, `disable-node`, `remove-node`,
  `sync`, `run`, `transfer`, and `maintenance`; compatibility commands persist
  their node configuration in the Node registry.
- `refine log`: activity, round logs, diagnostics logs, support bundles, and
  exported evidence. Actions include `list`, `tail`, `show`, `query`,
  `export`, and `bundle`.

Host and supervisor operations that do not belong to a product model should use
small explicit groups such as `refine system` for daemon lifecycle,
installation, update, status, and recovery. These commands are allowed because
they operate on Refine as installed software rather than on a project model.

`refine system start` is the public command for starting the local Refine
daemon. Starting the daemon also starts the daemon's local web/API server; this
is not a separate product mode. `system start` may expose foreground,
single-request, static-asset, cache, runtime-root, and port options needed by
development, tests, installers, and service managers. There should not be a
separate public `refine system web` command. Service metadata and daemon
bootstrap paths should use `refine system start --foreground` when they need a
long-running foreground process.

## System Model

```text
Desktop shell      Browser UI          CLI
 (Tauri)              |                 |
    |                 |                 |
    +-----------------+-----------------+
                      |
          port-scoped Refine daemon
                      |
              local web server
     HTTP, SSE, static assets,
     local-origin checks, request parsing, response shaping
                      |
                    workflow/tools
   supervisor, product workflow, host adapters,
   operations, storage orchestration, observability
                      |
                    model
       canonical records, workflow states,
       allowed operations, process kinds
                      |
 persisted project state + local runtime root/<port>/ runtime state
```

The supervisor daemon is the single local authority. It contains the local web
server that serves UI assets and exposes the HTTP and server-sent-event routes
used by Desktop, browser, and CLI surfaces. Route
handlers are transport adapters: they handle HTTP concerns, translate requests
and responses, and call `workflow/tools` for real work. Surfaces do not directly mutate
persisted state or own long-lived OS processes.

## Capability Model

Capabilities are the primary code-organization model for Refine's Rust
architecture. They are namespaces, service boundaries, trait families, storage
interfaces, and vocabulary for major areas of the system. They sit inside the
overall supervisor architecture described above; they are not separate daemons,
UI-specific action handlers, or a replacement for the daemon web-server
transport layer.

The intent is to prevent major concerns from being reimplemented across the
codebase. Logging should not be scattered through every module as ad hoc file
writes or UI messages; it should flow through one log abstraction. The same
pattern applies to storage, process supervision, Git, provider integration,
quality checks, chat, settings, diagnostics, and every other major capability.

Each capability should define:

- A stable namespace and module home.
- Public service traits and concrete implementations.
- Domain models, value types, and vocabulary used by that capability.
- The storage, process, network, or host-integration ports it owns.
- The events, logs, and diagnostics it emits through shared abstractions.
- The errors it exposes through shared error types and API translation.
- Test fixtures and fake implementations for isolated and black-box tests.
- Dependency rules that keep the capability from reaching into unrelated
  surfaces or duplicating another capability's responsibilities.

Product actions compose these capabilities through the daemon. For example, a
Goal execution workflow may use work-item, workflow claim, provider, process,
Git, storage, event, and log abstractions, but those abstractions remain
centralized instead of being rebuilt inside the Goal execution code.

## Model

Module: `model`; path: `src/model/`.

The Rust architecture should be data-first. State, model definitions, and
workflow rules are the stable center of the system; processing modules are
second-class consumers of that model. Code organization should make it possible
to answer questions about Refine state without reading executor, UI, provider,
or process code.

`model` owns the canonical Rust structs, enums, value types, persisted
record shapes, and workflow state machines used by the rest of the product. In
Rust terms, this is the domain model plus serialized record surface. It should
be the first place to inspect when asking what Refine stores, what states exist,
and which operations are allowed.

Responsibilities:

- Canonical product models: Project, Goal, Feature, Workflow, Cluster, Node, and
  Log.
- Workflow-state enums, transition tables, and allowed-operation rules.
- Durable model versions and migration-facing definitions.
- Serializable request and response payload types when they represent product
  state rather than HTTP, CLI, or desktop transport mechanics.
- Validation helpers that are pure functions over model values.
- Test fixtures for representative valid and invalid model states.

Rules:

- `model` must not perform I/O, spawn processes, call providers, route API
  requests, or know about UI surfaces.
- Processing modules ask `model` what states and operations are valid
  before mutating state or launching work.
- Avoid vague top-level buckets such as `process`, `events`, or `records`.
  OS process behavior belongs under `tools::host::process_supervision`; event
  delivery belongs under the daemon web server and observability; persisted
  record shapes live beside the product model they describe.

### Model Modules

The initial Rust model modules should mirror the product concepts already
visible in the Python implementation:

```text
src/model/
  project/
  goal/
  feature/
  workflow/
  cluster/
  node/
  log/
```

These properties are the current Python-derived baseline. Rust can refine type
names and split nested structs, but it should preserve the product vocabulary
unless a migration intentionally changes it.

### Project Model

Module: `model::project`; path: `src/model/project/`.

Owns the canonical shape of a Refine target-app attachment and project state.
This covers persisted `.refine` state in the isolated
`<app>/.git/refine-state-worktree/`, its `.git/refine-live-state/` mutation
projection, and port-scoped runtime selection state under the Refine checkout's
`run/` directory. The primary target-app worktree must never contain
`<app>/.refine`.

Properties:

- `ProjectConfig`: `schema_version`, `refine.version`, `created_at`,
  `updated_at`, and project-level `settings`.
- `ProjectStatus`: `attached`, `registry_enabled`, `target_root`,
  `refine_dir`, `config_path`, `schema`, `maintenance`, `apps`,
  `active_node_id`, `active_node`, and `message` when detached or invalid.
- `ProjectSchemaStatus`: `compatible`, `migration_required`,
  `schema_version`, `current_schema_version`, `reason`, `migration_id`,
  `migration_description`, `safe_auto`, `requires_cluster_quiescence`, and
  `operator_instructions`.
- `ProjectMaintenance`: `active`, `created_at`, `updated_at`, plus
  operator-supplied fields such as reason or details.
- `AppRegistry`: `version`, `active_app`, and `apps`.
- `RegisteredApp`: `name`, `path`, `added_at`, and `last_used_at`.
- `PrimaryRuntime`: `port` and `active_node_id` from `run/primary.json`.

Rules:

- `model::project` defines project identity, attachment status, schema status,
  app registry entries, and runtime-selection record shapes.
- It should not choose paths, read TOML/JSON, scan Git, or mutate `run/`; those
  behaviors belong in `tools::product::project_registry`,
  `tools::product::project_state`, and `tools::supervisor::runtime`.

### Goal Model

Module: `model::goal`; path: `src/model/goal/`.

Owns the canonical Goal, round, note, and Goal-owned quality/governance state.
The Python baseline stores persisted Goal state in `goal.json` and projects common
fields into `goals_index` as a rebuildable cache.

Properties:

- `Goal`: `id`, `name`, `status`, `priority`, `branch_name`, `feature_id`,
  `feature_order`, `node_id`, `created`, `updated`, `notes`, and `rounds`.
- `GoalIndexProjection`: `id`, `name`, `status`, `priority`, `reporter`,
  `round_count`, `created`, `updated`, `branch_name`, `node_id`,
  `feature_id`, `feature_order`, and `json_path`.
- `GoalNote`: `id`, `author`, `body`, `created`, and `updated`.
- `GoalRound`: `reporter`, `prompt`, `created`, `updated`, optional
  `guidance_decision`, and derived `logs` when a response hydrates sidecar
  round logs.
- `RoundGovernance`: `rule_state`, `meta_rule_state`, `product_state`,
  `constitution_state`, `governance_message`, `governance_details`,
  `governance_checked_at`, and `governance_rule_actions`.
- `RoundQuality`: `quality_state`, `quality_message`, `quality_details`, and
  `quality_checked_at`.
- `GoalPriority`: `low`, `medium`, or `high`.

Rules:

- `status` must use `model::workflow::GoalStatus`.
- `feature_id` and `feature_order` associate a Goal with a Feature; the ordered
  Feature membership is stored on the Goal.
- Round logs are represented by `model::log` entries. Storage may keep them in
  a sidecar JSONL file, but the model shape is shared with activity logs.

### Feature Model

Module: `model::feature`; path: `src/model/feature/`.

Owns Feature metadata and the derived rollup produced from ordered Goals.
The Python baseline stores persisted Feature metadata in `feature.json` and
projects common fields into `features_index`.

Properties:

- `Feature`: `id`, `name`, `description`, `reporter`, `node_id`, `created`,
  `updated`, and `json_path`.
- `FeatureIndexProjection`: `id`, `name`, `description`, `reporter`,
  `node_id`, `created`, `updated`, and `json_path`.
- `FeatureDetail`: `Feature` plus `goals`, `node_display_name`, and rollup
  fields.
- `FeatureRollup`: `status`, `goal_count`, `done_count`, `active_count`,
  `failed_count`, `cancelled_count`, `blocked_count`, and `next_goal`.

Rules:

- A Feature's workflow status is derived from its ordered Goals; it is not an
  independent persisted field in the Python baseline.
- Feature workflow actions can move eligible Goals to `backlog` or `todo`.
  Protected Goal statuses are `review`, `done`, `ready-merge`, and
  `build`.
- Cancelling a Feature is a system-owned cascade operation. It cancels
  `backlog`, `todo`, `in-progress`, `qa`, `ready-merge`, `build`,
  `review`, and `failed` Goals through the shared Goal cancel path, skips `done`
  Goals, and skips already `cancelled` Goals.
- Feature cancel must stop or reconcile active work before recording the final
  cancellation result, so users do not see running agent, QA, merge, or rebuild
  operations continue after the Feature was cancelled.

### Workflow Model

Module: `model::workflow`; path: `src/model/workflow/`.

Owns workflow states, transition rules, and allowed-operation decisions for
Goal and Feature workflows.

Properties:

- `GoalStatus`: `backlog`, `todo`, `in-progress`, `qa`, `ready-merge`,
  `build`, `review`, `done`, `failed`, and `cancelled`.
- `TerminalGoalStatus`: `done` and `cancelled`.
- `AutomatedGoalStatus`: `in-progress`, `qa`, `ready-merge`, and
  `build`.
- `UserStatusTransition`: currently `backlog -> todo`, `todo -> backlog`,
  `review -> todo`, `done -> review`, `failed -> todo`, and
  `cancelled -> todo`; same-status updates are no-ops.
- `BulkStatusTarget`: any Goal status except `in-progress`, `qa`, and
  `ready-merge`, plus the special `__last_workflow_state` restore operation.
- `FeatureWorkflowTarget`: `backlog` or `todo`.
- `FeatureProtectedStatus`: `review`, `done`, `ready-merge`, and
  `build`.
- `FeatureCancelStatus`: `backlog`, `todo`, `in-progress`, `qa`,
  `ready-merge`, `build`, `review`, and `failed`.

Rules:

- Manual status updates cannot enter system-owned states through ordinary
  metadata edits; dedicated workflow actions own those transitions.
- The model should answer whether an operation is allowed for a given Goal or
  Feature state. `workflow/tools` performs the operation only after the model approves
  it.
- Examples of operations the model should name: create Goal, edit Goal metadata,
  edit notes, submit new round, edit latest round, start implementation,
  cancel automation, retry agent, retry QA, retry merge, verify/review, merge,
  undo, delete, assign to Feature, remove from Feature, reorder in Feature,
  move Feature workflow, and cancel Feature.

### Cluster Operations Model

Module: `model::cluster`; path: `src/model/cluster/`.

Defines compatibility response shapes and validation vocabulary for operations
over a set of Nodes. Nodes themselves are canonical in `model::node` and are
persisted in the Node registry; runtime SSH execution belongs in
`tools::host::cluster`.

Properties:

- `Cluster`: `nodes` and `updated_at`.
- `ClusterNode`: compatibility alias for `Node`.
- `ClusterHealth`: compatibility alias for `NodeHealth`.
- `RemoteRunResult`: `node_id`, `command`, `remote_command`, `exit_code`,
  `stdout`, `stderr`, and `ok`.

Rules:

- Node ids used by cluster operations are lowercase ids that start with a letter
  or digit and use only lowercase letters, digits, `_`, or `-`.
- `ssh_host` must be a host, not a `user@host` string.
- Disabled nodes remain registered but cannot run remote Refine commands.

### Node Model

Module: `model::node`; path: `src/model/node/`.

Owns Refine node identity, node-scoped settings, and optional remote-management
configuration. A node is the unit of ownership for Goals and Features inside one
target app; a cluster is the set of nodes and the operations Refine can perform
over that set.

Properties:

- `Node`: `id`, `display_name`, `created_at`, `updated_at`, `enabled`,
  optional SSH/bootstrap fields, optional `health`, and `archived`.
- `NodeHealth`: at minimum `status` and `checked_at`, with room for
  provider-specific details.
- `NodeRegistry`: `nodes`.
- `ActiveNodeSelection`: `active_node_id`, `refine_dir`, and `updated_at`.
- `NodeSettings`: application, runtime, target-app config, and target-app
  runtime setting maps scoped to a node.
- `NodeOwnership`: the `node_id` fields on Goal and Feature records.

Rules:

- The active node cannot be archived.
- Runtime automation owns exactly one local node for the lifetime of the
  supervisor/worker process, even if the UI browses another node.
- Mutating a Goal or Feature requires ownership by the active local node unless
  a transfer operation explicitly changes ownership first.

### Log Model

Module: `model::log`; path: `src/model/log/`.

Owns the canonical log and activity entry shape. Writing, retention, indexing,
streaming, and support-bundle export belong in `tools::observability`.

Properties:

- `LogEntry`: `datetime`, `severity`, `category`, `message`, optional
  `details`, optional `actions`, optional `actor`, and optional `goal_id`.
- `ActivityEntry`: `id`, `datetime`, `severity`, `category`, `message`,
  optional `goal_id`, optional `actor`, optional `details`, and optional
  `actions`.
- `RoundLogEntry`: `LogEntry` plus `round_idx` when stored in sidecar JSONL.
- `LogAction`: action objects attached to an entry; the exact variants should
  become typed as Rust implementations replace Python's free-form dictionaries.
- `LogQuery`: `limit`, `offset`, `goal_id`, `since_id`, `severity`,
  `category`, `actor`, `q`, `sort`, and `direction`.

Rules:

- Activity entries and round logs share the same entry shape.
- Workflow transitions should be representable as log entries with
  `category = "state"` and messages that can identify the previous and next
  workflow state.
- `model::log` defines data; `tools::observability` owns append, search,
  cleanup, metrics, streaming, diagnostics, and support bundles.
- Storage serializes and deserializes model records; it does not invent shadow
  model definitions.
- UI, CLI, API, automation, process, provider, and quality code should all use
  the same model types for Goal state and allowed operations.
- If a workflow rule can be expressed as data or a pure state transition, it
  belongs in `model`, not in a runner, route handler, or button callback.

## Core Capabilities

### Installation And Update

Module: `tools::host::installation`; path: `src/tools/host/installation/`.

Owns abstractions for: install, repair, update, rollback, uninstall.

Requirements:

- Install Refine without requiring host Python.
- Register the daemon with the host OS where appropriate.
- Keep Desktop app updates and daemon/workflow/tools updates coherent.
- Detect and report stale, partial, or conflicting installs.
- Support rollback when an update fails before state migration completes.
- Preserve user data and target-app state across upgrades.

Source/dogfood promotion is a separate update channel owned by
`tools::host::source_promotion`; it must not change the published-release
installer contract. It reports the controller checkout, current commit,
configured remote and branch, and latest fetched commit through the shared
CLI/API/UI service. Promotion requires a clean checkout, fast-forward-only
ancestry, paused automation with no active Goal claim or non-daemon process,
and a successful locked release build of the candidate before the daemon
stops.

The promotion handoff runs as an external helper that outlives the initiating
HTTP request. It persists atomic stage state under the port runtime root,
rechecks Git and runtime preconditions before activation, advances the checked
out branch without reset or merge, restarts from the candidate binary, and
verifies daemon health. Restart failure attempts to restore the prior commit
and daemon and always records actionable recovery state. Tests use host fakes
and must never perform a real source promotion.

OS backends:

- macOS: signed app bundle, notarization, launchd or Login Item integration,
  keychain where appropriate.
- Windows: signed installer, Start Menu integration, background service or
  user-session daemon strategy, Windows Credential Manager where appropriate.
- Linux: CLI/web install path, systemd where available, best-effort process
  mode otherwise.

### Daemon Lifecycle

Module: `tools::supervisor::lifecycle`; path: `src/tools/supervisor/lifecycle/`.

Owns abstractions for: start, stop, restart, status, health, recover.

Requirements:

- One daemon owns one port-scoped local Refine runtime authority, matching the
  current Python runtime model.
- The daemon exposes local web-server routes for surfaces. The supported
  boundary is loopback/local access, not per-surface API authorization.
- Starting the daemon starts this local web/API server. Running without opening
  a browser is still a normal daemon start; running without the local
  web/API server is not a supported public mode.
- Foreground daemon execution is a lifecycle option for service managers,
  tests, and development, not a separate daemon mode.
- Status distinguishes daemon health, web availability, worker state, target-app
  state, active operations, and degraded integrations.
- Restart preserves attached app selection and running operation records.
- Crash recovery reconciles persisted state with OS process reality.
- Stop terminates or detaches managed processes according to their ownership
  policy.

### Surface Events

Module: `tools::supervisor::runtime`; path: `src/tools/supervisor/runtime/`.

Owns abstractions for: open UI, stream state, deliver notifications, and surface
runtime context.

Requirements:

- Desktop, browser, and CLI use the same local daemon routes.
- The daemon must bind and expose supported HTTP APIs only as a local control
  surface. Local mutation routes do not require authorization tokens.
- Web UI can stream activity, process output, operation progress, and chat events.
- Desktop can subscribe to events for badges, tray state, and notifications.

### Project Registry

Module: `tools::product::project_registry`; path: `src/tools/product/project_registry/`.

Owns abstractions for: register, attach, switch, detach, clone, remove, inspect.

Requirements:

- Refine tracks known target apps separately from the active app.
- App switching is a supervisor transaction.
- Runtime bookkeeping must not dirty tracked target-app files.
- Target app identity, path, Git root, remote, and health are explicit.
- Detached/no-app mode is supported.
- Switching should prepare or migrate the target app before making it active.

### Project State

Module: `tools::product::project_state`; path: `src/tools/product/project_state/`.

Owns abstractions for: initialize, read, mutate, migrate, sync, rebuild
projections.

Requirements:

- Durable Refine workflow state lives in project-local storage.
- Runtime state lives outside tracked target-app files.
- Rust should maintain a materialized projection of model records for fast
  list, count, filter, sort, and lookup queries.
- The first pass should persist the projection snapshot under the selected
  local runtime root's `<port>/cache/` directory, including source-file
  fingerprints, so startup can load the snapshot and rescan only changed,
  missing, or new persisted records.
- The projection snapshot and required indexes are part of
  `tools::product::project_state`; routes should ask tools for query results
  instead of rebuilding ad hoc indexes per surface.
- A full persisted-record scan remains the fallback when the projection snapshot
  is missing, corrupt, incompatible, or intentionally discarded.
- Durable records remain the source of truth. Projection state must be
  rebuildable after corruption or version upgrades.
- Mutations flow through shared workflow/tools services and emit audit/activity events.
- Migrations are versioned, idempotent, and observable.

### Work Items

Module: `tools::product::work_items`; path: `src/tools/product/work_items/`.

Owns abstractions for: create, import, deduplicate, list, update, transition, cancel,
delete, assign, reorder.

Requirements:

- Goal remains the executable unit of work.
- Feature remains an optional ordered group of Goals.
- Imports support AI extraction, CSV paste, CSV file, and structured review.
- UI and CLI call shared work-item operations.
- State transitions are validated centrally.
- Node or machine ownership is enforced before mutation or workflow claims.

### Workflow Engine

Module: `workflow`; path: `src/workflow/`.

Owns abstractions for: engine, context, behavior, claim, pause, resume, cancel,
retry, and workflow-state evaluation.

Requirements:

- Workflow is always on while the daemon is running.
- Workflow behavior modules evaluate eligible Goals by workflow state.
- Feature ordering is respected.
- Global, per-node, per-provider, and per-target-app concurrency limits are
  enforced centrally.
- Workflow claims survive daemon restart or are reconciled safely.
- Agents, QA, merge, governance, and target-app build are tools invoked by
  workflow behaviors.
- Public automation controls pause or resume workflow automation; there is no
  public manual trigger command for starting workflow work.

### Agent Providers

Module: `tools::host::agent_providers`; path: `src/tools/host/agent_providers/`.

Owns abstractions for: detect, configure, authenticate, invoke, parse, resume, diagnose.

Requirements:

- Provider integrations are adapters with declared capabilities.
- Refine should support both provider CLIs and direct provider APIs where useful.
- Provider auth state is treated as external and reported clearly.
- Event parsing is provider-specific but normalizes into shared round-log and
  chat event models.
- Provider session ids, resumability, limits, and failure modes are explicit.
- Missing providers block only workflows that require them.

### Process Supervision

Module: `tools::host::process_supervision`; path: `src/tools/host/process_supervision/`.

Owns abstractions for: launch, signal, wait, stream, inspect, limit, clean up.

Requirements:

- The daemon owns all managed OS process lifecycle.
- Surfaces never launch or kill managed target-app, agent, build, test, or
  helper processes directly.
- Managed processes have typed ownership: daemon, target app, agent, quality,
  import, maintenance, or user-initiated helper.
- Daemon bootstrap, provider CLIs, target-app commands, quality checks, Git and
  SSH maintenance commands, service-manager commands, diagnostics probes, and
  native secret-store command helpers should enter through process supervision
  unless they are inside the process-supervision backend itself.
- Short-lived commands still produce managed process records with captured
  stdout, stderr, exit status, ownership, and command context.
- Sensitive commands, such as native secret-store operations, must redact
  command details and avoid persisting secret stdin or token-bearing arguments
  in process records.
- Process groups, child cleanup, stdout/stderr streaming, stdin, exit status,
  resource limits, and cancellation are modeled explicitly.
- Resource isolation is capability-detected by OS backend.

OS backends should cover:

- systemd and cgroups on Linux where available.
- launchd, process groups, and best-effort resource controls on macOS.
- Windows Job Objects, services, console control events, and process groups on
  Windows.

### Target App Operations

Module: `tools::host::target_apps`; path: `src/tools/host/target_apps/`.

Owns abstractions for: configure, start, stop, restart, status, rebuild, open, diagnose.

Requirements:

- Target-app lifecycle instructions are configured per app for start, stop, and build.
- The daemon asks the configured agent to perform lifecycle work in the target app's environment.
- Deterministic target-app test and status commands may still be configured per app.
- Long-running target-app processes are supervised and visible.
- Rebuild and status checks produce structured results.
- Target-app failures do not crash the daemon or block unrelated Refine
  operations.

### Git And Worktrees

Module: `tools::host::git_worktrees`; path: `src/tools/host/git_worktrees/`.

Owns abstractions for: inspect, branch, worktree, diff, merge, rebase, commit, reset,
push, recover.

Requirements:

- Git operations are centralized and auditable.
- Agent implementation work uses isolated worktrees where the workflow requires
  isolation.
- Dirty-worktree checks distinguish user changes from Refine-owned runtime
  artifacts.
- Merge and conflict recovery are explicit operations with visible state.
- Destructive operations require narrow, intentional commands and clear audit
  records.

### Quality And Verification

Module: `tools::host::quality`; path: `src/tools/host/quality/`.

Owns abstractions for: run checks, screenshots, compare, gate, and workflow QA
policy. Workflow QA executes the enabled target-app test commands through the
target-app service under supervisor ownership.

Requirements:

- Quality checks are operations supervised by the daemon.
- Target-app tests are configured with `target_app_test_commands`; the
  compatibility `target_app_test_command` value tracks the first enabled command.
  Enabled commands run as supervised Quality-owned subprocesses.
- Results are persisted, visible, and tied to the relevant Goal, Feature, or app.
- Users can rerun, cancel, and inspect quality operations from any surface.

### Chat And Planning

Module: `tools::product::chat`; path: `src/tools/product/chat/`.

Owns abstractions for: start, resume, stream, attach to Goal or Feature, persist context.

Requirements:

- Chat sessions use shared provider adapters.
- Goal-attached chat and standalone chat have explicit storage and resumption
  semantics.
- Long-running provider priming or resume steps are observable.
- Chat events can produce importable rounds, Goals, or Feature plans.
- Chat records are persisted enough to survive daemon restart: Refine session id,
  mode, provider, provider session id when known, attached Goal or Feature id,
  created/updated timestamps, transcript events, importable artifacts, and
  closed/interrupted status are persisted outside in-memory process state.
- In-flight provider processes are runtime operations, not persisted conversation
  state. After a daemon crash or restart, unfinished turns are marked
  interrupted with enough diagnostic detail for the UI or CLI to show what
  happened and let the user resume or start a new turn.
- Provider resumption uses the persisted provider session id when the adapter
  supports resume. If the provider cannot resume, Refine still preserves the
  transcript and starts a fresh provider session with explicit user-visible
  status.
- Goal-attached and Feature-attached chats rebuild their product context from
  persisted Refine records on resume. Persisted transcripts should not be the only
  source of Goal, Feature, or round state.

### Observability And Diagnostics

Module: `tools::observability`; path: `src/tools/observability/`.

Owns abstractions for: activity, logs, metrics, doctor, support bundle.

Requirements:

- Every capability emits structured events.
- System operations have status, progress, timestamps, errors, and owning
  surface.
- Semantic release preparation and publication use `tools::host::release` from
  UI and CLI surfaces. Preparation analyzes commits since the prior semantic
  tag, updates version-bearing files and release documentation in an isolated
  release worktree, runs deterministic gates, and produces a reviewable branch
  and commit. Publication is a distinct operation requiring explicit
  confirmation and synchronized clean `main`; Git, GitHub, credentials,
  deployment, and verification actions sit behind the fakeable `ReleaseHost`.
- Release operation requests, progress, agent activity, results, and errors are
  stored under the runtime root so reconnect, retry, and resume do not depend on
  browser memory.
- Logs are queryable from UI and CLI.
- `doctor` reports daemon, install, OS backend, target app, Git, provider,
  browser, Docker, and storage health.
- Support bundles redact secrets by default.

### Security And Permissions

Module: `tools::supervisor::security`; path: `src/tools/supervisor/security/`.

Owns abstractions for: secret storage, command allowlists, redaction, and audit.

Requirements:

- Local daemon HTTP mutation APIs do not require authorization tokens.
- Desktop should store tokens and provider-related local secrets in OS-native
  secret storage when Refine owns them.
- Command execution must pass through explicit capability APIs.
- Surfaces should not expose arbitrary shell execution as a generic primitive.
- Sensitive paths, environment variables, and tokens are redacted in logs.

### Cluster And Multi-Node

Module: `tools::host::cluster`; path: `src/tools/host/cluster/`.

Owns abstractions for: node registry, transfer, sync, remote command, ownership.

Requirements:

- Local daemon identity is explicit.
- Work ownership is enforced before workflow claims or mutation.
- Project-state sync is a shared workflow/tools operation over the dedicated `refine/state` branch. It uses `<app>/.git/refine-live-state/` and `<app>/.git/refine-state-worktree/`, while Goal and standalone worktrees also remain under `.git/`; `<app>/.refine` never exists in the primary application worktree. It batches demand-driven mutations, fetches only the state branch for those mutations, fetches all remote branches on the configurable project update pulse, and never moves an application branch. The shared `git_remote` setting, defaulting to `origin`, applies to state and Goal/application operations. A missing configured remote prevents publication but not local exclusion, state-worktree initialization, or local state commits.
- Remote execution and cluster maintenance have bounded, visible operations.
- The Rust architecture should preserve the distinction between UI selection
  and runtime ownership.

## Component Architecture

Native Refine now owns the repository root. The long-lived `python` branch
preserves the 2.3.8 Python implementation; `main` is the Rust implementation.

Use one Rust product Cargo package at the repository root. Use Rust modules for
namespaces, code ownership, service traits, and abstraction boundaries. The
project may become a small Cargo workspace to host a thin desktop Tauri wrapper,
but capabilities should not be split into separate packages. The Rust product
remains one package; the desktop package exists only for native shell packaging
and Tauri integration.

Suggested repository layout:

```text
refine/
  Cargo.toml
  docs/
  run/
    primary.json
    <port>/
      cache/
  src/
    main.rs
    lib.rs
    surfaces/
      cli/
      desktop/
      web/
        static/
          css/
          images/
          js/
      web_server/
    workflow/
      behavior.rs
      behaviors/
      context.rs
      mod.rs
    tools/
      product/
        project_registry/
        project_state/
        work_items/
        chat/
        imports/
        nodes/
      host/
        installation/
        process_supervision/
        target_apps/
        git_worktrees/
        agent_providers/
        quality/
        cluster/
      supervisor/
        lifecycle/
        operations/
        security/
        runtime/
        config/
        errors/
        testing/
      observability/
        activity/
        logs/
        metrics/
        diagnostics/
        support_bundle/
    model/
      goal/
      feature/
      workflow/
      project/
      cluster/
      node/
      log/
  desktop/
    src-tauri/
      Cargo.toml
      src/
        main.rs
  xtask/
```

The complete web UI asset tree lives under `src/surfaces/web/static/`.
Rust product code lives under `src/`, and repository automation lives under
`xtask/`, so the native architecture is explicit at the repository root. The
Tauri wrapper lives under `desktop/src-tauri/` and depends on the Rust product
package. It should not own capabilities, persisted state rules, process lifecycle,
provider behavior, or workflow logic.

The local runtime root is not part of the Rust source tree. In checkout-based
development it may be the repository-root gitignored `run/` directory. In an
installed desktop or service deployment it may live under the OS-specific app
support, cache, or service state location. The shape inside that root should
remain familiar:
`primary.json` for the primary local runtime record and `<port>/cache/` for
port-scoped caches such as projection snapshots and rebuilt query indexes.
Rust should keep this logical convention unless a migration explicitly
documents a replacement.

### Module Direction

The architecture is module-first: the root Cargo package should focus on the
modules it directly implements. Rust modules should follow a one-way dependency
graph:

```text
surfaces
  surfaces::{cli, desktop, web, web_server}
      |
workflow
  workflow::{engine, context, behavior, behaviors}
      |
tools
  tools::supervisor::{lifecycle, operations, security, runtime, config}
  tools::product::{work_items, project_state, ...}
  tools::host::{process_supervision, target_apps, git_worktrees, ...}
      |
model
  model::{project, goal, feature, workflow, cluster, node, log}

side channel for all processing layers
observability
  tools::observability::{logs, activity, metrics, diagnostics}
```

Rules:

- `model::*` modules own canonical data types, persisted records,
  workflow states, operation rules, and pure validation. They should not depend
  on processing, surface, host, supervisor, or observability modules.
- `workflow::*` modules own workflow state movement and behavior orchestration.
- `tools::product::*` modules own persisted stores and product IO used by
  workflow and surfaces.
- `tools::host::*` modules own OS, Git, process, provider, browser, Docker,
  toolchain, target-app, quality, and cluster integration using model-defined
  process and ownership types.
- `tools::supervisor::*` modules own daemon authority, runtime lifecycle, operations,
  security, configuration, error translation, and testing support.
- `tools::observability::*` modules own the single abstraction for logs,
  activity, metrics, diagnostics, and support bundles. Processing modules emit
  through this abstraction; model modules do not.
- `surfaces::*` modules own entrypoints and adapters for CLI, web,
  desktop, and the daemon web server. Surface modules call supervisor-facing
  services; they do not directly mutate persisted .refine state or spawn managed
  processes.
- Avoid a generic `shared` module. Put code in the module that owns the concept.
  If a primitive is genuinely cross-cutting, give it a narrow named home inside
  the relevant container instead of creating a catch-all utility bucket.
- Test support lives under `tools::supervisor::testing` and per-capability test
  fixtures. It can depend broadly on production modules, but production modules
  cannot depend on test-only code.

### Architecture Support Modules

The following modules support the overall architecture rather than a single
product capability:

- `model`; path: `src/model/`. Owns canonical state types,
  workflow-state enums, allowed-operation rules, persisted record definitions, and
  pure validation.
- `tools::supervisor::runtime`; path: `src/tools/supervisor/runtime/`. Owns
  runtime bootstrap, OS path selection, instance identity, process startup
  context, and repo-root `run/` path resolution.
- `tools::supervisor::operations`; path: `src/tools/supervisor/operations/`. Owns the
  operation registry, operation handles, cancellation plumbing, and operation
  recovery coordination.
- `tools::supervisor::config`; path: `src/tools/supervisor/config/`. Owns
  loading, validating, and merging user, project, and runtime configuration.
- `tools::supervisor::errors`; path: `src/tools/supervisor/errors/`. Owns
  error categories and translation into daemon web-server responses and CLI
  output.
- `tools::supervisor::testing`; path: `src/tools/supervisor/testing/`. Owns
  black-box fixtures, fake supervisors, fake providers, fake process handles,
  and contract-test helpers.
- `surfaces::cli`; path: `src/surfaces/cli/`. Owns the CLI
  surface, model-oriented command tree, action-to-API adapters, and structured
  output formatting.
- `surfaces::desktop`; path: `src/surfaces/desktop/`. Owns
  desktop shell integration, native menu and tray hooks, update prompts, and
  narrow bridge command definitions used by the Tauri wrapper.
- `surfaces::web`; path: `src/surfaces/web/`. Owns the native web UI assets,
  generated client bindings, and asset packaging metadata. Static assets live
  under `src/surfaces/web/static/`.
- `surfaces::web_server`; path: `src/surfaces/web_server/`. Owns the
  local daemon web server: HTTP routes, server-sent-event streams, static asset
  serving, local-origin checks, request parsing, response shaping, and
  translation into supervisor and workflow/tools services.

`xtask/` should contain repository automation that is not part of the
shipped product: code generation, API contract export, fixture refresh, release
packaging, installer smoke tests, and migration checks.

## Daemon Web Server

The daemon should run a port-scoped local web server. This is the concrete
boundary for Desktop, browser, and CLI requests.

Responsibilities:

- Serve the web UI assets used by both the external browser and Tauri webview.
- Expose HTTP routes for request/response operations.
- Expose server-sent-event streams for operation events, logs, chat, and UI
  updates.
- Handle local-origin checks, request parsing, response shaping, and API version
  negotiation.
- Translate transport requests into supervisor-facing workflow/tools services.
- Keep route handlers thin; workflow logic, state mutation, process lifecycle,
  provider execution, Git behavior, and storage orchestration belong in
  `workflow/tools`.

The CLI should normally talk to the same daemon web server using structured
HTTP/JSON contracts. It may run limited bootstrap commands when the daemon is
not available, such as locating the checkout runtime, starting the daemon, or
printing diagnostics from local runtime files.

The local daemon web server is part of daemon lifecycle. CLI bootstrap should
use `refine system start` to create the port-scoped daemon and
`refine system start --foreground` for service-manager, test, and development
foreground execution. The CLI should not expose a separate user-facing
`system web` command for the same behavior.

The daemon API does not have to mirror the CLI command tree. API groups can be
transport- and capability-oriented where that produces cleaner contracts, while
the CLI remains model-oriented for operator ergonomics and script stability.
The CLI adapter maps model actions onto the appropriate daemon routes.

API requirements:

- Stable typed contracts.
- Module-oriented routes or methods that map clearly onto `workflow/tools` services.
- Streaming operation events.
- Idempotency keys for mutating long-running operations.
- Consistent error codes and machine-readable details.
- Version negotiation between Desktop, CLI, and daemon.

Representative API groups:

- `/system`: install state, daemon status, published-release update,
  source/dogfood check and promotion, doctor.
- `/apps`: target-app registry, attach, switch, detach, commands.
- `/work`: Goals, Features, imports, state transitions.
- `/agents`: provider configuration, auth, diagnostics.
- `/operations`: operation status, logs, cancel.
- `/processes`: managed process list and controls.
- `/quality`: checks and screenshots.
- `/chat`: sessions, messages, streaming events.
- `/settings`: project and runtime settings.

## Storage Model

Storage should distinguish:

- Refine install state.
- Daemon runtime state.
- Known target-app registry.
- Active target-app project state.
- Refine workflow state inside or alongside the target app.
- Durable chat session and transcript records.
- Index/cache state.
- Logs and telemetry.

Durable workflow state should be portable and Git-friendly where the current
Refine model requires collaboration. Runtime state should remain local and
should not dirty the target app.

The Rust implementation should start by preserving the current Python
application's storage split and path behavior. Use the Python application as
the source of truth for which records remain collaboration-visible and which
records are local runtime state. Any change to that split should be documented
as an intentional Rust architecture decision.

Durable records should deserialize into `model` types. Storage code may own
file layout, atomic writes, materialized projections, cache snapshots, and
cache rebuilds, but it should not define a second set of workflow structs or
status enums. Projection records mirror model records and must be rebuildable
from them.

The Rust first pass should include a persisted projection snapshot, not only an
in-memory startup scan. The snapshot is an implementation detail behind the
projection API; surfaces do not know whether the backend uses in-memory indexes,
cache files, or another cache structure. The daemon should:

- Load the projection snapshot from the selected local runtime root's
  `<port>/cache/` directory during startup.
- Validate the snapshot version and source-file fingerprints for persisted
  records such as `goal.json` and `feature.json`.
- Rescan only changed, missing, or newly discovered persisted records.
- Build in-memory indexes from the validated projection for nearly instant UI
  counts, facets, filters, sorts, and lookups.
- Update persisted records first on every mutation, then update the in-memory
  projection and persisted snapshot for responsiveness.
- Emit SSE updates after projection changes so surfaces can refresh without
  rescanning.
- Fall back to a full persisted-record scan when the snapshot is absent,
  incompatible, or corrupt.

This projection layer is the required cache abstraction. Common UI queries
such as "how many Goals are done?" must be answered by the projection API, not
by each surface scanning persisted records or inventing its own query path.

The first Rust cache choice is therefore:

- Durable model records remain JSON/source records.
- The local runtime root's `<port>/cache/` directory stores a versioned
  projection snapshot with persisted source fingerprints.
- The daemon loads the snapshot, incrementally refreshes changed records, and
  builds in-memory indexes.
- The projection API is the only supported query abstraction for surfaces.
- The first pass does not use a query database. It uses persisted model records,
  projection snapshots, and in-memory indexes.
- Projection updates do not need to provide database-style ACID semantics.
  Durable records remain the source of truth; projection corruption, partial
  snapshot writes, or missed cache updates are recovered by discarding the
  snapshot and rebuilding from persisted records.

### Required Projection Indexes

The current Python web UI has been scanned for cache needs. The Rust projection
layer must support at least the query patterns used by the Goals, Features,
Dashboard, Logs, Changes, Goal Detail, and bulk-operation surfaces.

Goal projection:

- Keep `GoalSummaryProjection` keyed by `goal_id` with `id`, `name`, `status`,
  `priority`, `reporter`, `round_count`, `created`, `updated`, `branch_name`,
  `node_id`, `feature_id`, `feature_order`, persisted JSON path, and display
  fields such as `node_display_name`.
- Index by status for workflow counts and status filters.
- Index by node, including current-node, all-node, and unknown-node queries.
- Index by reporter, priority, feature id, standalone/no-feature membership,
  and round count range.
- Maintain sorted views for `name`, `status`, `priority`, `reporter`,
  `round_count`, `node`, `updated`, `created`, and `id`, with stable
  tie-breakers.
- Maintain a text-search projection over Goal name, reporter, round content,
  and notes content.
- Maintain activity-linked Goal membership by severity, category, and actor so
  Goal list filters can include log/activity dimensions.
- Return stable matching Goal id sets for bulk update, transfer, feature
  assignment, and bulk delete operations across pagination.
- Return filtered status counts so workflow visualizations can reflect the
  current filter set without a second persisted scan.

Feature projection:

- Keep `FeatureSummaryProjection` keyed by `feature_id` with `id`, `name`,
  `description`, `reporter`, `node_id`, `created`, `updated`, and persisted JSON
  path.
- Index by node, reporter, derived feature status, updated, created, name, and
  id.
- Maintain ordered Goal ids per Feature by `feature_order`, with deterministic
  fallback ordering.
- Maintain derived rollups per Feature: `status`, `goal_count`, `done_count`,
  `active_count`, `failed_count`, `cancelled_count`, `blocked_count`, and
  `next_goal`.
- Maintain the current-node standalone Goal candidate set for assigning Goals to
  Features.

Activity and log projection:

- Keep activity entries keyed by activity id and indexed by datetime/id,
  severity, category, actor, and `goal_id`.
- Maintain distinct category, actor, and severity facet values.
- Maintain text search over activity message and details.
- Maintain the recent activity feed used by Dashboard.
- Maintain per-Goal and per-round log summaries: count, latest log, latest
  error log, latest state log, latest workflow log, and paged log entries.
- Support activity table sorting by datetime, severity, category, actor,
  `goal_id`, message, and id.

Change projection:

- Keep Refine merge rows keyed by commit and branch with commit sha, committed
  time, subject, `goal_id`, branch, and joined Goal display fields.
- Index by branch plus committed time, `goal_id`, Goal status, Goal priority, and
  text search over Goal name, commit, status, and subject.

Dashboard projection:

- Derive dashboard status counts from Goal indexes for all nodes and for the
  current node.
- Derive reporter stats grouped by reporter and status.
- Derive attention indicators from failed Goal counts, preflight state, runner
  reachability, and relevant runtime state.
- Reuse the recent activity projection instead of reading logs separately.

Runtime projection:

- Keep supervisor, process, background-operation, target-app, performance, and
  preflight snapshots in local runtime cache state rather than Git-visible
  persisted model records.
- These runtime projections still need indexed lookup and pagination where the
  UI asks for process, performance, or operation tables, but they are not part of the
  portable workflow model.

Project, settings, and source-tree caches:

- Keep project registry, active-project status, node registry, reporters,
  guidance, governance, quality, and application settings as small keyed maps
  with typed lookups. The current UI needs quick list and lookup behavior for
  these, not heavy secondary indexes.
- Keep file tree, file read, and file search caches separate from workflow
  projections. They describe the target application's source tree, not Refine
  workflow state, and should be invalidated by filesystem fingerprints or Git
  metadata.
- Keep in-flight chat turns, provider processes, and import preparation operations in
  runtime/operation state. Durable chat records preserve session metadata,
  transcripts, provider resume ids, and interrupted/closed status, but runtime
  process handles and transient progress buffers remain local runtime state.
- Keep cache rebuild, cleanup, and projection diagnostics observable through
  logs and performance metrics so the UI can explain slow startup or rebuild
  work.

Rust should preserve the current logical runtime-state convention while allowing
the physical root to vary by deployment:

- `primary.json`: primary local runtime record.
- `<port>/`: port-scoped daemon, process, socket, and UI runtime state.
- `<port>/cache/`: port-scoped caches, projection snapshots, and any rebuilt
  query indexes.

The runtime root is Refine-owned runtime bookkeeping. It may be checkout-local
for development or OS-local for an installed product, but it must stay outside
tracked target-app files and should not be written into a target app's tracked
`.gitignore`. If a target app needs an ignore rule for Refine runtime
bookkeeping, use repo-local Git exclude behavior rather than mutating tracked
application files.

All storage paths must be OS-specific and documented:

- macOS: app support, logs, cache, launchd plist locations.
- Windows: `%LOCALAPPDATA%`, `%APPDATA%`, service/task metadata, logs.
- Linux: XDG paths for user installs and systemd paths for service installs.

## Dependency Strategy

Refine itself should be native and should not require host Python.

Dependency classes:

- Required Rust product dependencies: bundled with Refine or implemented in Rust.
- Cache/query dependencies: use the projection snapshot and in-memory indexes
  described in the Storage Model. Do not add a query database dependency for
  workflow list, count, facet, search, sort, or lookup behavior.
- External prerequisites: Git is likely required for most useful workflows and
  should be detected early, but it is not bundled into the desktop app.
- Workflow prerequisites: browser automation dependencies, provider CLIs,
  language toolchains, package managers, Docker, Node/npm, and other target
  tools are installed by the user or target environment, not bundled into the
  desktop app.
- Provider dependencies: either provider CLI, provider API credentials, or both,
  depending on the configured adapter.

Dependency checks should be capability-scoped. Missing Docker should not block
Goal creation. Missing provider auth should block agent execution but not project
inspection. Missing browser automation should block browser QA but not ordinary
chat. `doctor`, first-run setup, and workflow preflight should report missing
prerequisites clearly instead of hiding or bundling them.

## Migration And Port Strategy

Port vertically by workflow, not horizontally by old Python module.

Suggested order:

1. Install, daemon lifecycle, status, doctor.
2. Target-app registry: attach, switch, detach, no-app mode.
3. Durable storage, projection snapshots, and projection rebuild.
4. Goal list/create/update/transition.
5. Model-oriented CLI coverage for the above.
6. Web UI connected to Rust APIs.
7. Desktop shell over the same APIs.
8. Agent provider detection and one provider execution path.
9. Worktree execution, round logs, review, and merge.
10. Feature support, import flows, chat, quality, cluster, and advanced
    diagnostics.

The current implementation should act as a behavior oracle during the port.
Where behavior changes intentionally, document the change as a Rust architecture
decision instead of preserving accidental compatibility.

Desktop-first onboarding and broader web UI redesign are not part of this
architecture decision. Reuse the existing web UI where useful during the port;
redesign can be handled later as a separate product/design pass.

## Testing Strategy

Testing should verify capabilities through public surfaces.
Parity matters: when a product capability is implemented in `workflow/tools`, tests
should prove it is reachable through web, desktop, and CLI surfaces unless a
documented product decision says otherwise.

Required layers:

- Unit tests for `model` state transitions, allowed-operation rules, and
  validators.
- Integration tests for storage, migrations, Git, process supervision, and
  provider adapters.
- Contract tests for daemon web-server request/response and event contracts.
- CLI tests with JSON output checks for the model-oriented command groups.
- Web/Desktop UI smoke tests through the shared API.
- Surface parity tests that verify workflow/tools capabilities are available through the
  CLI, browser UI, and Desktop webview.
- Black-box compatibility tests that run representative workflows against the
  Rust implementation.
- OS matrix tests for macOS, Windows, and Linux process/service behavior.

The black-box harness should treat Refine as a product surface, not as internal
modules. It should exercise UI and CLI behavior and avoid depending on private
implementation details.

## Resolved Decisions

- Runtime authority is port-scoped, matching the current Python application.
- Provider integrations should support both direct provider APIs and provider
  CLIs.
- Git-visible collaboration state and local runtime state should follow the
  current Python application's storage split unless a Rust architecture decision
  explicitly changes it.
- Web UI reuse versus redesign is intentionally deferred. Redesign work can be
  handled later as needed.
- The desktop bundle should not include Git, browser automation dependencies,
  provider-specific helper binaries, or target workflow toolchains. These are
  prerequisites that Refine detects, diagnoses, and reports.
- No minimum Rust MVP is required for this document. The implementation can be
  driven to completion workflow by workflow.

## Acceptance Criteria

- `docs/spec/rust-spec.md` defines web, desktop, and CLI surfaces over one
  supervisor daemon.
- The architecture is organized around system capabilities.
- The daemon is the sole local authority for process lifecycle and persisted
  workflow mutations.
- Refine itself has no required host Python dependency in the target Rust
  architecture.
- Target-app and provider dependencies are treated as explicit, scoped
  integrations.
- Business logic is shared across surfaces.
- Web, desktop, and CLI surfaces are expected to reach complete feature parity
  through shared `workflow/tools` modules.
- The Rust project contains its own web UI asset tree under
  `src/surfaces/web/static/`, so it does not depend on Python-owned static
  files.
- The document includes migration and testing strategy for a vertical port.
- The document defines `model` as the centralized model module for
  canonical state, workflow states, and allowed operations.
- The document defines the Rust cache choice as persisted model records plus a
  persisted projection snapshot under the selected local runtime root's
  `<port>/cache/` directory and in-memory indexes for the current UI's list,
  count, facet, search, sort, and lookup needs.
- The document records the resolved daemon, provider, storage, dependency, UI,
  and implementation-scope decisions.
- The document defines a workflow/tools Rust package at the repository root, leaves room
  for a thin Tauri wrapper package, assigns capabilities to modules and
  directory paths inside the concrete `tools::*` containers, and preserves the
  current logical runtime-state shape while allowing the physical runtime root
  to vary between checkout and installed deployments.
