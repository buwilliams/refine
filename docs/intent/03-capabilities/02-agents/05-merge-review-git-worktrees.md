# Merge, Review, And Git Worktrees

## Key Ideas

- **Git As Managed Safety Substrate**: history, diffs, rollback, isolation, and integration discipline should come from Git while Refine owns the mechanics.
- **Review As Boundary**: review is a meaningful workflow state, not a decorative approval label.
- **Worktree Isolation**: agent and standalone work should be isolated when that makes changes safer and easier to inspect.
- **Evidence-Based Merge**: merge decisions should be grounded in diffs, quality results, logs, and Goal intent.
- **Conflict As Invitation**: a conflicted merge or refresh is an invitation to resolve it, not a reason to abort; escalation is for genuine decisions, not first contact.
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
- Governance should acquire the integrated-target lease before final target revalidation and retain it through any candidate refresh, replacement gates, publication, integration, and settlement;
- Governance should merge and push the isolated candidate exactly once before Review;
- ordinary target advancement may refresh only a clean candidate whose recorded base resolves, is its ancestor, whose target descends from that base, and whose delta is linear and unambiguous. Old and replacement identities and gate evidence remain inspectable, and replacement Quality and Governance run before integration;
- agent resolution in place is the first response to a conflicted refresh: the resolver works on the preserved conflicted rebase in the Refine-owned refresh worktree — never the human checkout — with conflict markers that carry the merge base as well as both sides, and with the conflicted file list and both Goals' intents as context (the Goal prompt, the Round request, and the implementation reports of other Goals whose commits conflict). The repository lock never contains resolver work: the rebase attempt is one short hold, resolution runs unlocked gated by a no-remaining-markers check and a scope check that rejects any edit outside the conflicted files, and each rebase continue is its own short hold that first re-proves the target tip — a moved tip aborts the rebase and re-enters the bounded refresh-again loop. An interruption leaves the rebase stopped rather than wedged: the candidate branch never moved, so the next attempt aborts it and starts the recovery Round from a clean checkout. The resolver budget is bounded and shared between rejected outputs and further conflicted stops, and conflicted picks are continued, never skipped. A resolved replacement re-passes the linearity and provenance gate on its final delta, records the resolution note in the refresh evidence, and re-enters replacement Quality and Governance — resolution never moves, deletes, or rewrites the original candidate or its evidence. The merge ladder and resolution contract live in `docs/intent/03-capabilities/05-persistence-sync.md`;
- when resolution fails, is exhausted, or is disabled (`workflow_conflict_resolution: off`), the refresh falls back to exactly the prior behavior: it aborts and cleans the attempted rebase, leaves the original candidate, branch, and evidence intact, retains the conflicted file list plus target and conflict snapshots and the resolution attempts' note — a resolver that declares it cannot choose contributes its own question, and a spent budget adds one naming the Goal, branch, target, and remaining files — and queues one fenced `integration` automatic-retry Round that re-implements from a fresh base, using the same monotonic attempt count and configured budget as Quality and Governance;
- merge-time conflicts at integration and refresh-time rebase conflicts are one physical event under one policy: no path hard-fails a Goal on its first conflict; only the shared `integration_retry_exhausted` budget or an explicit needs-decision escalation ends it;
- an integrated current-Round candidate may be reconciled to Review when candidate-bound passed Governance and retained, fully normalized, or regenerated isolated Quality proof names that candidate and repository-coordinated observations prove it remains an ancestor of the local target and, when required, the published target. Unrelated descendant commits on a shared target are valid and both observed target snapshots remain inspectable;
- review should preserve human or agent judgment over the integrated result;
- approval should mark the reviewed integration accepted without merging or pushing again;
- failed or conflicted merges should create recoverable evidence; conflicted file lists are always retained in evidence, and nothing is silently destroyed;
- a failed candidate handoff should retain the latest implementation, Quality,
  or replacement candidate identity observed by that workflow attempt rather
  than an earlier runtime projection;
- standalone worktree output should be able to become structured Quality work.
- clean managed Goal worktrees outside Plan, Implement, Quality, and Governance
  should be hibernated by the shared maintenance capability after the configured
  retention delay when no live operation or process owns them. Automated-phase
  status remains a durable ownership fence across daemon settlement and restart
  gaps; recoverable branches recreate inactive checkouts on demand. Hibernation
  discards Git-ignored content with the checkout: an inactive Goal resumes from
  durable state alone, so ignored build and runtime artifacts are reproducible
  by definition and must not hold a checkout on disk. Dirty or untracked work,
  ambiguous ownership, standalone worktrees, and the state worktree remain
  protected. After the same retention delay,
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

Future merge and review should support larger composition flows: many agents, many worktrees, dependency-aware ordering, conflict prediction, staged rollout, generated review summaries, and automatic recovery proposals. In-place agent resolution should extend from the conflicted refresh to the conflicted integration merge itself, so a merge-time conflict is attempted before its fenced recovery Round rather than routed straight to it. Stage-boundary candidate base refresh should classify the candidate against the target at the Implement→Quality boundary so collisions surface earlier and smaller, leaving Governance-time refresh as the last line rather than the only one.

The future direction should still preserve Git's value as a transparent audit and recovery layer without exposing Git chores as product workflow. Users and agents should be able to see what changed, why it changed, how it was checked, and how to undo it through Refine.
