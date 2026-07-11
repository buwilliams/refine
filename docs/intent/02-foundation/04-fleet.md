# Fleet

## Key Ideas

- **Fleet As Composed Nodes**: a fleet is many nodes coordinating through the same durable state, claims, and Git remotes — not a new system layered on top.
- **Distribute Is The Mechanism**: there is no scheduler. Distribute is how work moves between nodes — every assignment, rebalance, and handoff is an invocation of this one operation.
- **Provision On Demand**: operating agents follow provider runbooks to create, credential, verify, and dispose of machines; Refine does not embed cloud control planes.
- **Credentials Follow Policy**: subscription logins and direct API keys are both legitimate; the provisioning agent applies the user's chosen posture without persisting secrets in Refine state.
- **Ephemeral Workers, Durable Evidence**: worker nodes should be rebuildable from Git and shared state; nothing irreplaceable lives on a worker.
- **Symmetric Sync**: every node pushes and pulls the same shared remotes; no node is a required intermediary for state.
- **Judgment Converges**: implementation can happen on any node, but review and merge converge to the node where people exercise judgment.
- **Any Infrastructure**: a fleet should run on a laptop, a rack of machines, or a cloud provider without changing the model.
- **Self-Hosting Proof**: the fleet is proven when Refine improves Refine — its own repository distributed across its own nodes.

## Purpose

The Fleet concept explains how Refine operates many nodes as one delivery system. Node establishes ownership: which actor holds each active Goal. Fleet establishes movement: how work spreads across nodes, how nodes come into existence with working agents, and how finished work converges back for review and merge.

The starting point should stay easy: one machine is already a fleet of one. Nothing about fleet operation should burden the single-node experience. The future direction is larger: a person or agent files a Feature, distributes its Goals across provisioned nodes, watches parallel implementation stream evidence back, and reviews the converged results in one place.

Refine needs this concept because parallel agents are only useful when the work can reach them and return from them. Ownership without distribution leaves every node working alone. Distribution without convergence scatters results no one can judge. Fleet names the full loop: provision, distribute, implement, converge, review, merge.

## Expected Role

Fleet should define how Refine thinks about work at scale:

- Distribute is the mechanism for moving work, not one option among several. There is no scheduler process deciding placement in the background; if work is on a different node than it was before, distribute was invoked — by a person, an agent, or policy.
- Distribute operates over registered nodes: it takes eligible Goals — captured or actionable, with no active claim — and reassigns their node ownership across enabled, healthy nodes. Reassigning ownership of unclaimed work is the one sanctioned exception to node ownership enforcement, because reassignment is the transfer.
- Convergence is distribution pointed home: moving a reviewable Goal back to the review node is the same operation, not a separate return path.
- Distribution strategies stay simple and inspectable: spread evenly, fill available capacity, or match Goals to nodes by provider. Workflow policy limits and Feature ordering are respected wherever work lands.
- Because distribute is an operation on shared capability, every surface gets it: a person from the CLI or browser, an agent through MCP. Automatic distribution, when it arrives, is policy invoking distribute on a cadence or trigger — never a second mechanism with its own placement logic.
- Provisioning runbooks cover both halves of a working node: the machine (a container image or host bootstrapped from Git) and the agent (provider CLIs installed, credentials materialized, authentication verified — not just binary presence). Refine begins at node registration and `node init`; provider lifecycle remains outside the binary.
- Credential posture is per-node policy. Long-lived nodes near a person may use subscription logins the way a developer machine does. Ephemeral workers should prefer metered, revocable API keys injected at boot. Secrets are held by a credential source the fleet trusts and never written into shared state or Git.
- State syncs symmetrically: each node commits its durable state, pulls/rebases, and pushes to the shared remote, retrying briefly when pushes race. Node ownership keeps writes disjoint so races stay rare. Sync runs after durable mutations and workflow passes, plus a configurable slow background cadence, so distribution becomes visible across the fleet without manual triggers.
- Implementation, quality, and governance evidence are produced where the work runs; branches push to the shared remote; the Goal converges to the review node where the judgment boundary lives. Merge happens once, where it is reviewed.
- Node health is reported, not assumed: distribute should only target nodes that are enabled and recently alive.

The fleet should remain decentralized in character. Any node can distribute, any node can implement, and the review node is a role, not a privileged service. Refine on a cloud provider is the same binary, the same flat files, and the same Git discipline as Refine on a laptop.

## Future Direction

The nearest proof is self-hosting: Refine attached to its own repository, distributing its own Goals across provisioned worker nodes, each running a different provider agent, with results converging to one review queue — and, after merge, redeploying its own workers from the code its agents wrote.

Beyond that, fleets may grow the orchestration the Node document anticipates — leases, work stealing, dependency-aware placement, conflict prediction, and capacity awareness across heterogeneous nodes. All of it should arrive as richer distribute strategies and policies, not as a scheduler alongside distribute. Infrastructure automation may invoke the provisioning runbook contract or a future external driver, but cloud lifecycle code should not enter Refine's core. None of this should displace the foundations: durable flat files, Git as the sync and safety substrate, explicit ownership, and review as the judgment boundary.

The central question this document protects is: how does Refine move work across many provisioned nodes and bring the results back to human judgment, without a central service, hidden scheduling, or state that cannot be rebuilt from Git?
