# Rust Architecture Spec

## Summary

Define a Rust-native Refine architecture with three user-facing surfaces:
**web**, **desktop**, and **CLI**. All surfaces talk to one local
supervisor daemon that owns host integration, process lifecycle, project
attachment, target-app operations, agent execution, and durable Refine state.
The reason to centralize real work in `core` is complete feature parity:
Desktop, browser, and CLI should expose the same product capabilities through
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
  product behavior through shared `core` modules.
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
capabilities should be treated as implementation gaps unless explicitly
documented as product decisions. This is the main reason `core` exists:
workflow semantics, state mutation, process lifecycle, provider execution, and
storage orchestration are implemented once and exposed through every surface.

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
Business logic belongs in Rust core services, not in frontend-only code. The
existing fully baked UI under `python/refine_ui/static/` should be copied into
the Rust project so native Refine is self-contained. During the port, that copy
is the Rust-owned web UI asset tree; once the Rust port is complete, the
`python/` directory can be deprecated without leaving the Rust product
dependent on Python-owned static files.

### CLI

CLI is the operator and automation surface.

Responsibilities:

- Start, stop, restart, status, doctor, update, and logs.
- Attach, switch, detach, and inspect target apps.
- Create, list, update, import, schedule, review, and merge Refine work.
- Emit human-readable output and structured JSON for automation.
- Call the same daemon APIs and core operations as the UI surfaces.

The CLI may also run limited bootstrap commands when the daemon is not yet
installed, but normal operation should go through the daemon.

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
     auth, request parsing, response shaping
                      |
                    core
   supervisor, product workflow, host adapters,
   jobs, storage orchestration, observability
                      |
                    model
       canonical records, workflow states,
       allowed operations, process kinds
                      |
 durable project state + run/<port>/ runtime state
```

The supervisor daemon is the single local authority. It contains the local web
server that serves UI assets and exposes the HTTP and server-sent-event routes
used by Desktop, browser, and CLI surfaces. Route
handlers are transport adapters: they handle HTTP concerns, translate requests
and responses, and call `core` for real work. Surfaces do not directly mutate
durable state or own long-lived OS processes.

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
Gap execution workflow may use work-item, scheduling, provider, process, Git,
storage, event, and log abstractions, but those abstractions remain centralized
instead of being rebuilt inside the Gap execution code.

## Model

Module: `model`; path: `rust/src/model/`.

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

- Canonical product models: Project, Gap, Feature, Workflow, Cluster, Node, and
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
  OS process behavior belongs under `core::host::process_supervision`; event
  delivery belongs under the daemon web server and observability; persisted
  record shapes live beside the product model they describe.

### Model Modules

The initial Rust model modules should mirror the product concepts already
visible in the Python implementation:

```text
rust/src/model/
  project/
  gap/
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

Module: `model::project`; path: `rust/src/model/project/`.

Owns the canonical shape of a Refine target-app attachment and project state.
This covers both durable `.refine` state in the target app and port-scoped
runtime selection state under the Refine checkout's `run/` directory.

Properties:

- `ProjectConfig`: `schema_version`, `refine.version`, `created_at`,
  `updated_at`, and project-level `settings`.
- `ProjectStatus`: `attached`, `registry_enabled`, `client_repo`,
  `volume_root`, `config_path`, `schema`, `maintenance`, `apps`,
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
  behaviors belong in `core::product::project_registry`,
  `core::product::project_state`, and `core::supervisor::runtime`.

### Gap Model

Module: `model::gap`; path: `rust/src/model/gap/`.

Owns the canonical Gap, round, note, and Gap-owned quality/governance state.
The Python baseline stores durable Gap state in `gap.json` and projects common
fields into `gaps_index` as a rebuildable cache.

Properties:

- `Gap`: `id`, `name`, `status`, `priority`, `branch_name`, `feature_id`,
  `feature_order`, `node_id`, `created`, `updated`, `notes`, and `rounds`.
- `GapIndexProjection`: `id`, `name`, `status`, `priority`, `reporter`,
  `round_count`, `created`, `updated`, `branch_name`, `node_id`,
  `feature_id`, `feature_order`, and `json_path`.
- `GapNote`: `id`, `author`, `body`, `created`, and `updated`.
- `GapRound`: `reporter`, `actual`, `target`, `created`, `updated`, optional
  `guidance_decision`, and derived `logs` when a response hydrates sidecar
  round logs.
