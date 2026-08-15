# Governance

## Key Ideas

- **Independent Gate**: Governance verifies the finalized candidate after Quality passes.
- **Exact Integration**: only a passing verdict may merge and push the recorded candidate into the target branch.
- **Bounded Recovery**: valid findings may draft fresh automatic Rounds; operational failures may not.

## Purpose

Governance ensures that product intent, constitution, rules, and enabled guidance were followed before the candidate changes the shared target branch.

## Expected Role

A managed governance reviewer reads the pinned context, finalized plan, implementation and Quality evidence, and exact candidate diff without editing files. Refine acquires the integrated-target workflow lease before final target revalidation. If the target advanced from a resolvable recorded base, the clean candidate branch still names the exact candidate, the target descends from the base, and the base-to-candidate delta is linear and unambiguous, Refine may rebase that delta. It atomically retains the old and replacement base/candidate identities, invalidates prior gate projections, and reruns exact replacement-candidate Quality and Governance. The lease remains held through candidate publication, merge or push, and evidence settlement. A passing structured verdict authorizes only that exact leased commit, and durable integration evidence is recorded before Review.

A valid governance finding must include both an analysis and a complete actionable recovery-Round request. Refine appends that Round and atomically returns the Goal to Todo. The initial Round is not a retry; by default, at most five automatic recovery Rounds may be generated across Quality and Governance combined. The project Governance setting controls that shared limit. If findings remain after the budget is exhausted, the Goal moves to Failed.

Provider errors, unreadable verdicts, authority races, and infrastructure failures are failures, not governance findings. Ambiguous candidate provenance or a refresh conflict aborts the rebase, leaves the original candidate and branch intact, retains the original handoff and gate evidence plus target and conflict snapshots, and atomically queues one fenced recovery Round. Replacement Quality findings likewise retain both identities and queue recovery. Each successor records `automatic_retry.kind = integration`, its source Round, and the next monotonic attempt in the same configured budget used by Quality and Governance. If that budget is spent, the source Round records an explicit exhausted outcome and all retained evidence before the Goal moves to Failed. These integration-recovery Rounds are not fabricated Governance findings and do not erase the prior audit chain.
