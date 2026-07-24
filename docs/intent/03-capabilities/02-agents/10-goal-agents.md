# Goal Agents

## Key Ideas

- **Workflow Owned**: a Goal Agent is the agent process implementing one active Goal, not a separate chat created by a surface.
- **Native Harness**: Refine launches the configured frontier-lab CLI in its own PTY and leaves the provider's TUI, tools, and conversation behavior intact.
- **Background By Default**: the agent works without requiring an attached user; CLI and browser surfaces may attach to the same live terminal when useful.
- **Instance Based**: every active Goal may have its own Goal Agent, so parallel Goals have distinct sessions, worktrees, process records, and evidence.
- **Automation With Escalation**: routine judgment and uncertainty remain autonomous. Only work that is impossible without a missing decision or authority becomes an explicit needs-input state.
- **One Agent Truth**: opening a Goal Agent never launches a second conversational agent for that Goal.
- **Pinned Context Contract**: before launch, the current Round records the exact governance, workflow summary, enabled guidance candidates, Goal fields, previous Rounds, and current request used by the agent.
- **Same-Turn Guidance**: the implementing agent selects applicable guidance while implementing and returns that selection with its completion signal; Refine does not spend a separate provider turn classifying guidance.

## Purpose

Goal Agents make automated workflow and interactive agent use the same thing. Refine should not run a hidden one-shot agent for implementation and then open a different agent when a user wants to inspect or steer the work. It should launch the real implementation agent once, keep it in the background, and let supported surfaces attach to its native TUI.

This preserves the value already supplied by frontier agent harnesses. Refine owns orchestration context, process lifecycle, worktree isolation, workflow state, and evidence. The configured CLI owns its conversation UX, tools, approvals, and provider-specific capabilities.

## Expected Role

When a Goal enters implementation, Refine:

- creates or reuses the Goal's isolated implementation worktree;
- launches one configured CLI agent in a PTY with current Goal, Round, workflow, and completion context;
- records the session as an ordinary managed process tied to the Goal and workflow execution;
- keeps terminal output and an input channel available while the process runs;
- lets the browser Open Agent action and `refine agent open <goal-id>` attach to that same session;
- continues automated workflow when the agent completes;
- keeps the session and workflow claim alive when the agent explicitly reports that user input is required.

The completion and needs-input signals are control state, not a replacement transcript protocol. Durable product truth remains in Goal records, Git changes, logs, governance, quality evidence, and workflow state.

A user attachment is optional. The agent should make reasonable implementation decisions autonomously and only request input when work is impossible without a real product decision, missing authority, or unavailable fact. When input is required, Refine should expose the question through process and activity state. The user answers directly in the native TUI, after which the same agent continues.

Silence is ordinary execution, not an implicit request for help. Refine must not
infer needs-input from elapsed time or lack of terminal output. A silent Goal
Agent remains working and should make the best decision supported by its current
context.

General toolbar Agents are independent sessions. Plan Mode and Standalone remain role-specific sessions. Goal Agents are keyed by Goal instance because several Goals may be implemented in parallel.

The pinned Round context is immutable for that implementation attempt. Post-implementation governance consumes the same pinned governance snapshot, so a mid-turn settings edit cannot make implementation and evaluation reason from different rules. Refine records applied and skipped guidance candidates as structured Round evidence.

The CLI opens a general Agent by default. `--profile plan|standalone` opens the
role-specific sessions, while `--profile goal` takes a Goal id and attaches to
the workflow-owned Goal Agent.

## Future Direction

Goal Agent sessions should become increasingly recoverable across runner or daemon restarts without allowing duplicate ownership. Future work may add durable resume, richer attention routing, fleet-aware attachment, terminal multiplexing, and explicit handoff between cooperating agents.

The invariant should remain: workflow launches the agent that does the work, and every surface opens that agent rather than constructing a substitute.
