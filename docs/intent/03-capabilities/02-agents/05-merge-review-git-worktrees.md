# Merge, Review, And Git Worktrees

## Key Ideas

- **Git As Managed Safety Substrate**: history, diffs, rollback, isolation, and integration discipline should come from Git while Refine owns the mechanics.
- **Review As Boundary**: review is a meaningful workflow state, not a decorative approval label.
- **Worktree Isolation**: agent and standalone work should be isolated when that makes changes safer and easier to inspect.
- **Evidence-Based Merge**: merge decisions should be grounded in diffs, quality results, logs, and Goal intent.
- **Recoverable Handoff**: work should move from implementation to review to merge without losing context.

## Purpose

Merge, review, and Git worktrees exist to make autonomous and semi-autonomous changes safe enough to use. Refine should let agents make real changes, but those changes need isolation, inspection, quality evidence, and a clear handoff path.

Git is central because it is existing infrastructure users already trust. Refine should leverage branches, worktrees, diffs, logs, and integration behavior rather than inventing a hidden change system. Those mechanics are an implementation boundary: users approve, retry, or undo product work; they should not need to synchronize, switch branches, merge, rebase, push, or repair Git for Refine.

## Expected Role

This capability should connect workflow with the user's source repository:

- implementation work can happen in an isolated branch or worktree;
- ready-merge state should mean there is a reviewable change with enough evidence;
- review should preserve human or agent judgment before final integration;
- approval should integrate the isolated candidate and connect the result back to Goal and Feature intent;
- failed or conflicted merges should create recoverable evidence;
- standalone worktree output should be able to become structured ready-merge work.

Review should be a real boundary in workflow. It lets later ordered Feature work proceed when appropriate, but it should not erase the need for evidence or final judgment.

## Future Direction

Future merge and review should support larger composition flows: many agents, many worktrees, dependency-aware ordering, conflict prediction, staged rollout, generated review summaries, and automatic recovery proposals.

The future direction should still preserve Git's value as a transparent audit and recovery layer without exposing Git chores as product workflow. Users and agents should be able to see what changed, why it changed, how it was checked, and how to undo it through Refine.
