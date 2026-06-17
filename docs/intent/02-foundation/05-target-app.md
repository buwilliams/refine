# Target App

## Key Ideas

- **Attached Work Context**: Refine acts on a target app, not an abstract task list.
- **Local Target-App Authority**: the active app, `.refine` state, runtime root, commands, and Git context define where work happens.
- **Detached Is Valid**: Refine can operate without an attached app, but that mode should be explicit.
- **Configuration As Intent**: target-app commands, guidance, reporters, quality settings, and governance describe how the app should be worked on.
- **Multi-App Ready**: users and agents should be able to reason about which app is active and switch context safely.

## Purpose

Target App exists because software work only becomes meaningful when it is attached to the system being changed. A Gap, Feature, chat, process, quality run, or merge action needs to know which repository, runtime, guidance, and target-app commands it belongs to.

Refine should make that context explicit. The user and every agent should be able to answer: which app is attached, where is its durable state, how is it run, how is it tested, what guidance applies, and what runtime state belongs to this app?

## Expected Role

Target App should be the foundation that ties durable work to the real project. It should connect:

- the active app and project registry;
- `.refine` product state under the target app;
- runtime state for the local daemon and processes;
- target-app commands for start, stop, rebuild, test, and health checks;
- guidance, governance, quality settings, reporters, and app-specific defaults;
- Git repository and worktree context.

Surfaces should make attached and detached states obvious. Tools and workflow should not guess which app they are operating on when shared target-app context can make the answer explicit.

Target App should not become a hosted account model by default. Its first job is local clarity: Refine knows the project it is helping with, and that knowledge is durable enough for people and agents to inspect.

## Future Direction

As Refine grows, Target App may span many repositories, services, deployments, and nodes. Future agents may infer configuration, update commands, detect project shape, and propose better operating defaults.

The intent should remain stable: Refine must know what software it is acting on, how to operate it, how to verify it, and which durable context should guide work against it.
