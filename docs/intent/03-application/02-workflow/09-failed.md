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

Failed should also protect ordered work. If a failed Goal blocks a Feature, Refine should make that relationship visible.

## What Happens

When a Goal is failed:

- Refine stops treating the current attempt as successful.
- Failure evidence is preserved: process output, provider output, quality failures, governance findings, logs, changed files, and error messages.
- The visible failure summary should state the concrete authoritative cause when structured gate
  evidence provides one, while complete evidence remains available for audit and recovery.
- Active local processes should be stopped or left with explicit recovery evidence.
- Users or agents can inspect the failure and choose a recovery path: retry, submit a new round, split the work, reassign it, cancel it, or leave it failed.
- If the Goal blocks ordered Feature work, that blockage should be visible.
- Failed state should be a decision point, not an evidence sink.

Structured-output failures retain the original invalid response and diagnostic. Integration races, merge conflicts, refresh failures, and failed checks retain their candidate, branch, worktree, handoff, gates, and target observations. They emit the source step's Error event without generating a recovery Round or rerunning work. Configured Error handlers receive a bounded handling window; an explicit revision-fenced workflow decision may redirect the Goal with context. Without that decision, the Goal settles in Failed. A successful handler response alone does not change this outcome.

## Future Direction

Future failed behavior should support automated diagnosis, recovery planning, dependency-aware rerouting, and agent handoff. The goal is not to avoid all failure; it is to make failure a productive workflow state.
