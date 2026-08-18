# Toolbar

## Key Ideas

- **Lazy Utility Dock**: the toolbar starts empty and creates a tab only when the user asks for one.
- **Native Agent Harnesses**: agent interaction uses the configured frontier-lab CLI in a real terminal rather than a Refine-owned chat imitation.
- **Independent Agents**: every Agent command starts a distinct general-purpose agent session; agents are not coupled to Goal Agent turns or an automatic Supervisor role.
- **Shared Terminal Surface**: Terminal, Agent, Agent in Worktree, Planing Agent, Goal, and Standalone use one terminal renderer and backend lifecycle.
- **Reporter Utilities**: Todo List uses the selected Reporter and shared
  target-app state rather than browser storage.
- **Recoverable State**: live sessions reattach after navigation or reload without making browser storage the source of process truth.

## Purpose

The toolbar keeps supporting work close at hand without eagerly launching processes or replacing the main route. Refine orchestrates agents, workflow, and evidence, while native agent harnesses retain their conversation, tool-call, approval, and rendering UX.

The add menu appears immediately after the Toolbar label and offers:

- Agent;
- Agent in Worktree;
- System;
- Files;
- Todo List;
- Terminal;
- Planing Agent.

Each selection creates or opens only the requested surface. Repeated Agent selections create independent sessions with unique labels such as Agent, Agent 2, and Agent 3. Agent in Worktree and Standalone use isolated Refine worktrees. Goal tabs attach to the workflow-owned Goal Agent already implementing that Goal and never launch a duplicate. Opening a live Goal tab waits for the exact runtime session to acknowledge one-way protection from its idle and completion watchdogs; an exit, mismatched session, or unavailable acknowledgment fails the open instead of returning a stale snapshot. This exemption is specific to a Toolbar-opened workflow Goal Agent and does not change other terminal attachment paths.

## Lifecycle

- a fresh app session starts with no permanent tabs and no active process;
- a page refresh restores that browser session's explicitly opened tabs and
  verifies their process state;
- each tab has a close action;
- closing an interactive terminal removes its browser tab immediately and asks
  the backend to stop its managed process without waiting for termination or
  workflow settlement;
- a tab whose process already exited or no longer exists closes locally without
  requiring a successful stop request;
- closing a Goal Agent tab uses the supported backend stop path, which preserves
  process settlement, worktree and branch retention, Goal requeue, and audit
  semantics after the browser surface detaches;
- acknowledged Goal Agent timeout protection affects only automatic idle and
  completion termination; valid completion, needs-input, explicit Stop and Goal
  cancellation, natural exit, transcript capture, and workflow settlement keep
  their existing authority;
- when an explicit Stop is already in progress, closing its tab only detaches
  the browser surface and never sends a duplicate Stop; the original
  workflow-aware stop settlement continues and any late failure remains visible;
- stopping an agent keeps the rest of the Toolbar interactive, and an
  authoritative terminal-exit event releases the terminal UI even while
  workflow stop settlement is still finishing;
- when explicit cancellation already won, Stop reports the terminal cancelled
  result and retained worktree evidence instead of promising that the Goal
  returned to todo;
- terminal sizing ignores hidden or unmeasurable layouts, retains the last valid
  geometry, and refits the renderer and managed PTY together after restore,
  resize, and fullscreen transitions;
- the Add menu is anchored to its Toolbar control, so it follows the collapsed,
  resized, and fullscreen Toolbar positions;
- an interrupted browser event stream is not evidence that the managed process exited;
- terminal state remains tab-specific, including process identifier, provider, current directory, output, and worktree identity;
- reselecting an Agent, Planing Agent, Goal, or Standalone terminal preserves
  its session and scrollback while presenting its latest output at the bottom;
- every Agent terminal receives the resolved active Refine executable and checkout so it can reliably use the correct CLI;
- normal Agent and Planing Agent toolbar sessions begin with the existing Refine
  Node, prioritizing investigation, current-Node management, and direct answers;
- the browser explicitly identifies toolbar-launched terminal sessions, and the
  shared server never infers toolbar guidance eligibility from an Agent profile;
- those toolbar agents route requested repository changes into a new complete,
  eligible Goal rather than implementing outside the workflow;
- when continuing a failed Goal, those toolbar agents preserve its evidence and
  retained work, append an evidence-preserving recovery Round, and return the
  Goal to workflow eligibility through supported Refine interfaces;
- changing target apps stops live target-scoped interactive terminals before clearing project-specific browser state.
- Todo List keeps named lists in a compact rail and gives the selected list the
  rest of the workspace. Adding and completing todos are primary actions;
  completed items are visually separated, editing stays inline, and rename or
  delete controls remain available from the selected list's options menu.
  Changing the selected Reporter reloads that Reporter's lists, and explicit
  Refresh reconciles state synchronized from another Node without polling.

The former automatic and toolbar-specific Supervisor Agent is retired. Upgrade cleanup stops its legacy managed processes and removes its durable session, state, locks, settings, API, and toolbar entry. Refine's process supervisor remains an infrastructure capability and is not an agent profile.

## Boundary

The toolbar exposes shared backend capability. General Agents never directly
edit durable Goal state, conceal failures, approve or merge for the user,
destructively discard retained work, or begin ongoing supervision without a
request. Todo data is authoritative in
the target app's inspectable Refine state; the tab only renders and invokes the
shared todo API. The toolbar does not implement workflow transitions, duplicate
Goal ownership, agent turn scheduling, or an alternate conversation protocol.
The node-first operating contract applies only to normal Agent and Planing Agent
toolbar sessions; it does not alter Goal diagnostics, Agent in Worktree, or the
planning, implementation, Quality, and Governance workflow agents.

Future versions may add fleet-level views for active Goals, pending approvals, process health, Goal evidence, and multiple native agents while retaining lazy creation and explicit lifecycle ownership.
