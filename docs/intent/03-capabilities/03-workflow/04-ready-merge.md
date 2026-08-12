# Ready Merge

## Key Ideas

- **Integration Queue**: ready-merge is the merger-owned queue for exact candidate integration.
- **Evidence Handoff**: diffs, logs, quality output, and agent notes should travel with the Goal.
- **Not Done Yet**: ready-merge is a handoff state, not completion.

## Purpose

Ready-merge exists because implementation and integration are different responsibilities. Agents may produce changes in isolation, but Refine needs a state where those changes are integrated before the target app is rebuilt and the result is reviewed.

## Expected Role

Ready-merge should connect the Goal to its Git branch or worktree, exact candidate commit, recorded base and target branch, pinned remote, quality evidence, logs, and review context. It serializes integration; it is not a candidate-preparation label.

This state is especially important for standalone and agent-generated work because it preserves handoff without losing the isolated change context.

## What Happens

When a Goal is ready-merge:

- Refine has a reviewable change, usually in a branch or worktree.
- The Goal should point to changed files, commits or diff context, agent output, logs, and source evidence.
- Refine integrates the exact candidate under the repository coordination boundary after any configured pre-merge Quality checks.
- Ready Merge, rebuild, and post-build Quality hold one exclusive integrated-target workflow lane through review handoff. Concurrent Goals may implement in parallel but cannot interleave effects on the shared target checkout.
- Successful merge and push evidence is durable before the Goal advances. A retry or restart proves existing integration instead of merging or pushing the candidate twice.
- A durable lane marker attributes residue after interruption. The next owner quarantines tracked residue only when the marker proves the checkout was clean at handoff; unattributed user work remains preserved and blocks mutation.
- Rebuild runs against the integrated target checkout when configured; otherwise Refine records an explicit skip.
- Post-build Quality runs against the exact integrated target commit after rebuild, so review reflects the composed target app rather than only an isolated candidate.
- Users and agents can inspect the change without losing the Goal's original intent.
- Conflicts, stale candidates, ownership or revision races, and push failures preserve the candidate and evidence and use the failed/retry path rather than advancing.
- Runner restart leaves Ready Merge, Build, and QA Goals in their synchronized state for automatic rescheduling. A replacement worker reuses any exact semantic evidence already written and repeats only the incomplete idempotent work.

## Future Direction

Future ready-merge behavior should support generated merge summaries, conflict prediction, dependency-aware ordering, staged integration, and agent-assisted review preparation.
