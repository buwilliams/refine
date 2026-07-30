# Scale, Reliability, And Resource Adaptation

## Status

Draft. Analysis complete and verified against the tree at `8dca163a`. No
implementation started.

## Summary

Refine 4.x has no incremental state model and no resource model. Every read of
project state is a whole-project operation, the scheduler's promotion pass is
super-linear in Goal count, and both run under a single global exclusive lock
that is also held across untimed Git commands.

The result is a scaling wall whose position is set by *Goal count × repository
size*. A development machine with a small dogfooding backlog and a small target
repository sits comfortably on one side of it. A deployed node with a larger
repository, more data, and fewer resources sits on the other — and fails in ways
that look like unrelated state-machine bugs.

This spec identifies the three root causes, and specifies five changes that
together make Refine degrade gracefully under resource pressure instead of
breaking, while scaling up aggressively when resources are plentiful.

## Motivation

Refine ran smoothly under local dogfooding and began failing immediately on a
client's deployed infrastructure. The deployed environment differs on every axis
that matters:

| Axis | Development | Deployed |
| --- | --- | --- |
| Memory | Large | 20 GB |
| Cores | Many | 2 × 2.4 GHz |
| Disk | Large | 250 GB |
| Goal count | Tens | <1000 |
| Target repository size | Small | Large |
| Governance / quality failures | Rare | Common |

Two days of fixes across commits `2cdaab64`..`8dca163a` addressed symptoms
rather than causes. Each of those eight commits is downstream of the same
stall-under-load behavior:

| Commit | Symptom addressed | Underlying cause |
| --- | --- | --- |
| `2cdaab64` | Unbounded managed worktrees | No disk budget |
| `5c5a126f` | Inactive Goal worktrees retained | No disk budget |
| `d707b15f` | Already-merged Goal retries | Merge state drifted during stall |
| `d1f58fa4` | Concurrent already-merged reconciliation | Global lock contention |
| `b4add9f9` | Stale Ready Merge candidates | Scheduler starved by promotion cost |
| `65ecea02` | Deregistered direct daemons | Wall-clock readiness timeout fired on healthy work |
| `1650b7a3` | Claim preparation failures | Cascading failure from stalled scheduler |
| `8dca163a` | Merge evidence vs branch state | State drift after recovery of non-failures |

Building recovery layers for a system that stalls under load makes the state
machine harder to reason about, and the layers themselves become a source of
state drift. The stall must be fixed at the cause.

## Goals

- A node with 20 GB RAM, 250 GB disk, and 2 × 2.4 GHz cores runs a Refine target
  app with ~5M Goals, 2 parallel agents, and frequent Governance and quality
  failures.
- Throughput scales *up* with available resources rather than being pinned to a
  fixed constant.
- Resource scarcity produces slower progress, never failure or state corruption.
- A slow component is never misclassified as a failed component.
- Scheduler cost is bounded by *work in flight*, not by project history.

## Non-Goals

- Changing the durable state format. `goal.json` / `feature.json` under sharded
  directories remains the source of truth, unchanged.
- Changing the Git sync model. `refine/state` remains the cross-node mechanism.
- Introducing a database, binary store, or any non-diffable format. See
  Constraints.
- Reducing functionality to gain performance. Search, activity, and dashboard
  projections all remain; they move off the hot path rather than disappearing.

## Constraints

**All durable state must remain plain text.** Git is Refine's synchronization
mechanism across nodes (`REFINE_STATE_BRANCH` = `refine/state`,
`src/tools/host/git_sync/mod.rs:30`). Line-oriented plain text is what makes
`git diff` readable and three-way merge possible. Any binary or single-blob
format — SQLite included — breaks conflict resolution and is therefore excluded,
regardless of its performance characteristics.

**Any synced file is a conflict surface.** A single file written by every node
on every mutation conflicts on every concurrent change and would require a
custom merge driver. Derived data must therefore be node-local and uncommitted,
never synced.

The existing layout already respects both constraints and should be preserved:

