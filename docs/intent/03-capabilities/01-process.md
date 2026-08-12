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
- Process owns one effective launch environment assembled in actual precedence
  order from inheritance, configured agent values, per-process overrides, and
  direct API-key removals. Managed processes and PTYs apply that owned value
  after clearing their builders, so every final agent argv is preflighted
  against the exact environment passed to spawn, with per-string and aggregate
  budgets. Native stdin remains available where it preserves provider
  semantics; environment pressure can reselect an operation-owned prompt file
  and bounded bootstrap. Spawn errors preserve their original OS cause and
  never trigger blind E2BIG retries.
- workflow Goal Agents run as PTY-backed managed processes with shared transcript,
  input, resize, attention, and lifecycle state, so CLI and browser attachments
  observe the same process.
- execution-time plan, criticize, revise, and implement phases each have a
  distinct managed-process and operation identity under the same workflow
  execution. Process metadata names the active implementation phase so shared
  attachment, cancellation, pause, logs, failure, and settlement operate on the
  current process rather than launching a duplicate.
- the daemon remains a responsive control plane while supervised runners own workflow and Git synchronization waits.
- one shared host daemon-lifecycle capability selects port-scoped systemd or
  launchd control versus supervised direct-process fallback from the selected
  runtime and installation context. CLI, HTTP/API, update, and
  maintenance callers submit lifecycle intent without owning parallel
  reachability, confirmation, evidence, recovery, or legacy-migration state
  machines. Work initiated inside the managed daemon that must stop or restart
  it first persists its operation receipt and moves execution into a separate
  systemd transient service, launchd submitted job, or detached direct-process
  session. The handoff then records the same observed lifecycle status,
  evidence, failure, and recovery guidance as synchronous host callers. Source
  promotion durably backs up and replaces an activated service registration
  with the built candidate before shutdown. If candidate verification fails,
  rollback retains the backup until the prior registration is restored and
  verified, forcibly replaces the live candidate through the shared
  service-manager restart path, and fresh-probes reachability and live
  executable identity. It records success only when the registered and live
  executables both resolve to the prior binary; command failures, a surviving
  candidate, and indeterminate identity remain explicit partial evidence with
  actionable recovery. Promotion success likewise requires the live daemon to
  identify the candidate executable.
  Direct fallback shutdown retains signal errors and uses a fresh tri-state
  reachability probe; only observed non-reachability is persisted as stopped.
- todo is a state-only queue. Promotion and claiming do not create a repository
  copy; only a capacity-admitted running claim may durably transition its Goal
  to in-progress and then materialize that Goal round's implementation worktree.
  A large todo queue therefore consumes workflow state, not one worktree per Goal.
- admission and cadence adapt to observed host CPU, memory, and disk headroom
  unless an operator deliberately sets a cap. Slow operations remain healthy
  while they make observable progress; silence and hard elapsed limits are
  distinct failure signals.
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
  separate shared maintenance operation outside Stop; ordinary retention is a
  successful result, not a partial failure. The operation is dry-run by default
  when invoked directly and a separately supervised cleanup worker applies the
  configured inactive-worktree retention delay even while the workflow worker
  is occupied. The default makes inactive worktrees eligible immediately and
  the cleanup worker scans every registered target app at least once per
  minute. Goal status controls workflow semantics, not physical checkout
  retention: any clean managed Goal worktree without a live Workflow claim,
  operation, or process may be hibernated. Recognized or explicitly configured
  generated cache paths may be removed first, but any other ignored content
  protects the worktree. Dirty, ambiguously owned, standalone, and state
  worktrees are retained. Recoverable candidate branches remain available for
  lazy recreation. A Refine-owned branch may be deleted locally and upstream
  only when durable integration evidence names its exact candidate, that exact
  SHA is still the branch tip, and the candidate is an ancestor of the local
  and remote target tips. Both deletions use exact-SHA compare-and-delete
  fences, so an advanced branch is retained. Termination intent is an
  explicit authoritative input to the shared capability: interactive user Stop
  ordinarily requeues, while Stop of a workflow Round carrying governed
  implementation-planning evidence fails that attempt so only a fresh Round can
  receive a new claim and execution identity. Explicit Goal or workflow
  cancellation remains terminal cancelled. Process discovery and claim presence
  are settlement evidence, never a source for guessing that intent. Explicit cancellation has monotonic
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
