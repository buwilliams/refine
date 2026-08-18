# Browser

## Key Ideas

- **Primary Human Surface**: the browser makes Refine understandable and operable for people.
- **Shared Product Model**: browser behavior should remain a thin adapter over Model and Application behavior.
- **Static App, Local Daemon**: the UI should stay lightweight and call the local daemon for Application behavior.
- **SSE-Driven State**: live browser state should arrive through server-sent events, never periodic UI polling.
- **Operational Console**: the surface should combine work management, system visibility, chat, files, terminal, settings, and review.
- **Agent-First Compatibility**: the UI should expose intent and state without becoming the only way to operate Refine.

## Purpose

The browser surface exists to make agentic software work visible. It gives people a control room for Goals, Features, workflow state, changes, logs, settings, target-app status, agent status, files, terminal sessions, and standalone conversations.

It should reduce ambiguity. A user should be able to see what work exists, what state it is in, what agents are doing, what changed, what failed, and what can happen next.

## Expected Role

The browser surface is currently the richest human interface. It should optimize for overview, inspection, correction, review, and intervention.

Current implementation details that matter to intent:

- the web UI is a vanilla JavaScript single-page app with no frontend build step;
- the shell contains a topbar, banners, `#main`, toolbar dock, and Guide panel;
- hash routing drives Dashboard, Features, Goals, Changes, Logs, Settings, Node, Project, modals, import, and Plan flows;
- static assets call the local daemon API for product and runtime state;
- initial loads and reconnects reconcile authoritative HTTP state once, while SSE exclusively drives subsequent live updates and background-operation progress;
- local and remote browser sessions should expose the same product semantics.

The UI should not become a second product implementation. Product rules and orchestration belong in Application; host mechanisms belong in Infrastructure.

## Future Direction

As AI improves, the browser surface should become more supervisory than manual. People may spend less time driving every step and more time inspecting plans, approving risky changes, reviewing evidence, and intervening when automation needs judgment.

Future UI work should make agent fleets understandable: what they are doing, why they are doing it, how work composes, where risk is concentrated, and what evidence supports the next action.
