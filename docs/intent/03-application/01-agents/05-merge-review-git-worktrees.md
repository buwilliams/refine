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

This Application behavior should connect workflow with the user's source repository:

- automated Goal work requires an admitted managed linked worktree;
- ordinary Todo queueing remains state-only regardless of queue size; an implementation
  worktree is created only after scheduler capacity is acquired and the Goal is
  durably in Plan;
- a Round branch is born at the base the Goal records — the resolved merge
  target — and never at whatever branch the shared checkout has open, so the
  recorded base is an ancestor of the candidate by construction rather than by
  coincidence. Before that base is pinned, the local merge target is
  fast-forwarded to its remote when it cleanly can be, advisory and never
  fatal. Users should not have to park their checkout on the merge target, or
  remember to pull it, for their Goals to be integrable;
- Governance should acquire the integrated-target lease before final target revalidation and retain it through any candidate refresh, replacement gates, publication, integration, and settlement;
- Governance should merge and push the isolated candidate exactly once before Review;
- ordinary target advancement may refresh only a clean candidate whose recorded base resolves, is its ancestor, whose target descends from that base, and whose delta is linear and unambiguous. Old and replacement identities and gate evidence remain inspectable, and replacement Quality and Governance run before integration;
- the candidate is refreshed at the Implement→Quality boundary and under the Governance lease. A successful repin clears stale gate evidence and retains previous candidate identities. Target movement or conflict stops the attempt without retrying;
- failed refreshes preserve the original candidate, branch, handoff, and conflict observations. Error handlers and operators may request a new attempt through shared workflow controls;
- recovery Rounds are explicit decisions. Refine does not generate them when refresh or integration fails;
- merge-time conflicts, refresh-time conflicts, and stale candidates retain their distinct diagnostics under one failure policy: emit the source step’s Error event, allow a bounded handling window when configured, then default to Failed unless an explicit workflow decision redirects the Goal;
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

Automated Goal work requires a canonical, registered linked worktree in the intended repository's managed location, bound to the current Goal, Round, and branch or exact candidate. A matching branch in the primary checkout or an unrelated linked checkout is not ownership. Admission verifies the registration and its backlink; launches and candidate writes revalidate that commitment. Missing checkouts may be recreated from unambiguous retained branch and candidate evidence. Changed registrations, wrong branches, or ownership conflicts stop with an actionable infrastructure error and preserve existing files and index state. Recovery Rounds use their own worktrees and retain the source Round's checkout. Branches are created at the declared target base, independently of the developer's checked-out branch.

Executable Backlog and Todo lifecycle Skills have separate invocation-owned branches and managed paths, pinned to a configured target ref and immutable source commit. Their checkout does not become the Round implementation candidate or rewrite its base. Admission records logical ownership before creation and physical registration before launch; restart reuses the original registration and preserves dirty work. An existing branch or path without the original registration is an explicit recovery error. Disabled and context-only bindings need no checkout.

Legacy post-build Quality evidence is regenerated, when required by reconciliation, in an isolated checkout of the exact source candidate. Retained passing or terminal failed proofs and the existing main-branch integration, checkout synchronization, and human Review boundaries remain authoritative.

## Future Direction

Future merge and review should support larger composition flows: many agents, many worktrees, dependency-aware ordering, conflict prediction, staged rollout, generated review summaries, and automatic recovery proposals. Skills can propose or perform repairs and request the next workflow outcome through the shared surfaces, keeping retries explicit and auditable.

The future direction should still preserve Git's value as a transparent audit and recovery layer without exposing Git chores as product workflow. Users and agents should be able to see what changed, why it changed, how it was checked, and how to undo it through Refine.
