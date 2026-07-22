# Supervisor Agent

## Key Ideas

- **One Shared Supervisor Truth**: the daemon, CLI, API, and toolbar read the same small durable lifecycle, health, observation, and failure projection.
- **Process And Agent Are Distinct**: the supervisor process keeps Refine runtime work alive; the supervisor agent observes and explains workflow work within that process boundary.
- **Ordinary CLI Agent**: the supervisor agent uses the configured `agent_cli` and the same provider, process supervision, limits, streaming, session resume, queue, cancellation, and failure handling as every other Refine agent.
- **One Capacity Truth**: workflow and supervisor turns acquire atomic leases from the same global, node, provider, and target-app capacity policy at provider-launch time.
- **One Provider Truth**: configured `agent_cli` controls session dispatch, capacity accounting, process evidence, and API state; provider-specific resume state is reset when configuration changes.
- **Existing Backend Evidence**: workflow, process, Git-sync, projection, operation, and activity services remain authoritative; the supervisor projection does not recreate their rules.
- **Conversation Reuses Chat**: automatic evidence, user prompts, and follow-ups share one ordinary chat session and transcript.
- **Visible While Idle**: the capability remains discoverable and promptable when no Goal is active.
- **No Manual Lifecycle Controls**: the toolbar does not offer Start or Stop because the shared capability is always observing workflow work or waiting idle.

## Purpose

The supervisor agent exists so active Goal work does not require a person to keep a separate terminal or Codex conversation open just to notice failures. When queued or active work appears, Refine starts or resumes exactly one configured CLI-agent session and gives it current shared backend evidence plus Refine's existing CLI/API tools. New evidence and toolbar prompts enter that same queue and transcript.

## Recovery Boundary

The supervisor agent may invoke safe, idempotent operations already owned by Refine. The workflow engine remains responsible for moving interrupted Goals to `failed`, preserving the existing explicit-retry rule. The supervisor may identify lost or quiet work and point to an existing retry operation, but it does not implement its own repair rules.

It must not rewrite source, discard a worktree, force a merge, hide a provider or authentication failure, invent authorization, or loop indefinitely over a failing repair. Those cases become actionable failure events. User steering may guide investigation, but it does not silently expand authority.

The workflow runner coordinates the supervisor agent outside browser request handling. It queues the first CLI-agent turn before Goal evaluation can block on its own provider process. Provider dispatch then uses the shared capacity lease service: at cap 1 a supervisor and Goal turn never overlap, while at cap 2 they may overlap when the node, provider, and target-app caps also allow it. Waiting system evidence and user follow-ups remain durable in the chat queue until a lease is available. The CLI process exits and releases its lease after each turn; the durable session remains available for queued system evidence, user follow-ups, and provider-session resume. When no workflow work remains, Refine queues no further automatic turns.

Stall evidence combines Goal state with the existing process registry and process-output activity. An old Goal timestamp alone is never enough to call a live, output-producing agent stalled. Reads and no-op reconciliation do not rewrite the durable projection, avoiding continuous state-sync churn.

## Lifecycle

- `idle`: no queued or automated Goal work exists; observation and prompts remain available.
- `observing`: one or more Goals are queued or in an automated workflow stage.
- `healthy`: no current issue needs attention.
- `attention`: failed work is visible and can be retried deliberately.
- `degraded`: a stall or runtime failure needs action.

Daemon restarts rebuild the adapter state from existing evidence, retain the singleton supervisor conversation, recover interrupted chat turns through the normal chat service, and reclaim capacity leases whose holder process no longer exists. Success, provider failure, interruption, cancellation, closed sessions, and abandoned dispatch all release capacity without creating a second scheduler. A project lock, durable context key, and singleton session lookup prevent concurrent Goals or reconnects from launching duplicate supervisor turns.

All Goal and Supervisor provider processes register under the runtime managed-agent process root. Stop and retry locate work by the structured session or execution metadata in that registry and request cross-platform termination through the Process capability. The registry remains authoritative until the child exits, and its capacity lease is not released during that stopping window. Legacy port-root records remain observable only for upgrade cleanup; new dispatch never creates them there.
