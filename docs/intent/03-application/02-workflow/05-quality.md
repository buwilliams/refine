# Quality

## Key Ideas

- **Independent Correction**: a fresh agent reviews both the finalized plan and implementation.
- **Test Evidence**: Quality writes targeted tests when needed, may use sufficient existing tests, and runs the relevant checks to passing.
- **Reuse Accepted Reviews**: Refine reuses the selected agents’ accepted decisions when they describe the current candidate.
- **Exact Candidate**: corrections and results are bound to the committed Goal Round and isolated candidate.
- **Explicit Recovery**: failed Quality preserves its evidence; a new attempt or Round requires a workflow decision.

## Purpose

Quality lets AI review and correct the work using the Goal, context, and user-defined Quality Skills. The AI decides what to investigate and when the work is ready.

## Expected Role

A Quality pass first refreshes the candidate onto the current target, so the evidence it produces is bound to the base the candidate will actually merge into rather than to a base the fleet has moved past. A target that has not moved, or that already contains the candidate, costs one classification and changes nothing. A refresh repins the Round's base and candidate and clears that Round's stale gate evidence, so the gate that follows runs against the new base. A refresh failure preserves both candidate identities and enters Error handling before defaulting to Failed. It does not generate a recovery Round.

The Quality agent inspects the plan, diff, implementation report, repository, and configured project tests. It adds or updates tests when that improves coverage, or uses sufficient existing tests without requiring a special rationale. It runs meaningful tests, fixes blocking failures including those outside the Goal’s scope, and repeats until they pass. Finding failures is work to correct, not a final failure outcome. Quality reports success after verified corrections; it stops unsuccessfully only for a blocker it cannot resolve within its authority or environment and explains that blocker. Governance reviews how the overall Goal came together.

Refine records checkout content after accepted Skill reports, commits Quality corrections, and updates the exact candidate identity. A review is reusable only for the selected Skill instructions, current workflow step, and final candidate content. A later correction can require an earlier reviewer to examine the changed candidate. Refine waits for configured required reviewers and follows their decisions; it does not require an evidence list, test commands, or per-test coverage, and never executes shell text from a report.

Refine records agent decisions, optional reports, provider attempts, and the identities needed to relate a decision to the Goal, Round, workflow step, and exact candidate. Historical storage names such as `QualityProof` and `skill_evidence` remain compatible; they identify which configured agents reviewed which work, not the quality or completeness of their supporting material. Empty Quality gates pass without agent checks. A passing decision advances to Governance. A failed decision emits Quality Error and retains the candidate, checkout, and reports. A handler or operator can explicitly request another attempt or a Plan Round; Refine does not retry failed work automatically.

Already-integrated work remains candidate-bound: complete legacy evidence may be normalized, but incomplete proof causes Refine to materialize a clean managed checkout of the exact source candidate and regenerate isolated Quality. The merged target or one of its descendants is never evaluated as a substitute for that candidate.

A valid failed result from that exact-candidate regeneration settles the originating Round as an explicit already-merged Quality failure. The first failed proof and terminal operation are restart-recoverable and immutable evidence: repeated handling cannot call the approval resolver, transition to Review, or launch a later evaluation that replaces the failure.

Quality does not maintain an automatic recovery budget. Provider, parsing, test-harness, authority, and infrastructure failures follow the same Error/default-Failed boundary without hidden relaunches. Standalone worktree handoff enters Quality directly.

## Future Direction

Improve reusable diagnostic Skills and candidate-bound evidence while keeping recovery explicit through the shared workflow surfaces.

Quality runs only configured Skills. When no Quality Skill applies, the step succeeds without an agent. Old Quality settings can be retained or converted to Skills during configuration migration; they never dispatch a separate legacy evaluator.
