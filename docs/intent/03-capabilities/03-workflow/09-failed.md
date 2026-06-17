# Failed

## Key Ideas

- **Recoverable Failure**: failed means work could not complete successfully, but evidence should remain useful.
- **Actionable Evidence**: failure should explain what happened and what can happen next.
- **Not A Dead End**: failed work may be retried, rerouted, split, or given a new round.

## Purpose

Failed exists because agentic work will often hit limits: bad assumptions, command failures, conflicts, missing context, provider errors, governance issues, or target-app problems.

The state should make failure visible and useful rather than hiding it behind logs or transient UI messages.

## Expected Role

Failed should preserve process output, agent notes, quality failures, governance findings, changed files, and any partial progress. It should support recovery rounds, new instructions, reassignment, or cancellation.

Failed should also protect ordered work. If a failed Gap blocks a Feature, Refine should make that relationship visible.

## What Happens

When a Gap is failed:

- Refine stops treating the current attempt as successful.
- Failure evidence is preserved: process output, provider output, quality failures, governance findings, logs, changed files, and error messages.
- Active claims should be closed or made recoverable.
- Users or agents can inspect the failure and choose a recovery path: retry, submit a new round, split the work, reassign it, cancel it, or leave it failed.
- If the Gap blocks ordered Feature work, that blockage should be visible.
- Failed state should be a decision point, not an evidence sink.

## Future Direction

Future failed behavior should support automated diagnosis, recovery planning, dependency-aware rerouting, and agent handoff. The goal is not to avoid all failure; it is to make failure a productive workflow state.
