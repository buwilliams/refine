# Navigation

## Purpose

Refine uses one persistent left rail and one content area. The logo is centered in its own row. The rail contains Search, New, Node, Reporter, a Tools menu with an always-visible list of open windows, a Main menu with persistent screen rows, Planning, and independently collapsible Skills and Hubs sections. Dashboard has no separate global navigation.

## Behavior

- Main contains Dashboard, Features, Goals, Changes, Control, and Settings. Control contains process management and target app Build, Start/Stop, and Check actions. Settings opens Nodes by default.
- Tools lists the explicitly opened tools and agent sessions. Each destination occupies the right content area. Exactly one destination is active across both sections.
- The whole Skills or Hubs header row toggles its section. Collapsing a section does not change the active screen or stop a session.
- Node and Reporter use full-row pickers, showing the selected values in the expanded rail. Node selection uses authoritative IDs and the existing runtime context-switching behavior. With no attached app, Node is disabled.
- A borderless “Collapse menu <<” control at the bottom collapses the rail to icons; “>>” expands it. It stays available while the navigation list scrolls. Labels remain available through accessible names and hover titles. Rail and section preferences are stored locally. On narrow screens, navigation is a drawer with a dismissible backdrop.
- Search remains above the collapsible sections and displays Ctrl+K, or ⌘K on Mac. The shortcut opens the shared command palette from all content, including terminals, agents, and dialogs.
- Main destinations and windows participate in browser history. Switching screens retains each Main destination’s live content in the page session. Inactive hosts are detached so global selectors see only the active screen. Returning restores its controls, draft, selection, and scroll position.
- Dashboard and Goals translate shared current/all Node scope for new destinations. An already-open screen restores its own URL. Explicit parameterized links update it.

## Creation, management, and support

New is a full-width row immediately below Search and before Node, independent of collapsible sections. Its anchored menu lists New Goal, New Plan, New Feature, and Import in that order. These use the shared creation commands and their current Node, Reporter, validation, retained drafts, and confirmation behavior. Each New Plan selection opens a fresh Planning Agent session through the existing planning flow. Opening or dismissing the menu creates nothing.

New remains accessible by label and hover title in the collapsed rail. The menu stays within the viewport and supports keyboard opening, arrow navigation, Home/End, and visible focus. Escape dismisses it and restores focus to New; outside clicks and selection also dismiss it. Only one rail menu is open at a time. On mobile, selection closes the drawer and releases its focus trap and workspace inert state before the selected flow opens.

The Tools dropdown offers Agent, Agent in Worktree, System, Files, Terminal, and Planning Agent. Goal-specific agent and log windows remain available from their existing actions. Repeated agent launches create independent sessions. Opening or closing the Tools dropdown does not hide existing windows. Context menus align to the top of their opening row and shift upward as needed to stay within the viewport. Node and Reporter menus identify their context with a heading; Add Node and Add Reporter open the shared creation flows.

Creation actions remain available in their page headers and Search. Settings → Workspace controls & support contains shared creation shortcuts, workflow and target-app quick controls, source update, contact, and appearance controls. Skills and Hubs have their own rail sections, including their Add and Manage actions. Settings owns configuration; Control owns process management.

Enabled Custom Skills use shared parameter preflight and open an agent window. Hub entries open sites in a separate browser tab. Published sites use `/hub/sites/<site>/`, and previews use `/hub/preview/<site>/`, under the existing server access boundary.

## Reporter orientation

When an attached app has no valid browser-local Reporter selection, Refine asks the user to choose or create a Reporter after loading the shared list. It never infers identity from the first entry. The orientation dialog yields to other dialogs; dismissing it leaves identity unselected for that page lifetime. The Reporter row remains available for later selection.

The navigation section order is Main, Tools, Planning, Skills, Hubs. Planning has its own board picker and Create board action, following the Tools dropdown pattern. Boards are shared across project nodes. It replaces the former Todo tool; choosing a Reporter does not filter its boards.

Main uses the same full-row context menu and persistent-row styling as Tools and Planning, with the bundled panels-top-left icon. It supports keyboard opening, arrows, Home/End, Escape with focus restoration, outside dismissal, collapsed rail, and mobile drawer selection. Open rows remain visible when the menu closes. There is one instance per destination. Closing releases its retained state and focuses the first remaining Main row, falling back to Dashboard. Dirty Settings work requires confirmation.

Main menu items, screen rows, Search, direct links, and browser history use the same lifecycle. Goal and Feature details keep their owning underlay. Screen activation refreshes clean content without resetting interactions; pending reads are fenced by activation and Node context. Node or target-app changes invalidate retained data, and external changes preserve unsaved drafts as disabled stale surfaces. Browser reload reconstructs the current route, not DOM or unsaved forms. Tool session lifetime is independent of screen activation.
