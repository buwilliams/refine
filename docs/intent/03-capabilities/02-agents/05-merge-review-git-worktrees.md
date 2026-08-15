# Merge, Review, And Git Worktrees

## Key Ideas

- **Git As Managed Safety Substrate**: history, diffs, rollback, isolation, and integration discipline should come from Git while Refine owns the mechanics.
- **Review As Boundary**: review is a meaningful workflow state, not a decorative approval label.
- **Worktree Isolation**: agent and standalone work should be isolated when that makes changes safer and easier to inspect.
- **Evidence-Based Merge**: merge decisions should be grounded in diffs, quality results, logs, and Goal intent.
- **Recoverable Handoff**: work should move from isolated implementation through Quality, Governance, integration, and acceptance without losing context.

## Purpose

Merge, review, and Git worktrees exist to make autonomous and semi-autonomous changes safe enough to use. Refine should let agents make real changes, but those changes need isolation, inspection, quality evidence, and a clear handoff path.

Git is central because it is existing infrastructure users already trust. Refine should leverage branches, worktrees, diffs, logs, and integration behavior rather than inventing a hidden change system. Those mechanics are an implementation boundary: users approve, retry, or undo product work; they should not need to synchronize, switch branches, merge, rebase, push, or repair Git for Refine.

## Expected Role

This capability should connect workflow with the user's source repository:

- implementation work can happen in an isolated branch or worktree;
- todo Goals remain state-only regardless of queue size; an implementation
  worktree is created only after scheduler capacity is acquired and the Goal is
  durably in Plan;
- Governance should serialize exact candidate integration after Quality passes;
- Governance should merge and push the isolated candidate exactly once before Review;
- review should preserve human or agent judgment over the integrated result;
- approval should mark the reviewed integration accepted without merging or pushing again;
- failed or conflicted merges should create recoverable evidence;
- standalone worktree output should be able to become structured Quality work.
- clean managed Goal worktrees in any status should be hibernated by the shared
  maintenance capability after the configured retention delay when no live
  live operation or process owns them. Recoverable branches recreate the
  checkout on demand. Dirty work, ambiguous ownership, standalone worktrees,
  the state worktree, and unrecognized ignored content remain protected.
  Generated cache paths are removable only when recognized by Refine or
  explicitly configured for the target app. After the same retention delay,
  maintenance independently inventories exact `refine/<goal>/round-N` refs
  locally and on the configured remote. It may retire local and upstream ref
  names for done, cancelled, or deleted Goals only when each exact tip remains
  reachable from the exact configured remote merge-target snapshot. Remote
  deletion atomically fences both the candidate and target SHAs; checked-out
  branches may lose only their upstream ref. Active or failed Goals, active
  ownership, malformed or ambiguous refs, unique commits, inspection failures,
  and `refine/state` remain protected. Goal, Round, process, and target-reachable
  commit evidence remains inspectable after a safe ref name is retired;

Review should be a real boundary in workflow. It lets later ordered Feature work proceed when appropriate, but it should not erase the need for evidence or final judgment.

## Future Direction

Future merge and review should support larger composition flows: many agents, many worktrees, dependency-aware ordering, conflict prediction, staged rollout, generated review summaries, and automatic recovery proposals.

The future direction should still preserve Git's value as a transparent audit and recovery layer without exposing Git chores as product workflow. Users and agents should be able to see what changed, why it changed, how it was checked, and how to undo it through Refine.
