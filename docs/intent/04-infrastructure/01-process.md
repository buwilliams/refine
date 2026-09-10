# Process

## Key Ideas

- **Local Observation**: process records describe work observed on one node; they are not synchronized workflow authority.
- **Supervisor Ownership**: Refine should know which processes it owns and why they exist.
- **Recoverable Work**: stopping or losing a process should preserve Goal, Round, branch, worktree, and evidence so work can be started again cheaply.
- **Soft Capacity**: admission uses observed live processes and configured limits, not durable reservations.
- **Application Authority**: Application decides why work runs and whether it remains authorized; Infrastructure observes, launches, supervises, and terminates processes.
- **Shared Mechanism**: every Application path and Surface uses the same process supervision rather than inventing its own runtime ownership.
- **Checkout Ownership**: one product home owns its executable and every port runtime beneath its `run/` directory.

## Purpose

Refine runs target apps, agents, quality checks, imports, maintenance tasks, terminals, and background operations. Process Infrastructure makes that local execution visible and controllable without making process identity part of synchronized product state. Application supplies the purpose, authority, and settlement rules; process supervision supplies the host mechanism.

A user or agent should be able to answer what is running on this node, why it was started, where its output is, whether it is alive, and whether it can be stopped.

The product home is derived from the executable that was invoked, never from
the caller's working directory, home directory, XDG state, or platform support
directory. A source product home is the exact Git checkout or linked worktree;
a published product home may be gitless when its release marker and
`bin/refine` identify the deployed release. `./r system service-install`
bootstraps those deployed artifacts when the production binary is missing,
then registers and activates
the port-scoped OS service. It does not acquire or update Refine source; that
authority remains with the installation runbook and `./r system update`. Both
modes own `<product-home>/run`, and
port `P` owns only `<product-home>/run/P`. Stateful helpers and provider Agents
must receive that exact port root. Relative `run` is compatibility syntax for
the owning checkout, not an invitation to resolve against the caller's CWD.
Executable mode is independent of runtime ownership: an installed invocation
and its children use `<product-home>/bin/refine`, while a source/debug
invocation and its children use the active checkout-owned Cargo executable.
Workers and direct lifecycle handoffs inherit that active executable; only
installed service registration, deployed update, and source activation require
the stable `bin/refine` path.

The running executable also carries immutable build provenance. A published
release classification requires an exact source build whose commit has the
semantic version tag matching the executable's package version. Every other
executable is a source runtime. Source status compares that embedded commit
with the owning checkout's live HEAD and with the local and upstream commit
identities from the durable source-update cache. Refine claims that the
executable is running from HEAD, or that HEAD is current, behind, ahead, or
diverged from upstream, only when all identities agree and the cache is fresh;
missing, stale, dirty-build, or mismatched provenance remains explicitly
unknown.

Refine owns no configuration or state files outside the synchronized project
state and `<product-home>/run`. Agent credentials and toolchain configuration
belong to the host: Refine invokes agents with the host's shell environment —
captured from the login shell because a daemonized Refine does not inherit it —
and never reads Refine-owned files from the user home, XDG paths, or anywhere
else to assemble it.

Quality commands also need to discover host toolchains that an interactive
shell places on `PATH`, but repository-selected commands must not receive the
shell's arbitrary exports or credentials. The process launch policy therefore
overlays only `PATH`, `PATHEXT`, and the vetted non-secret .NET discovery roots
`DOTNET_ROOT`, `DOTNET_ROOT_X64`, and `DOTNET_ROOT_X86` for Quality, between the
daemon's inherited environment and explicit process overrides. Capture failure
leaves the inherited environment unchanged. Refine does not hardcode a dotnet
installation path; changes to shell startup configuration take effect after the
daemon restarts and recaptures the host-shell environment.

## Expected Role

Managed processes record local facts such as owner, pid, state, label, output paths, limits, start time, exit code, Goal, Round, workflow state, provider, and target app. Node-local identifiers may connect local logs, sessions, operations, and child processes, and evidence may cite them as provenance. They do not grant permission to mutate a Goal or act as resumable workflow state. This is the Infrastructure boundary: mechanisms report what the host did while Application remains responsible for what that evidence means.

Goal status and node assignment determine whether automated work is still authorized. A worker rereads those fields before a workflow transition or consequential side effect. Process records help avoid needless duplicate work on one node and provide controls; they are advisory when deciding capacity or recovery.

