# Toolbar

## Key Ideas

- **Persistent Utility Dock**: the toolbar is always available without replacing the main route.
- **Multi-Tool Surface**: Supervisor, System, Files, Terminal, Standalone, Goal chat, and live Goal logs belong together as operational aids.
- **Contextual Chat**: chat can attach to a Goal or remain standalone.
- **Stateful But Recoverable**: toolbar state should persist enough to survive normal navigation without owning product state.

## Purpose

The toolbar exists to keep supporting work close at hand. Users need system notices, source files, terminal access, standalone agent conversations, and Goal chat while they inspect or edit the main surface.

It should reduce context switching without turning every tool into a full page. In particular, users should be able to follow an agent's Goal activity in place while continuing to inspect the product.

## Expected Role

The toolbar should provide persistent assistance for work in progress:

- Supervisor shows durable workflow health and supports conversational steering without an event stream cluttering the conversation;
- System shows local notices, operation events, and supervisor observations and recoveries;
- Files lets users inspect project files and search source;
- Terminal exposes controlled shell access;
- Standalone supports broad agent conversation and worktree-backed experiments;
- Goal chat supports conversation tied to specific work;
- Goal log tails show recent canonical Goal activity and append new entries from the shared event stream.

Current implementation details that matter to intent:

- toolbar tab order is Supervisor, System, Files, Terminal, Standalone, then opened Goal chat and log tabs;
- Supervisor is always present, including while idle, and reads shared backend state;
- Supervisor has no manual Start or Stop control because its shared backend capability is always observing or idle;
- Supervisor provider and authentication failures remain visible with retry guidance instead of being reduced to an idle state;
- tab state is stored in browser local storage;
- only relevant chat tabs are polled;
- output and pending messages are session-specific;
- pending user messages appear in the transcript rather than in a separate editor, while the prompt input remains ready for the next message;
- prompt drafts, focus, caret selection, and input scroll survive toolbar DOM redraws and remain tab-specific;
- agent status and activity stay inline with the transcript, with activity collapsed by default;
- Supervisor transcripts show user steering and agent responses; automatic evidence and provider/tool progress remain internal or under collapsed Activity;
- toolbar layout can be expanded or resized without changing main route.

The toolbar should expose shared backend capability, not implement its own product rules.

## Future Direction

Future toolbar behavior may become an agent cockpit: active claims, pending approvals, fleet events, file context, live terminals, and conversational steering.

The dock should remain a utility surface that follows the user through the product rather than a destination page competing with the main work surfaces.
