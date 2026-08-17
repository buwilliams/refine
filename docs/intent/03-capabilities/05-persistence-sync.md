# Persistence Sync

## Key Ideas

- **Sync Is Product Core**: Refine's two primary benefits are governance and fleet management, and syncing is the heart of fleet management. Persistence sync is a first-class capability, not plumbing.
- **One Capability, Policy Differences**: refine state and target-app code are all files synced over git. State and code differ only in policy — acceptance gates, resolution context, escalation route — never in mechanism.
- **Conflicts Are Invitations**: a conflict is an invitation to resolve it as best as possible. It only escalates when the system truly needs help understanding which path to take.
- **Deterministic Speed, Agent Judgment**: no conflict means deterministic code handles it in milliseconds. A conflict means an agent tries. Deterministic code prepares, gates, retains, and publishes, but never decides which side wins.
- **Nothing Is Silently Destroyed**: every losing side of every publication is retained as a ref before overwrite, and publication is compare-and-swap.

## Purpose

Fleet management only works if every node can trust that its durable state converges with every other node's. Persistence sync provides that convergence for everything Refine keeps in git: the synchronized state branch, candidate branches, and target-app fetches. It exists as one capability because state sync, candidate refresh, candidate integration, and operator recovery are the same problem — merging divergent lines of files — and splitting them multiplies locking disciplines, error shapes, and failure policies without adding judgment anywhere.

The capability's rule is that convergence should almost never require a person. Divergence without overlap is resolved deterministically and instantly. Divergence with overlap is resolved by an agent that understands both intents. Only genuine ambiguity — a choice the system cannot understand on its own — surfaces to an operator, and it surfaces as a domain-terms question about goals and intents, never as a fence or a raw git failure.

## Expected Role

Every merge-shaped operation walks the same deterministic ladder, each rung strictly cheaper than the next. Equal trees finish immediately. Ancestry classification fast-forwards ancestor-related heads so they never enter a merge at all. An in-memory merge commits clean results without materializing a worktree. A structural JSON driver merges state members it can prove disjoint — it is a merge driver, not a judge, and anything contested falls through. Remaining conflicts go to agent resolution. Only when resolution is exhausted does the operation escalate as NeedsDecision, carrying a domain-terms question an operator can answer in one read.

Agent resolution follows a fixed contract. The agent works in an isolated, Refine-owned workspace, never the human checkout, and receives base, ours, and theirs plus domain context: for state, the goal records and ownership doctrine as guidance; for code, the goal prompts and round intents so it understands both intents, not just both diffs. Deterministic acceptance gates validate the output — state must parse, satisfy schema, and hold record invariants; code re-enters exact-candidate Quality. A rejected output re-prompts the agent; it never fences. After two attempts the operation becomes NeedsDecision. The agent never runs inside the repository lock: one short hold pins inputs and materializes the workspace, and a second short hold re-verifies the pinned inputs and publishes. A resolution's entire state lives in refs under `refs/refine/resolve/<id>`, so any crash at any point is answered by rerunning — everything re-derives, and publication is idempotent because it is compare-and-swap. There are no side files and no parallel journals; the resolution note is recorded as workflow evidence.

State and code share this machinery and differ only in policy. State gates on parse, schema, and record invariants; resolves with goal records and ownership doctrine; escalates to an operator decision on the sync surface. Code gates on build and exact-candidate Quality; resolves with goal prompts and round intents; escalates to a fenced recovery Round with a bounded budget.

The surface converges on one command family. `sync` runs the ladder for everything the node manages. `sync --preview` is a read-only divergence summary that writes nothing on error. `sync --authority live|remote [--path]` is recovery — and recovery is simply sync with a decision attached: the chosen side becomes a merge commit, the losing ref is retained, and rerunning is a no-op. Fleet sync is pure orchestration of the same code path across nodes; there is no fleet-specific sync mechanism. Until the collapsed surface ships, the CLI surface doc and the state-sync-recovery runbook describe the commands that exist today (`project sync`, `project state-recovery`).

Changes to this capability are gated by the multi-node sync simulation harness: simulated nodes against a local remote under adversarial timing, crash-and-rerun interleavings, and scripted resolvers, checking convergence, no lost work, stable identity, and crash-only reruns after every interleaving.

## Future Direction

The capability arrives in stages, each shippable alone: first the module carve plus the simulation harness, then the state-sync pipeline swap that deletes the baseline and arbitration machinery and collapses the command surface — `sync` supersedes `project sync` and `project state-recovery`, and the preview-file apply ceremony is deleted — then the agent resolution wiring, then base refresh at stage boundaries so collisions are resolved while the implementing agent's context is fresh. Beyond rollout, sync should get ahead of conflicts rather than only resolving them: predicting likely collisions before they land and ordering fleet work with awareness of dependencies so divergence that would need judgment is created less often.
