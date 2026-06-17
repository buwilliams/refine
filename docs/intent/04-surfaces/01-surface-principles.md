# Surface Principles

## Key Ideas

- **Surfaces Are Adapters**: they expose model, workflow, process, and tools without owning their meaning.
- **Same Capability, Different Ergonomics**: CLI, browser, API, desktop, and agent surfaces should differ by interaction style, not by product semantics.
- **Agent-First Direction**: surfaces should increasingly support direct agent operation.
- **Human Accessibility**: browser and desktop surfaces should make Refine usable by people who do not want to operate only through a terminal.
- **No Surface Monoculture**: the system should not depend on one interface surviving forever.

## Purpose

Surfaces exist so different actors can use Refine well. A person may need visual overview, command palette navigation, settings, logs, or review affordances. A CLI user may need reliable commands and JSON output. A future agent may need direct capability access with little or no human-style UI.

The purpose of a surface is to make shared capability usable in a context, not to create a separate product.

## Expected Role

Every surface should preserve the same underlying model and workflow semantics. If a Gap cannot transition in the browser, it should not be secretly allowed by the CLI. If a Feature order matters in workflow, the UI should explain it rather than hide it.

Surfaces should:

- call shared services or daemon routes,
- expose state clearly,
- avoid duplicating workflow logic,
- make errors visible and recoverable,
- route user-visible notices into durable activity or System surfaces when appropriate,
- stay replaceable as AI-native interfaces improve.

The current implementation has CLI, web, web server/API, and desktop modules. The browser and desktop are currently the richest human surfaces; the CLI is the most reliable command surface; the API exists mainly as the local daemon contract that shared surfaces use.

## Future Direction

Future surfaces may be voice, IDE-native, agent-native, MCP-style, terminal-first, or fully autonomous. Refine should be ready for that by keeping the system's meaning below the surface layer.

The eventual dominant surface may be no visible UI at all: an AI system reading intent, inspecting state, and operating shared capabilities directly. The browser and desktop should still remain valuable for oversight, review, explanation, and manual intervention.
