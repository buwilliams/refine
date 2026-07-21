# Target App

## Key Ideas

- **Attached Work Context**: Refine acts on a target app, not an abstract task list.
- **Local Target-App Authority**: the active app, its external Refine state, runtime root, commands, and Git context define where work happens.
- **Durable And Inspectable State**: product, runtime, workflow, and derived state should survive interruption and remain readable.
- **Flat Files First**: target-app state should be easy to inspect, copy, version, repair, and read by agents.
- **Detached Is Valid**: Refine can operate without an attached app, but that mode should be explicit.
- **Multi-App Ready**: users and agents should be able to reason about which app is active and switch context safely.

## Purpose

Target App exists because software work only becomes meaningful when it is attached to the system being changed. A Goal, Feature, chat, process, quality run, import, or merge action needs to know which repository, runtime, guidance, commands, state, and Git context it belongs to.

This foundation concept also explains state and storage. Refine should know what app it is acting on, what it knows about that app, where that knowledge lives, and which runtime context is currently active.

Without durable target-app context, agentic work becomes chat transcript memory and shell side effects. Refine should instead make work explicit, local, inspectable, and recoverable.

## Expected Role

Target App should be the foundation that ties durable work to the real project. It should connect:

- the active app and project registry;
- durable product state in the repository's Git-owned Refine layout, with `.refine` checked out only in the isolated `refine/state` worktree;
- runtime state for the local daemon, processes, operations, claims, and caches;
- workflow state that explains what can happen next;
- projection snapshots and other derived state that keep the product fast without becoming authoritative;
- target-app lifecycle instructions for start, stop, and rebuild, plus deterministic commands for tests and health checks;
- guidance, governance, quality settings, reporters, and app-specific defaults;
- Git repository, branch, and worktree context.

The current implementation details that matter to intent are:

- durable product state is associated with the target app but never appears at `<app>/.refine` or in the primary worktree: the local mutation projection is `<app>/.git/refine-live-state/` and the branch checkout is `<app>/.git/refine-state-worktree/.refine/`;
- runtime state is separated from durable product state so processes and daemons can recover cleanly;
- flat JSON-like records keep Goals, Features, settings, guidance, governance, and logs inspectable;
- projections and caches exist for speed but should be rebuildable from durable state;
- Git provides history, isolation, rollback, and merge discipline;
- shared services and daemon routes should coordinate state mutation so surfaces do not compete for authority.

The important boundary is source of truth. Caches, indexes, projections, and UI state are allowed and necessary for performance, but they should not replace durable target-app state. If a cache is wrong, the repair path should be refresh or rebuild, not manual database surgery.

Surfaces should make attached and detached states obvious. Tools and workflow should not guess which app they are operating on when shared target-app context can make the answer explicit.

Target App should not become a hosted account model by default. Its first job is local clarity: Refine knows the software it is helping with, and that knowledge is durable enough for people and agents to inspect.

## Future Direction

As Refine grows, Target App may span many repositories, services, deployments, and nodes. Future agents may infer configuration, update commands, detect project shape, propose better operating defaults, and coordinate work across multiple target apps.

At larger scale, Refine may need stronger replication, distributed coordination, richer indexing, or optional server-backed storage. Those additions should be scale adaptations, not replacements for local ownership, inspectability, Git compatibility, and agent readability.

The intent should remain stable: Refine must know what software it is acting on, how to operate it, how to verify it, where its state lives, what runtime context is active, and which durable context should guide work against it.
