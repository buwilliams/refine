# Settings

## Key Ideas

- **Settings Are Operational Context**: settings determine how Refine works, not just how it looks.
- **Domain Placement**: controls should live with the feature domain they affect.
- **Detached Safe**: settings should handle no-app and app-switch states explicitly.
- **Scoped Loading**: settings surfaces should fetch only the data needed for the active tab.
- **Guide Connected**: settings fields should connect to guidance where users need explanation.

## Purpose

Settings exist to configure Refine's relationship to the project, node, target app, agents, runtime, quality, governance, guidance, reporters, processes, and performance.

They should help users make Refine work correctly in their environment without requiring deep knowledge of the internal implementation.

## Expected Role

Settings should be split by product domain. Project concerns belong with project surfaces. Node and runtime concerns belong with node surfaces. Quality belongs with quality. Governance belongs with governance. This keeps configuration understandable and prevents generic settings sprawl.

Current implementation details that matter to intent:

- settings render through shared `renderSettingsSurface` flows;
- settings data loads are scoped by active surface and tab;
- detached mode short-circuits app-scoped calls and keeps app management actionable;
- target-app settings, quality settings, runtime settings, reporters, governance, guidance, processes, and performance are separate concerns;
- Guide icons and guidance surfaces are expected to help explain fields.

Settings should avoid overfetching and avoid hiding invalid states. If Refine is detached, paused, misconfigured, or missing a target app command, the settings surface should make that clear.

## Future Direction

Future settings should become more inferential. Agents should be able to inspect the project, propose target-app commands, explain tradeoffs, and safely update configuration with evidence.

The surface should move toward guided configuration and reviewable changes, not a growing form full of disconnected knobs.
