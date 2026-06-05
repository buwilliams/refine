# Rust Architecture Spec

## Summary

Define a Rust-native Refine architecture with three user-facing surfaces:
**web**, **desktop**, and **CLI**. All surfaces talk to one local
supervisor daemon that owns host integration, process lifecycle, project
attachment, target-app operations, agent execution, and durable Refine state.

The Rust direction is not a Tauri wrapper around the current Python runtime. It
is a product architecture for a native local Refine platform. The current
Python implementation remains useful as the behavioral baseline during the
port, but the target architecture should not require host Python or `uv` for
Refine itself.

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
- Support an incremental port that can be validated workflow by workflow.

## Non-Goals

- Preserve Python module boundaries in Rust.
- Recreate the current web app as a separate product from Desktop.
- Let Tauri own Refine process lifecycle directly.
- Hide all target-app dependencies. Git, Node, Docker, browsers, provider auth,
  and language toolchains may still be necessary for specific workflows.
- Duplicate workflow logic in the web UI, desktop UI, and CLI.
- Require a local browser for the desktop surface.

## Product Surfaces

Refine has three surfaces over one local system.

### Desktop

Desktop is the default download-and-use surface for macOS and Windows.

Responsibilities:

- Install, update, launch, and stop the local Refine daemon.
- Render the shared Refine UI in a native webview.
- Provide native window, tray, menu, notification, and deep-link integration.
- Guide first-run setup: target app, provider, dependency checks, and auth.
- Surface daemon status, logs, diagnostics, and recovery actions.
- Broker only narrow native commands to the daemon. Desktop must not start
  target apps, agent processes, workers, or rebuilds itself.

Tauri is the preferred desktop shell. Tauri commands should be a thin native
bridge to daemon APIs and OS shell integration, not a parallel backend.

### Web

Web is the browser surface served by the daemon.

Responsibilities:

- Provide the full Refine product UI.
- Work locally inside Desktop's webview and in an external browser.
- Support remote or headless installs where Desktop is not present.
- Use the same HTTP, WebSocket, or server-sent-event APIs as Desktop.

The web surface should remain deployable as static assets served by the daemon.
Business logic belongs in Rust core services, not in frontend-only code.

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
Desktop UI        Web UI             CLI
   |                |                 |
   +----------------+-----------------+
                    |
           Local API / IPC boundary
                    |
          Rust supervisor daemon
                    |
    +---------------+-------------------------------+
    |               |                               |
 Core services   Host integrations              Storage
    |               |                               |
 Agents         Git / OS / process / app       Project state
 Target apps    provider / browser / Docker    Runtime state