- `RoundGovernance`: `rule_state`, `meta_rule_state`, `product_state`,
  `constitution_state`, `governance_message`, `governance_details`,
  `governance_checked_at`, and `governance_rule_actions`.
- `RoundQuality`: `quality_state`, `quality_message`, `quality_details`, and
  `quality_checked_at`.
- `GapPriority`: `low`, `medium`, or `high`.

Rules:

- `status` must use `model::workflow::GapStatus`.
- `feature_id` and `feature_order` associate a Gap with a Feature; the ordered
  Feature membership is stored on the Gap.
- Round logs are represented by `model::log` entries. Storage may keep them in
  a sidecar JSONL file, but the model shape is shared with activity logs.

### Feature Model

Module: `model::feature`; path: `rust/src/model/feature/`.

Owns Feature metadata and the derived rollup produced from ordered Gaps.
The Python baseline stores durable Feature metadata in `feature.json` and
projects common fields into `features_index`.

Properties:

- `Feature`: `id`, `name`, `description`, `reporter`, `node_id`, `created`,
  `updated`, and `json_path`.
- `FeatureIndexProjection`: `id`, `name`, `description`, `reporter`,
  `node_id`, `created`, `updated`, and `json_path`.
- `FeatureDetail`: `Feature` plus `gaps`, `node_display_name`, and rollup
  fields.
- `FeatureRollup`: `status`, `gap_count`, `done_count`, `active_count`,
  `failed_count`, `cancelled_count`, `blocked_count`, and `next_gap`.

Rules:

- A Feature's workflow status is derived from its ordered Gaps; it is not an
  independent durable field in the Python baseline.
- Feature workflow actions can move eligible Gaps to `backlog` or `todo`.
  Protected Gap statuses are `review`, `done`, `ready-merge`, and
  `awaiting-rebuild`.
- Cancelling a Feature cancels non-terminal Gaps where allowed and skips
  already terminal or ineligible Gaps.

### Workflow Model

Module: `model::workflow`; path: `rust/src/model/workflow/`.

Owns workflow states, transition rules, and allowed-operation decisions for
Gap and Feature workflows.

Properties:

- `GapStatus`: `backlog`, `todo`, `in-progress`, `qa`, `ready-merge`,
  `awaiting-rebuild`, `review`, `done`, `failed`, and `cancelled`.
- `TerminalGapStatus`: `done` and `cancelled`.
- `AutomatedGapStatus`: `in-progress`, `qa`, `ready-merge`, and
  `awaiting-rebuild`.
- `UserStatusTransition`: currently `backlog -> todo`, `todo -> backlog`,
  `review -> todo`, `done -> review`, `failed -> todo`, and
  `cancelled -> todo`; same-status updates are no-ops.
- `BulkStatusTarget`: any Gap status except `in-progress`, `qa`, and
  `ready-merge`, plus the special `__last_workflow_state` restore operation.
- `FeatureWorkflowTarget`: `backlog` or `todo`.
- `FeatureProtectedStatus`: `review`, `done`, `ready-merge`, and
  `awaiting-rebuild`.

Rules:

- Manual status updates cannot enter system-owned states through ordinary
  metadata edits; dedicated workflow actions own those transitions.
- The model should answer whether an operation is allowed for a given Gap or
  Feature state. `core` performs the operation only after the model approves
  it.
- Examples of operations the model should name: create Gap, edit Gap metadata,
  edit notes, submit new round, edit latest round, start implementation,
  cancel automation, retry agent, retry QA, retry merge, verify/review, merge,
  undo, delete, assign to Feature, remove from Feature, reorder in Feature,
  move Feature workflow, and cancel Feature.

### Cluster Model

Module: `model::cluster`; path: `rust/src/model/cluster/`.

Owns the git-synced cluster registry shape and cluster-level state. Runtime SSH
execution belongs in `core::host::cluster`; the model only defines the records
and validation vocabulary.

Properties:

- `Cluster`: `nodes` and `updated_at`.
- `ClusterNode`: `id`, `display_name`, `ssh_host`, `ssh_port`,
  `refine_checkout`, `target_app_path`, `refine_port`, `enabled`, `health`,
  `created_at`, and `updated_at`.
- `ClusterHealth`: at minimum `status` and `checked_at`, with room for
  provider-specific details.
- `RemoteRunResult`: `node_id`, `command`, `remote_command`, `exit_code`,
  `stdout`, `stderr`, and `ok`.

Rules:

