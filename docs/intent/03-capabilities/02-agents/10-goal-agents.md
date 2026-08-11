# Goal Agents

## Key Ideas

- **Workflow Owned**: a Goal Agent is the agent process implementing one active Goal, not a separate chat created by a surface.
- **Native Harness**: Refine launches the configured frontier-lab CLI in its own PTY and leaves the provider's TUI, tools, and conversation behavior intact.
- **Background By Default**: the agent works without requiring an attached user; CLI and browser surfaces may attach to the same live terminal when useful.
- **Instance Based**: every active Goal may have its own Goal Agent, so parallel Goals have distinct sessions, worktrees, process records, and evidence.
- **Automation With Escalation**: routine judgment and uncertainty remain autonomous. Only work that is impossible without a missing decision or authority becomes an explicit needs-input state.
- **One Agent Truth**: opening a Goal Agent never launches a second conversational agent for that Goal.
- **Pinned Context Contract**: before launch, the current Round records the exact governance, workflow summary, enabled guidance candidates, Goal fields, previous Rounds, and current request used by the agent.
- **One Shared Specification**: every workflow-owned Goal Agent receives that pinned object through the same flat Markdown specification, independent of provider or attachment surface.
- **Same-Turn Guidance**: the implementing agent selects applicable guidance while implementing and returns that selection with its completion signal; Refine does not spend a separate provider turn classifying guidance.
- **Governed Implementation Planning**: three fresh Goal-scoped agent processes propose, independently criticize, and revise a plan before the fresh implementation agent starts; Workflow owns their order and evidence while agents own judgment.

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
- pins context once, then launches plan, criticize, revise, and implement as
  distinct supervised processes against that snapshot and implementation
  worktree. Opening the Goal resolves the currently active phase process; it
  does not create a diagnostic or implementation duplicate.
- treats an explicit process Stop as interruption rather than Goal cancellation:
  after confirmed exit it releases the exact claim, returns the Goal to todo
  under its pinned Node owner, and retains every implementation worktree and
  branch with durable recovery evidence. This successful retention is visible
  through shared Process results; cleanup is a separate explicit
  human-controlled operation. A prior or racing explicit Goal cancellation
  remains terminal: Stop can settle remaining execution resources but never
  resurrects the Goal as todo.

The completion and needs-input signals are control state, not a replacement transcript protocol. Durable product truth remains in Goal records, Git changes, logs, governance, quality evidence, and workflow state.

A user attachment is optional. The agent should make reasonable implementation decisions autonomously and only request input when work is impossible without a real product decision, missing authority, or unavailable fact. When input is required, Refine should expose the question through process and activity state. The user answers directly in the native TUI, after which the same agent continues.

Silence is ordinary execution, not an implicit request for help. Refine must not
infer needs-input from elapsed time or lack of terminal output. A silent Goal
Agent remains working and should make the best decision supported by its current
context.

General toolbar Agents, Plan Mode agents, and Standalone worktree agents are
independent instances: every open action launches a new managed terminal session.
Goal Agents are the intentional exception. They are keyed by Goal instance so
every surface attaches to the one workflow-owned agent implementing that Goal.

The pinned Round context is immutable for that implementation attempt. It
remains a durable, versioned internal object, while the shared prompt capability
renders it once through `src/prompts/spec.md` as readable Markdown. The
specification presents Refine context, product intent, rationale, rules and
zero-based Guidance candidates, chronological previous Rounds, and the
authoritative latest Round. The latest Round request is the final substantive
instruction. Provider launch code and CLI or browser attachment surfaces do not
serialize a second representation.

Post-implementation governance consumes the same pinned governance snapshot, so
a mid-turn settings edit cannot make implementation and evaluation reason from
different rules. Refine records applied and skipped guidance candidates as
structured Round evidence.

Planning-phase processes are observational. Refine records exact Git state
before and after each judgment. Any difference is retained with the transcript,
process, operation, and failure evidence and blocks implementation without a
reset or cleanup. A shared read-only execution boundary may be used only when it
preserves the managed session's writable completion channel; exact Git mutation
detection remains the provider-independent backstop.
Completed proposal, criticism, and final-plan artifacts are immutable evidence.
Later phases consume them, and implementation records
per-checklist completion, deviation, rejection, or blockage separately.

Planning phases do not resume after a runner, daemon, or provider-process
interruption. Shared Process and Workflow settlement fails the attempt and
retains available output, identities, logs, branch, and worktree evidence. A
supported follow-up Round starts again at Plan with a new claim, execution, and
per-Round planning record; it never reuses the interrupted execution identity or
overwrites the earlier Round's evidence.

The CLI opens a general Agent by default. `--profile plan|standalone` opens the
role-specific sessions, while `--profile goal` takes a Goal id and attaches to
the workflow-owned Goal Agent.

## Future Direction

Goal Agent sessions should become increasingly recoverable across runner or daemon restarts without allowing duplicate ownership. Future work may add durable resume, richer attention routing, fleet-aware attachment, terminal multiplexing, and explicit handoff between cooperating agents.

The invariant should remain: workflow launches the agent that does the work, and every surface opens that agent rather than constructing a substitute.
