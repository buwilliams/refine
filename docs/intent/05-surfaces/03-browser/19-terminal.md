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
- text copy and paste have the same browser contract across Terminal, Agent,
  Planning Agent, Goal, Agent in Worktree, and Skill profiles, regardless of
  provider. Native browser Copy/Paste menu actions and control-C/V (command-C/V
  on macOS), plus Ctrl+Insert/Shift+Insert where supported by the browser, are
  authoritative and do not require asynchronous clipboard access;
- all Agent, Custom Skill, and shell terminal tabs expose a selection hint: Shift-drag on
  Windows/Linux and Option-drag on macOS select text even when the application
  captures mouse input; ordinary mouse gestures still reach the application;
- a keyboard-accessible Copy selection control follows the current selection;
  it and native Copy work with retained output after exit or disconnection,
  preserving the originating tab, selection, and renderer;
- returning to a tab with selected output or an unfinished copy preserves that
  context without forced scrolling, including selection made while reattaching;
  restarting remains an explicit header action while copying;
- browser copy and paste shortcuts are scoped to the focused shared terminal:
  Ctrl+C, Ctrl+Shift+C, and Cmd+C copy selected text without interrupting the PTY,
  while Ctrl+C without a selection retains normal terminal semantics; controls
  outside the terminal, including the manual copy field, keep browser shortcuts;
- copying reports success only after a supported copy method succeeds; if
  automatic copying is blocked, the originating tab offers the captured text in
  a standard selectable field with instructions for manual copying, without
  stealing focus from another tab;
- control-Enter inserts an editable line break in native agent TUI prompts;
- focused Alt+Up and Alt+Down preserve their modifiers across all shared
  terminal profiles and providers; the attached application owns what those
  shortcuts do;
- control-Z is consumed by Agent terminal profiles so it cannot suspend the
  attached agent TUI, while ordinary shell terminals retain job control;
- clipboard text, including multiline text, uses xterm's terminal-native paste
  semantics before reaching the managed input route, preserving bracketed-paste
  framing and line endings as the attached PTY application expects. One accepted
  paste produces one input operation and cannot cross into a replacement session;
- terminal-specific control-Shift-C/V shortcuts use asynchronous clipboard access
  as a separate fallback. Browsers may restrict that API by permission or secure
  context; blocked copies retain the existing manual recovery field, and paste
  failures are visible and point to native browser clipboard gestures.
  Pending reads are discarded if their session, renderer, active tab, or focus
  no longer matches. Browser and operating-system clipboard restrictions still
  apply, and clipboard text is never read without a user clipboard gesture;
- output is retained up to a bounded size in the UI;
- terminal sessions run through the local daemon rather than raw browser execution;
- worktree-aware terminal behavior supports merge and standalone workflows.

Clipboard attempts and recovery text are transient browser state owned by their
originating tab. They do not change process or workflow authority.

Terminal should remain an operational tool. Product workflow state should still be changed through shared Application behavior, not by undocumented shell side effects.

## Future Direction

Future terminal behavior may be increasingly agent-driven: agents may request shell sessions, explain commands, capture evidence, and hand outputs back into workflow.

The surface should evolve toward auditable command execution with clear context, provenance, and recovery.
