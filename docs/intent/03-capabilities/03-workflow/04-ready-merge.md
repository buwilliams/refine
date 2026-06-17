# Ready Merge

## Key Ideas

- **Reviewable Change**: ready-merge means work produced a change that can be inspected.
- **Evidence Handoff**: diffs, logs, quality output, and agent notes should travel with the Gap.
- **Not Done Yet**: ready-merge is a handoff state, not completion.

## Purpose

Ready-merge exists because implementation and integration are different responsibilities. Agents may produce changes, but Refine needs a state where those changes can be reviewed, checked, and prepared for merge.

## Expected Role

Ready-merge should connect the Gap to its Git branch or worktree, changed files, quality evidence, logs, and review context. It should make the next action obvious: inspect, build, QA, review, merge, request more work, or fail.

This state is especially important for standalone and agent-generated work because it preserves handoff without losing the isolated change context.

## What Happens

When a Gap is ready-merge:

- Refine has a reviewable change, usually in a branch or worktree.
- The Gap should point to changed files, commits or diff context, agent output, logs, and source evidence.
- The system prepares the work for build, QA, review, merge, or a new round.
- Users and agents can inspect the change without losing the Gap's original intent.
- If the handoff is incomplete, conflicted, or unsafe, the Gap should move to failed or request more work rather than pretending it is complete.

## Future Direction

Future ready-merge behavior should support generated merge summaries, conflict prediction, dependency-aware ordering, staged integration, and agent-assisted review preparation.
