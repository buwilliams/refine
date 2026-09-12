# Nav

## Key Ideas

- **Orientation First**: nav should tell the user where they are and which app/node context is active.
- **Primary Work Paths**: Dashboard, Features, Goals, and Changes are first-class routes.
- **Context Controls**: app status, reporter, agent status, command palette, and create actions belong in the shell.
- **Stable Entry Points**: nav should be predictable enough for repeated daily use and future agent-driven UI control.

## Purpose

Navigation exists to make Refine's operating context immediately visible and to move users to the main work surfaces without ceremony.

The topbar is not just a list of pages. It shows the active node, active app context, reporter context, target-app status, agent status, command palette access, Guide access, management links, appearance preference, and primary create actions.

## Expected Role

Nav should keep the system grounded. If the user is attached to the wrong app, using the wrong reporter, or agents are active, the shell should make that context visible before the user takes action.

When an attached app has no valid browser-local Reporter selection, the shell should gently ask who the user is after the shared Reporter list is available. The user chooses an existing Reporter or creates one through the shared Reporter capability; Refine does not infer identity from the first available Reporter. The orientation dialog yields to route and utility dialogs so the shell presents one accessible modal context at a time.

Reporter selection remains local to the browser and can always be changed or created later under `Controls > Reporter`. Dismissing the first-load orientation leaves identity unselected for the rest of that page lifetime rather than repeatedly interrupting the user.

`Controls > Node`, immediately beside Reporter, displays and switches the runtime-local active Node for the attached app. The selector is reconciled from project status and the non-archived Node registry, shows display names for orientation, and keeps Node IDs authoritative for selection and activation. With no attached app it remains disabled and shows `No node` rather than implying an active context.

The current browser shell uses Dashboard, Features, Goals, and Changes as the main nav items. Management actions live in context menus so the main nav stays focused on work. The bright primary create action is `+ New Goal`, with related creation flows available nearby.

The `Controls > Settings` management entry consolidates Node and Governance configuration and uses Processes (`/#/settings/processes`) as its stable destination so local runtime work is immediately visible. This entry does not change the adjacent active Node selector or its context-switching behavior.

A separate Controls Skills section and command palette group list enabled Custom Skills applicable to the active project and node. Launch forms use the shared parameter preflight and open only when inputs are needed. Definition changes and node switches refresh this list.

Dashboard and Goals navigation carries their shared current/all Node scope in the URL. The URL remains the filter source of truth so reload, sharing, and browser history preserve that scope; a specific named-Node Goals filter is not projected onto Dashboard.

Nav should not hide important operating state or shell preferences in deep settings pages. Active app, node, target-app status, agent status, and the browser-local light/dark appearance toggle are part of the user's working context.

Whenever the running Refine checkout and update channel are discoverable, the
Controls menu exposes one compact source-update control independently of the
attached target app. It reads authoritative cached state and moves through
current, stale, checking, available, Agent progress, reconnecting, success,
failure, and retry states. Update is one-click authorization with no second
confirmation; concurrent clients converge on the same operation and attempt.

The Controls Skills section uses the same section labels and menu rows as the rest of Controls. Enabled Custom Skills are followed by **Add skill...**, which opens the New Skill modal without leaving the current screen. Running a Skill opens an agent tab after any required inputs are collected.

Controls also contains **Knowledge Hub**, listing sites for the attached app with an **Add site...** action. Management lives in the Settings **Knowledge Hub** tab immediately after **Skills**, rather than a Controls management modal. Opening a site uses a new tab. Sites use the same Refine web server: published sites at `/hub/sites/<site>/` and drafts at `/hub/preview/<site>/`. Hosting shares the installation's existing access boundary; publication selects the asset manifest and read-only collections, not a separate listener or account system.

Knowledge Hub entries in Controls use the shared management-item layout, including icon spacing, full-width controls, and section labels.

## Future Direction

Future navigation may become more command-palette and agent-driven. As agents take over more work, nav should help people jump to exceptions, evidence, pending review, active processes, and high-risk changes.

The nav should remain quiet and utilitarian: fewer marketing surfaces, more direct access to the work and system state that matter.
