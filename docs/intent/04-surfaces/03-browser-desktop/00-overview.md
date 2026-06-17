# Browser Desktop

## Key Ideas

- **Primary Human Surface**: browser and desktop make Refine understandable and operable for people.
- **Same Product, Packaged Differently**: desktop should wrap the browser experience rather than fork product behavior.
- **Static App, Local Daemon**: the UI should stay lightweight and call the local daemon for capability.
- **Operational Console**: the surface should combine work management, system visibility, chat, files, terminal, settings, and review.
- **Agent-First Compatibility**: the UI should expose intent and state without becoming the only way to operate Refine.

## Purpose

The browser-desktop surface exists to make agentic software work visible. It gives people a control room for Gaps, Features, workflow state, changes, logs, settings, target-app status, agent status, files, terminal sessions, and standalone conversations.

It should reduce ambiguity. A user should be able to see what work exists, what state it is in, what agents are doing, what changed, what failed, and what can happen next.

## Expected Role

The browser-desktop surface is currently the richest human interface. It should optimize for overview, inspection, correction, review, and intervention.

Current implementation details that matter to intent:

- the web UI is a vanilla JavaScript single-page app with no frontend build step;
- the shell contains a topbar, banners, `#main`, toolbar dock, and Guide panel;
- hash routing drives Dashboard, Features, Gaps, Changes, Logs, Settings, Node, Project, modals, import, and Plan flows;
- static assets call the local daemon API for product and runtime state;
- desktop should be understood as a packaging and accessibility surface over the same local capability.

The UI should not become a second product implementation. Product rules belong in shared services and workflow capabilities.

## Future Direction

As AI improves, the browser-desktop surface should become more supervisory than manual. People may spend less time driving every step and more time inspecting plans, approving risky changes, reviewing evidence, and intervening when automation needs judgment.

Future UI work should make agent fleets understandable: what they are doing, why they are doing it, how work composes, where risk is concentrated, and what evidence supports the next action.
