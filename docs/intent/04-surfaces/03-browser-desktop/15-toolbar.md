# Toolbar

## Key Ideas

- **Lazy Utility Dock**: the toolbar starts empty and creates a tab only when the user asks for one.
- **Native Agent Harnesses**: agent interaction uses the configured frontier-lab CLI in a real terminal rather than a Refine-owned chat imitation.
- **Independent Agents**: every Agent command starts a distinct general-purpose agent session; agents are not coupled to Goal Agent turns or an automatic Supervisor role.
- **Shared Terminal Surface**: Terminal, Agent, Agent in Worktree, Planing Agent, Goal, and Standalone use one terminal renderer and backend lifecycle.
- **Recoverable State**: live sessions reattach after navigation or reload without making browser storage the source of process truth.

## Purpose

The toolbar keeps supporting work close at hand without eagerly launching processes or replacing the main route. Refine orchestrates agents, workflow, and evidence, while native agent harnesses retain their conversation, tool-call, approval, and rendering UX.

The add menu appears immediately after the Toolbar label and offers:

- Agent;
- Agent in Worktree;
- System;
- Files;
- Terminal;
- Planing Agent.

Each selection creates or opens only the requested surface. Repeated Agent selections create independent sessions with unique labels such as Agent, Agent 2, and Agent 3. Agent in Worktree and Standalone use isolated Refine worktrees. Goal tabs attach to the workflow-owned Goal Agent already implementing that Goal and never launch a duplicate.

## Lifecycle

- a fresh app session starts with no permanent tabs and no active process;
- a page refresh restores that browser session's explicitly opened tabs and
  verifies their process state;
- each tab has a close action;
- closing an interactive terminal asks the backend to stop its managed process before removing the tab;
- a tab whose process already exited or no longer exists closes locally without
  requiring a successful stop request;
- closing a Goal Agent tab uses the supported backend stop path, which preserves
  workflow cancellation, claim-settlement, and audit semantics before the tab
  disappears;
- stopping an agent keeps the rest of the Toolbar interactive, and an
  authoritative terminal-exit event releases the terminal UI even while
  workflow cancellation settlement is still finishing;
- the Add menu is anchored to its Toolbar control, so it follows the collapsed,
  resized, and fullscreen Toolbar positions;
- an interrupted browser event stream is not evidence that the managed process exited;
- terminal state remains tab-specific, including process identifier, provider, current directory, output, and worktree identity;
- every Agent terminal receives the resolved active Refine executable and checkout so it can reliably use the correct CLI;
- changing target apps stops live target-scoped interactive terminals before clearing project-specific browser state.

The former automatic and toolbar-specific Supervisor Agent is retired. Upgrade cleanup stops its legacy managed processes and removes its durable session, state, lock, capacity leases, settings, API, and toolbar entry. Refine's process supervisor remains an infrastructure capability and is not an agent profile.

## Boundary

The toolbar exposes shared backend capability. It does not implement workflow transitions, duplicate Goal ownership, agent turn scheduling, or an alternate conversation protocol.

Future versions may add fleet-level views for active claims, pending approvals, process health, Goal evidence, and multiple native agents while retaining lazy creation and explicit lifecycle ownership.