- `<git-common-dir>/refine-live-state/` — live durable state, outside the
  application worktree (`src/tools/host/project_layout.rs:8`)
- `<git-common-dir>/refine-state-worktree/` — the worktree on `refine/state`
  that mirrors it for sync (`src/tools/host/project_layout.rs:9`)
- `runtime/` — node-local host state, gitignored, never committed

## Root Causes

### 1. The projection is a whole-project operation on the hot path

`FileProjectStateStore::rebuild_projection`
(`src/tools/product/project_state/store/projection_store.rs:76`) reads and
parses every `goal.json` and `feature.json`, hashes the full contents of every
`logs.jsonl`, materializes the entire project in memory, and writes it back as
pretty-printed JSON.

The cache does not bound this:

- `source_fingerprints_match` (`store/core.rs:96`) — the *cache hit* path — does
  a full recursive directory walk and `stat`s every Goal, Feature, and log file
  on every check. Cost is O(N) even when nothing changed.
- `fingerprint_content_hash` (`project_state/helpers.rs:70`) reads whole file
  contents byte-at-a-time. `logs.jsonl` files grow without bound.
- Fingerprints are per-file with no generation counter, so **any single Goal
  write invalidates the entire snapshot**. Write amplification is O(N) per
  mutation and O(N²) across a batch.
- `HOT_PROJECTIONS` (`src/surfaces/web_server/runtime.rs:51`) is an unbounded
  `BTreeMap` that deep-clones the full snapshot on every read.
- Every workflow claim receives its own projection cache directory,
  `cache/workflow/<claim_id>` (`src/workflow/execution.rs:226`) — a full copy of
  entire project state per claim. No code path prunes these. Disk growth is
  unbounded and proportional to claims ever created.

`promote()` (`src/workflow/automation.rs:16`) performs **two** full projection
resolutions per call — one via `promote_backlog_to_todo_for_refine_dir`, one via
`projection_snapshot` — and runs approximately every second
(`ACTIVE_WORK_REPLENISH_INTERVAL`, `src/workflow/mod.rs:24`; `WORKFLOW_INTERVAL`,
`src/process/runner.rs:37`), while holding the global lock.

#### The projection materializes every log line ever written

`rebuild_projection` calls `project_goal_round_activity`
(`project_state/store/activity.rs:49`), which walks every Goal, reads its
**entire** log file through `all_round_logs`, and materializes one
`ActivitySummaryProjection` — each with its own `searchable_text` — **per log
line**. The whole structure is held in memory and serialized into the snapshot
on disk.

Its consumer is `recent_activity_ids`, which takes 50 entries
(`projection_store.rs`). The full-corpus materialization exists to produce a
50-item dashboard list.

This is the dominant memory cost, and it scales with **failure volume rather
than Goal count**: every Governance rejection, quality failure, and retry
appends log lines, and every one of them becomes a resident projection entry on
the next rebuild. An environment with frequent Governance and quality failures
therefore degrades far more sharply than its Goal count predicts — which is the
observed deployment behavior that Goal count alone does not explain.

#### Root cause of the root cause: index and detail are fused

`GoalIndexProjection` (`src/model/goal/mod.rs:53`) is 14 scalar fields — roughly
200 bytes, and **everything the scheduler needs**.

`GoalSummaryProjection` (`src/tools/product/project_state/types.rs:54`) adds
`searchable_text`: every note body and every Round prompt, concatenated and
unbounded.

That second field is the sole reason projection rebuild must open and fully
parse every `goal.json`. The scheduler pays for full-text search indexing on
every tick.

### 2. Promotion is super-linear in Goal count

`src/workflow/automation.rs:47-63`:

```rust
.filter(|projection| Self::feature_claim_eligible(&snapshot, projection))
.filter(|projection| Self::priority_claim_eligible_excluding(
    &snapshot, projection, &quarantined_goal_ids))
```

- `feature_claim_eligible` (`src/workflow/policy.rs:495`) scans all Goals twice.
  Applied per Goal: **O(N²)**.
