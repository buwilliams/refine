# Rust file cohesion audit

Date: 2026-07-26

This audit applies the project rule that roughly 500 lines is a review trigger,
not a mechanical cap. The baseline was the tracked Rust inventory recorded in
Goal `00000SZ7FC7AJGZ49XT675R003`. Files were reviewed for mixed responsibilities,
unrelated method families, adapter/domain coupling, and omnibus tests.

## Responsibility splits

The critical and high groups now have physical responsibility ownership:

- `workflow` separates durable state, policy/capacity, execution, Ready Merge
  fences, automation, and capability-focused tests.
- `web_server/work_routes` separates Goal, Feature, import, activity/change,
  file/terminal, and test responsibilities. Goal, Feature, and import routes are
  split again by authoring, membership, lifecycle, query, export, round, and
  persistence concerns.
- `process_control` separates process discovery, termination, cancellation
  settlement, durable receipts, and focused tests.
- `cli/dispatch` separates Website, System, Project, Goal, Feature, Todo,
  Workflow, Node, Cluster, Log, Agent, discovery, and daemon-transport command
  families.
- `work_items/service` separates persistence, Goal authoring, Feature ownership,
  bulk operations, workflow transitions, rounds/metadata, and focused tests.
- `web_server/tests` and `cli/tests` are capability-named module trees rather
  than omnibus files.
- `subprocess` separates registry, execution, output, termination, the
  supervisor adapter, and focused tests.
- `chat` separates sessions, standalone worktrees, provider turns, queues, the
  service adapter, and focused tests.
- `http` separates runtime startup, background loops, transport, SSE, static
  content, documentation rendering, and the daemon adapter.
- `target_apps` separates lifecycle, command execution, generated configuration,
  state, and focused tests.
- `git_worktrees` separates change inspection, Git command execution,
  integration, worktree lifecycle, the service adapter, and focused tests.
- `project_routes` additionally separates dashboard, Node/Cluster, target-app,
  project registry, settings/governance, and Todo adapters.

Public surface types and re-exports remain at their established module paths.
The new child modules contain real Rust items and implementations; there are no
numbered chunks, line-range shards, or `include!` fragments.

## Baseline inventory disposition

