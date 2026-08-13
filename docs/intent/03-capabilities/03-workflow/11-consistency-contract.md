# Shared Workflow Consistency Contract

## Key Ideas

- **Goal State Is Authority**: synchronized Goal status, node assignment, Round, and semantic evidence decide what work may advance.
- **Execution Is Local**: workers and process identifiers are transient observations on one node.
- **Evidence Before Transition**: semantic evidence is durable before the Goal state that depends on it.
- **Conflicts Preserve Newer Decisions**: stale workers cannot overwrite reassignment, cancellation, a new Round, or another transition.
- **Cheap Restart**: nonterminal work may be run again; idempotence and readback replace durable execution ownership.

## Purpose

Multiple nodes and replaceable surfaces can act on the same synchronized Goals. This contract keeps those actors consistent while avoiding a second durable state machine for locks, reservations, or worker identity.

## Authority And Identity

Workflow owns Goal status, node assignment, Round history, and workflow decisions. Process owns node-local operations and managed-process facts. Git owns repository, worktree, ref, commit, and integration facts. Activity and evidence record what happened. Projections and surfaces own no authoritative workflow state.

Stable synchronized relationships use target-app, Goal, Round, Feature, and Git identities. Node-local operation, process, and session identifiers support control and may be cited as execution provenance, but they never authorize a workflow mutation or act as a resumable workflow checkpoint.

## Required Invariants

A worker may start only after reading an eligible Goal assigned to its node. A
zero-Round Goal is never eligible. Todo start atomically rereads status, node,
exact authored Round count, non-empty request, and record revision before the
Plan write and before Git or agent side effects. A mismatched observation
stops that worker without rewriting the Goal.

Concurrent execution is tolerated as at-least-once work. Behaviors must be idempotent or detect already-produced semantic evidence. Restart may begin a fresh worker for the same nonterminal Goal; the new worker consumes the same synchronized instructions and preserved artifacts.

Implementation planning persists typed proposal, criticism, revision, and implementation evidence with Goal/Round, context digest, branch, target branch, and base commit. Updates compare against the complete prior planning value. They do not persist the identity of the process that produced them.

Governance rereads authority immediately before its first Git side effect under the repository lock. Cancellation or reassignment before that boundary prevents integration. Once integration begins it may finish and persist exact integration evidence even if cancellation arrives; cancellation remains the Goal's terminal status and no rollback is attempted.

The selected Round index and request remain the execution axis through
Plan, Implement, Quality, and Governance.
Workflow-owned evidence may advance the record revision, but it may not silently
switch to another Round. Restart recovery never synthesizes a generic Round:
zero-Round Plan, Implement, Quality, or Governance Goals are preserved, diagnosed,
and skipped while valid siblings continue.

Synchronization resolves one narrow ownership race: when a queued Goal is reassigned concurrently with automated work starting on its previously authoritative node, the start wins and the reassignment request is discarded. Other competing lifecycle changes remain conflicts.

Browser, CLI, API, MCP, and agent tools submit Goal or process intent through shared capabilities. They do not expose a parallel workflow-execution resource or infer authority from a local process identifier.

## Future Direction

Coordination may become more distributed, but stronger machinery should be added only when duplicate idempotent work is materially more expensive than maintaining it. The default remains synchronized semantic state, transient local execution, strict stale-write rejection, and observable recovery.
