# Operate the supervisor agent

Use this runbook when Goal automation appears idle, stalled, interrupted, or unhealthy.

## Inspect

Run:

```text
refine agent supervisor
```

Read `lifecycle`, `health`, the active/queued/failed counts, and the newest events. The browser Toolbar > Supervisor tab shows the same state and updates while it is open.

## Ask for help

Queued or active Goal work starts the configured CLI provider automatically. Open the persistent Supervisor toolbar tab to see that same session and send steering. System evidence and user follow-ups sent during a provider turn share the normal chat queue. Navigation or a UI reconnect restores the same session and transcript.

Provider and authentication errors are shown as chat failures. Fix provider access through the normal agent configuration and authentication commands; do not treat those failures as workflow recovery.

## Recover

Automatic recovery is deliberately narrow. On daemon or workflow-runner restart, the existing workflow engine restores the worker and marks interrupted Goals failed so they can be restarted later. The supervisor agent uses existing Refine operations; it does not own a parallel recovery engine. A lost or stalled Goal is reported but is not force-merged, reset, or deleted.

For an actionable failed Goal, inspect its evidence and use the ordinary retry command for the failed stage. For example:

```text
refine goal retry GOAL_ID --stage quality
```

Use the exact stage recommended by the Goal state. Re-run `refine agent supervisor` and confirm that counts and events reflect the outcome.

## Verify

A healthy no-work system reports `lifecycle: idle` and starts no more automatic turns. Active work reports `lifecycle: observing`, one supervisor session ID, and the configured provider. A live workflow process that is still producing output must not be reported stalled. Any bounded recovery or unrecoverable provider/auth failure remains visible to the CLI, API, toolbar, and shared transcript.
