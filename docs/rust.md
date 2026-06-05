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

- Preserve old Python package or module boundaries in Rust.
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

Capabilities are the primary code-organization model for Refine's Rust
architecture. They are namespaces, service boundaries, trait families, storage
interfaces, and vocabulary for major areas of the system. They sit inside the
overall supervisor architecture described above; they are not separate daemons,
UI-specific action handlers, or a replacement for the local API boundary.

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

## Core Capabilities

### Installation And Update

Module: `capabilities::installation`; path: `rust/src/capabilities/installation/`.

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

Module: `capabilities::daemon_lifecycle`; path: `rust/src/capabilities/daemon_lifecycle/`.

Owns abstractions for: start, stop, restart, status, health, recover.

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

Module: `capabilities::surface_session`; path: `rust/src/capabilities/surface_session/`.

Owns abstractions for: open UI, authenticate local surface, stream state, deliver
notifications.

Requirements:

- Desktop, browser, and CLI use a shared local auth model.
- The daemon should not expose unauthenticated mutation APIs on a public
  interface.
- Web UI can stream activity, process output, job progress, and chat events.
- Desktop can subscribe to events for badges, tray state, and notifications.

### Project Registry

Module: `capabilities::project_registry`; path: `rust/src/capabilities/project_registry/`.

Owns abstractions for: register, attach, switch, detach, clone, remove, inspect.

Requirements:

- Refine tracks known target apps separately from the active app.
- App switching is a supervisor transaction.
- Runtime bookkeeping must not dirty tracked target-app files.
- Target app identity, path, Git root, remote, and health are explicit.
- Detached/no-app mode is supported.
- Switching should prepare or migrate the target app before making it active.

### Project State

Module: `capabilities::project_state`; path: `rust/src/capabilities/project_state/`.

Owns abstractions for: initialize, read, mutate, migrate, sync, rebuild indexes.

Requirements:

- Durable Refine workflow state lives in project-local storage.
- Runtime state lives outside tracked target-app files.
- SQLite or another index can accelerate queries, but durable records must be
  rebuildable after corruption or version upgrades.
- Mutations flow through shared core services and emit audit/activity events.
- Migrations are versioned, idempotent, and observable.

### Work Items

Module: `capabilities::work_items`; path: `rust/src/capabilities/work_items/`.

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

Module: `capabilities::scheduling`; path: `rust/src/capabilities/scheduling/`.

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

Module: `capabilities::agent_providers`; path: `rust/src/capabilities/agent_providers/`.

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

Module: `capabilities::process_supervision`; path: `rust/src/capabilities/process_supervision/`.

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

Module: `capabilities::target_apps`; path: `rust/src/capabilities/target_apps/`.

Owns abstractions for: configure, start, stop, restart, status, rebuild, open, diagnose.

Requirements:

- Target-app commands are configured per app.
- The daemon runs configured commands in the target app's environment.
- Long-running target-app processes are supervised and visible.
- Rebuild and status checks produce structured results.
- Target-app failures do not crash the daemon or block unrelated Refine
  operations.

### Git And Worktrees

Module: `capabilities::git_worktrees`; path: `rust/src/capabilities/git_worktrees/`.

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

Module: `capabilities::quality`; path: `rust/src/capabilities/quality/`.

Owns abstractions for: run checks, browser QA, regressions, screenshots, compare, gate.

Requirements:

- Quality checks are jobs supervised by the daemon.
- Browser automation dependencies are discovered and repaired separately from
  Refine's own runtime dependencies.
- Results are persisted, visible, and tied to the relevant Gap, Feature, or app.
- Users can rerun, cancel, and inspect quality jobs from any surface.

### Chat And Planning

Module: `capabilities::chat`; path: `rust/src/capabilities/chat/`.

Owns abstractions for: start, resume, stream, attach to Gap or Feature, persist context.

Requirements:

- Chat sessions use shared provider adapters.
- Gap-attached chat and standalone chat have explicit storage and resumption
  semantics.
- Long-running provider priming or resume steps are observable.
- Chat events can produce importable rounds, Gaps, or Feature plans.

### Observability And Diagnostics

Module: `capabilities::observability`; path: `rust/src/capabilities/observability/`.

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

Module: `capabilities::security`; path: `rust/src/capabilities/security/`.

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

Module: `capabilities::cluster`; path: `rust/src/capabilities/cluster/`.

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
  rust/
    Cargo.toml
    src/
      main.rs
      lib.rs
      architecture/
        supervisor/
        local_api/
        surfaces/
          cli/
          desktop/
          web/
        runtime/
        jobs/
        config/
        errors/
        testing/
      capabilities/
        installation/
        daemon_lifecycle/
        surface_session/
        project_registry/
        project_state/
        work_items/
        scheduling/
        agent_providers/
        process_supervision/
        target_apps/
        git_worktrees/
        quality/
        chat/
        observability/
        security/
        cluster/
      shared/
        ids/
        paths/
        time/
        serialization/
    desktop/
      src-tauri/
        Cargo.toml
        src/
          main.rs
    xtask/
