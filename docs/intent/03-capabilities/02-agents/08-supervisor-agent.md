# Supervisor Agent

## Key Ideas

- **One Shared Supervisor Truth**: the daemon, CLI, API, and toolbar read the same small durable lifecycle, health, observation, and failure projection.
- **Process And Agent Are Distinct**: the supervisor process keeps Refine runtime work alive; the supervisor agent observes and explains workflow work within that process boundary.
- **Ordinary CLI Agent**: the supervisor agent uses the configured `agent_cli` and the same provider, process supervision, limits, streaming, session resume, queue, cancellation, and failure handling as every other Refine agent.
- **Existing Backend Evidence**: workflow, process, Git-sync, projection, operation, and activity services remain authoritative; the supervisor projection does not recreate their rules.
- **Conversation Reuses Chat**: automatic evidence, user prompts, and follow-ups share one ordinary chat session and transcript.
- **Visible While Idle**: the capability remains discoverable and promptable when no Goal is active.

## Purpose

The supervisor agent exists so active Goal work does not require a person to keep a separate terminal or Codex conversation open just to notice failures. When queued or active work appears, Refine starts or resumes exactly one configured CLI-agent session and gives it current shared backend evidence plus Refine's existing CLI/API tools. New evidence and toolbar prompts enter that same queue and transcript.

## Recovery Boundary

The supervisor agent may invoke safe, idempotent operations already owned by Refine. The workflow engine remains responsible for moving interrupted Goals to `failed`, preserving the existing explicit-retry rule. The supervisor may identify lost or quiet work and point to an existing retry operation, but it does not implement its own repair rules.

It must not rewrite source, discard a worktree, force a merge, hide a provider or authentication failure, invent authorization, or loop indefinitely over a failing repair. Those cases become actionable failure events. User steering may guide investigation, but it does not silently expand authority.

The workflow runner coordinates the supervisor agent outside browser request handling. It queues the first CLI-agent turn before Goal evaluation can block on its own provider process, so both use the normal process supervisor and global concurrency limits. The CLI process exits after each turn; the durable session remains available for queued system evidence, user follow-ups, and provider-session resume. When no workflow work remains, Refine queues no further automatic turns.

Stall evidence combines Goal state with the existing process registry and process-output activity. An old Goal timestamp alone is never enough to call a live, output-producing agent stalled. Reads and no-op reconciliation do not rewrite the durable projection, avoiding continuous state-sync churn.

## Lifecycle

- `idle`: no queued or automated Goal work exists; observation and prompts remain available.
- `observing`: one or more Goals are queued or in an automated workflow stage.
- `healthy`: no current issue needs attention.
- `attention`: failed work is visible and can be retried deliberately.
- `degraded`: a stall or runtime failure needs action.

Daemon restarts rebuild the adapter state from existing evidence, retain the singleton supervisor conversation, and recover interrupted chat turns through the normal chat service. A project lock, durable context key, and singleton session lookup prevent concurrent Goals or reconnects from launching duplicate supervisor turns.
