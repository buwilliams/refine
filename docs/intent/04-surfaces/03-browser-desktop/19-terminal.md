# Terminal

## Key Ideas

- **Controlled Shell Access**: terminal access should be powerful but tied to known project/worktree context.
- **Worktree Awareness**: commands should run in intentional directories, not arbitrary hidden locations.
- **Operational Proximity**: users should be able to inspect and act without leaving Refine.
- **Observable Sessions**: terminal output, connection state, resize, input, and exit should be handled explicitly.

## Purpose

The Terminal surface exists because software work often requires direct shell access. Refine should let users and agents inspect the environment, run commands, and debug work from the same operational console.

The goal is not to replace a full terminal application. It is to provide contextual command execution close to the work.

## Expected Role

Terminal should be constrained enough to be understandable and powerful enough to be useful. It should prefer discovered project or Git worktree contexts and expose session state clearly.

Current implementation details that matter to intent:

- terminal is a toolbar tab;
- backend routes create terminal sessions, send input, resize, stop, and stream events;
- browser copy and paste shortcuts are scoped to the focused shared terminal
  surface: copying an active selection does not interrupt the PTY, while
  control-C without a selection retains normal terminal semantics;
- clipboard text, including multiline text, is sent unchanged through the
  managed terminal input route, and clipboard access failures remain visible;
- output is retained up to a bounded size in the UI;
- terminal sessions run through the local daemon rather than raw browser execution;
- worktree-aware terminal behavior supports merge and standalone workflows.

Terminal should remain an operational tool. Product workflow state should still be changed through shared Refine capabilities, not by undocumented shell side effects.

## Future Direction

Future terminal behavior may be increasingly agent-driven: agents may request shell sessions, explain commands, capture evidence, and hand outputs back into workflow.

The surface should evolve toward auditable command execution with clear context, provenance, and recovery.