- `priority_claim_eligible_excluding` (`src/workflow/policy.rs:540`) scans all
  Goals, and for each candidate surviving its short-circuit conditions calls
  `feature_claim_eligible` — itself O(N). Applied per Goal: **O(N³)**.

The cubic term is gated by short-circuits (`status == Todo`, same node, strictly
higher priority), so the practical floor is quadratic. It reaches cubic exactly
when many Todo Goals share a Feature and a priority band — which is what a real
client backlog looks like and what a small dogfooding backlog does not.

Order of magnitude, per one-second tick:

| Goals | N³ predicate evaluations |
| --- | --- |
| 30 (dev) | ~2.7 × 10⁴ — invisible |
| 1,000 (client) | ~10⁹ — minutes of CPU on one 2.4 GHz core |

The scheduler cannot complete a pass before the next is due. Claims sit in
Running with no live executor; Ready Merge candidates go stale; merge evidence
drifts from branch state. This is the direct mechanism behind the eight
symptom-fix commits.

### 3. One global lock, held across unbounded-latency work

`acquire_workflow_coordination` (`src/process/supervisor/coordination.rs:27`)
takes a single exclusive `flock` on `.workflow-coordination.lock`, with a
**blocking acquire and no timeout**. It gates `promote()`
(`automation.rs:17`), every Goal mutation
(`work_items/service/persistence.rs:24`), and worktree cleanup
(`worktree_cleanup/mod.rs:105`).

The work held under it is unbounded:

- Worktree cleanup takes coordination *plus* the repository Git lock, then loads
  every Goal summary **and reads the full JSON record of every Goal into memory
  at once** (`worktree_cleanup/mod.rs:114-123`) — every 60 seconds.
- Every Git invocation is `Command::new("git")...output()` with **no timeout**
  (`src/tools/host/git_worktrees/commands.rs:130`,
  `src/tools/host/source_promotion/git_support.rs:178`). On a large repository,
  `git worktree list`, `fetch`, and `status` can take minutes.

A slow Git call on a large repository stalls the global lock, which stalls
promotion, while the one-second replenish timer keeps firing.

### 4. The same state is scanned three times, by subsystems unaware of each other

Three independent O(N) full-content traversals of the same durable state exist:

| Scan | Location | Purpose |
| --- | --- | --- |
| `rebuild_projection` | `project_state/store/projection_store.rs:76` | Build the projection |
| `collect_source_fingerprints` | `project_state/store/core.rs:135` | Invalidate the projection |
| `durable_state_map` | `git_sync/state_codec.rs:37` | Three-way merge baseline for sync |

The third reads and hashes every durable file and persists the map to
`STATE_BASELINE_FILE` under the Git common directory — node-local, pretty-printed
JSON — feeding `state_conflicts` for base/local/remote comparison.

Beyond the cost, three scans can disagree with each other about what is on disk.

### 5. Goal logs are synced against design intent (live defect)

`is_runtime_only_refine_path` (`git_sync/state_files.rs:105`) excludes paths whose
**first component** is `run | runtime | logs | support-bundles | provider-bin`.

Goal logs are written to `goals/<shard>/<rest>/logs.jsonl`
(`observability/logs/mod.rs:215`). The first component is `goals`, so the `logs`
exclusion never matches. `is_transient_refine_path` does not match them either —
it covers only `.lock`, `.tmp`, and `.refine-sync-*`.

**Every node's agent logs are therefore committed to `refine/state` and pushed to
every other node.** Design intent is that logs remain node-local and only the
implementation report is synced.

Git history is append-only, so volume already committed cannot be reclaimed by
excluding the files going forward without a history rewrite. In an environment
with frequent Governance and quality failures this compounds on every sync
operation.

This is independently actionable and should be fixed ahead of the structural
work.

### 6. There is no resource model (the missing capability)

