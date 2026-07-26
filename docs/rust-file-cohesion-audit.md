# Rust file cohesion audit

Date: 2026-07-26

The Round 2 baseline is integrated commit
`bb204cc64cefe05fb7b109e8689465c4ba12b7fc`. The tracked inventory was measured
at the start of the Round with:

```sh
git ls-files '*.rs' | xargs wc -l | sort -nr
```

It contained 54 Rust files above the roughly 500-line review trigger, including
11 above 1,000 lines. The trigger was applied as a structural review, not a
mechanical cap.

## Round 2 responsibility splits

Round 2 removed every file above 1,000 lines and reduced the review-trigger
inventory to 35 files. This follow-up reduces that inventory to 33 files.
Stable public paths are preserved through parent-module re-exports.

- The declarative Clap schema is split into Agent, Cluster, Feature, Goal, Log,
  Node, Project, System, Todo, and Workflow command families. `Cli` and
  `Commands` remain the single top-level grammar and catalog contract.
- The compiled CLI integration crate is split by the same capability families;
  the multi-instance crate separates state replication, Ready Merge child
  processes, and cancellation scenarios from shared process fixtures.
- Quality separates settings migration, operation execution, cancellation,
  settlement/evidence persistence, and provider-output parsing.
- The durable operation registry separates storage, registration/logs,
  recovery, cancellation, completion, trait adaptation, and pure state helpers.
- Configuration separates Settings, Governance, Guidance, Reporters, their
  codecs, and shared atomic JSON persistence.
- Git sync separates transaction orchestration, state-worktree management, Git
  commands, durable-state codecs/files, and repository locks.
- Release separates request orchestration, publication stages, shell-host
  delivery, and planning/version evidence. Source promotion separates queuing,
  validation, execution/settlement, host activation, and Git cleanup.
- Goal export separates Goal/commit selection, Jira/CSV rendering,
  character-budget enforcement, evidence summaries, and tests.
- Agent providers separate provider execution, provider command grammar,
  activity formatting, and output/session parsing. Agent sessions separate the
  PTY execution loop from attachment APIs and command/signal codecs.
- Runner separates dispatch, Workflow, Git sync, project sync, Jira export,
  worker specifications, and scheduling. Subprocess separates command
  construction, workflow registration, identity persistence, OS signaling, and
  test hooks from its existing execution/registry/output/termination adapters.
- Installation separates durable install state, OS backend registration,
  lifecycle operations, and backend/service specifications. Cluster separates
  local registry/ownership from remote bootstrap and SSH transport.
- Imports separates draft conversion/dependency ordering, CSV parsing,
  provider extraction, and Plan normalization. Project-state storage separates
  cache/fingerprint ownership, Goal, Feature, activity/change projections,
  projection publication, and shared projection helpers.
- Process security separates the native secret store, command policy/audit
  adapter, and supervised host-command codec.
- Work-item service parents now delegate Goal filters, round/log helpers,
  operation validation, and record persistence/revision helpers in addition to
  the Round 1 responsibility modules.
- Operation routes are split into release, operation journal, Workflow,
  process, pause, installation/source, Agent/secret, and diagnostics route
  families.

No numbered chunks, line-range modules, `include!` fragments, or generated Rust
files were introduced.

## Follow-up responsibility splits

- Workflow's public parent now contains only the shared claim/policy/execution
  contracts and composition. State persistence, settings parsing,
  retry/execution-context hydration, governance prompt/verdict handling, and
  Goal-agent prompt/context projection have responsibility-named owners.
- Work routes now use the parent only for route-family composition and the
  shared server node lookup. Import extraction/transport support, terminal
  profile and standalone-worktree lifecycle, and Feature response/reorder
  codecs moved to named children.
- Chat's public parent now contains only serialized session/queue contracts,
  the service trait, and composition. Prompt context, transcript/artifact
  projection, queue formatting, session identity/storage primitives, operation
  evidence, and standalone naming moved to named children.
