# Standalone

## Key Ideas

- **Unattached Exploration**: Standalone supports work that is not yet attached to a Goal.
- **Native Agent Terminal**: it runs the configured agent harness in the shared toolbar terminal surface.
- **Worktree Backed**: every session runs in an isolated Refine-owned Git worktree.
- **Restartable Workspace**: stopping the process preserves the worktree, and restarting the tab reuses that exact validated branch and path.
- **Workflow Through Refine**: useful output becomes durable through Refine's existing CLI, API, and product operations rather than transcript extraction.

## Purpose

The Standalone surface exists for broad agent collaboration that is not yet attached to a specific Goal. It supports exploration, planning, debugging, experiments, and implementation without risking the target app's primary checkout.

This matters because not all useful work starts as a clean Goal. Refine should provide a safe workspace and orchestration context while leaving the actual agent interaction to the configured native harness.

## Expected Role

Standalone is a permanent toolbar tab with explicit Start, Stop, and Restart controls. Starting it creates a Refine-owned branch and worktree, launches the configured agent there with a concise standalone prompt, and registers the PTY as an ordinary managed process. Its output streams through the shared terminal implementation and remains independent from other toolbar terminals.

Stopping ends the agent process but does not delete or discard the worktree. Restart validates and reuses the recorded Refine worktree. The agent or user can inspect the result and use ordinary Refine commands or product surfaces to create Goals, Features, or merge handoffs.

Standalone should not become a separate workflow engine or a Refine-owned chat protocol. Its successful output should become durable work through existing Refine capabilities.

## Future Direction

Standalone may become the primary launch point for broad design changes and experiments. Future work should improve worktree visibility, evidence, and handoff while continuing to inherit conversation and tool UX from the configured agent harness.
