# Node

## Key Ideas

- **Node As Owner**: active work is owned by a node so responsibility is explicit.
- **Decentralized Scale Primitive**: nodes are what let Refine move from one agent on one machine to many agents across many machines.
- **One Agent To Many Agents**: Refine should start simply with one local agent and scale toward many agents working in parallel.
- **Local First, Multi-Node Ready**: the system should work well on one machine without blocking future fleets of machines.
- **Explicit Work Ownership**: every active Goal should be owned by a node clearly enough to prevent accidental overlap.
- **Ordered When It Matters**: parallel work should respect Feature ordering, dependencies, and review boundaries.
- **Coordination Without Centralization By Default**: scale should come from durable state, claims, nodes, and Git-backed evidence before requiring hosted infrastructure.
- **Recoverable Handoffs**: work should leave enough state and evidence for another agent, node, or person to continue safely.

## Purpose

The Node concept explains how Refine grows from a single local assistant into a system that can coordinate many agents across many machines. Node is the foundation for decentralized scale: work can be parallelized only when the system can say which node owns each active Goal, which node is running a process, and which node is reporting state.

The starting point should be easy: one user, one target app, one local daemon, one agent working one Goal. The future direction is larger: multiple agents, fleet nodes, runner workers, ordered Features, parallel Goal execution, review gates, quality checks, and merge handoffs.

Refine needs this concept because agentic software work will not stay single-threaded. Stronger agents will be able to decompose, implement, review, test, and merge work concurrently. Without explicit ownership and ordering, parallelism becomes confusion. With the right foundation, parallelism becomes faster software composition.

A node is the durable owner of active work. It may represent the local daemon, a runner worker, a machine, or a future distributed actor. The local daemon remains the responsive control plane while supervised runner processes execute agent and Git work. Agents may perform the work, but Refine should be able to say which node owns the Goal right now. That ownership is what lets many agents operate in parallel without collapsing into duplicated effort or hidden conflict.

Node identity has two deliberately different parts. The durable node id is the authoritative ownership key stored on Goals, Features, workflow claims, processes, and synchronized state. A display name is only a human label for that same id; it must never be interpreted as ownership of another machine or environment. The active node selection is runtime-local and project-scoped, while the node registry is shared project state. Attach, switch, synchronization, and daemon restart must resolve the local selection against the currently attached registry rather than combining unrelated values from those two scopes.

The id `default` is the reserved single-node compatibility identity. Its canonical label is `Default`. New explicit renames record display-name authority and remain valid, including an intentional rename of `default`; non-default node ids and their legitimate labels remain distinct. A legacy non-canonical label on `default` without authority metadata is ambiguous because older shared state could have copied a host-specific or environment-specific name onto the compatibility entry. Refine must keep the raw registry value inspectable, render the canonical label instead, and report a diagnostic with an operator action: rename `default` to `Default` to discard the stale label, or explicitly rename it to the intended label to confirm it. Refine must not silently rewrite ambiguous shared state.

## Expected Role

Node should define how Refine thinks about parallel work:

- Goals are the smallest schedulable work units.
- Features preserve larger intent and ordering when order matters.
- Active Goals are owned by a node.
- Node ownership identifies which local or distributed actor is responsible for current progress.
- Workflow claims prevent multiple nodes or agents from silently working the same Goal.
- Nodes identify local or distributed actors that can own work, run processes, or report state.
- Project status, node APIs, Goal list/detail projections, dashboard labels, CLI output, and browser surfaces resolve node labels through the same identity contract.
- Fleet concepts let multiple Refine instances coordinate without making the product depend on one UI.
- Git worktrees and branches isolate concurrent changes so parallel work remains reviewable.
- Logs, quality results, process records, and diffs provide evidence for handoff and recovery.

Parallelism should be opportunistic, not reckless. Independent Goals can move at the same time. Ordered Goals should wait when earlier work must finish first. Review can be a meaningful boundary that lets later work proceed when the system has enough evidence, but merge should still preserve final judgment and traceability.

This node model should remain decentralized. Refine may eventually support stronger hosted or distributed coordination, but users should not need a central service before they can benefit from multiple agents or machines. Durable local state, claims, nodes, Git, and observable processes should carry as much coordination as possible.

## Future Direction

Future Refine systems may coordinate fleets of agents across repositories, machines, services, and environments. Nodes may represent local daemons, remote runners, specialized implementers, reviewers, quality executors, migration agents, deployment-aware actors, or future AI systems with different capabilities.

The future direction is explicit orchestration without losing local ownership: richer claims, leases, capacity, dependency graphs, conflict prediction, work stealing, node health, provenance, and recovery. As agents become more capable, Refine should help them divide work, respect dependencies, share evidence, and converge changes through review and merge.

The central question this document protects is: how does Refine let many agents work at once while each active Goal has a clear node owner, order is respected, and evidence survives handoff?
