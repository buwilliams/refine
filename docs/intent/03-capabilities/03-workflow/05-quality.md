# Quality

## Key Ideas

- **Independent Correction**: a fresh agent reviews both the finalized plan and implementation.
- **Test Evidence**: Quality writes targeted tests when needed, may use sufficient existing tests, and runs the relevant checks to passing.
- **Exact Candidate**: corrections and results are bound to the committed Goal Round and isolated candidate.

## Purpose

Quality turns implementation confidence into evidence and corrects defects before Governance can authorize integration.

## Expected Role

The Quality agent inspects the plan, diff, implementation report, repository, and configured project tests. It adds or updates tests when that improves coverage, or uses sufficient existing tests without requiring a special rationale. It runs relevant tests, diagnoses failures, corrects implementation or tests, and repeats until the selected checks pass or a real failure is reported.

Refine commits Quality corrections, updates the exact candidate identity, and records supervised commands, exit codes, output, and test results. A passing candidate advances to Governance. Provider, test-harness, candidate, or infrastructure failures move the Goal to Failed and never consume the Governance automatic-retry budget. Standalone worktree handoff enters this state directly.
