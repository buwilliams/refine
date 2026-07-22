# Toolbar

## Key Ideas

- **Persistent Utility Dock**: the toolbar is always available without replacing the main route.
- **Native Agent Harnesses**: agent interaction uses the configured frontier-lab CLI in a real terminal rather than a Refine-owned chat imitation.
- **Profiles, Not Custom UIs**: Terminal, Supervisor, Plan Mode, Goal, and Standalone share one terminal surface and differ only by working directory, prompt context, agent launch, and optional worktree.
- **Managed Lifecycle**: every shell or agent terminal is an explicit Start/Stop process owned by the daemon process manager.
- **Stateful But Recoverable**: toolbar state should persist enough to reattach to a live terminal after normal navigation or reload without owning product state.

## Purpose

The toolbar exists to keep supporting work close at hand. Users need system notices, source files, live terminals, configured agent harnesses, and Goal logs while they inspect or edit the main surface.

Refine should orchestrate agents, teams, workflow, and evidence. It should not recreate the conversation, tool-call, approval, and rendering UX already owned by agent harness CLIs.

## Expected Role

The toolbar should provide persistent assistance for work in progress:

- Supervisor launches the configured agent with instructions to monitor the target app's Refine workflow, investigate and fix issues within its authority, and verify work;
- System shows local notices, operation events, and supervisor observations and recoveries;
- Files lets users inspect project files and search source;
- Terminal launches a standard interactive shell without an agent;
- Plan Mode launches the configured agent with planning context and an optional starting prompt;
- Goal launches the configured agent with current Goal or Feature context;
- Standalone launches the configured agent in an isolated Git worktree for broad exploration or implementation;
- Goal log tails show recent canonical Goal activity and append new entries from the shared event stream.

Current implementation details that matter to intent:

- persistent tab order is Supervisor, System, Files, Terminal, and Standalone, with Plan, Goal, and Goal-log tabs opened contextually;
- all five terminal profiles use the same terminal renderer, input, output stream, resize behavior, and Start/Stop/Restart controls;
- each terminal session is registered as an `interactive_session` in the daemon's ordinary process registry, so the toolbar and Processes surface share lifecycle truth;
- the configured `agent_cli` selects the native agent executable and interactive prompt form for every agent profile;
- terminal profile state is tab-specific, including process/session identifiers, provider, current directory, output, and standalone worktree identity;
- reload reattaches to a live terminal by session identifier; a stopped or lost session remains restartable;
- stopping a standalone session preserves its worktree, and restarting it reuses the same validated Refine worktree;
- changing target apps stops live toolbar terminals before clearing project-specific browser state;
- toolbar layout can be expanded or resized without changing the main route.

The toolbar should expose shared backend capability, not implement its own product rules or agent conversation protocol.

## Future Direction

Future toolbar behavior may become a fleet cockpit: active claims, pending approvals, process health, Goal evidence, files, and multiple native agent terminals.

The dock should remain a utility surface that follows the user through the product rather than a destination page competing with the main work surfaces.
