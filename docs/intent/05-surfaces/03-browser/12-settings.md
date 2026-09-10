# Settings

## Key Ideas

- **Settings Are Operational Context**: settings determine how Refine works, not just how it looks.
- **Domain Placement**: controls should live with the feature domain they affect.
- **Detached Safe**: settings should handle no-app and app-switch states explicitly.
- **Scoped Loading**: settings surfaces should fetch only the data needed for the active tab.
- **Guide Connected**: settings fields should connect to guidance where users need explanation.

## Purpose

Settings exist to configure Refine's relationship to the project, node, target app, agents, runtime, Skills, reporters, processes, and performance.

They should help users make Refine work correctly in their environment without requiring deep knowledge of the internal implementation.

## Expected Role

Settings consolidates the former Node and Governance navigation entries under `/#/settings/<tab>`. Skills have a tab in the node configuration surface; Events remain an internal concept. Governance and Quality remain workflow steps; their instructions are configured as Skills. Retired configuration routes redirect to Settings without retaining competing editors.

The tab order is Processes, Application, Reporters, Skills, Target App, and Runtime. Release work is configured as a Custom Skill, with no separate development tab.

Current implementation details that matter to intent:

- settings render through shared `renderSettingsSurface` flows;
- settings data loads are scoped by active surface and tab;
- detached mode short-circuits app-scoped calls and keeps app management actionable;
- target-app settings, runtime settings, reporters, Skills, processes, and performance are separate concerns;
- Runtime presents a blank parallel-run cap as `Automatic`. Automatic admission applies the node's resource-budget percentage to both detected logical CPU cores and currently available memory; the default is 70 percent, leaving 30 percent for shared-host work. Entering a positive parallel-run cap is an explicit absolute node-level override. Unset node, provider, and target-app limits inherit the resulting global limit;
- node runtime settings include a validated state-sync stale threshold. Its default is longer than the normal remote-fetch cadence so routine scheduling jitter does not degrade health;
- the Nodes view keeps fleet bootstrap health separate from state-sync health: the active node uses this daemon's evidence and other nodes remain unknown without direct evidence;
- Skills use plain labels and clickable rows with keyboard activation. Each row shows its trigger and has a Status toggle that saves enabled state without opening the editor, preserving the trigger and refreshing manual discovery. Conflicting writes refresh the current configuration;
- the Skill modal leads with rendered instructions and Markdown editing. Its title names the Skill, and one settings section below contains all configuration controls. Trigger, Scope, and Status share a responsive row;
- editors reuse shared modal, input, table, and segmented styles. Parameter choices appear only for choice parameters, and new Skills have no Delete action;
- a Skill and its trigger save atomically under the shared configuration revision. Editors retain drafts after a conflict. Node switches fence submissions and preserve or explicitly discard unsaved drafts;
- Custom Skills are standalone actions in the selected project. Browser launches never inherit an open Goal. Launch forms collect typed parameters and defaults, then open the selected Skill in an agent tab with normal transcript, reconnect, and stop controls. Headless execution history retains findings, errors, process evidence, and cancellation;
- Guide icons and guidance surfaces are expected to help explain fields.

Settings should avoid overfetching and avoid hiding invalid states. If Refine is detached, paused, misconfigured, or missing a target app command, the settings surface should make that clear.

## Future Direction

Future settings should become more inferential. Agents should be able to inspect the project, propose target-app lifecycle instructions and deterministic checks, explain tradeoffs, and safely update configuration with evidence.

The surface should move toward guided configuration and reviewable changes, not a growing form full of disconnected knobs.
