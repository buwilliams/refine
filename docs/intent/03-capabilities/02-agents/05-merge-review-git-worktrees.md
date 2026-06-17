# Merge, Review, And Git Worktrees

## Key Ideas

- **Git As Safety Substrate**: history, diffs, rollback, isolation, and merge discipline should come from Git where possible.
- **Review As Boundary**: review is a meaningful workflow state, not a decorative approval label.
- **Worktree Isolation**: agent and standalone work should be isolated when that makes changes safer and easier to inspect.
- **Evidence-Based Merge**: merge decisions should be grounded in diffs, quality results, logs, and Gap intent.
- **Recoverable Handoff**: work should move from implementation to review to merge without losing context.

## Purpose

Merge, review, and Git worktrees exist to make autonomous and semi-autonomous changes safe enough to use. Refine should let agents make real changes, but those changes need isolation, inspection, quality evidence, and a clear handoff path.

Git is central because it is existing infrastructure users already trust. Refine should leverage branches, worktrees, diffs, logs, and merge behavior rather than inventing a hidden change system.

## Expected Role

This capability should connect workflow with the user's source repository:

- implementation work can happen in an isolated branch or worktree;
- ready-merge state should mean there is a reviewable change with enough evidence;
- review should preserve human or agent judgment before final integration;
- merge actions should connect back to Gap and Feature intent;
- failed or conflicted merges should create recoverable evidence;
- standalone worktree output should be able to become structured ready-merge work.

Review should be a real boundary in workflow. It lets later ordered Feature work proceed when appropriate, but it should not erase the need for evidence or final judgment.

## Future Direction

Future merge and review should support larger composition flows: many agents, many worktrees, dependency-aware ordering, conflict prediction, staged rollout, generated review summaries, and automatic recovery proposals.

The future direction should still preserve Git's value as a transparent audit and recovery layer. Even if AI systems become much better at merging, users and agents should be able to see what changed, why it changed, how it was checked, and how to roll it back.