Verified absent across the tree: no `available_memory`, no
`free_space`/`statvfs`, no `num_cpus`/`available_parallelism`, no cgroup or
rlimit awareness. The only memory settings that exist —
`worker_memory_limit_mb`, `ui_memory_limit_mb`
(`src/process/supervisor/config/settings_codec.rs:14`) — are pass-through
configuration for spawned subprocesses. **Refine never observes its own host.**

Consequently:

- Concurrency is a static constant: `global_limit: 2`
  (`src/workflow/mod.rs:79`), identical on a 2-core node and a 32-core node.
- Loop cadences are hardcoded (`src/process/runner.rs:37-43`).
- Timeouts are fixed wall-clock, e.g. `BACKGROUND_DAEMON_READY_TIMEOUT` = 120 s
  (`src/process/supervisor/lifecycle/mod.rs:18`).

The last point compounds the others: **a fixed wall-clock timeout on a machine
slower than the one it was tuned on fires on healthy work.** The system then
recovers components that never failed, which is itself a source of state
corruption.

## Design

### D1. Promotion becomes linear

Restate the existing semantics without changing them.

`priority_claim_eligible_excluding` returns true iff no *other* feature-eligible,
non-excluded Todo Goal on the same node has a higher priority rank. That is
equivalent to: **eligible iff the Goal's priority rank equals the maximum rank
over that set.** Compute the maximum once per node in a single pass, then filter.

`feature_claim_eligible`: bucket active Goals by `(node_id, feature_id)` once and
sort each bucket by `feature_order` once. Eligibility becomes a lookup within the
bucket rather than two full scans.

O(N³) → O(N). No format change, no migration, no index required. Self-contained
within `automation.rs` and `policy.rs`.

Note the asymmetry to preserve: the outer filter admits
`Todo | ReadyMerge | Build | Qa`, but the priority comparison considers only
*Todo* others. A ReadyMerge Goal is priority-eligible iff no feature-eligible
Todo Goal outranks it. The precomputed per-node maximum satisfies both cases.

### D2. Progress-based timeouts

Replace wall-clock deadlines with absence-of-progress deadlines. A component
fails only when there has been no observable progress — no output, no heartbeat,
no state change — for the configured window.

A slower machine then takes longer and is not told it failed. This is expected to
eliminate a substantial share of the state drift addressed by `65ecea02`,
`b4add9f9`, `d707b15f`, and `8dca163a`.

### D3. Node-local derived index, invalidated by Git

Introduce an index under `runtime/index/` — node-local, gitignored, never
committed, therefore **zero conflict surface**. It is fully derivable from
`goal.json` files; a corrupt or missing index is always safe to rebuild.

Format is line-oriented JSONL: append-only writes with periodic compaction when
tombstones exceed 50% of a shard. This preserves the project's plain-text
property for debuggability even though the file is not synced.

**Three change sources, three cheap paths:**

| Source | Detection | Cost |
| --- | --- | --- |
| Local mutation | Write-through; the mutator updates its own entry | O(1) |
| Remote sync | `git diff --name-only <indexed_sha> <state_sha>` on `refine/state` | O(changed) |
| Cold start / corruption | Full rebuild | O(N), once per clone |

The indexed SHA is stored in the index header. Git already maintains the changed-
file set; this replaces the O(N) stat walk of `source_fingerprints_match` with a
single `git diff`. The two sources are genuinely distinct — `git diff` cannot see
uncommitted edits in `refine-live-state/`, which is precisely why local mutations
must write through rather than rely on the SHA alone.

**Shard the index to mirror existing Goal sharding** (`runtime/index/<prefix>.jsonl`)
so compaction rewrites one shard rather than the whole file, and so partial loads
are possible.

### D4. Separate index, search, and detail

Split the three concerns currently fused in `GoalSummaryProjection`:

| Tier | Contents | Load policy |
| --- | --- | --- |
| Index | `GoalIndexProjection` only (~200 B) | Hot; always available |
| Search | `searchable_text` | Two-tier; see below |
| Activity | Round-log entries | Per Goal or per page, on demand |
| Detail | Rounds, notes | By Goal ID, on demand; never in bulk |

