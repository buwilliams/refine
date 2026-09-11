# Governance

## Key Ideas

- **Independent Gate**: Governance verifies the finalized candidate after Quality passes.
- **Exact Integration**: only a passing verdict may merge and push the recorded candidate into the target branch.
- **Explicit Recovery**: findings and operational failures preserve evidence and require an explicit decision before another attempt.

## Purpose

Governance lets AI judge the work using the Goal, context, and user-defined Governance Skill instructions before integration. Refine follows that decision without imposing its own substantive acceptance rules. It records one Governance decision for the reviewed candidate, not separate product, constitution, or meta-rule grades. The stored `rule_state` field carries that decision for compatibility. Older grade fields remain historical data; they do not affect workflow, recovery, agent context, or displayed results.

## Expected Role

A managed governance reviewer reads the pinned context, finalized plan, implementation and Quality evidence, and exact candidate diff. Skill instructions and current user authorization determine whether the agent may edit or redirect work. Changes do not authorize integrating an unreviewed candidate; current workflow and exact-candidate checks still apply. Refine acquires the integrated-target workflow lease before final target revalidation. If the target advanced from a resolvable recorded base, the clean candidate branch still names the exact candidate, the target descends from the base, and the base-to-candidate delta is linear and unambiguous, Refine may rebase that delta. It atomically retains the old and replacement base/candidate identities, invalidates prior gate projections, and reruns exact replacement-candidate Quality and Governance. The lease remains held through candidate publication, merge or push, and evidence settlement. A passing structured verdict authorizes only that exact leased commit, and durable integration evidence is recorded before Review.

Under the lease, the repository lock is held only for Git work: one hold attempts the candidate refresh onto the target tip, a second proves that tip is unchanged and merges. The governance verdict, any replacement-candidate Quality proof, and conflicted-refresh agent resolution are agent invocations that run between or outside those holds under a no-output stall budget, so a slow or hung agent can never wedge every other Goal's repository operations. A conflicted refresh ends its hold with the rebase preserved; resolution edits that workspace unlocked, and every rebase continue is its own short hold that first re-proves the target tip. If the target advances between holds — including during resolution — the attempt ends with the race evidence retained and enters Error handling.

A Governance failure needs no evidence list, structured violations, or recovery proposal. Any analysis, rule identities, or suggestions the AI supplies remain available as context. It emits Governance Error; it does not append an automatic Round. Error handlers and operators can use API, CLI, MCP, or Web workflow controls to move the Goal with context or explicitly retry. Without an accepted redirect, the Goal moves to Failed when handling ends or its deadline expires.

Provider errors, unreadable verdicts, authority races, refresh conflicts, and integration failures retain their original candidate, branch, handoff, gate evidence, and target snapshots. No hidden provider-report repair, integration retry, or generated recovery Round replaces that history. A later explicit Plan decision creates a new auditable Round.

Forced integration is a separate, explicitly requested operation against a pinned candidate. It records bypassed workflow requirements and the actual Git result. Forced Done changes status only. Neither override bypasses ownership, stale-request fencing, process coordination, required inputs, or Git compare-and-swap authority.

## Future Direction

Make significant findings easier for reusable Skills and people to resolve through the shared controls while preserving exact-candidate integration and durable audit evidence.
