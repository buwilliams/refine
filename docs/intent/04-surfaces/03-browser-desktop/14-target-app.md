# Target App

## Key Ideas

- **Attached Work Context**: Refine operates on a target app, and that context must be visible.
- **Lifecycle Control**: users should be able to start, stop, rebuild, test, and inspect the target app.
- **Generated Configuration**: agents can help infer commands, but users should review them.
- **Wrapper Safety**: generated command wrappers should make app operation repeatable without recursion or hidden state.
- **Detached Mode**: no-app state is supported and should remain actionable.

## Purpose

The Target App surface exists to connect Refine to the software it is improving. Refine is not useful in the abstract; it needs to know which app is attached, how to run it, how to test it, and how to observe it.

This surface turns project-specific operational knowledge into durable settings and controls.

## Expected Role

Target App should expose app attachment, app switching, generated commands, start/stop/rebuild behavior, health checks, and test command configuration.

Current implementation details that matter to intent:

- project status and app registry state determine the active app;
- target-app commands live in settings and can be generated;
- target-app lifecycle should run through shared host services and supervised processes;
- detached state should render explicit setup and app-management options;
- quality checks should use target-app test settings rather than browser-only scaffolding.

Target App should be practical. It should help Refine operate the real project with the least new infrastructure possible.

## Future Direction

Future Target App behavior should be increasingly inferential. Agents should inspect the repo, infer commands, run checks, explain failures, and update configuration with reviewable evidence.

At larger scale, target-app concepts may extend across many repos and services, but the same intent remains: Refine must know what software it is acting on and how to verify that software still works.
