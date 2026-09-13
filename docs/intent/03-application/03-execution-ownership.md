# Execution Ownership

## Key Ideas

- **Semantic Ownership Is Synchronized**: Goal status and node assignment say which node may advance the work.
- **Worker Ownership Is Transient**: a node may track its current workers in memory and local process records.
- **Completed Evidence Is Reusable**: replacement workers consume accepted receipts; failed verdicts require a workflow decision before another attempt; unfinished work continues after prior execution ownership is proven released.
- **Idempotence Over Reservation**: workers prove or reuse semantic results instead of reserving durable execution slots.
- **Decisions Precede Cleanup**: a different-step selection changes the Goal first; the daemon then stops superseded execution.

## Purpose

Execution ownership answers two different questions without combining them into one persisted concept:

- Which node is allowed to move this Goal forward?
- Which local worker, if any, is currently trying to do that work?

The synchronized Goal answers the first question. Node-local runtime state answers the second. Keeping them separate prevents process lifetimes, restarts, and partial synchronization from becoming part of the product's durable workflow model.

## Expected Role

A node schedules a Goal only when its status is actionable and its `node_id` names that node. The scheduler keeps an in-memory set to avoid launching the same Goal twice in one runner and observes live managed processes for soft capacity. These are efficiency controls, not authority.

Each worker receives the Goal, Round, selected context, and semantic instructions. Before a transition or consequential side effect it rereads the synchronized Goal. If the step occurrence, status, node, or Round changed, it stops. The existing Goal lifecycle generation identifies an occurrence even when a step name repeats. An already started Git publication child may finish its individual operation; preparation is stoppable and each subsequent publication rechecks the current occurrence. A local integration awaiting push remains incomplete history until publication finishes. A daemon restart may replace the execution process and consume completed planning, Git, quality, and integration receipts. A failed verdict requires an explicit decision before another attempt. An interrupted execution with no completed verdict remains scheduled in the current step and Round; a replacement continues retained work only after prior execution is proven stopped.

Two nodes may briefly believe work is available because synchronization is delayed. Durable state convergence decides the outcome. A valid one-sided reassignment is preserved when it races with automated start on the old node. Independent conflicting decisions remain visible through state-sync conflict handling rather than being hidden by timestamps or local runtime records.

Stop and Cancel have different meanings. Stop targets a local process and conditionally fails an unchanged Goal. Cancel writes terminal synchronized Goal intent and then attempts to stop matching local processes. Governance rereads Round, current step occurrence, node, status, and current decision before candidate refresh, replacement Quality, replacement Governance, publication, integration, and settlement. Cancellation before the leased integration boundary blocks further work; once publication or integration has begun it does not trigger an unsafe rollback, and exact integration evidence is still recorded.

Planning and Quality reread the originating Round, step occurrence, node, status, and process cancellation state before accepting a provider response. A late response from superseded work remains observable but cannot mutate replacement work. Refine does not silently repair a completed provider verdict by relaunching it.

Explicit workflow selection uses the same intent-first ordering as cancellation. The current step is a no-op assignment, while a different step records the decision and supersedes the old occurrence before process work. Independent daemon maintenance then stops every obsolete managed Goal agent, including a Skill that submitted the action, using its original registration and process-group evidence. The occurrence records the cleanup boundary; there is no separate execution clock or request-local cleanup queue. Normal automated phase transitions keep their enclosing execution alive. Cleanup also runs for parked and terminal Goals and while admission is paused; uncertain termination is reported and retried without undoing the decision or stopping a newer replacement.

Preparation on every executable step shares the same workspace and evidence rules. Coherent work is reused or reconstructed. Unusable generated work may be left as history and reproduced from the existing authored Round on a fresh execution branch, with a recorded return to Plan and applicable checks. A restart alone does not reopen a completed failed verdict or invent a missing request.

Retired execution-coordination and cancellation-journal files are one-way cleanup inputs. They are removed during recovery and never imported into the new authority model.

A workflow worker incarnation has a unique launch token tied to its port runtime, managed process registration and OS start identity. Scheduling observations belong to that exact incarnation and canonical target; a reused PID, another runtime or an earlier target cannot make a replacement healthy. Runtime-local ownership evidence connects child work to the worker that launched it without granting Goal mutation authority. New managed Linux launches use a dedicated subreaper established before workload execution: orphaned descendants retain that ancestor across setsid and reparenting, including when their parent exits and is reaped before any observation. The guardian runs in its own small executable image and writes an identity-bound exit receipt only after the kernel reports that no children remain. Legacy, externally registered and unsupported-platform work lacks that complete lifetime proof and remains explicitly unverified when coverage cannot be established. Child processes inherit the ownership token but do not become schedulers: worker target fencing uses explicit launch context, and tick publication requires the actual worker registration. Independent observers merge descendant witnesses under a bounded shared record lock so stale observations cannot erase ownership evidence.

The daemon's workflow recovery path holds one local launch fence. It stops the old worker, rescans the final set of owned groups, confirms exit and then waits for a replacement tick. One shared ownership assessment distinguishes live work, proven exit and unverified evidence. Live and unverified ownership retain deduplicated capacity, operations, transcripts and worktree protection even after loss of the primary registration. An empty scan, an old confirmed-exit boolean or later quiescence cannot repair an earlier coverage gap. Recovery refuses replacement and shared health identifies the process, uncertainty and supported status/doctor inspection commands. It retains group and recovery evidence on inspection, termination or readiness failure. Cleanup and deadline maintenance remain independently runnable during this wait. Status, process diagnostics and next-action guidance consume the same Application health assessment; Infrastructure supplies observations and termination mechanisms.

PTY session settlement treats the workload result and the execution scope separately. A validated workload-status receipt finalizes provider output and structured results; guardian status only describes the ownership helper. Completion, natural exit, capture faults and deadlines use shared bounded scope termination. Missing proof retains local capacity and artifacts, refuses replacement and reports uncertainty without granting new Goal settlement authority. A failed scope settlement preserves the original provider failure and any structured workload result in local evidence.

Standard subprocess capture also separates workload results from scope exit. A bounded final drain releases the caller while surviving or unverified descendants still consume deduplicated capacity and protect evidence. Incomplete capture is explicit and cannot be accepted as a complete machine-readable result. This does not grant new Goal mutation authority or permit worker replacement before proven scope exit.

Worker replacement uses the shared process ownership assessment to recover a missing group record from a verified, complete original launch-scope receipt. It retains uncertain ownership and artifacts when that evidence is missing or invalid. A recovered exited scope releases capacity without signalling any current process; replacement health still requires the new worker's own scheduler tick.

Claims record responsibility and provenance within the current workflow occurrence. Replacing or retaining a claim does not determine whether work is eligible. Earlier claims, results and retained work survive supersession as history. Unresolved settlement is assessed against the current occurrence by the shared workflow recovery and eligibility assessment; runtime failure evidence never survives a later workflow decision as a veto.

## Future Direction

Refine should keep execution ownership proportional to the cost of duplicate work. If future non-idempotent operations require stronger coordination, that protection should be scoped to the side-effect boundary itself rather than recreating a general durable worker-lock system for every workflow step.
