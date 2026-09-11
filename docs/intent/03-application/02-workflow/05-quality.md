# Quality

## Key Ideas

- **Independent Correction**: a fresh agent reviews both the finalized plan and implementation.
- **Test Evidence**: Quality writes targeted tests when needed, may use sufficient existing tests, and runs the relevant checks to passing.
- **Reuse Accepted Reviews**: a gate requests missing evidence and supervises proposed checks instead of unconditionally launching another review.
- **Exact Candidate**: corrections and results are bound to the committed Goal Round and isolated candidate.
- **Explicit Recovery**: failed Quality preserves its evidence; a new attempt or Round requires a workflow decision.

## Purpose

Quality turns implementation confidence into evidence and corrects defects before Governance can authorize integration.

## Expected Role

A Quality pass first refreshes the candidate onto the current target, so the evidence it produces is bound to the base the candidate will actually merge into rather than to a base the fleet has moved past. A target that has not moved, or that already contains the candidate, costs one classification and changes nothing. A refresh repins the Round's base and candidate and clears that Round's stale gate evidence, so the gate that follows runs against the new base. A refresh failure preserves both candidate identities and enters Error handling before defaulting to Failed. It does not generate a recovery Round.

The Quality agent inspects the plan, diff, implementation report, repository, and configured project tests. It adds or updates tests when that improves coverage, or uses sufficient existing tests without requiring a special rationale. It runs relevant tests, diagnoses failures, corrects implementation or tests, and repeats until the selected checks pass or a real failure is reported.

Refine records the observed checkout content after each accepted Skill report, commits Quality corrections, and updates the exact candidate identity. A review whose pinned requirements and observed content match the final candidate supplies the proposed checks directly after its result contract and original workspace registration are revalidated. A later Skill correction invalidates an earlier review of different content; only missing reviews are requested observationally. Valid failed findings for that candidate remain failures. Agent-reported test success never replaces supervised execution.

Refine records supervised commands, exit codes, output, test results, provider-response attempts, and a versioned proof naming the Goal, zero-based Round, scope, operation, checked and source commits, state, timestamp, and complete coverage of the selected Skill requirements. Requirements are pinned for the workflow occurrence, and proof records the accepted invocation for each required binding. Empty Quality gates record explicit empty coverage and pass without agent checks. An unreadable evaluation records an output-contract error after its first provider attempt. A passing candidate advances to Governance. A failed verdict emits Quality Error and preserves the originating candidate, checkout, command evidence, and report. A handler or operator may explicitly request another attempt or a new Plan Round through the shared workflow controls. Such a Round retains the candidate and the earlier attempt's audit trail.

Already-integrated work remains candidate-bound: complete legacy evidence may be normalized, but incomplete proof causes Refine to materialize a clean managed checkout of the exact source candidate and regenerate isolated Quality. The merged target or one of its descendants is never evaluated as a substitute for that candidate.

A valid failed result from that exact-candidate regeneration settles the originating Round as an explicit already-merged Quality failure. The first failed proof and terminal operation are restart-recoverable and immutable evidence: repeated handling cannot call the approval resolver, transition to Review, or launch a later evaluation that replaces the failure.

Quality does not maintain an automatic recovery budget. Provider, parsing, test-harness, authority, and infrastructure failures follow the same Error/default-Failed boundary without hidden relaunches. Standalone worktree handoff enters Quality directly.

## Future Direction

Improve reusable diagnostic Skills and candidate-bound evidence while keeping recovery explicit through the shared workflow surfaces.