#### Activity is queried, not materialized

Round-log activity leaves the eager projection entirely. Nothing needs every log
line resident: the dashboard needs a recent slice, the Activity screen needs a
page, and a Goal view needs one Goal's entries.

- **Dashboard recency** comes from a bounded tail rather than a full-corpus sort.
- **Activity paging** reads only the Goals on the requested page.
- **Per-Goal activity** reads that Goal's sidecar.

`GoalSummaryProjection::activity_ids` and `DashboardProjection::recent_activity_ids`
stop being precomputed cross-references into a resident map. Because activity
volume tracks failures rather than Goals, this is what decouples memory from how
badly the fleet is doing — the current design charges the scheduler for every
retry the system has ever performed.

#### Search is two-tier

Search is used often, usually scoped, occasionally global. These are different
problems:

- **Scoped search requires no new machinery.** Every field users scope by —
  `status`, `feature_id`, `node_id`, `reporter`, `assignee` — is already in
  `GoalIndexProjection`. Filter on the index first, then read detail only for
  survivors. Scoping typically narrows to hundreds of records.
- **Global search uses a lazily built inverted index** — node-local, sharded to
  match Goal sharding, built on first global query rather than eagerly, then
  maintained incrementally by the same change detection as D3. Cost is not paid
  until the capability is used.

Because Goal logs are node-local (D8), `searchable_text` covers only notes and
Round prompts from `goal.json` — a materially smaller corpus than the current
projection assembles.

**The hot set is the decision that makes 5M Goals work.** At 5M Goals even a
200-byte index is ~1 GB — loadable on a 20 GB node but not parseable every tick.
The scheduler only ever cares about non-terminal Goals; in a mature project the
overwhelming majority are Done or Cancelled.

Maintain `runtime/index/active.jsonl` containing only Goals in
`Todo | InProgress | ReadyMerge | Build | Qa | Review`. The scheduler loads only
this. **Its size is bounded by work in flight, not by project history.** Goals
leave the hot set on reaching a terminal status. Dashboard totals come from
maintained counters rather than record scans.

Combined with D1, the scheduler's N becomes hundreds of active Goals rather than
millions of total Goals.

**Delete `cache/workflow/<claim_id>` outright** rather than fixing it. With an
index and on-demand detail loading, a claim needs its Goal record at a pinned
revision, not a copy of the whole project.

### D5. Replace the global lock with three targeted mechanisms

The single global `flock` conflates three unrelated problems. Address each
directly.

| Problem | Mechanism |
| --- | --- |
| Concurrent `goal.json` writes | Optimistic concurrency on the existing `workflow_revision` field (`work_items/service/record_persistence.rs:59`): read, modify, compare-revision, atomic-rename write; retry on mismatch |
| Shared claim / automation state | **Single-writer.** One scheduler owns `workflow-automation-state.json`. Web and CLI enqueue intents as individual files under `runtime/intents/`. Separate files never conflict, so no lock is required |
| Git serialization | A repository lock scoped to the **individual Git command**, with a hard command timeout and a deadline on acquisition. Never held across state logic |

`workflow_revision` already exists on every record; the optimistic-concurrency
primitive is half-built. Nothing blocks indefinitely, and nothing holds a lock
across an unbounded operation.

#### Mutations become asynchronous

Mutating requests return `202 Accepted` and the surface polls for completion,
rather than blocking until the scheduler drains the intent. This aligns with the
lazy-loading model in D4 and keeps request latency independent of scheduler
state.

Intents are one file per intent under `runtime/intents/`, named with a monotonic
sequence and a UUID; a sorted directory listing gives total order.

Crash semantics require no additional bookkeeping. The scheduler reads, applies,
then deletes. A crash between apply and delete replays the intent — and since
apply is a compare-`workflow_revision` write, the revision has already advanced,
so replay is a no-op. **Optimistic concurrency makes intent replay idempotent for
free**, which is a direct consequence of choosing D5's mechanism for Goal writes.

