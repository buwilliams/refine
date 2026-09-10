# Settings

## Key Ideas

- **Settings Are Operational Context**: settings determine how Refine works, not just how it looks.
- **Domain Placement**: controls should live with the feature domain they affect.
- **Detached Safe**: settings should handle no-app and app-switch states explicitly.
- **Scoped Loading**: settings surfaces should fetch only the data needed for the active tab.
- **Guide Connected**: settings fields should connect to guidance where users need explanation.

## Purpose

Settings exist to configure Refine's relationship to the project, node, target app, agents, runtime, Events, Skills, reporters, processes, and performance.

They should help users make Refine work correctly in their environment without requiring deep knowledge of the internal implementation.

## Expected Role

Settings consolidates the former Node and Governance navigation entries under `/#/settings/<tab>`. Events and Skills have their own tabs in the node configuration surface. Governance and Quality remain workflow steps; their instructions are configured as Skills. Retired configuration routes redirect to Settings without retaining competing editors.

The tab order is Processes, Application, Reporters, Skills, Events, Target App, Runtime, and Refine (dev).

Current implementation details that matter to intent:

- settings render through shared `renderSettingsSurface` flows;
- settings data loads are scoped by active surface and tab;
- detached mode short-circuits app-scoped calls and keeps app management actionable;
- target-app settings, runtime settings, reporters, Events, Skills, processes, and performance are separate concerns;
- Runtime presents a blank parallel-run cap as `Automatic`. Automatic admission applies the node's resource-budget percentage to both detected logical CPU cores and currently available memory; the default is 70 percent, leaving 30 percent for shared-host work. Entering a positive parallel-run cap is an explicit absolute node-level override. Unset node, provider, and target-app limits inherit the resulting global limit;
- node runtime settings include a validated state-sync stale threshold. Its default is longer than the normal remote-fetch cadence so routine scheduling jitter does not degrade health;
- Runtime includes Auto-approve for email-request Goals processed by that node. The validated `auto_approve` boolean defaults to `false` on new and existing installations, leaving Goals in Review for manual acceptance or a follow-up Round. The shared settings API, CLI, runtime copy action, and existing edit/autosave flow use the same setting. Changes apply on the next approval attempt, including requests already waiting in Review, without restarting the worker. When enabled, the local email `auto_approve_after_seconds` delay runs from the first observed Review time, recorded even while disabled. Invalid or unreadable settings prevent automatic acceptance; manual acceptance and notifications after Done remain available;
- the Nodes view keeps fleet bootstrap health separate from state-sync health: the active node uses this daemon's evidence and other nodes remain unknown without direct evidence;
- Events lists custom user Events only. Skills and Events use plain row labels and clickable rows, including keyboard activation. The Skill editor assigns system or custom Events to the Skill and configures execution mode, order, scope, overrides, and context mappings;
- editors reuse shared modal, input, table, and segmented controls. Scope, result role, and enabled state use toggles. Parameter choices appear only for choice parameters, and unsaved new definitions have no Delete action;
- Events and Skills share a configuration revision. A Skill and its Event assignments save atomically. Editors retain drafts and refresh the underlying state after a conflict. Node switches fence submissions and preserve or explicitly discard unsaved drafts;
- custom Event launch forms collect typed parameters and defaults, and execution history exposes findings, errors, process evidence, and cancellation;
- Guide icons and guidance surfaces are expected to help explain fields.

Settings should avoid overfetching and avoid hiding invalid states. If Refine is detached, paused, misconfigured, or missing a target app command, the settings surface should make that clear.

## Future Direction

Future settings should become more inferential. Agents should be able to inspect the project, propose target-app lifecycle instructions and deterministic checks, explain tradeoffs, and safely update configuration with evidence.

The surface should move toward guided configuration and reviewable changes, not a growing form full of disconnected knobs.