Automatic agent admission is conservative for a shared host. It spends no more
than the node's configured percentage of detected logical cores and currently
available memory. The product default is 70 percent, leaving 30 percent for
Docker, target apps, and other expensive processes outside Refine's ownership.
Admission always permits at least one slot and reserves at least 2 GiB for each
agent workload. A workload is the managed agent root together with all of its
descendants, so provider subprocesses, builds, and checks are included rather
than hidden behind a small parent RSS. Only a complete stable live process-tree
sample may raise the observed reservation above that floor; an incomplete tree
retains the floor, and unavailable host-memory telemetry falls back to one
automatic slot.

Stopping a Goal worker confirms its exit, retains its branch and worktree, and conditionally returns the same Goal attempt to `todo` only if the Goal status, Round, update, and node assignment still match the worker's starting observation. Explicit Goal cancellation is monotonic: the Goal becomes `cancelled` first and local process termination follows as best-effort cleanup. Neither path rolls synchronized Goal state back because local cleanup failed.

After daemon restart, live-process recovery terminates stale owned workers and removes retired execution-coordination files. Any nonterminal Goal remains eligible for a new idempotent worker. Planning artifacts, Git observations, and semantic outputs remain durable; a prior process identity is only provenance, not ownership that must be recovered.

Restart-safe source activation is the bounded exception to ordinary supervisor
ownership because the helper replaces the daemon and supervisor that launched
it. The operation registry remains authoritative through a revision-fenced
attempt: reservation is not liveness, submission records a mechanism-specific
identity and receipt, and one helper must atomically claim with its attempt
nonce before any side effect. Recovery observes that exact systemd, launchd,
or detached-process identity; it adopts one live claimant or settles zero,
stale, duplicate, or ambiguous evidence visibly and retryably. Cancellation
fences claims first and cannot become terminal until the exact helper is gone
or safely reconciled.

Source inspection and promotion additionally require the owning product home
to be a usable Git checkout. A gitless release remains fully valid for normal
CLI, daemon, web, MCP, provider, and published-update operation, but source Git
commands fail closed with an actionable diagnostic. Candidate activation
replaces the stable `<product-home>/bin/refine` atomically; daemon registration
continues to name that stable path while the existing attempt-fenced handoff
proves restart and rollback identity.

Worktree cleanup is separate from Stop and cancellation; neither process action deletes evidence. Retention-delayed maintenance may hibernate a clean inactive worktree when no live local process or operation uses it, while dirty, ambiguous, standalone, and state worktrees remain protected. The same maintenance pass may retire a done, cancelled, or deleted Goal's local and configured-remote round-ref names only after proving their exact tips reachable from the exact remote merge-target snapshot. Candidate and target movement fails closed, checked-out branches retain their local ref, and Goal, Round, process, and target-history evidence remains inspectable.

`workflow_paused` is the canonical shared automation gate. Pausing blocks new Goal admission and lets automatic Git sync and inactive-worktree cleanup quiesce at safe repository-operation boundaries. Already active Goal executions continue unless their Agents are stopped separately. The daemon, API, and runner supervision remain available; quiesced repository workers settle normally instead of being treated as failed or permanently terminated. Resuming makes admission and those workers eligible to run again.