- Cluster node ids are lowercase ids that start with a letter or digit and use
  only lowercase letters, digits, `_`, or `-`.
- `ssh_host` must be a host, not a `user@host` string.
- Disabled cluster nodes remain registered but cannot run remote Refine
  commands.

### Node Model

Module: `model::node`; path: `rust/src/model/node/`.

Owns local Refine node identity and node-scoped settings. A node is the unit of
ownership for Gaps and Features inside one target app; a cluster node is the
remote-registration form of a node.

Properties:

- `Node`: `id`, `display_name`, `created_at`, `updated_at`, and `archived`.
- `NodeRegistry`: `nodes`.
- `ActiveNodeSelection`: `active_node_id`, `volume_root`, and `updated_at`.
- `NodeSettings`: application, runtime, target-app config, and target-app
  runtime setting maps scoped to a node.
- `NodeOwnership`: the `node_id` fields on Gap and Feature records.

Rules:

- The active node cannot be archived.
- Runtime automation owns exactly one local node for the lifetime of the
  supervisor/worker process, even if the UI browses another node.
- Mutating a Gap or Feature requires ownership by the active local node unless
  a transfer operation explicitly changes ownership first.

### Log Model

Module: `model::log`; path: `rust/src/model/log/`.

Owns the canonical log and activity entry shape. Writing, retention, indexing,
streaming, and support-bundle export belong in `core::observability`.

Properties:

- `LogEntry`: `datetime`, `severity`, `category`, `message`, optional
  `details`, optional `actions`, optional `actor`, and optional `gap_id`.
- `ActivityEntry`: `id`, `datetime`, `severity`, `category`, `message`,
  optional `gap_id`, optional `actor`, optional `details`, and optional
  `actions`.
- `RoundLogEntry`: `LogEntry` plus `round_idx` when stored in sidecar JSONL.
- `LogAction`: action objects attached to an entry; the exact variants should
  become typed as Rust implementations replace Python's free-form dictionaries.
- `LogQuery`: `limit`, `offset`, `gap_id`, `since_id`, `severity`,
  `category`, `actor`, `q`, `sort`, and `direction`.

Rules:

- Activity entries and round logs share the same entry shape.
- Workflow transitions should be representable as log entries with
  `category = "state"` and messages that can identify the previous and next
  workflow state.
- `model::log` defines data; `core::observability` owns append, search,
  cleanup, metrics, streaming, diagnostics, and support bundles.
- Storage serializes and deserializes model records; it does not invent shadow
  model definitions.
- UI, CLI, API, scheduler, process, provider, and quality code should all use
  the same model types for Gap state and allowed operations.
- If a workflow rule can be expressed as data or a pure state transition, it
  belongs in `model`, not in a runner, route handler, or button callback.

## Core Capabilities

### Installation And Update

Module: `core::host::installation`; path: `rust/src/core/host/installation/`.

Owns abstractions for: install, repair, update, rollback, uninstall.

Requirements:

- Install Refine without requiring host Python.
- Register the daemon with the host OS where appropriate.
- Keep Desktop app updates and daemon/core updates coherent.
- Detect and report stale, partial, or conflicting installs.
- Support rollback when an update fails before state migration completes.
- Preserve user data and target-app state across upgrades.

OS backends:

- macOS: signed app bundle, notarization, launchd or Login Item integration,
  keychain where appropriate.
- Windows: signed installer, Start Menu integration, background service or
  user-session daemon strategy, Windows Credential Manager where appropriate.
- Linux: CLI/web install path, systemd where available, best-effort process
  mode otherwise.

### Daemon Lifecycle

Module: `core::supervisor::lifecycle`; path: `rust/src/core/supervisor/lifecycle/`.

Owns abstractions for: start, stop, restart, status, health, recover.

Requirements:

- One daemon owns one port-scoped local Refine runtime authority, matching the
  current Python runtime model.
- The daemon exposes local authenticated web-server routes for surfaces.
- Status distinguishes daemon health, web availability, worker state, target-app
  state, active operations, and degraded integrations.
- Restart preserves attached app selection and running operation records.
- Crash recovery reconciles durable state with OS process reality.
- Stop terminates or detaches managed processes according to their ownership
  policy.

### Surface Session

Module: `core::supervisor::sessions`; path: `rust/src/core/supervisor/sessions/`.

Owns abstractions for: open UI, authenticate local surface, stream state, deliver
notifications.

Requirements:

- Desktop, browser, and CLI use a shared local auth model.
- The daemon should not expose unauthenticated mutation APIs on a public
  interface.
