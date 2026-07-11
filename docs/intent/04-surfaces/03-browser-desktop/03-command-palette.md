# Command Palette

## Key Ideas

- **Fast Navigation**: users should be able to jump to work and tools without hunting through the UI.
- **Action Discovery**: commands should expose what Refine can do in the current context.
- **Keyboard First**: repeated workflows should not require pointer-heavy navigation.
- **Shared Commands**: palette entries should call the same commands as buttons and routes.

## Purpose

The command palette exists to make Refine fast for experienced users and accessible to agents or keyboard-driven workflows. It should provide a compact way to open routes, invoke actions, search files, start flows, and reach tool surfaces.

It turns the UI from a set of pages into an addressable command surface.

## Expected Role

The command palette should sit above individual screens. It should know about global actions, current-route actions, toolbar tools, file search, creation flows, and management surfaces.

Current implementation details that matter to intent:

- the topbar exposes the palette as a persistent shell action;
- command registration is shared rather than hardcoded only in page buttons;
- file search integrates with toolbar file behavior;
- keyboard selection should make the next Enter action explicit.

The palette should not become an unrelated shortcut list. It should mirror real product capability.

## Future Direction

Future command palettes should become more agent-aware and intent-aware. A user may type an outcome, and Refine may route that to a Goal, Feature, search, setting, tool, or agent conversation.

The long-term direction is a command surface that can be used by humans and AI systems as a compact index of Refine capabilities.
