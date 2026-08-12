# Guide

## Key Ideas

- **Persistent Help Surface**: Guide should explain Refine without taking users out of the app.
- **Get Started And Reference**: setup guidance and browseable reference are different modes.
- **Field-Level Help**: settings and controls should link to relevant guidance where possible.
- **App-Scoped State**: Guide progress and context should follow the attached app.
- **Not A Wizard**: guidance should be an outline or panel, not a forced page-state workflow.

## Purpose

The Guide surface exists to help users configure and understand Refine in context. It should make setup, target-app attachment, settings, governance, guidance, and system concepts discoverable without turning the product into a tutorial.

It also helps future agents and maintainers see where the UI expects explanatory support.

## Expected Role

Guide should act as a persistent right-side panel that can open to relevant entries from labels, fields, empty states, and setup flows.

Current implementation details that matter to intent:

- Guide is part of the browser shell, not a standalone route only;
- Get Started and Reference are distinct information modes;
- Guide state is scoped by app;
- app add, switch, remove, and detach flows must avoid stale cross-app guidance state;
- settings labels should prefer field-level Guide links where explanation matters.

Guide should stay terse and actionable. It should explain just enough for users and agents to infer the next step.

## Future Direction

Future Guide behavior should become more adaptive. Agents may update guidance from discovered project facts, explain configuration decisions, or surface context-sensitive advice.

The Guide should remain an intent-preserving support layer: close to the UI, grounded in the project, and readable by humans and agents.
