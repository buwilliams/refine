# Governance

## Key Ideas

- **Independent Gate**: Governance verifies the finalized candidate after Quality passes.
- **Exact Integration**: only a passing verdict may merge and push the recorded candidate into the target branch.
- **Bounded Recovery**: valid findings may draft fresh automatic Rounds; operational failures may not.

## Purpose

Governance ensures that product intent, constitution, rules, and enabled guidance were followed before the candidate changes the shared target branch.

## Expected Role

A managed governance reviewer reads the pinned context, finalized plan, implementation and Quality evidence, and exact candidate diff without editing files. A passing structured verdict authorizes Refine to publish and integrate that exact commit under the repository coordination boundary. Durable push and integration evidence is recorded before the Goal advances to Review.

A valid governance finding must include both an analysis and a complete actionable recovery-Round request. Refine appends that Round and atomically returns the Goal to Todo. The initial Round is not a retry; by default, at most five automatic recovery Rounds may be generated across Quality and Governance combined. The project Governance setting controls that shared limit. If findings remain after the budget is exhausted, the Goal moves to Failed.

Provider errors, unreadable verdicts, Git conflicts, stale candidates, authority races, and infrastructure failures are failures, not governance findings. They preserve evidence and do not create or consume automatic recovery Rounds.