### D6. Resource governor

Sample at startup and periodically thereafter:

- Cores via `std::thread::available_parallelism`
- Available memory via `/proc/meminfo` `MemAvailable` (Linux), `sysctl` (macOS)
- Free disk via `statvfs`

**Scale up, not merely down.** The present flat default of 2 ignores a capable
machine entirely. Derive concurrency from measured capacity with a floor of 1.

**Measure rather than assume.** Feed observed agent RSS back into the budget so
limits reflect real cost. This is how the governor stays aggressive without
gambling — it learns the actual footprint instead of reserving against a
pessimistic guess.

**Derive loop cadence from capacity** rather than hardcoding
`src/process/runner.rs:37-43`.

The one place to be strict is disk: check free space against observed worktree
size *before* creating a worktree. Refusing to start is backpressure; half-
creating a worktree on a full disk is corruption.

### D7. One index serves the scheduler, invalidation, and sync

The three traversals in Root Cause 4 collapse into one node-local index under
`runtime/index/`. One entry per durable state file:

| Field | Updated by | Recovery if lost |
| --- | --- | --- |
| `path` | traversal | from disk |
| `current_hash` | write-through on mutation; `git diff` on remote sync | recompute from disk |
| `synced_hash` | successful push/pull only | recompute from `refine/state` at the header SHA |
| Parsed index fields | write-through; `goal.json` / `feature.json` only | reparse from disk |

The index header records the indexed commit SHA. **This is the same SHA that
identifies the sync checkpoint** — D3's invalidation reference and the sync
baseline are the same value, which is the clearest signal the two mechanisms are
one.

This preserves three-way merge semantics rather than compromising them. The
baseline's defining property is that it lags — it is state as of last successful
sync — and that is retained by making the checkpoint a *column* (`synced_hash`)
rather than a separate file. `current_hash` and `synced_hash` have different
update triggers, which is a reason for two columns, not two structures.

The historical column is recoverable despite not being derivable from working-
tree state: `synced_hash` is a checkpoint *of `refine/state`*, so it can be
recomputed by reading files at the recorded commit. Losing the index is therefore
never lossy, only expensive — the same rebuild-safe property the rest of D3
depends on.

Scope widens accordingly: index every durable file, carrying parsed fields only
where they apply. Sync reads its merge base from a column instead of performing
a fresh O(N) content scan.

### D8. Goal logs become node-local

Fixes Root Cause 5. Two parts, and the second matters more than the first.

**Stop syncing them.** Goal logs are node-local by design; only the
implementation report is durable synced state.

**Move them out of `refine-live-state/` into `runtime/`.** Adding an exclusion
rule alone would stop the bleeding, but leaves ephemeral data in the durable
namespace where the next exclusion-matching bug can resurface — which is exactly
how Root Cause 5 arose. Relocation makes the category error unrepresentable.

Consequently the projection stops fingerprinting `logs.jsonl` (`core.rs:109`,
`core.rs:145`, `projection_store.rs:136`). Today node-local ephemeral data
invalidates a projection of durable state; that path disappears.

Retention simplifies once logs are node-local, since no cross-node coordination
is involved: a per-Goal size cap with rotation bounds runaway retry loops, and
compaction on reaching a terminal status keeps historical Goals cheap.

Volume already committed to `refine/state` is not reclaimed by this change. That
requires a separate history-rewrite decision, out of scope here.

## Sequencing

| # | Change | Depends on | Rationale |
| --- | --- | --- | --- |
| 0 | D8 — stop syncing Goal logs | — | Live defect; compounding cost on every sync; independently shippable |
| 1 | D1 — linear promotion | — | No format change; largest immediate relief; verifiable against existing `ready_merge` and `capacity` suites |
| 2 | D2 — progress-based timeouts | — | Stops the false-failure cascade so real behavior becomes observable |
| 3 | D3 + D4 + D7 — unified index, hot set, tier split | — | The structural fix |
| 4 | D5 — lock redesign and async mutations | 3 | Optimistic concurrency is only cheap once reads are cheap |
| 5 | D6 — resource governor | 2 | Independent; benefits from progress-based timeouts landing first |

