# Process

## Key Ideas

- **Supervisor Ownership**: the system should know which processes it owns and why they exist.
- **Observable Execution**: long-running work should produce inspectable status, logs, and controls.
- **Recoverability**: process state should survive daemon restarts well enough to explain current reality.
- **Bounded Power**: Refine should run useful commands while keeping command ownership and authorization explicit.
- **Surface Independence**: process control should be available to CLI, browser, API, and agents through shared capability.

## Purpose

Process management exists because agentic software work is not just data mutation. Refine has to run target apps, agents, quality checks, import extraction, maintenance tasks, terminal sessions, and background operations.

Those processes need to be visible. A user or agent should be able to answer: what is running, who started it, what is it doing, where are its logs, can it be stopped, and did it succeed?

## Expected Role

The process capability should be the local operating substrate under workflow and tools. It should make process execution durable enough for real work without turning Refine into a general-purpose operating system.

Current implementation details that matter to intent:

- managed processes have owners such as daemon, runner, target app, agent, quality, import, maintenance, and user helper.
- process records include pid, state, label, details, output paths, limits, start time, and exit code.
- process metadata can attach workflow, Goal, session, mode, and runner context.
- managed agent launches carry prompt transport evidence rather than prompt
  content: transport kind, UTF-8 byte count, SHA-256, owner, and lifecycle.
  Authorization strings, routine process details, and persisted stdin redact the
  authoritative prompt. Process and session ownership retain any secure prompt
  file until the provider reaches a terminal handoff; recovery retains files
  for verified live owners and reaps only verified orphans.
- every final agent argv is preflighted against per-argument and aggregate
  argv-plus-environment budgets before spawn. Native stdin remains available
  where it preserves provider semantics; oversized argv prompts use an
  operation-owned runtime file and a bounded bootstrap. Spawn errors preserve
  their original OS cause and never trigger blind E2BIG retries.
- workflow Goal Agents run as PTY-backed managed processes with shared transcript,
  input, resize, attention, and lifecycle state, so CLI and browser attachments
  observe the same process.
- the daemon remains a responsive control plane while supervised runners own workflow and Git synchronization waits.
- pause state can stop background processes or pause agents.
- stopping any managed, interactive, chat, or synthetic agent uses one shared
  capability, including Workflow claim settlement, rather than splitting
  process and Workflow semantics. A user Stop retains ownership evidence,
  confirms exact process exit, and records that outcome before registry or
  identity cleanup. It then releases the exact claim and capacity, returns the
  Goal to todo under the Node owner pinned before exit, and settles the operation
  and receipts. Stop never removes workflow worktrees or branches, regardless of
  whether they are pristine, tracked-dirty, untracked, ignored, or changing
  concurrently. Every recorded worktree and branch remains available with
  explicit durable retention and recovery evidence. Worktree cleanup is a
  separate explicit human-controlled operation outside Stop; ordinary retention
  is a successful result, not a partial failure. Termination intent is an
  explicit authoritative input to the shared capability: interactive user Stop
  requeues, while explicit Goal or workflow cancellation remains terminal
  cancelled. Process discovery and claim presence are settlement evidence, never
  a source for guessing that intent. Explicit cancellation has monotonic
  precedence: when durable Goal state is already cancelled, or becomes cancelled
  after Stop preflight, Stop may finish process, operation, claim, and capacity
  settlement but cannot return the Goal to todo. Journals, process receipts,
  replay, partial evidence, and surface results record the requested intent
  separately from the authoritative intent so recovery preserves that precedence.
  Workflow spawn-to-registration and cancellation share one execution fence:
  an empty target-bound process lookup is never treated as exit evidence, and a
  delayed worker cannot launch after its exact claim has settled.
  Settlement atomically validates and cancels the exact claim, applies the
  requested linked-Goal outcome, releases claim capacity, and preserves the
  execution, round, pinned Node owner, Goal revision/status, competing-owner,
  and retained-worktree fences. Claim, capacity, Goal, and retention evidence
  are journaled with their exact before/after states as one semantic settlement.
  Schema-compatible replay accepts prior cancellation and cleanup journals but
  never resumes their destructive worktree-removal stages; all still-present
  targets become retained recovery evidence. A semantic write failure restores
  the exact semantic pre-settlement state while advancing
  monotonic workflow, claim-decision, and Goal revisions to record the attempted
  settlement and rollback. An interruption is replayed before terminal-state
  shortcuts after restart. Settlement journals synchronize their temporary file
  before atomic replacement and synchronize the containing directory after the
  rename. Rollback evidence records each Goal, claim, and capacity restore
  outcome plus the exact restored Goal value, so replay can accept the
  restoration's monotonic revision without weakening its ownership fence or
  overwriting newer work. Workflow coordination is acquired before workflow
  and Goal mutation locks, including concurrent Ready Merge work, so settlement
  cannot deadlock or bypass those fences. Both normal persistence and replay
  preserve the complete existing workflow policy and target-app context, and
  confirmed process exit remains explicit. A stop uses the Goal owner pinned by
  the validated process/claim fence instead of rediscovering whichever Node the
  serving daemon currently calls active. Missing registration-time PID
  identity, mismatched or stale workflow ownership, failed or resisted stops,
  and process cleanup failures keep the Goal active, while every post-exit failure
  retains a truthful receipt with exit, registry/identity cleanup, claim/Goal,
  retained-worktree, cause, and supported recovery evidence.
  Single and bulk Goal cancellation both delegate to this capability. Bulk
  cancellation settles each selected Goal independently and returns explicit
  per-Goal failures, so one partial outcome is never reported as batch success.
  Public cancellation results report `cancelled: true` only after reading back a
  durably cancelled Goal; partial process, claim, capacity, or replay outcomes
  retain structured evidence without claiming terminal Goal success.
- the browser System and Processes surfaces read shared process state rather than inventing their own status.
- live observers enumerate and capture output through the shared Process
  capability. A listed process record or output artifact removed by ordinary
  reaping before capture is a terminal, skipped observation; permission,
  persistent I/O, serialization, and process-identity failures remain
  actionable.
- the browser Goal terminal resolves its session to the managed Goal Agent and
  delegates Stop to this same capability; only non-Goal local terminals retain
  process-only terminal shutdown behavior.

Process management should favor visibility and recovery over hiding execution behind polished UI messages. If something is running, failing, or waiting, Refine should be able to show it.

## Future Direction

As agent fleets grow, process management should evolve toward orchestration: resource limits, queues, priorities, remote nodes, cancellation, isolation, provenance, and health checks.

The future process layer should support many agents and target apps without losing local debuggability. Superintelligent systems may automate most decisions, but they will still need an observable substrate that explains what was launched, what happened, and what changed.