- Web UI can stream activity, process output, job progress, and chat events.
- Desktop can subscribe to events for badges, tray state, and notifications.

### Project Registry

Module: `core::product::project_registry`; path: `rust/src/core/product/project_registry/`.

Owns abstractions for: register, attach, switch, detach, clone, remove, inspect.

Requirements:

- Refine tracks known target apps separately from the active app.
- App switching is a supervisor transaction.
- Runtime bookkeeping must not dirty tracked target-app files.
- Target app identity, path, Git root, remote, and health are explicit.
- Detached/no-app mode is supported.
- Switching should prepare or migrate the target app before making it active.

### Project State

Module: `core::product::project_state`; path: `rust/src/core/product/project_state/`.

Owns abstractions for: initialize, read, mutate, migrate, sync, rebuild
projections.

Requirements:

- Durable Refine workflow state lives in project-local storage.
- Runtime state lives outside tracked target-app files.
- Rust should maintain a materialized projection of model records for fast
  list, count, filter, sort, and lookup queries.
- The first pass should persist the projection snapshot under
  `run/<port>/cache/`, including source-file fingerprints, so startup can load
  the snapshot and rescan only changed, missing, or new durable records.
- The projection snapshot and required indexes are part of
  `core::product::project_state`; routes should ask core for query results
  instead of rebuilding ad hoc indexes per surface.
- A full durable-record scan remains the fallback when the projection snapshot
  is missing, corrupt, incompatible, or intentionally discarded.
- Durable records remain the source of truth. Projection state must be
  rebuildable after corruption or version upgrades.
- Mutations flow through shared core services and emit audit/activity events.
- Migrations are versioned, idempotent, and observable.

### Work Items

Module: `core::product::work_items`; path: `rust/src/core/product/work_items/`.

Owns abstractions for: create, import, deduplicate, list, update, transition, cancel,
delete, assign, reorder.

Requirements:

- Gap remains the executable unit of work.
- Feature remains an optional ordered group of Gaps.
- Imports support AI extraction, CSV paste, CSV file, and structured review.
- UI and CLI call shared work-item operations.
- State transitions are validated centrally.
- Node or machine ownership is enforced before mutation or scheduling.

### Scheduling And Execution

Module: `core::product::scheduling`; path: `rust/src/core/product/scheduling/`.

Owns abstractions for: promote, reserve, dispatch, pause, resume, cancel, retry.

Requirements:

- Scheduler chooses eligible work from durable state.
- Feature ordering is respected.
- Global, per-node, per-provider, and per-target-app concurrency limits are
  enforced centrally.
- Execution reservations survive daemon restart or are reconciled safely.
- Manual user controls can pause agents, target-app processes, specific jobs,
  or all automation.

### Agent Providers

Module: `core::host::agent_providers`; path: `rust/src/core/host/agent_providers/`.

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

Module: `core::host::process_supervision`; path: `rust/src/core/host/process_supervision/`.

Owns abstractions for: launch, signal, wait, stream, inspect, limit, clean up.

Requirements:

- The daemon owns all managed OS process lifecycle.
- Surfaces never launch or kill managed target-app, agent, rebuild, test, or
  helper processes directly.
- Managed processes have typed ownership: daemon, target app, agent, quality,
  import, maintenance, or user-initiated helper.
- Process groups, child cleanup, stdout/stderr streaming, stdin, exit status,
  resource limits, and cancellation are modeled explicitly.
- Resource isolation is capability-detected by OS backend.

OS backends should cover:

- systemd and cgroups on Linux where available.
- launchd, process groups, and best-effort resource controls on macOS.
- Windows Job Objects, services, console control events, and process groups on
  Windows.

### Target App Operations

Module: `core::host::target_apps`; path: `rust/src/core/host/target_apps/`.

Owns abstractions for: configure, start, stop, restart, status, rebuild, open, diagnose.

Requirements:

- Target-app commands are configured per app.
- The daemon runs configured commands in the target app's environment.
- Long-running target-app processes are supervised and visible.
- Rebuild and status checks produce structured results.
- Target-app failures do not crash the daemon or block unrelated Refine
  operations.

### Git And Worktrees

Module: `core::host::git_worktrees`; path: `rust/src/core/host/git_worktrees/`.

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

Module: `core::host::quality`; path: `rust/src/core/host/quality/`.

Owns abstractions for: run checks, browser QA, regressions, screenshots, compare, gate.