- The production children directly changed here use explicit imports. Existing
  leaf route modules that still use `super::*` now see only an explicit,
  responsibility-family dependency catalog rather than the broad work-routes
  parent. Remaining broad wildcard parent imports are test-only.
- The HTTP parent and dispatch catalog were re-read because their earlier prose
  claimed composition-only behavior. Their dispositions below now enumerate
  the actual item families and the invariant that keeps each exception
  reviewable.

## Round 2 Governance recovery

- Import persistence now has one responsibility-named owner at
  `src/tools/product/imports/persistence.rs`. It owns duplicate-decision
  validation and execution, Goal rounds and metadata, Feature destination
  mutation, dependency order, progress/cancellation, created-record accounting,
  and rollback evidence.
- `FileImportService`, direct CLI imports, and synchronous/background daemon
  imports delegate to that owner. The web import contract contains only
  extraction/response formatting and the operation-registry observer adapter;
  no web module writes Goals or Features for import.
- Shared capability tests are separated from web transport and cross-surface
  parity tests. The final inventory remains 33 files at or above the roughly
  500-line review trigger; no new production or test exception was introduced.

## Post-round review-trigger dispositions

The following inventory is measured from the complete candidate tree after
formatting, including the new files. Each remaining file was re-read from its
actual items and method families.

