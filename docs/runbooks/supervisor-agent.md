# Operate the supervisor agent

Use this runbook when Goal automation appears idle, stalled, interrupted, or unhealthy.

## Inspect

Run:

```text
refine agent supervisor
```

Read `lifecycle`, `health`, the active/queued/failed counts, and the newest events. The browser System tab also shows supervisor observations and recoveries. The Supervisor tab is a separate user-controlled agent terminal, not a rendering of this automated session.

## Ask for help

Queued or active Goal work starts the configured CLI provider for the automated supervisor as needed. To investigate interactively, open the persistent Supervisor toolbar tab and select Start. Refine launches the configured native agent harness in the target app checkout with monitoring, repair, and verification context. Its conversation stays inside the native CLI terminal and is distinct from the automated supervisor's backend session.

Both automated and interactive Supervisor sessions use the configured `agent_cli`; their launch APIs do not accept a provider override. Automated Supervisor and Goal turns share the configured agent concurrency caps. The interactive toolbar terminal is visible in the Processes surface as an `interactive_session` with Supervisor role and provider metadata.

Changing `agent_cli` also migrates the durable Supervisor session before its next dispatch. An idle session keeps its transcript but resets provider-specific resume state. If the old provider is still running, Refine signals it through the managed-agent process registry, closes that session truthfully, and queues work on a configured-provider replacement only after the old process exits.

Provider and authentication errors from automation remain visible in supervisor status and events. Interactive launch or CLI authentication errors remain visible in the Supervisor terminal. Fix provider access through the normal agent configuration and authentication commands; do not treat those failures as workflow recovery.

## Recover

Automatic recovery is deliberately narrow. On daemon or workflow-runner restart, the existing workflow engine restores the worker and marks interrupted Goals failed so they can be restarted later. The supervisor agent uses existing Refine operations; it does not own a parallel recovery engine. A lost or stalled Goal is reported but is not force-merged, reset, or deleted.

The toolbar Stop action terminates the interactive PTY through the same managed-process registry exposed by the Processes surface. Stop does not disable or cancel the distinct automated supervisor capability. Restart launches a new interactive process with the same Supervisor profile.

For an actionable failed Goal, inspect its evidence and use the ordinary retry command for the failed stage. For example:

```text
refine goal retry GOAL_ID --stage quality
```

Use the exact stage recommended by the Goal state. Re-run `refine agent supervisor` and confirm that counts and events reflect the outcome.

## Verify

A healthy no-work system reports `lifecycle: idle` and starts no more automatic turns. Active work reports `lifecycle: observing`, one automated supervisor session ID, and the configured provider. A live workflow process that is still producing output must not be reported stalled. Any bounded recovery or unrecoverable provider/auth failure remains visible to the CLI, API, and System evidence. Separately, a started Supervisor terminal appears in Processes and can be stopped from either surface.