| Baseline file | Disposition |
| --- | --- |
| `src/surfaces/web_server/tests.rs` | Split into the capability tree described above; shared fixtures remain in `tests/mod.rs`. |
| `src/workflow/mod.rs` | Split into `state`, `policy`, `execution`, `ready_merge`, and `automation`; embedded tests were split by capability. |
| `src/surfaces/web_server/work_routes.rs` | Split into Goal, Feature, import, activity/change, and file/terminal route trees. |
| `src/tools/product/process_control.rs` | Split into discovery, termination, settlement, and receipt modules; tests split into ownership, cleanup, settlement, and termination. |
| `src/surfaces/cli/dispatch.rs` | Split by command family plus daemon transport. |
| `src/tools/product/work_items/service.rs` | Split into persistence, authoring, Features, bulk operations, workflow, and rounds/metadata. |
| `src/surfaces/cli/tests.rs` | Split by CLI command family. |
| `src/process/subprocess/mod.rs` | Split into registry, execution, output, termination, and service-adapter modules. |
| `src/tools/product/chat/mod.rs` | Split into sessions, standalone worktrees, provider turns, queues, and service adapter. |
| `src/surfaces/web_server/http.rs` | Split into runtime, loops, transport, SSE, static content, docs, and daemon adapter. |
| `src/tools/host/target_apps/mod.rs` | Split into lifecycle, commands, generation, state, and focused tests. |
| `src/tools/host/git_worktrees/mod.rs` | Split into changes, commands, integration, worktrees, adapter, and focused tests. |
| `src/tools/host/git_sync/mod.rs` | Embedded tests were extracted and split. Production exception: one serialized repository-sync transaction owns fetch, divergence classification, fast-forward, operation evidence, and repository locking; splitting those phases would obscure the transaction invariant. |
| `src/process/supervisor/operations/mod.rs` | Embedded tests extracted. Production exception: this is the authoritative durable operation-journal state machine; registration, progress, cancellation, recovery, and settlement all mutate one record/log contract. |
| `src/tools/host/source_promotion.rs` | Embedded tests extracted. Production exception: activation, rollback, recovery, and audit evidence are one source-promotion state machine with a shared durable receipt. |
| `src/surfaces/web_server/project_routes.rs` | Split into dashboard, Nodes/Cluster, target app, projects, settings/governance, and Todos. |
| `src/tools/host/quality/service.rs` | Exception: the file is the authoritative Quality execution state machine; command observation, cancellation settlement, result persistence, screenshots, and evidence implement one operation lifecycle. Tests remain physically separate and are split by configuration, execution, results, and screenshots. |
| `src/tools/host/release.rs` | Embedded tests extracted. Exception: planning, preparation, publication, and recovery are phases of one release transaction with shared version and receipt invariants. |
| `src/process/supervisor/config/mod.rs` | Embedded tests extracted. Exception: the file is the shared JSON configuration codec and atomic persistence boundary; settings, Governance, Guidance, and Reporter records use the same normalization and durability rules. |
| `src/tools/product/work_items/tests.rs` | Split into Goal, Feature, bulk, workflow, and round/metadata suites; Feature tests are further split by authoring, membership, and lifecycle. |
| `src/tools/product/merging/mod.rs` | Embedded tests extracted and split. Exception: candidate integration, review, conflict recovery, and rollback form one auditable merge transaction. |
| `src/tools/host/installation/mod.rs` | Embedded tests extracted. Exception: install, repair, rollback, and uninstall share one manifest/service-manager transaction and recovery contract. |
| `src/surfaces/cli/actions.rs` | Exception: this is the declarative Clap command schema. The enum hierarchy is one generated CLI contract consumed by parsing, catalog generation, and dispatch; physical splits would make the command grammar harder to review. |
| `src/process/agent_sessions.rs` | Embedded tests extracted. Exception: PTY creation, attachment, input/output, attention, and cleanup are one managed-session lifecycle with shared ownership checks. |
| `src/tools/host/agent_providers/mod.rs` | Embedded tests extracted. Exception: provider discovery, configuration, authentication, invocation, and resume implement one replaceable host-provider adapter contract. |
| `src/tools/product/goal_exports/mod.rs` | Exception: format selection, evidence prioritization, Jira budgeting, CSV rendering, and delivery are one deterministic Goal-export renderer. |
| `src/tools/host/quality/tests.rs` | Split into configuration, execution, results, and screenshot suites. |
| `tests/cli_surface.rs` | Exception: a dedicated end-to-end integration crate verifying the compiled CLI surface across command families; shared process and repository fixtures intentionally remain in one crate. |
| `src/tools/host/cluster/mod.rs` | Embedded tests extracted. Exception: remote Node discovery, bootstrap, command execution, transfer, and health share one SSH transport and Node ownership boundary. |
| `tests/multi_instance_sync.rs` | Exception: a dedicated multi-process integration crate whose scenarios share one distributed fixture and synchronization timeline. |
| `src/process/runner.rs` | Embedded tests extracted. Exception: worker decoding, supervised launch, cancellation, result persistence, and recovery are one runner-worker protocol. |
| `src/tools/product/imports/mod.rs` | Embedded tests extracted. Exception: structured extraction, normalization, validation, ordering, and CSV parsing implement one import contract before persistence. |
| `src/surfaces/web_server/server.rs` | Embedded tests extracted. Exception: this is the single in-process HTTP route table; it deliberately maps method/path pairs to capability handlers without owning their domain behavior. |
| `src/surfaces/web_server/operation_routes.rs` | Exception: a thin route adapter for operation, process, pause, and supervised-worker endpoints backed by the shared process/operation capability. |
| `src/process/supervisor/security/mod.rs` | Embedded tests extracted. Exception: command authorization, environment filtering, resource policy, and redaction form one process-security boundary. |
| `src/tools/product/project_registry/mod.rs` | Embedded tests extracted. Exception: registry attach, switch, detach, clone, and migration all mutate one authoritative active-project registry. |
| `src/workflow/behaviors/mod.rs` | Embedded tests extracted. Exception: the stage implementations are a compact set behind one `WorkflowBehavior` contract and share the same context, evidence, and transition invariants. |
| `src/tools/product/project_state/store.rs` | Exception: snapshot refresh, cache validation, atomic publication, and recovery form one projection-store transaction. |
| `src/tools/host/deployed_update.rs` | Embedded tests extracted; now below the review trigger. |
| `src/tools/product/project_state/tests.rs` | Split into cache, Goal, Feature, and activity suites. |
| `src/tools/product/project_state/query.rs` | Exception: query filtering, paging, sorting, and response shaping operate on one immutable projection snapshot. |
| `src/surfaces/web_server/support/terminal.rs` | Exception: terminal launch, PTY I/O, resize, event replay, status, and stop are one managed-terminal adapter with shared session ownership. |
| `src/process/supervisor/lifecycle/mod.rs` | Embedded tests extracted. Exception: daemon start, status, repair, stop, and health probing share one PID/port lifecycle contract. |
| `src/tools/observability/metrics/mod.rs` | Embedded tests extracted. Exception: metric recording, bounded persistence, aggregation, and cleanup implement one metrics store. |
| `src/surfaces/web_server/runtime.rs` | Embedded tests extracted. Exception: runtime projection reconciliation is one derived view over processes, operations, and target-app status. |
| `src/workflow/context.rs` | Exception: this is the shared per-Goal workflow context and its transition/evidence helpers; state ownership remains in the services it composes. |
| `src/surfaces/web_server/support/files.rs` | Exception: file-tree enumeration, bounded reads, and search share one path-confinement and source-file adapter. |
| `src/tools/product/nodes/mod.rs` | Embedded tests extracted; now below the review trigger. |
| `src/surfaces/web_server/quality_chat_routes.rs` | Exception: a thin route adapter for Quality and chat endpoints; domain behavior remains in their capabilities. |
| `tests/support/integration.rs` | Exception: shared integration-fixture construction and teardown used by the external test crates; splitting would duplicate lifecycle ownership. |

## Post-split review-trigger exceptions

Some new responsibility modules remain modestly above 500 lines. They were
reviewed and retained because they contain one named concern: the static
documentation navigation catalog/renderer, integrated cancellation-settlement
and workflow-execution scenarios, Goal/Feature CLI command families, and shared
parent modules that hold types and invariants used by several child modules.
These are review-trigger exceptions, not permission to add unrelated behavior.

No generated Rust files were found in the audited inventory.