| Lines | File | Exact disposition |
| ---: | --- | --- |
| 998 | `src/surfaces/web_server/server.rs` | Retained as the authoritative HTTP/MCP method-and-path dispatch catalog. Besides delegation it contains the MCP forwarding adapter, route-specific path extraction, and the mutation-to-projection-refresh classification; these items jointly enforce one ordered route/refresh contract. Any handler implementation or unrelated codec would reopen the boundary. |
| 837 | `src/workflow/behaviors/mod.rs` | Retained as the closed `WorkflowBehavior` stage implementation and its shared stage-evaluation pipeline. The adjacent helpers assemble pinned Goal-agent context, enforce the applicable-Guidance decision, run Quality/governance evaluation, and record the evidence consumed by those stages. They share the same transition/evidence contract; unrelated Workflow persistence or scheduling remains outside. |
| 774 | `src/surfaces/web_server/http/docs.rs` | Retained as one static HTTP documentation catalog and renderer. Navigation identity, anchors, and rendered sections are checked as one ordered document; splitting would hide broken links and ordering drift across files. |
| 761 | `src/tools/host/target_apps/mod.rs` | Retained as the target-app generated-configuration grammar: defaults, wrapper script rendering, command/environment encoding, and reachability derivation all produce one `TargetAppGeneratedConfig`. Lifecycle mutation is already in child modules; separating these mutually dependent render steps would obscure the generated-file contract. |
| 748 | `src/workflow/tests/execution.rs` | Retained as the execution-state-machine test matrix sharing one deterministic Workflow fixture. The scenarios assert one ordered claim/start/settle evidence protocol; dividing the matrix would duplicate fixture mutation and make missing state transitions harder to see. |
| 744 | `src/workflow/tests/cancellation.rs` | Retained as the cancellation race test matrix sharing one execution and cancellation-journal fixture. Exact-once settlement is established by the sequence across scenarios, so separate fixtures would weaken the race invariant. |
| 741 | `src/tools/product/merging/mod.rs` | Retained as the authoritative candidate-integration transaction. Fence verification, merge/rebase selection, persistence, rollback, and existing-integration verification all protect one `RoundIntegration` receipt; moving a phase would permit mutation without the adjacent fence/receipt checks. |
| 710 | `src/tools/product/process_control.rs` | Retained as the process-control facade and ownership model. Discovery is already delegated; the parent keeps the one typed ownership/receipt contract used by termination and settlement children so nested-process authorization cannot diverge. |
| 690 | `src/surfaces/cli/dispatch/goals.rs` | Retained as the Goal CLI adapter. Every branch converts one `GoalAction` variant into the shared Goal capability and JSON contract; keeping the exhaustive match together makes grammar/dispatch drift detectable. |
| 684 | `src/tools/product/project_registry/mod.rs` | Retained as the single active-project registry mutation boundary. Attach, clone, switch, detach, and migration all atomically rewrite the same registry and active-app pointer; splitting writers would make their mutual exclusion and migration ordering less legible. |
| 676 | `src/tools/product/project_state/query.rs` | Retained as the immutable projection-query algebra. Filtering, sorting, pagination, and response shaping share one snapshot and stable cursor ordering; physical separation would allow filters and cursor comparison to drift. |
| 675 | `src/surfaces/web_server/tests/operations_processes/events/project.rs` | Retained as the project-runtime event reconciliation integration matrix. All scenarios share one SSE cursor and project-operation fixture and jointly prove that snapshots and events converge without polling. |
| 648 | `src/surfaces/web_server/http.rs` | Retained as the authoritative HTTP daemon lifetime and transport contract. It owns `WireResponse`, daemon/Axum state construction and shutdown, terminal-event route parsing, automation and Git-sync loop scheduling, and small wire helpers. Those families meet at one server lifetime and shutdown boundary; request handlers, SSE state, docs, and static rendering remain delegated. |
| 637 | `tests/multi_instance_sync.rs` | Retained as shared multi-process fixture infrastructure only: daemon startup, HTTP/Git helpers, Ready Merge fixture construction, and child-process launching. Scenarios now live in named children; splitting fixture ownership would create competing teardown and port/process owners. |
| 632 | `src/surfaces/web_server/support/terminal.rs` | Retained as the web adapter for one managed terminal session. Launch, PTY I/O, resize, replay, status, and stop all enforce the same session owner/token and monotonic event sequence; splitting handlers would duplicate those checks. |
| 596 | `src/tools/product/process_control/tests/settlement.rs` | Retained as the cancellation-settlement receipt matrix. The cases share one seeded process tree and durable receipt fixture and compare exact terminal outcomes; separate setup would no longer prove identical inputs settle identically. |
| 586 | `src/tools/host/source_promotion/tests.rs` | Retained as the promotion failure/recovery matrix over one mock host and durable promotion receipt. The scenarios verify cleanup and rollback at each stage of the same receipt protocol; duplicating the host would weaken stage-to-stage equivalence. |
| 580 | `src/workflow/context.rs` | Retained as the per-Goal Workflow dependency/evidence context. Its helpers provide one typed gateway to state, Git, process, quality, governance, and logs so stage implementations cannot construct inconsistent evidence roots. |
| 570 | `src/tools/product/process_control/settlement.rs` | Retained as the exact cancellation-settlement algorithm. Process exit confirmation, workflow cancellation receipt, operation journal, and Goal transition are deliberately adjacent so terminal state cannot be published before every durable prerequisite. |
| 569 | `src/surfaces/web_server/support/files.rs` | Retained as one path-confined source-file adapter. Tree enumeration, bounded reads, and search all use the same canonical-root validation; splitting would risk one access path omitting confinement. |
| 564 | `src/surfaces/cli/dispatch/features.rs` | Retained as the exhaustive `FeatureAction` adapter. All branches preserve one Feature JSON/error contract, and one match is the review point proving every declared Feature command is dispatched. |
| 554 | `src/tools/product/work_items/service/rounds_and_metadata.rs` | Retained as the atomic Goal-round mutation owner. Round add/edit, metadata update, and latest-round derivation rewrite one Goal record under the same revision check; separating them would create multiple round writers. |
| 546 | `src/workflow/policy.rs` | Retained as the complete Workflow admission/transition policy table. Capacity, dependency, node ownership, pause, and terminal-state predicates are evaluated together before a claim; distributing predicates would hide contradictory admission rules. |
| 545 | `src/tools/product/process_control/termination.rs` | Retained as the supervised termination algorithm for one owned process tree. Ownership validation, signal escalation, exit confirmation, cleanup, and receipt creation must remain adjacent so cleanup cannot precede confirmed identity/exit. |
| 538 | `src/surfaces/web_server/runtime.rs` | Retained as the derived runtime snapshot reconciler. It combines process, operation, target-app, and Workflow projections into one versioned browser snapshot; splitting producers would remove the single point that enforces snapshot consistency. |
| 531 | `src/tools/product/work_items/service/bulk_operations.rs` | Retained as the atomic bulk-Goal mutation loop. Eligibility, per-Goal protection, rollback bookkeeping, and summary evidence execute under one mutation lease; extracting a phase would allow partial results to bypass rollback accounting. |
| 522 | `src/surfaces/web_server/quality_chat_routes.rs` | Retained as the shared conversational-evaluation route adapter. Quality starts and chat turns share one operation-registration and error envelope used by the browser; domain execution remains in Quality and Chat capabilities. |
| 519 | `src/surfaces/cli/dispatch.rs` | Retained as the top-level `Commands` dispatcher and shared daemon transport/error codec. Command-family behavior is delegated; the exhaustive root match proves every top-level Clap command has exactly one adapter. |
| 516 | `src/tools/host/cluster/mod.rs` | Retained as the local Cluster registry and Goal-ownership transaction. Remote bootstrap/SSH is now delegated; the parent keeps registry load/migration, distribution, transfer, and maintenance because they atomically update the same Node ownership map. |
| 516 | `tests/support/integration.rs` | Retained as external-suite fixture construction and teardown. One owner allocates daemon ports, repositories, runtime roots, processes, and cleanup; splitting would permit two fixture components to race over teardown. |
| 509 | `src/process/supervisor/lifecycle/mod.rs` | Retained as the daemon PID/port ownership protocol. Start/status/repair/stop and probing all interpret one PID record and executable identity; another writer would make stale-PID recovery ambiguous. |
| 506 | `src/tools/observability/metrics/mod.rs` | Retained as one bounded metrics-store implementation. Append, retention cleanup, aggregation, and persistence share the same ordering and size bounds; splitting would let reads and cleanup disagree about the retained window. |
| 501 | `src/surfaces/web_server/work_routes/activity_routes.rs` | Retained as the project-observability and operator-repair route adapter. Activity/change queries, undo and hard-reset operations, cache rebuild, and performance retention all publish through the same operation registry and refresh the same runtime/project projections. Domain mutation remains in Git, activity, metrics, and projection capabilities; adding an unrelated route family reopens this boundary. |

These are review-trigger exceptions, not permission to add unrelated behavior.
Any new item family should reopen the physical boundary.

## Guidance verification

The live read-only `GET /api/guidance` response on port 8082 contained exactly
one enabled `Cohesive code files` entry with the persisted Rule and Instructions
from Round 1. No Guidance mutation or duplicate entry was created.

Workflow now derives whether the committed candidate changed code files from
the shared Git capability. When code changed, the exact enabled Rule cannot be
recorded in `skipped`; an omitted index fails the auditable completion instead.
Focused tests cover required application and explicit no-code skipping.

## Rollback replay stress

The rollback replay fixture now owns a five-minute child lifetime so concurrent
suite scheduling cannot let its 30-second process expire before settlement.
Production identity checks remain fail-closed while the PID is alive; a second
liveness check recognizes the exact exit when the process disappears between
the first probe and the post-signal identity read.

The focused test passed 16 invocations at concurrency four with:

```sh
seq 1 16 | xargs -P 4 -I RUN sh -c \
  'cargo test --lib tools::product::process_control::tests::settlement::rollback_failed_after_goal_restore_replays_from_exact_restored_revision -- --exact >/dev/null'
```
