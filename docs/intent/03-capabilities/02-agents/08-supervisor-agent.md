# Supervisor Agent

## Key Ideas

- **One Shared Supervisor Truth**: the daemon, CLI, API, and System surface read the same small durable lifecycle, health, observation, and failure projection.
- **Process And Agent Are Distinct**: the supervisor process keeps Refine runtime work alive; the supervisor agent observes and explains workflow work within that process boundary.
- **Ordinary CLI Agent**: the supervisor agent uses the configured `agent_cli` and the same provider, process supervision, limits, streaming, session resume, queue, cancellation, and failure handling as every other Refine agent.
- **One Capacity Truth**: workflow and supervisor turns acquire atomic leases from the same global, node, provider, and target-app capacity policy at provider-launch time.
- **One Provider Truth**: configured `agent_cli` controls session dispatch, capacity accounting, process evidence, and API state; provider-specific resume state is reset when configuration changes.
- **Existing Backend Evidence**: workflow, process, Git-sync, projection, operation, and activity services remain authoritative; the supervisor projection does not recreate their rules.
- **Automation Reuses Backend Chat**: automatic evidence and automated follow-ups may share the durable backend chat capability without requiring a browser chat UI.
- **Toolbar Session Is Distinct**: the Supervisor toolbar tab is a user-started native agent terminal with monitoring context, not a renderer or lifecycle control for the automated supervisor session.

## Purpose

The automated supervisor agent exists so active Goal work does not require a person to keep a separate terminal or agent conversation open just to notice failures. When queued or active work appears, Refine starts or resumes exactly one configured CLI-agent session and gives it current shared backend evidence plus Refine's existing CLI/API tools. Automatic evidence stays in that backend automation session.

The user-controlled Supervisor toolbar profile launches a separate configured agent in the target app checkout with a concise prompt to monitor, investigate, fix, and verify the workflow using Refine's CLI and repository evidence. It is registered in the ordinary process manager and has explicit Start, Stop, and Restart controls. The native harness owns its conversation and approval UX.

Both forms are natural agents for finding unknowns across active work. They should compare the workflow map with process, Git, projection, operation, and activity evidence; follow blind-spot paths; and prototype bounded recovery when that is the fastest safe way to learn. Product judgment and new authority remain user decisions.

## Recovery Boundary

The supervisor agent may invoke safe, idempotent operations already owned by Refine. The workflow engine remains responsible for moving interrupted Goals to `failed`, preserving the existing explicit-retry rule. The supervisor may identify lost or quiet work and point to an existing retry operation, but it does not implement its own repair rules.

It must not rewrite source, discard a worktree, force a merge, hide a provider or authentication failure, invent authorization, or loop indefinitely over a failing repair. Those cases become actionable failure events. User steering may guide investigation, but it does not silently expand authority.

The workflow runner coordinates the supervisor agent outside browser request handling. It queues the first CLI-agent turn before Goal evaluation can block on its own provider process. Provider dispatch then uses the shared capacity lease service: at cap 1 a supervisor and Goal turn never overlap, while at cap 2 they may overlap when the node, provider, and target-app caps also allow it. Waiting system evidence is compacted to the latest non-terminal Goal snapshot, while user follow-ups remain distinct and durable until a lease is available. The CLI process exits and releases its lease after each turn; the durable session remains available for queued context, user follow-ups, and provider-session resume. When no workflow work remains, Refine queues no further automatic turns.

Stall evidence combines Goal state with the existing process registry and process-output activity. An old Goal timestamp alone is never enough to call a live, output-producing agent stalled. Reads and no-op reconciliation do not rewrite the durable projection, avoiding continuous state-sync churn.

## Lifecycle

- `idle`: no queued or automated Goal work exists; observation and prompts remain available.
- `observing`: one or more Goals are queued or in an automated workflow stage.
- `healthy`: no current issue needs attention.
- `attention`: failed work is visible and can be retried deliberately.
- `degraded`: a stall or runtime failure needs action.

Daemon restarts rebuild the automated adapter state from existing evidence, retain its singleton backend session, recover interrupted turns through the normal chat service, and reclaim capacity leases whose holder process no longer exists. Success, provider failure, interruption, cancellation, closed sessions, and abandoned dispatch all release capacity without creating a second scheduler. A project lock, durable context key, singleton session lookup, and one replaceable internal context entry prevent concurrent Goals or reconnects from launching duplicate automated turns or flooding the queue. Browser Supervisor terminals instead reattach by their PTY session identifier when still live and become restartable when the process has exited.

All Goal, automated Supervisor, and interactive Supervisor terminal processes register under the runtime managed-process root with structured role, provider, session, Goal or Feature, and worktree metadata as applicable. Stop and retry locate work through that registry and request cross-platform termination through the Process capability. The registry remains authoritative until the child exits. Legacy port-root records remain observable only for upgrade cleanup; new dispatch never creates them there.