```

The supervisor daemon is the single local authority. Surfaces request
capabilities from the daemon; they do not directly mutate durable state or own
long-lived OS processes.

## Capability Model

Every product action should map to a system capability. Capabilities are stable
contracts exposed through the daemon and reused by web, desktop, and CLI.

Each capability should define:

- Inputs and validation rules.
- Required authority and permission checks.
- Durable state mutations.
- Runtime side effects.
- Events emitted for UI updates and logs.
- Recovery behavior after crash or restart.
- Human-readable and machine-readable error shapes.

## Core Capabilities

### Installation And Update

Capability: install, repair, update, rollback, uninstall.

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

Capability: start, stop, restart, status, health, recover.

Requirements:

- One daemon owns one local Refine runtime authority.
- The daemon exposes a local authenticated API for surfaces.
- Status distinguishes daemon health, web availability, worker state, target-app
  state, active operations, and degraded integrations.
- Restart preserves attached app selection and running operation records.
- Crash recovery reconciles durable state with OS process reality.
- Stop terminates or detaches managed processes according to their ownership
  policy.

### Surface Session

Capability: open UI, authenticate local surface, stream state, deliver
notifications.

Requirements:

- Desktop, browser, and CLI use a shared local auth model.
- The daemon should not expose unauthenticated mutation APIs on a public
  interface.
- Web UI can stream activity, process output, job progress, and chat events.
- Desktop can subscribe to events for badges, tray state, and notifications.

### Project Registry

Capability: register, attach, switch, detach, clone, remove, inspect.

Requirements:

- Refine tracks known target apps separately from the active app.
- App switching is a supervisor transaction.
- Runtime bookkeeping must not dirty tracked target-app files.
- Target app identity, path, Git root, remote, and health are explicit.
- Detached/no-app mode is supported.
- Switching should prepare or migrate the target app before making it active.

### Project State

Capability: initialize, read, mutate, migrate, sync, rebuild indexes.

Requirements:

- Durable Refine workflow state lives in project-local storage.
- Runtime state lives outside tracked target-app files.
- SQLite or another index can accelerate queries, but durable records must be
  rebuildable after corruption or version upgrades.
- Mutations flow through shared core services and emit audit/activity events.
- Migrations are versioned, idempotent, and observable.

### Work Items

Capability: create, import, deduplicate, list, update, transition, cancel,
delete, assign, reorder.

Requirements:

- Gap remains the executable unit of work.
- Feature remains an optional ordered group of Gaps.
- Imports support AI extraction, CSV paste, CSV file, and structured review.
- UI and CLI call shared work-item operations.
- State transitions are validated centrally.
- Node or machine ownership is enforced before mutation or scheduling.

### Scheduling And Execution

Capability: promote, reserve, dispatch, pause, resume, cancel, retry.

Requirements:

- Scheduler chooses eligible work from durable state.
- Feature ordering is respected.
- Global, per-node, per-provider, and per-target-app concurrency limits are
  enforced centrally.
- Execution reservations survive daemon restart or are reconciled safely.
- Manual user controls can pause agents, target-app processes, specific jobs,
  or all automation.

### Agent Providers

Capability: detect, configure, authenticate, invoke, parse, resume, diagnose.

Requirements:

- Provider integrations are adapters with declared capabilities.
- Refine should support both provider CLIs and direct provider APIs where useful.
- Provider auth state is treated as external and reported clearly.
- Event parsing is provider-specific but normalizes into shared round-log and
  chat event models.
- Provider session ids, resumability, limits, and failure modes are explicit.
- Missing providers block only workflows that require them.

### Process Supervision

Capability: launch, signal, wait, stream, inspect, limit, clean up.

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

Capability: configure, start, stop, restart, status, rebuild, open, diagnose.

Requirements:

- Target-app commands are configured per app.
- The daemon runs configured commands in the target app's environment.
- Long-running target-app processes are supervised and visible.
- Rebuild and status checks produce structured results.
- Target-app failures do not crash the daemon or block unrelated Refine
  operations.

### Git And Worktrees

Capability: inspect, branch, worktree, diff, merge, rebase, commit, reset,
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

Capability: run checks, browser QA, regressions, screenshots, compare, gate.

Requirements:

- Quality checks are jobs supervised by the daemon.
- Browser automation dependencies are discovered and repaired separately from
  Refine's own runtime dependencies.
- Results are persisted, visible, and tied to the relevant Gap, Feature, or app.
- Users can rerun, cancel, and inspect quality jobs from any surface.

### Chat And Planning

Capability: start, resume, stream, attach to Gap or Feature, persist context.

Requirements:

- Chat sessions use shared provider adapters.
- Gap-attached chat and standalone chat have explicit storage and resumption
  semantics.
- Long-running provider priming or resume steps are observable.
- Chat events can produce importable rounds, Gaps, or Feature plans.

### Observability And Diagnostics

Capability: activity, logs, metrics, doctor, support bundle.

Requirements:

- Every capability emits structured events.
- System operations have status, progress, timestamps, errors, and owning
  surface.
- Logs are queryable from UI and CLI.
- `doctor` reports daemon, install, OS backend, target app, Git, provider,
  browser, Docker, and storage health.
- Support bundles redact secrets by default.

### Security And Permissions

Capability: local auth, secret storage, command authorization, audit.

Requirements:

- Local mutation APIs require an authorization token or equivalent local trust
  mechanism.
- Desktop should store tokens and provider-related local secrets in OS-native
  secret storage when Refine owns them.
- Command execution must pass through explicit capability APIs.
- Surfaces should not expose arbitrary shell execution as a generic primitive.
- Sensitive paths, environment variables, and tokens are redacted in logs.

### Cluster And Multi-Node

Capability: node registry, transfer, sync, remote command, ownership.

Requirements:

- Local daemon identity is explicit.
- Work ownership is enforced before scheduling or mutation.
- Project-state sync is a shared core operation.
- Remote execution and cluster maintenance have bounded, visible operations.
- The Rust architecture should preserve the distinction between UI selection
  and runtime ownership.

## Component Architecture

The Rust implementation should live entirely under `rust/`. That directory is
the Cargo workspace root for the native Refine project. The current Python
implementation lives under `python/`, and the two implementations should remain
side-by-side during the port instead of interleaving Rust crates with Python
packages at the repository root.

Crates should map to capability boundaries and dependency direction, not to the
old Python package layout. A surface crate should not contain business rules
that belong in core services.

Suggested repository layout:

```text
refine/
  docs/
  python/
  rust/
    Cargo.toml
    crates/
      refine-core/
      refine-api/
      refine-daemon/
      refine-cli/
      refine-desktop/
      refine-web/
      refine-storage/
      refine-events/
      refine-git/
      refine-process/
      refine-integrations/
      refine-quality/
      refine-chat/
      refine-cluster/
      refine-config/
      refine-test-support/
    apps/
      desktop/
      web/
    xtask/
