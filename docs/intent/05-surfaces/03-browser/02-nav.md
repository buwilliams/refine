# Nav

## Key Ideas

- **Orientation First**: nav should tell the user where they are and which app/node context is active.
- **Primary Work Paths**: Dashboard, Features, Goals, Changes, and Logs are first-class routes.
- **Context Controls**: app status, reporter, agent status, command palette, and create actions belong in the shell.
- **Stable Entry Points**: nav should be predictable enough for repeated daily use and future agent-driven UI control.

## Purpose

Navigation exists to make Refine's operating context immediately visible and to move users to the main work surfaces without ceremony.

The topbar is not just a list of pages. It shows the active node, active app context, reporter context, target-app status, agent status, command palette access, Guide access, management links, appearance preference, and primary create actions.

## Expected Role

Nav should keep the system grounded. If the user is attached to the wrong app, using the wrong reporter, or agents are active, the shell should make that context visible before the user takes action.

When an attached app has no valid browser-local Reporter selection, the shell should gently ask who the user is after the shared Reporter list is available. The user chooses an existing Reporter or creates one through the shared Reporter capability; Refine does not infer identity from the first available Reporter. The orientation dialog yields to route and utility dialogs so the shell presents one accessible modal context at a time.

Reporter selection remains local to the browser and can always be changed or created later under `Settings > Reporter`. Dismissing the first-load orientation leaves identity unselected for the rest of that page lifetime rather than repeatedly interrupting the user.

`Settings > Node`, immediately beside Reporter, displays and switches the runtime-local active Node for the attached app. The selector is reconciled from project status and the non-archived Node registry, shows display names for orientation, and keeps Node IDs authoritative for selection and activation. With no attached app it remains disabled and shows `No node` rather than implying an active context.

The current browser shell uses Dashboard, Features, Goals, Changes, and Logs as the main nav items. Management actions live in context menus so the main nav stays focused on work. The bright primary create action is `+ New Goal`, with related creation flows available nearby.

The Settings dropdown's configuration entry consolidates Node and Governance configuration and uses Processes (`/#/settings/processes`) as its stable destination so local runtime work is immediately visible. This entry does not change the adjacent active Node selector or its context-switching behavior.

The dedicated Skills dropdown and command palette group list enabled Custom Skills applicable to the active project and node. Launch forms use the shared parameter preflight and open only when inputs are needed. Definition changes and node switches refresh this list.

Dashboard and Goals navigation carries their shared current/all Node scope in the URL. The URL remains the filter source of truth so reload, sharing, and browser history preserve that scope; a specific named-Node Goals filter is not projected onto Dashboard.

Nav should not hide important operating state or shell preferences in deep settings pages. Active app, node, target-app status, agent status, and the browser-local light/dark appearance toggle are part of the user's working context.

Whenever the running Refine checkout and update channel are discoverable, the
Settings menu exposes one compact source-update control independently of the
attached target app. It reads authoritative cached state and moves through
current, stale, checking, available, Agent progress, reconnecting, success,
failure, and retry states. Update is one-click authorization with no second
confirmation; concurrent clients converge on the same operation and attempt.

The right-aligned shell actions appear in order: **Skills**, **Sites**, **Settings**, and **+ New Goal**. Settings retains application status, Node and Reporter selectors, agent status, command palette, updates, appearance, Guide, and configuration access. New Goal retains its related creation actions.

Skills lists enabled Custom Skills for the active node, then a separator and **New Skill**, **Manage Skills**. New Skill opens the existing creation modal; Manage Skills navigates to `#/settings/skills`. Running a Skill opens an agent tab after any required inputs are collected.

Sites lists the attached app's Knowledge Hub sites, then a separator and **New Site**, **Manage Sites**. New Site opens the existing creation modal; Manage Sites navigates to `#/settings/knowledge-hub`. Opening a site uses a new tab: published sites at `/hub/sites/<site>/` and drafts at `/hub/preview/<site>/`. Hosting shares the installation's existing access boundary.

Both menus use shared summary, icon, menu-row, panel, border, and spacing styles. Creation and management remain available for empty or failed lists; failures appear separately from the action group. Lists refresh on opening, relevant definition changes, and node-context changes, with stale responses discarded. The shell owns mutual exclusion, action dismissal, outside clicks, and Escape. Panels remain scrollable and within the viewport on narrow screens and with long lists.

## Future Direction

Future navigation may become more command-palette and agent-driven. As agents take over more work, nav should help people jump to exceptions, evidence, pending review, active processes, and high-risk changes.

The nav should remain quiet and utilitarian: fewer marketing surfaces, more direct access to the work and system state that matter.