Requirements:

- Quality checks are jobs supervised by the daemon.
- Browser automation dependencies are discovered and repaired separately from
  Refine's own runtime dependencies.
- Results are persisted, visible, and tied to the relevant Gap, Feature, or app.
- Users can rerun, cancel, and inspect quality jobs from any surface.

### Chat And Planning

Module: `core::product::chat`; path: `rust/src/core/product/chat/`.

Owns abstractions for: start, resume, stream, attach to Gap or Feature, persist context.

Requirements:

- Chat sessions use shared provider adapters.
- Gap-attached chat and standalone chat have explicit storage and resumption
  semantics.
- Long-running provider priming or resume steps are observable.
- Chat events can produce importable rounds, Gaps, or Feature plans.

### Observability And Diagnostics

Module: `core::observability`; path: `rust/src/core/observability/`.

Owns abstractions for: activity, logs, metrics, doctor, support bundle.

Requirements:

- Every capability emits structured events.
- System operations have status, progress, timestamps, errors, and owning
  surface.
- Logs are queryable from UI and CLI.
- `doctor` reports daemon, install, OS backend, target app, Git, provider,
  browser, Docker, and storage health.
- Support bundles redact secrets by default.

### Security And Permissions

Module: `core::supervisor::security`; path: `rust/src/core/supervisor/security/`.

Owns abstractions for: local auth, secret storage, command authorization, audit.

Requirements:

- Local mutation APIs require an authorization token or equivalent local trust
  mechanism.
- Desktop should store tokens and provider-related local secrets in OS-native
  secret storage when Refine owns them.
- Command execution must pass through explicit capability APIs.
- Surfaces should not expose arbitrary shell execution as a generic primitive.
- Sensitive paths, environment variables, and tokens are redacted in logs.

### Cluster And Multi-Node

Module: `core::host::cluster`; path: `rust/src/core/host/cluster/`.

Owns abstractions for: node registry, transfer, sync, remote command, ownership.

Requirements:

- Local daemon identity is explicit.
- Work ownership is enforced before scheduling or mutation.
- Project-state sync is a shared core operation.
- Remote execution and cluster maintenance have bounded, visible operations.
- The Rust architecture should preserve the distinction between UI selection
  and runtime ownership.

## Component Architecture

The Rust implementation should live entirely under `rust/`. That directory is
the Rust project root for native Refine. The current Python implementation
lives under `python/`, and the two implementations should remain side-by-side
during the port instead of interleaving Rust modules with Python packages at the
repository root.

Start with one core product Cargo package under `rust/`. Use Rust modules for
namespaces, code ownership, service traits, and abstraction boundaries. The Rust
project may become a small Cargo workspace to host a thin desktop Tauri wrapper,
but capabilities should not be split into separate packages. The core product
remains one package; the desktop package exists only for native shell packaging
and Tauri integration.

Suggested repository layout:

```text
refine/
  docs/
  python/
  run/
    primary.json
    <port>/
      cache/
  rust/
    Cargo.toml
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
      core/
        product/
          project_registry/
          project_state/
          work_items/
          scheduling/
          chat/
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
          sessions/
          jobs/
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
        gap/
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

`python/` remains the current implementation and behavior oracle during the
port. The existing UI assets in `python/refine_ui/static/` should be copied
into `rust/src/surfaces/web/static/` so the Rust project has its own complete
UI asset tree. New core product code should live under `rust/src/`, and
repository automation should live under `rust/xtask/`, so the native
architecture is explicit and contained within the Rust project.
If Tauri requires a separate package, it should live under
`rust/desktop/src-tauri/` and depend on the core product package. It should not
own capabilities, durable state rules, process lifecycle, provider behavior, or
workflow logic.

`run/` is a repository-root runtime directory, not part of the Rust source
tree. It should be gitignored and retained as the local runtime-state home used
by the current Python implementation: `run/primary.json` for the primary local
runtime record and `run/<port>/cache/` for port-scoped caches such as
projection snapshots and rebuilt query indexes. Rust should keep this
convention unless a migration explicitly
documents a replacement.

### Module Direction

The architecture is conceptual: the repository already separates runtimes
through `python/` and `rust/`, and the Rust package should focus on the modules
it directly implements. Rust modules should follow a one-way dependency graph:

```text
surfaces
  surfaces::{cli, desktop, web, web_server}
      |
core
  core::supervisor::{lifecycle, sessions, jobs, security, runtime, config}
  core::product::{work_items, scheduling, project_state, ...}
  core::host::{process_supervision, target_apps, git_worktrees, ...}
      |