```

`python/` remains the current implementation and behavior oracle during the
port. New Rust code should live under `rust/crates/`, `rust/apps/`, and
`rust/xtask/` so the native architecture is explicit and contained within the
Rust workspace.

### Workspace Crates

- `refine-core`: domain models, Gaps, Features, state transitions, validation,
  scheduling rules, work-item operations, capability orchestration, and
  migration interfaces. This crate owns product semantics and should have no
  dependency on HTTP, Tauri, process execution, or concrete storage backends.
- `refine-api`: local API contracts, request and response types, error shapes,
  event stream schemas, auth claims, version negotiation, and generated client
  bindings. This crate defines the public contract shared by daemon, web,
  desktop, CLI, and tests.
- `refine-daemon`: supervisor runtime, API server, IPC server, event bus, job
  registry, lifecycle management, operation recovery, and composition of core
  services with host integrations. This crate is the only long-running local
  authority.
- `refine-cli`: command-line surface over daemon APIs plus limited bootstrap
  commands for install, start, repair, status, and doctor when the daemon is
  unavailable.
- `refine-desktop`: Tauri shell integration, native menus, tray, notifications,
  updater, deep links, local daemon bootstrap, and narrow native commands that
  call the daemon instead of owning Refine processes.
- `refine-web`: compiled frontend assets, generated API client, UI route
  definitions, and static asset packaging. This crate or package should not own
  workflow rules.
- `refine-storage`: durable JSON records, app registry storage, runtime storage,
  SQLite or index storage, migrations, atomic writes, cache rebuild, and backup
  or restore helpers.
- `refine-events`: activity records, log records, progress events, telemetry
  schemas, support-bundle data models, redaction helpers, and event formatting
  shared by UI and CLI.
- `refine-git`: Git inspection, branch operations, worktree operations, diff,
  commit, merge, rebase, push, conflict recovery, and dirty-worktree
  classification.
- `refine-process`: cross-platform process spawning, process groups, streaming
  output, stdin, signals, cancellation, exit status, child cleanup, and resource
  controls.
- `refine-integrations`: provider adapters, provider CLI detection, direct API
  provider adapters, Docker detection, browser automation detection, language
  toolchain detection, package-manager detection, and target-app dependency
  diagnostics.
- `refine-quality`: check execution, browser QA, screenshots, regression jobs,
  comparison results, quality gates, and quality-job persistence.
- `refine-chat`: chat sessions, provider stream normalization, Gap-attached chat,
  standalone chat, resumability, round-log conversion, and chat-derived import
  operations.
- `refine-cluster`: node registry, node ownership, project-state sync, remote
  command orchestration, transfer operations, and maintenance locking.
- `refine-config`: user settings, project settings, runtime settings, dependency
  policy, governance settings, and OS path resolution.
- `refine-test-support`: fixtures, fake providers, fake process backends, temp
  storage helpers, API contract helpers, and black-box harness adapters. This
  crate should be dev-only and must not become a production dependency.

### Dependency Direction

Crates should follow a one-way dependency graph:

```text
surfaces
  refine-cli / refine-desktop / refine-web
      |
daemon
  refine-daemon
      |
capabilities and adapters
  refine-core / refine-storage / refine-git / refine-process
  refine-integrations / refine-quality / refine-chat / refine-cluster
      |