D8 splits into two shippable pieces: the exclusion fix stops the bleeding
immediately, while relocating logs into `runtime/` can land with D3/D4, since
that is when the projection stops fingerprinting them anyway.

## Resolved Decisions

- **Search is two-tier** — scoped search filters on index fields with no new
  machinery; global search uses a lazily built node-local inverted index. See D4.
- **Goal logs are node-local** and relocate out of `refine-live-state/`. Only the
  implementation report remains durable synced state. See D8.
- **The index and the sync baseline unify** into one structure with separate
  `current_hash` and `synced_hash` columns. See D7.
- **Mutations are asynchronous** — `202 Accepted` plus polling, not a bounded
  synchronous wait. See D5.
- **External-edit detection needs no separate mechanism.** The earlier concern
  that hand-edits to `refine-live-state/` bypass write-through is resolved by
  D7: `current_hash` is a content hash, so external edits are detected the same
  way sync already detects them today.

## Acceptance Criteria

- Promotion cost is linear in active Goal count. A synthetic backlog of 10,000
  Todo Goals sharing a Feature and priority band completes a promotion pass
  within one `ACTIVE_WORK_REPLENISH_INTERVAL`.
- Scheduler working-set memory is bounded by active Goal count and does not grow
  with total Goal count. Verified with a 1M-Goal fixture where >99% are terminal.
- No projection rebuild is triggered by a Goal write. A single Goal mutation
  updates O(1) index entries.
- Remote state changes are detected in O(changed files); no code path stats every
  Goal file on a cache-hit.
- `runtime/` contains no unbounded per-claim caches; disk usage attributable to
  Refine state is bounded and reclaimed.
- No lock is held across a Git invocation. Every Git command has a hard timeout;
  every lock acquisition has a deadline.
- A component is never marked failed while making observable progress, at any
  machine speed.
- Concurrency observably increases on a higher-resource host and decreases on a
  constrained one, without configuration changes.
- Disk exhaustion produces refusal-to-start with a clear diagnostic, never a
  partially created worktree.
- No `logs.jsonl` file is present in the durable state tree or reachable from a
  `refine/state` commit created after D8 lands. A regression test asserts the
  exclusion holds for sharded per-Goal paths, not only top-level directories.
- Durable state is traversed once per reconciliation, not three times. Sync
  obtains its merge base from an index column without a content scan.
- Scoped search over a filtered subset performs no full-corpus read.
- A mutating request returns without waiting on scheduler progress, and a
  scheduler crash between intent apply and delete produces no duplicate effect.
- Reference environment (20 GB RAM, 250 GB disk, 2 × 2.4 GHz cores) sustains a
  target app with ~5M Goals and 2 parallel agents through repeated Governance and
  quality failures without state drift.

## Open Questions

- **Reclaiming already-synced log volume.** D8 stops future growth, but volume
  already committed to `refine/state` persists in history. Whether to rewrite
  that history — and the coordination cost across nodes if so — is undecided.
- **Migration for existing deployments.** Relocating logs from
  `refine-live-state/` to `runtime/` needs a migration step for nodes that
  already have them in place, including what happens to logs that exist only in
  the synced branch on a node that never wrote them.
- **Inverted index sizing at N ≈ 10⁶.** The lazily built global search index is
  specified in shape but not in format. Term-based versus trigram postings, and
  the resulting on-disk footprint against the 250 GB budget, need measurement
  before commitment. Deliberately deferred until D3/D4 land and real corpus sizes
  are observable.
- **Concurrency formula for D6.** The governor is specified to derive concurrency
  from measured agent RSS rather than a fixed constant, but the actual function
  requires observed footprint data that does not exist yet.
- **Index compaction thresholds.** The 50%-tombstone trigger is a starting
  heuristic, not a measured one.