model
  model::{project, gap, feature, workflow, cluster, node, log}

side channel for all processing layers
observability
  core::observability::{logs, activity, metrics, diagnostics}
```

Rules:

- `model::*` modules own canonical data types, persisted records,
  workflow states, operation rules, and pure validation. They should not depend
  on processing, surface, host, supervisor, or observability modules.
- `core::product::*` modules own Refine domain behavior and durable workflow
  semantics built on `model`.
- `core::host::*` modules own OS, Git, process, provider, browser, Docker,
  toolchain, target-app, quality, and cluster integration using model-defined
  process and ownership types.
- `core::supervisor::*` modules own daemon authority, runtime lifecycle,
  sessions, jobs, security, configuration, error translation, and testing
  support.
- `core::observability::*` modules own the single abstraction for logs,
  activity, metrics, diagnostics, and support bundles. Processing modules emit
  through this abstraction; model modules do not.
- `surfaces::*` modules own entrypoints and adapters for CLI, web,
  desktop, and the daemon web server. Surface modules call supervisor-facing
  services; they do not directly mutate durable Refine state or spawn managed
  processes.
- Avoid a generic `shared` module. Put code in the module that owns the concept.
  If a primitive is genuinely cross-cutting, give it a narrow named home inside
  the relevant container instead of creating a catch-all utility bucket.
- Test support lives under `core::supervisor::testing` and per-capability test
  fixtures. It can depend broadly on production modules, but production modules
  cannot depend on test-only code.

### Architecture Support Modules

The following modules support the overall architecture rather than a single
product capability:

- `model`; path: `rust/src/model/`. Owns canonical state types,
  workflow-state enums, allowed-operation rules, persisted record definitions, and
  pure validation.
- `core::supervisor::runtime`; path: `rust/src/core/supervisor/runtime/`. Owns
  runtime bootstrap, OS path selection, instance identity, process startup
  context, and repo-root `run/` path resolution.
- `core::supervisor::jobs`; path: `rust/src/core/supervisor/jobs/`. Owns the
  job registry, operation handles, cancellation plumbing, and operation
  recovery coordination.
- `core::supervisor::config`; path: `rust/src/core/supervisor/config/`. Owns
  loading, validating, and merging user, project, and runtime configuration.
- `core::supervisor::errors`; path: `rust/src/core/supervisor/errors/`. Owns
  error categories and translation into daemon web-server responses and CLI
  output.
- `core::supervisor::testing`; path: `rust/src/core/supervisor/testing/`. Owns
  black-box fixtures, fake supervisors, fake providers, fake process handles,
  and contract-test helpers.
- `surfaces::cli`; path: `rust/src/surfaces/cli/`. Owns the CLI
  surface and structured output formatting.
- `surfaces::desktop`; path: `rust/src/surfaces/desktop/`. Owns
  desktop shell integration, native menu and tray hooks, update prompts, and
  narrow bridge command definitions used by the Tauri wrapper.
- `surfaces::web`; path: `rust/src/surfaces/web/`. Owns the Rust copy of the
  web UI assets, generated client bindings, and asset packaging metadata.
  Initial assets should come from `python/refine_ui/static/` and live under
  `rust/src/surfaces/web/static/`.
- `surfaces::web_server`; path: `rust/src/surfaces/web_server/`. Owns the
  local daemon web server: HTTP routes, server-sent-event streams, static asset
  serving, auth extraction, request parsing, response shaping, and translation
  into supervisor and core services.

`rust/xtask/` should contain repository automation that is not part of the
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
- Handle auth, local-origin checks, request parsing, response shaping, and API
  version negotiation.
- Translate transport requests into supervisor-facing core services.
- Keep route handlers thin; workflow logic, state mutation, process lifecycle,
  provider execution, Git behavior, and storage orchestration belong in
  `core`.

The CLI should normally talk to the same daemon web server using structured
HTTP/JSON contracts. It may run limited bootstrap commands when the daemon is
not available, such as locating the checkout runtime, starting the daemon, or
printing diagnostics from local runtime files.

API requirements:

- Stable typed contracts.
- Module-oriented routes or methods that map clearly onto `core` services.
- Streaming operation events.
- Idempotency keys for mutating long-running operations.
- Consistent error codes and machine-readable details.
- Version negotiation between Desktop, CLI, and daemon.

Representative API groups:

- `/system`: install state, daemon status, update, doctor.
- `/apps`: target-app registry, attach, switch, detach, commands.
- `/work`: Gaps, Features, imports, state transitions.
- `/agents`: provider configuration, auth, diagnostics.
- `/jobs`: operation status, logs, cancel, retry.
- `/processes`: managed process list and controls.
- `/quality`: checks, regressions, screenshots.
- `/chat`: sessions, messages, streaming events.
- `/settings`: project and runtime settings.

## Storage Model

Storage should distinguish:

- Refine install state.
- Daemon runtime state.
- Known target-app registry.
- Active target-app project state.
- Refine workflow state inside or alongside the target app.
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
in-memory startup scan. The daemon should:

- Load the projection snapshot from `run/<port>/cache/` during startup.
- Validate the snapshot version and source-file fingerprints for durable
  records such as `gap.json` and `feature.json`.
- Rescan only changed, missing, or newly discovered durable records.
- Build in-memory indexes from the validated projection for nearly instant UI
  counts, facets, filters, sorts, and lookups.
- Update durable records and projection state together on every mutation.
- Emit SSE updates after projection changes so surfaces can refresh without
  rescanning.
- Fall back to a full durable-record scan when the snapshot is absent,
  incompatible, or corrupt.

This projection layer is the required cache abstraction. Common UI queries
such as "how many Gaps are done?" must be answered by the projection API, not
by introducing a separate query database.

The first Rust cache choice is therefore:

- Durable model records remain JSON/source records.
- `run/<port>/cache/` stores a versioned projection snapshot with durable
  source fingerprints.
- The daemon loads the snapshot, incrementally refreshes changed records, and
  builds in-memory indexes.
- The projection API is the only supported query abstraction for surfaces.
- The first pass does not use a query database. It uses durable model records,
  projection snapshots, and in-memory indexes.

### Required Projection Indexes

The current Python web UI has been scanned for cache needs. The Rust projection
layer must support at least the query patterns used by the Gaps, Features,
Dashboard, Logs, Changes, Gap Detail, and bulk-operation surfaces.

Gap projection:

- Keep `GapSummaryProjection` keyed by `gap_id` with `id`, `name`, `status`,
  `priority`, `reporter`, `round_count`, `created`, `updated`, `branch_name`,
  `node_id`, `feature_id`, `feature_order`, durable JSON path, and display
  fields such as `node_display_name`.
- Index by status for workflow counts and status filters.
- Index by node, including current-node, all-node, and unknown-node queries.
- Index by reporter, priority, feature id, standalone/no-feature membership,
  and round count range.
- Maintain sorted views for `name`, `status`, `priority`, `reporter`,
  `round_count`, `node`, `updated`, `created`, and `id`, with stable
  tie-breakers.
- Maintain a text-search projection over Gap name, reporter, round content,
  and notes content.
- Maintain activity-linked Gap membership by severity, category, and actor so
  Gap list filters can include log/activity dimensions.
- Return stable matching Gap id sets for bulk update, transfer, feature
  assignment, and bulk delete operations across pagination.
- Return filtered status counts so workflow visualizations can reflect the
  current filter set without a second durable scan.

Feature projection:

- Keep `FeatureSummaryProjection` keyed by `feature_id` with `id`, `name`,
  `description`, `reporter`, `node_id`, `created`, `updated`, and durable JSON
  path.
- Index by node, reporter, derived feature status, updated, created, name, and
  id.
- Maintain ordered Gap ids per Feature by `feature_order`, with deterministic
  fallback ordering.
- Maintain derived rollups per Feature: `status`, `gap_count`, `done_count`,
  `active_count`, `failed_count`, `cancelled_count`, `blocked_count`, and
  `next_gap`.
- Maintain the current-node standalone Gap candidate set for assigning Gaps to
  Features.

Activity and log projection:

- Keep activity entries keyed by activity id and indexed by datetime/id,
  severity, category, actor, and `gap_id`.
- Maintain distinct category, actor, and severity facet values.
- Maintain text search over activity message and details.
- Maintain the recent activity feed used by Dashboard.
- Maintain per-Gap and per-round log summaries: count, latest log, latest
  error log, latest state log, latest workflow log, and paged log entries.
- Support activity table sorting by datetime, severity, category, actor,
  `gap_id`, message, and id.

Change projection:

- Keep Refine merge rows keyed by commit and branch with commit sha, committed
  time, subject, `gap_id`, branch, and joined Gap display fields.
- Index by branch plus committed time, `gap_id`, Gap status, Gap priority, and
  text search over Gap name, commit, status, and subject.

Dashboard projection:

- Derive dashboard status counts from Gap indexes for all nodes and for the
  current node.
- Derive reporter stats grouped by reporter and status.
- Derive attention indicators from failed Gap counts, preflight state, runner
  reachability, and relevant runtime state.
- Reuse the recent activity projection instead of reading logs separately.

Runtime projection:

- Keep supervisor, process, background-job, target-app, performance, and
  preflight snapshots in local runtime cache state rather than Git-visible
  durable model records.
- These runtime projections still need indexed lookup and pagination where the
  UI asks for process, performance, or job tables, but they are not part of the
  portable workflow model.

Project, settings, and source-tree caches:

- Keep project registry, active-project status, node registry, cluster registry,
  reporters, guidance, governance, quality, and application settings as small
  keyed maps with typed lookups. The current UI needs quick list and lookup
  behavior for these, not heavy secondary indexes.
- Keep file tree, file read, and file search caches separate from workflow
  projections. They describe the target application's source tree, not Refine
  workflow state, and should be invalidated by filesystem fingerprints or Git
  metadata.
- Keep chat sessions and import preparation jobs in runtime/job state. They may
  need lookup by session id or job id, but they should not become durable model
  indexes unless a product workflow explicitly requires collaboration-visible
  state.
- Keep cache rebuild, cleanup, and projection diagnostics observable through
  logs and performance metrics so the UI can explain slow startup or rebuild
  work.

The checkout root keeps a gitignored `run/` directory for local runtime state.
Rust should preserve this convention because the Python implementation already
uses it successfully:

- `run/primary.json`: primary local runtime record.
- `run/<port>/`: port-scoped daemon, process, socket, and UI runtime state.
- `run/<port>/cache/`: port-scoped caches, projection snapshots, and any
  rebuilt query indexes.

`run/` is Refine-owned runtime bookkeeping. It must stay outside tracked
target-app files and should not be written into a target app's tracked
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

- Required core dependencies: bundled with Refine or implemented in Rust.
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
Gap creation. Missing provider auth should block agent execution but not project
inspection. Missing browser automation should block browser QA but not ordinary
chat. `doctor`, first-run setup, and workflow preflight should report missing
prerequisites clearly instead of hiding or bundling them.

## Migration And Port Strategy

Port vertically by workflow, not horizontally by old Python module.

Suggested order:

1. Install, daemon lifecycle, status, doctor.
2. Target-app registry: attach, switch, detach, no-app mode.
3. Durable storage, projection snapshots, and projection rebuild.
4. Gap list/create/update/transition.
5. CLI parity for the above.
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
Parity matters: when a product capability is implemented in `core`, tests
should prove it is reachable through web, desktop, and CLI surfaces unless a
documented product decision says otherwise.

Required layers:

- Unit tests for `model` state transitions, allowed-operation rules, and
  validators.
- Integration tests for storage, migrations, Git, process supervision, and
  provider adapters.
- Contract tests for daemon web-server request/response and event contracts.
- CLI tests with JSON output checks.
- Web/Desktop UI smoke tests through the shared API.
- Surface parity tests that verify core capabilities are available through the
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
- The daemon is the sole local authority for process lifecycle and durable
  workflow mutations.
- Refine itself has no required host Python dependency in the target Rust
  architecture.
- Target-app and provider dependencies are treated as explicit, scoped
  integrations.
- Business logic is shared across surfaces.
- Web, desktop, and CLI surfaces are expected to reach complete feature parity
  through shared `core` modules.
- The Rust project contains its own copied web UI asset tree derived from
  `python/refine_ui/static/`, so Rust does not depend on Python-owned static
  files after the port is complete.
- The document includes migration and testing strategy for a vertical port.
- The document defines `model` as the centralized model module for
  canonical state, workflow states, and allowed operations.
- The document defines the Rust cache choice as durable model records plus a
  persisted projection snapshot under `run/<port>/cache/` and in-memory indexes
  for the current UI's list, count, facet, search, sort, and lookup needs.
- The document records the resolved daemon, provider, storage, dependency, UI,
  and implementation-scope decisions.
- The document defines a core Rust package under `rust/`, leaves room for a
  thin Tauri wrapper package, assigns capabilities to modules and directory
  paths inside the concrete `core::*` containers, preserves repo-root
  gitignored `run/` runtime state.