contracts and shared types
  refine-api / refine-events / refine-config
```

Rules:

- `refine-core` may depend on shared contract and configuration types, but not
  on `refine-daemon`, `refine-cli`, `refine-desktop`, `refine-web`, Tauri,
  Axum, SQL implementation details, or OS process APIs.
- `refine-api` should remain transport-oriented but server-neutral. It should
  not call daemon services or storage.
- `refine-daemon` composes capabilities. It can depend on adapter crates, but
  adapter crates should not depend back on the daemon.
- Surface crates call `refine-api` contracts and daemon clients. They should not
  directly mutate durable Refine state or spawn managed processes.
- Storage, Git, process, provider, quality, chat, and cluster crates expose
  typed services to `refine-core` or `refine-daemon`; they do not reach into UI
  state.
- Test support can depend broadly on production crates, but production crates
  cannot depend on test support.

### Internal Directory Pattern

Each crate should expose a small public API and organize implementation by
capability. Prefer shallow directories with clear service boundaries:

```text
rust/crates/refine-core/src/
  lib.rs
  capabilities/
  models/
  transitions/
  scheduling/
  work_items/
  migrations/

rust/crates/refine-daemon/src/
  main.rs
  server/
  ipc/
  supervisor/
  jobs/
  operations/
  recovery/
  auth/

rust/crates/refine-storage/src/
  lib.rs
  records/
  indexes/
  migrations/
  paths/
  atomic/

rust/crates/refine-integrations/src/
  lib.rs
  providers/
  docker/
  browsers/
  toolchains/
  target_apps/
