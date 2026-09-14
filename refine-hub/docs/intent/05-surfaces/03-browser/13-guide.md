# Refine Hub and product help

Refine Hub replaces the separate Guide panel. It is the permanent, built-in Knowledge Hub site for product documentation, release notes, runbooks, and design intent.

## Outcome

Users can understand Refine and discover changes through one documentation surface. The application and website serve the same versioned content. Release notes explain “What you need to know,” with a subsection and screenshots for each major change.

## Guardrails

- Bundle content with the executable so it works offline and matches the installed release.
- Keep the built-in site read-only and non-removable across UI, API, CLI, and MCP operations.
- Keep user-created sites in project state; product updates must not overwrite them.
- Settings help links open the relevant product reference entry.
- Keep existing public documentation URLs working as content moves.
- Store authored documentation and assets under `refine-hub/`. Retain repository entry points, executable prompt templates, and required licenses with their owners.
