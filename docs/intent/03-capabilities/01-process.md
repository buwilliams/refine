# Process

## Key Ideas

- **Local Observation**: process records describe work observed on one node; they are not synchronized workflow authority.
- **Supervisor Ownership**: Refine should know which processes it owns and why they exist.
- **Recoverable Work**: stopping or losing a process should preserve Goal, Round, branch, worktree, and evidence so work can be started again cheaply.
- **Soft Capacity**: admission uses observed live processes and configured limits, not durable reservations.
- **Shared Control**: CLI, browser, API, and agents use the same process capability.
- **Checkout Ownership**: one product home owns its executable and every port runtime beneath its `run/` directory.

## Purpose

Refine runs target apps, agents, quality checks, imports, maintenance tasks, terminals, and background operations. The Process capability makes that local execution visible and controllable without making process identity part of synchronized product state.

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

## Expected Role

Managed processes record local facts such as owner, pid, state, label, output paths, limits, start time, exit code, Goal, Round, workflow state, provider, and target app. Node-local identifiers may connect local logs, sessions, operations, and child processes, and evidence may cite them as provenance. They do not grant permission to mutate a Goal or act as resumable workflow state.

Goal status and node assignment determine whether automated work is still authorized. A worker rereads those fields before a workflow transition or consequential side effect. Process records help avoid needless duplicate work on one node and provide controls; they are advisory when deciding capacity or recovery.

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

## Future Direction

Process management should gain better resource observation, isolation, health checks, remote-node visibility, and provenance without turning node-local runtime facts into synchronized locks. Scaling should preserve the cheap-restart model: durable semantic work, transient workers, and clear Goal authority.
