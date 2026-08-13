# Goal Agents

## Key Ideas

- **Workflow Owned**: a Goal Agent implements one active Goal; surfaces attach to it rather than creating a substitute.
- **Native Harness**: Refine launches the configured provider CLI in its own managed PTY.
- **Background By Default**: the agent works without requiring an attached user.
- **Pinned Context**: the current Round records the exact semantic context used by every planning and implementation phase.
- **Replaceable Process**: the local session may be lost and restarted without changing synchronized authority.

## Purpose

Goal Agents make automated workflow and interactive inspection the same experience. Refine owns context, process lifecycle, worktree isolation, workflow state, and evidence; the configured CLI owns its tools, approvals, conversation behavior, and provider-specific UX.

## Expected Role

When a Goal enters implementation, Refine creates or reuses its isolated worktree, pins Goal/Round/governance/guidance context, and launches the current Plan, Criticize, Revise, or Implement phase as a managed process. Browser and CLI attachment resolve the current local phase process and never create a duplicate merely to inspect it.

The shared prompt serializer renders pinned context as readable Markdown. Planning phases must leave Git unchanged. Completed proposal, criticism, final-plan, checklist, verification, governance, and implementation artifacts are synchronized semantic evidence; process, operation, and session identifiers are local and are not copied into those artifacts. Completion signals use one typed contract across phases, including zero-based integer indexes for applied Guidance. Refine tolerates a briefly partial signal write, surfaces a stable schema error instead of polling it forever, and applies the configured agent hard cap when a live CLI never produces a valid completion signal.

If input is genuinely required, the live local agent may wait and expose that state. Silence alone does not imply a request for help. If the process or daemon restarts, a replacement worker may consume the same pinned context and preserved planning artifacts. Workflow status, node assignment, and Round determine whether it may continue.

Stopping the local agent confirms exit, retains its worktree and branch, and conditionally requeues an otherwise unchanged Goal. Explicit Goal cancellation remains terminal and cannot be weakened by Stop. General toolbar, Plan Mode, and Standalone agents remain independent sessions; Goal Agents are the intentional Goal-keyed attachment exception.

## Future Direction

Goal Agents should gain better session continuity, attention routing, fleet-aware attachment, and cooperative handoff while preserving one principle: synchronized Goal state owns meaning, and every surface observes the local agent actually doing the work.
