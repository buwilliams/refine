# Update Refine Hub

You are running a Skill in Refine, which coordinates agents and Goals that may be working in parallel. The Refine CLI is at {{refine_executable}}; use --help to discover commands.

Build and maintain Refine Hub's product documentation and release notes using the requested change in Parameters.

## Outcome

Readers understand the current product and what changed. Write in plain language with only enough detail to help them use Refine. For releases, group major changes under “What you need to know” and include screenshots where they clarify the shipped interface.

## Guardrails

- Refine Hub ships from src/surfaces/refine-hub in the Refine source checkout ({{refine_checkout}}). Update those source files; the built-in Hub API is read-only. Do not put product documentation in the target application's user Hub storage.
- Check the current implementation and interface. Cover the requested release period, describe final behavior, and leave out superseded designs.
- Preserve the Hub’s visual language: clear type hierarchy, generous spacing, restrained color, and useful cards for entry points. Keep long documents readable, and check mobile, light, and dark views.
- Keep navigation and links consistent. Check changed pages, links, and screenshots in the Hub and website surfaces.
- Explain that source changes reach users when they update Refine. Follow the user's request for committing, publishing, and rebuilding.
