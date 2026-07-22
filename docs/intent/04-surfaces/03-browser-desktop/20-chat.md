# Browser Agent Interaction

## Key Ideas

- **Harness Native**: the browser embeds real terminal sessions running the configured agent CLI instead of maintaining a custom chat transcript and input protocol.
- **Context Through Launch**: Supervisor, Plan Mode, Goal, and Standalone inject concise role and work context when the native agent starts.
- **One Terminal Primitive**: agent profiles reuse the same PTY, streaming, resize, and managed-process implementation as the plain Terminal tab.
- **Durable Work Outside Conversation**: Goals, Features, rounds, logs, governance, and workflow remain Refine state; a terminal transcript is not product truth.
- **Explicit Lifecycle**: users start, stop, and restart every browser agent session.

## Purpose

The browser agent surface lets users work with frontier agent harnesses without leaving Refine's orchestration context. Refine supplies the target app, role, Goal or Feature context, and isolated worktree where appropriate. The native CLI owns conversation UX, tools, approvals, and provider-specific capabilities.

This removes Refine's former custom browser chat UI from Supervisor, Plan Mode, Goal, and Standalone tabs. The backend chat capability may still support automated workflow or non-browser adapters, but it does not define the toolbar interaction model.

## Expected Role

- Supervisor starts a configured agent prepared to observe and help with the active Refine workflow.
- Plan Mode starts a configured agent prepared to explore a feature or Goal plan and use Refine's CLI to persist selected work.
- Goal starts a configured agent with fresh durable Goal or Feature context.
- Standalone starts a configured agent inside an isolated, reusable worktree.

Browser persistence is limited to what is needed to reconnect the terminal: its managed process/session identity, profile metadata, provider, working directory, and standalone worktree. Refine does not parse a native harness transcript to infer durable work. The agent or user creates and changes Refine work through the existing CLI, API, or product surfaces.

## Future Direction

New agent harness features should normally arrive through the configured CLI rather than by adding parallel browser controls. Refine's browser work should focus on fleet visibility, context routing, safe process lifecycle, worktree ownership, and durable workflow handoff.
