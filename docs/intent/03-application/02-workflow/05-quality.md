# Quality

## Key Ideas

- **Independent Correction**: a fresh agent reviews both the finalized plan and implementation.
- **Test Evidence**: Quality writes targeted tests when needed, may use sufficient existing tests, and runs the relevant checks to passing.
- **Exact Candidate**: corrections and results are bound to the committed Goal Round and isolated candidate.
- **Bounded Recovery**: valid Quality findings produce a fresh implementation Round; operational failures do not.

## Purpose

Quality turns implementation confidence into evidence and corrects defects before Governance can authorize integration.

## Expected Role

A Quality pass first refreshes the candidate onto the current target, so the evidence it produces is bound to the base the candidate will actually merge into rather than to a base the fleet has moved past. A target that has not moved, or that already contains the candidate, costs one classification and changes nothing. A refresh repins the Round's base and candidate and clears that Round's stale gate evidence, so the gate that follows runs against the new base. A conflicted refresh is resolved in place; when resolution fails, is exhausted, or is disabled, the pass ends by queueing the same fenced integration recovery Round the Governance-time refresh queues — returning the Goal to Todo, or to Failed once the shared automatic budget is exhausted — rather than judging the candidate against a base that no longer exists.

The Quality agent inspects the plan, diff, implementation report, repository, and configured project tests. It adds or updates tests when that improves coverage, or uses sufficient existing tests without requiring a special rationale. It runs relevant tests, diagnoses failures, corrects implementation or tests, and repeats until the selected checks pass or a real failure is reported.

Refine commits Quality corrections, updates the exact candidate identity, and records supervised commands, exit codes, output, test results, provider-response attempts, and a versioned proof naming the Goal, zero-based Round, scope, operation, checked and source commits, state, and timestamp. An unreadable evaluation receives at most two diagnostic repair invocations before a distinct output-contract failure. A passing candidate advances to Governance. When the supervised result is a valid failed Quality verdict, a separate read-only investigation records the cause and drafts a complete actionable Round; Refine appends it and returns the Goal to Todo. That recovery Round continues on the retained candidate worktree and skips replanning (see Todo and Plan).

Already-integrated work remains candidate-bound: complete legacy evidence may be normalized, but incomplete proof causes Refine to materialize a clean managed checkout of the exact source candidate and regenerate isolated Quality. The merged target or one of its descendants is never evaluated as a substitute for that candidate.

A valid failed result from that exact-candidate regeneration settles the originating Round as an explicit already-merged Quality failure. The first failed proof and terminal operation are restart-recoverable and immutable evidence: repeated handling cannot call the approval resolver, transition to Review, or launch a later evaluation that replaces the failure.

Quality, Governance, and integration-conflict recovery share one automatic recovery budget across the Round chain. The initial Round is not a retry; by default, at most five automatic recovery Rounds may be generated in total. If Quality findings or a recoverable integration race remain after that budget is exhausted, the Goal moves to Failed with the exhausted evidence retained. Provider, parsing, test-harness, authority, or infrastructure failures move the Goal to Failed without creating or consuming a recovery Round. Standalone worktree handoff enters Quality directly.