```

Directory names should describe product capabilities or host integration
domains. Avoid directories named after temporary implementation mechanisms, old
Python modules, or individual UI pages unless the crate is a surface crate.

### Application Directories

`rust/apps/desktop/` should contain the Tauri app manifest, platform-specific
icons, desktop packaging metadata, updater configuration, and webview
entrypoints. Its Rust commands should delegate to `rust/crates/refine-desktop`.

`rust/apps/web/` should contain the frontend source, build configuration,
generated API client, static assets, and UI tests. Built assets are packaged for
both the daemon-served browser surface and the desktop webview.

`rust/xtask/` should contain repository automation that is not part of the
shipped product: code generation, API schema export, fixture refresh, release
packaging, installer smoke tests, and migration checks.

### Capability To Crate Map

| Capability | Primary crate | Supporting crates |
| --- | --- | --- |
| Install and update | `refine-desktop`, `refine-cli` | `refine-daemon`, `refine-config`, `refine-events` |
| Daemon lifecycle | `refine-daemon` | `refine-process`, `refine-config`, `refine-events` |
| Surface session | `refine-daemon` | `refine-api`, `refine-events`, `refine-config` |
| Project registry | `refine-core` | `refine-storage`, `refine-git`, `refine-config` |
| Project state | `refine-core` | `refine-storage`, `refine-events` |
| Work items | `refine-core` | `refine-storage`, `refine-events` |
| Scheduling and execution | `refine-core`, `refine-daemon` | `refine-process`, `refine-integrations`, `refine-events` |
| Agent providers | `refine-integrations` | `refine-chat`, `refine-events`, `refine-config` |
| Process supervision | `refine-process` | `refine-daemon`, `refine-events` |
| Target app operations | `refine-core`, `refine-daemon` | `refine-process`, `refine-integrations`, `refine-config` |
| Git and worktrees | `refine-git` | `refine-core`, `refine-events` |
| Quality and verification | `refine-quality` | `refine-process`, `refine-integrations`, `refine-storage` |
| Chat and planning | `refine-chat` | `refine-integrations`, `refine-storage`, `refine-events` |
| Observability and diagnostics | `refine-events`, `refine-daemon` | all capability crates |
| Security and permissions | `refine-daemon`, `refine-config` | `refine-api`, `refine-events` |
| Cluster and multi-node | `refine-cluster` | `refine-core`, `refine-storage`, `refine-process` |

This map is a design tool, not a license to make every capability a separate
process or service. The daemon remains one local authority; crates exist to keep
code ownership, tests, and dependency direction understandable.

### Initial Implementation Cut

The first Rust milestone should create only the crates needed to prove the
architecture without fragmenting the code too early:

- `refine-core` for app registry, daemon status models, basic work-item models,
  and validation.
- `refine-api` for status, doctor, app registry, and structured error
  contracts.
- `refine-storage` for OS path resolution, app registry records, runtime state,
  and atomic writes.
- `refine-process` for daemon-owned process inspection and a minimal managed
  process abstraction.
- `refine-daemon` for local API serving, lifecycle status, event emission, and
  composition of the first services.
- `refine-cli` for `start`, `stop`, `status`, `doctor`, `apps list`, `apps
  attach`, `apps switch`, and `apps detach`.
- `refine-test-support` for fake storage, fake process state, and API contract
  fixtures.

Defer `refine-desktop`, `refine-web`, `refine-integrations`, `refine-quality`,
`refine-chat`, and `refine-cluster` until the first daemon, storage, project
registry, and CLI contracts are stable. Their directories can exist as empty
workspace placeholders only if that helps packaging or CI; otherwise, add them
when their first capability lands.

## Local API

The daemon should expose two local interfaces:

- HTTP/WebSocket API for web and desktop UI.
- Local IPC or loopback HTTP for CLI and privileged desktop bridge commands.

API requirements:

- Stable typed contracts.
- Capability-oriented routes or methods.
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

All storage paths must be OS-specific and documented:

- macOS: app support, logs, cache, launchd plist locations.
- Windows: `%LOCALAPPDATA%`, `%APPDATA%`, service/task metadata, logs.
- Linux: XDG paths for user installs and systemd paths for service installs.

## Dependency Strategy

Refine itself should be native and should not require host Python.

Dependency classes:

- Required core dependencies: bundled with Refine or implemented in Rust.
- Required external dependencies: Git is likely required for most useful
  workflows and should be detected early.
- Optional workflow dependencies: Node/npm, Docker, Playwright browsers,
  provider CLIs, language toolchains, package managers.
- Provider dependencies: either provider CLI, provider API credentials, or both,
  depending on the configured adapter.

Dependency checks should be capability-scoped. Missing Docker should not block
Gap creation. Missing provider auth should block agent execution but not project
inspection. Missing browser automation should block browser QA but not ordinary
chat.

## Migration And Port Strategy

Port vertically by workflow, not horizontally by old Python module.

Suggested order:

1. Install, daemon lifecycle, status, doctor.
2. Target-app registry: attach, switch, detach, no-app mode.
3. Durable storage and index rebuild.
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

## Testing Strategy

Testing should verify capabilities through public surfaces.

Required layers:

- Unit tests for core state transitions and validators.
- Integration tests for storage, migrations, Git, process supervision, and
  provider adapters.
- Contract tests for local API request/response and event schemas.
- CLI tests with JSON output checks.
- Web/Desktop UI smoke tests through the shared API.
- Black-box compatibility tests that run representative workflows against the
  Rust implementation.
- OS matrix tests for macOS, Windows, and Linux process/service behavior.

The black-box harness should treat Refine as a product surface, not as internal
modules. It should exercise UI and CLI behavior and avoid depending on private
implementation details.

## Open Questions

- Should the daemon be per-user, per-checkout, or capable of managing multiple
  target apps from one user-level authority?
- Should provider integrations prefer direct APIs, CLIs, or both?
- Which state must remain Git-visible for collaboration, and which should move
  to local app support paths?
- How much of the web UI should be reused unchanged versus redesigned for
  Desktop-first onboarding?
- Should the desktop bundle include Git, browser automation dependencies, or
  any provider-specific helper binaries?
- What is the minimum Rust MVP that proves the direction before porting advanced
  workflows?

## Acceptance Criteria

- `docs/rust.md` defines web, desktop, and CLI surfaces over one supervisor
  daemon.
- The architecture is organized around system capabilities.
- The daemon is the sole local authority for process lifecycle and durable
  workflow mutations.
- Refine itself has no required host Python dependency in the target Rust
  architecture.
- Target-app and provider dependencies are treated as explicit, scoped
  integrations.
- Business logic is shared across surfaces.
- The document includes migration and testing strategy for a vertical port.
- The document defines a Cargo workspace under `rust/`, crate responsibilities,
  dependency direction, application directories, and the initial Rust
  implementation cut.