Managed Goal agent launches require explicit cwd and a retained workspace admission, including provider-session resumes and fresh-launch fallbacks. Process Infrastructure revalidates the linked registration immediately before launch; Application supplies and checks the Goal, Round, and candidate authority. Inherited Git repository, worktree, index, object-store, and configuration redirection is removed after host-shell capture for agents, Quality checks, and managed Git commands. Deliberate internal per-command overrides, such as an isolated temporary index, remain supported. These checks do not attach general Toolbar or standalone sessions to a Goal.
Registration-time owned-group records preserve the original process identity, runtime, start time and descendant witnesses independently of the active process record, including when identity is already unavailable at registration. A leader exit does not prove group exit. Linux managed launches establish a dedicated subreaper before executing the workload. Its small executable image retains orphan ancestry across setsid and parent exit; only a kernel no-children result produces a receipt bound to that registration, runtime, workload PID and proof-file identity. The workload cannot inherit the receipt descriptor. Launch uses a bounded registration/start handshake; no workload runs before its identity and scope are registered. Standard and PTY commands share this helper lifecycle, preserving prepared environment, working directory, streams, signal/exit status and original limits. Workload completion is available before the whole scope exits, so a detached descendant retains capacity without withholding the workload result. The helper survives launcher loss long enough to reap owned work, forwarding an original parent-death limit to its unreaped workload. Loss of the guardian without its receipt, legacy records and external registrations without equivalent coverage remain unverified. Empty snapshots and legacy confirmed-exit flags cannot authorize release. All capacity, recovery, deadline, operation, registry and cleanup consumers use the shared live/exited/unverified assessment. A shared bounded runtime fence serializes overlapping tree stops across worker and Agent registries. Termination first quiesces the witnessed process tree and verifies stable membership while the ownership guardian remains available to adopt and reap descendants. Quiescence alone is not lifetime coverage and cannot repair an earlier ownership gap. Group signalling requires an identity witness within that group; an escaped descendant cannot authorize a reused group number. A bounded quiescence failure retains evidence and resumes only survivors suspended by that stop attempt. Automatic recovery uses this shared mechanism and retains transcripts, registrations and evidence until owned work is confirmed stopped. Platforms without sufficient group inspection report unavailable evidence and refuse unsafe replacement.

Application's `workers::maintenance::maintain_daemon` is the shared daemon-maintenance interface for deadline enforcement and operation reconciliation, including Goal DR74050DA12C5442DF797B8AB6. It runs separately from workflow recovery and repository cleanup; there is no second daemon reaper for that Goal to add. Recorded Agent and Skill deadlines retain their original start and configured limits. Idle checks use recorded output, attached input, resize and signal-file activity, fall back to the original process start before any activity is recorded, suspend for human input, and preserve the accepted Toolbar timeout exemption. Each registration's inspection or termination failure is reported without preventing other deadline checks. Deadline operation settlement verifies exit while holding the same operation lock that fences new launches, preserves newer revisions and terminal intent, and retains pending reconciliation evidence for retry.

Cleanup supervision runs independently of admission and workflow restart waits. Transient pause-state or repository inspection failures are retried; repository contention is deferred with bounded lock waits. Existing pause boundaries, retention delays, live-use checks, dirty-worktree protection and exact local/remote ref-retirement fences remain authoritative.

PTY completion and failure keep the guardian available while the shared supervisor terminates and reaps descendants. Termination uses one monotonic two-second coordination and proof budget; output capture uses interruptible reads and a bounded final drain. Guardian handle reaping is deferred through process supervision, so an uncertain descendant cannot block session return. A gated launch abort uses the same kernel scope-exit receipt after reaping its unstarted workload.

Artifact retirement belongs to the process supervisor. Seven-day log retention rechecks ownership, registration, handoff and age under bounded coordination before deleting stdout, stderr or stdin artifacts. Missing primary registration is insufficient: live, unverified, missing, corrupt or unreadable ownership preserves evidence. Cleanup and handoff lock inodes remain stable across retirement so existing waiters and new consumers cannot acquire different locks for the same process. Persistent supervisor lock files are excluded from the application Git inventory by a runtime-local ignore marker without replacing an existing ignore policy. Workload-terminal registrations with missing group records still retain capacity.

Standard capture polls nonblocking stdout and stderr in bounded batches. The first validated workload exit starts a fixed 200 ms final drain; output activity cannot extend it, and original completion and stall deadlines remain effective through capture settlement. Quiet inherited pipes and continuously writing descendants cannot withhold the workload result. Closing capture does not terminate descendants to obtain EOF; writers may receive SIGPIPE or EPIPE and ownership remains pending until trustworthy scope-exit proof. Each returned stream reports EOF, capture failure or incomplete capture. Returned buffering is limited to 16 MiB per stream while observed output continues to its artifact and callback; exceeding that limit is explicitly incomplete. Incomplete results retain a local identity-bearing capture receipt with workload status, failure and artifact paths, plus bounded captured prefixes when capture fails. Output-dependent consumers reject incomplete capture through the shared result contract before parsing it. Capture faults and deadlines use bounded shared termination, preserve the original fault, and defer guardian handle reaping. Commands without a guardian retain registration and start-identity checks through deadline settlement.

## Future Direction

Process Infrastructure should gain better resource observation, isolation, health checks, remote-node visibility, and provenance without turning node-local runtime facts into synchronized locks. Scaling should preserve the cheap-restart model: durable semantic work, transient workers, and clear Application authority over Goals.