```

`python/` remains the current implementation and behavior oracle during the
port. New core product code should live under `rust/src/`, and repository
automation should live under `rust/xtask/`, so the native architecture is
explicit and contained within the Rust project.
If Tauri requires a separate package, it should live under
`rust/desktop/src-tauri/` and depend on the core product package. It should not
own capabilities, durable state rules, process lifecycle, provider behavior, or
workflow logic.

### Module Direction

Modules should follow a one-way dependency graph:

```text
surfaces
  architecture::surfaces::{cli, desktop, web}
      |
daemon
  architecture::supervisor / architecture::local_api / architecture::jobs
      |
capabilities
  capabilities::{work_items, scheduling, process_supervision, ...}
      |
shared foundations
  shared::{ids, paths, time, serialization}
```

Rules:

- `architecture::*` modules compose capabilities into the supervisor, local API,
  jobs, runtime lifecycle, and user surfaces.
- `capabilities::*` modules own product and host-integration abstractions. Each
  capability has exactly one top-level module and directory, listed in the Core
  Capabilities section.
- Capability modules can depend on `shared::*` foundations and on service
  traits from other capabilities when composition requires it, but they should
  not reach into surface modules.
- Surface modules call the local API or supervisor-facing service facades. They
  should not directly mutate durable Refine state or spawn managed processes.
- Shared modules are small foundations. They should not accumulate product
  logic just because multiple modules need the same helper.
- Test support lives under `architecture::testing` and per-capability test
  fixtures. It can depend broadly on production modules, but production modules
  cannot depend on test-only code.

### Architecture Support Modules

The following modules support the overall architecture rather than a single
product capability:

- `architecture::supervisor`; path: `rust/src/architecture/supervisor/`. Owns
  daemon authority, startup composition, runtime ownership, and controlled
  shutdown.
- `architecture::local_api`; path: `rust/src/architecture/local_api/`. Owns
  HTTP, WebSocket, local IPC, auth extraction, request routing, and API
  translation into capability services.
- `architecture::surfaces::cli`; path: `rust/src/architecture/surfaces/cli/`.
  Owns the CLI surface and structured output formatting.
- `architecture::surfaces::desktop`; path:
  `rust/src/architecture/surfaces/desktop/`. Owns desktop shell integration,
  native menu and tray hooks, update prompts, and narrow bridge command
  definitions used by the Tauri wrapper.
- `architecture::surfaces::web`; path: `rust/src/architecture/surfaces/web/`.
  Owns serving or packaging the web UI assets and generated client bindings.
- `architecture::runtime`; path: `rust/src/architecture/runtime/`. Owns runtime
  bootstrap, OS path selection, instance identity, and process startup context.
- `architecture::jobs`; path: `rust/src/architecture/jobs/`. Owns the job
  registry, operation handles, cancellation plumbing, and operation recovery
  coordination.
- `architecture::config`; path: `rust/src/architecture/config/`. Owns loading,
  validating, and merging user, project, and runtime configuration.
- `architecture::errors`; path: `rust/src/architecture/errors/`. Owns shared
  error categories and translation into local API and CLI output.
- `architecture::testing`; path: `rust/src/architecture/testing/`. Owns
  black-box fixtures, fake supervisors, fake providers, fake process handles,
  and contract-test helpers.

`rust/xtask/` should contain repository automation that is not part of the
shipped product: code generation, API schema export, fixture refresh, release
packaging, installer smoke tests, and migration checks.

### Initial Implementation Cut

The first Rust milestone should create only the modules needed to prove the
architecture without fragmenting the code too early:

- `architecture::supervisor`, `architecture::local_api`,
  `architecture::surfaces::cli`, `architecture::runtime`,
  `architecture::config`, `architecture::errors`, and `architecture::testing`.
- `capabilities::daemon_lifecycle`, `capabilities::surface_session`,
  `capabilities::project_registry`, `capabilities::project_state`,
  `capabilities::work_items`, `capabilities::process_supervision`, and
  `capabilities::observability`.
- `shared::ids`, `shared::paths`, `shared::time`, and
  `shared::serialization`.

Defer desktop shell, web asset packaging, provider execution, quality, chat,
and cluster modules until the first supervisor, local API, project registry,
storage, process, log, and CLI contracts are stable. Their directories should
be added when their first concrete abstraction lands.

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
- The document defines a core Rust package under `rust/`, leaves room for a
  thin Tauri wrapper package, assigns capabilities to modules and directory
  paths, defines supporting architecture modules, and identifies the initial
  Rust implementation cut.
