# Features Index

Nav & Content: Dashboard
	Node scope switcher: Current node, All nodes (#/?node=all)
	Banners (global error/attention; "Re-check auth" action when runtime unreachable)
	Workflow Visualization
		Status cards: backlog, todo, in-progress, qa, ready-merge, build, review, done, failed, cancelled
		"AI" badge on agent-managed statuses (todo, in-progress, qa, ready-merge, build)
		Goal count per status
		Click a status opens Goals screen filtered to that status
	Awaiting your review
		Expand/Collapse (state persists), count pill, "Needs attention" pill + action links
		Review goals table: checkbox, goal name (link), updated, actions
		Per row: Verify, Add round
		Select all (indeterminate state), Verify selected (count, disabled if none)
		Add round modal: prompt textarea, Submit new round, Cancel
		Empty state: "You're clear. No review items..."
	Reporter throughput
		Expand/Collapse (state persists), count pill
		Table: reporter, active, done, reported, completion rate %
		Click row opens Goals filtered by reporter
		Empty state: "No reporter activity yet."
Nav & Content: Features
	URL Query string filters
	Filter card Expand/Collapse, "Filtered" pill when active
	Filters: Search, Status, Reporters, Nodes, # of entries, Clear filters button
	Features table
		Columns: name, status, progress, current / next goal, reporter, node, updated
		Sortable, sort direction arrows
		Click row to open Feature modal
		Pagination: # of # entries, first, prev, Page #, next, last
Nav & Content: Goals
	Workflow Visualization scoped to Goal filters (click status updates filters)
	URL Query string filters (q, status, reporter, feature, rounds, node, severity, category, actor, limit, page, sort, dir)
	Filter & bulk actions card Expand/Collapse, "Filtered" pill when active
	Filters: Search, Status, Reporter, Nodes, Feature ID (or standalone), Round >=, Round <=, Severities, Categories, Actors, Entries (50/100/250/500/1000), Clear filters button
	Bulk actions: Select page, Status, Priority, Reporter, Assign Feature, Transfer node, Delete
		Filter-scoped selection (select-all-matching across pages vs per-page), excluded IDs, indeterminate header checkbox
		Each opens its own Bulk Action modal (see Modals)
	Goals table
		Selection checkboxes (shown when filter card expanded), select-all header
		Columns: name, status, priority, reporter, feature (link + order #), node, updated
		Column Sorting (all but feature), default updated desc
		Click row to open Goal Modal
		Pagination: # of # entries, first, prev, Page #, next, last
Nav & Content: Changes
	Git Visualization scoped to Change filters (goals merged by day, week, month, year)
	URL Query string filters (q, status, priority, limit, page)
	Filter card Expand/Collapse, "Filtered" pill when active
	Filters: search (goal/commit/status), status, priority, entries, clear filters button
	Branch info (merge target branch; empty state if unresolved)
	Changes table
		Columns: when, goal (link or "Unlinked Goal"), status, priority, merge commit (abbrev hash), actions
		Column Sorting
		Undo button per row (disabled if cancelled)
			Confirmation modal: undo the approved implementation, reconcile automatically, move Goal to cancelled
		Click row to open Goal Modal
		Pagination: # of # entries, first, prev, Page #, next, last
Nav & Content: Logs
	Log Visualization scoped to Log filters (by day, week, month by severity: info, warn, error)
	URL Query string filters (severity, category, actor, goal_id, q, limit, page, sort, dir)
	Filter card Expand/Collapse, "Filtered" pill when active
	Filters: search, goal id, severity, category, actor, entries, clear filter button
	Logs table
		Columns: when, severity (badge), category, actor, goal (link)
		Column Sorting, default datetime desc
		Rows show message; "Show details" expand/collapse for extra detail
		Pagination: # of # entries, first, prev, Page #, next, last
Nav & Guide: Manage Drop-down
	App status (green, yellow, red), app name / Reporter
	Reporter selector (global)
	Drop-down
		Guide (slide-out panel)
			Header: title, close (x), resizable width (drag), tab strip
			Get Started (checklist)
				Field Card
					Expand/Collapse (chevron)
					Status cycle: todo (unchecked), complete (checkmark), skipped (dash)
					Description + default/action text
					Prev (disabled if first)
					Use default (when available)
					Skip
					Complete
			Reference
				Field Search (live filter)
				Category list (collapsible sections, complete icon)
				Field
					Field navigation (click to open/select)
					Field explanations (description + guide text)
			Field guide links throughout settings (info icon → opens guide to that item)
		Node (#/node)
			Tabs: Application, Reporters, Processes, Performance, Target App Config, Runtime Config, Refine (dev)
			Application
				Target application: select known app, status indicator, app path
				Add app (file path / Git clone URL / new directory), Switch to selected (migration check), Remove selected (danger), Copy from node, Generate with AI
				Disabled unless supervisor/registry enabled
				Nodes section: table (name + active pill, ID, goal counts, optional host/port/status), Activate, Rename, Connection, Bootstrap (over SSH), Enable/Disable, Archive (danger), Create node
			Reporters
				List with counts; Rename (cascades rounds), Merge (move goals to destination), Remove (danger, keeps history), Add reporter
			Processes
				Process management table: name (supervisor parent/child expand), status, PID, CPU priority, max memory, details
					Actions by kind: Pause/Unpause agents, Stop/Start background, Hard reset worktree, Cancel (agent), Stop (chat), target app Start/Stop/Build/Sync/Check
				Subprocesses table: name, status, PID, CPU priority, max memory, elapsed (live), details
					Actions: Rebuild, Generate, Clean up (days dropdown, danger)
				Projection cache rebuild progress
			Performance
				Summary: operation, count, failures, avg/p95/max latency, last seen
				Recent events: when, operation, elapsed, outcome, goal, provider, mode, resource, rows; pagination
				Filters (operation, outcome, limit, clear), "Filtered" pill
				Refresh, Prune old metrics, Clear metrics (danger)
			Target App Config
				Scope label, Copy from node, Generate with AI
				Application scope: agent subpath, merge target branch
				Target app: URL, start/stop/build agent instructions, test/status commands, automatic build (never/on merge/hourly/daily), daily build hour, working directory, environment overrides (JSON), timeouts, log path, generated notes
				Optional checks: HTTP URL, TCP host + port, process check command
			Runtime Config
				Scope label, Copy from node
				Runtime: parallel-run cap, branch name pattern ({goal_id}), agent idle/hard timeouts, worker/UI memory limits, worker CPU priority, resource isolation mode, rate/token-limit pause, standalone chat idle timeout, auto-promote backlog→todo, target repo update pulse, file browser ignore patterns
				AI Provider: provider selector (Claude Code / Codex / Gemini / Copilot / Smoke AI), auth help, Re-check auth (pre-flight)
				Runtime upgrade banner (current + latest versions, copy upgrade command)
			Refine (dev)
				Semantic releases: version preview, reviewable preparation Goal, persisted activity, explicitly confirmed publication
				Dogfood source: checkout and upstream status, update check, promotion readiness, promote and restart controls
		Governance (#/project)
			Tabs: Governance, Quality, Guidance
			Governance
				Product (markdown field: edit/preview/save)
				Constitution (markdown field)
				Rules: list (text input + Remove per rule), Add rule, Generate rules (needs product + constitution), autosave
			Quality
				Quality is active for every Goal candidate; the candidate round pins whether pre_merge runs before Build or post_build runs after Build
				Business requirements + agent instructions (markdown fields)
				Tests: one project-wide plain-text outcome per line; the agent proposes a command and Refine records the observed supervised execution
				Legacy transition: formerly enabled target-app QA commands remain enforced until replaced by non-empty plain-text tests
			Guidance
				Table: name, status pill (Enabled/Disabled), rule excerpt; click row to edit
				Add guidance
				Guidance modal: name, rule (when to apply), instructions, status toggle, Cancel, Delete (danger, edit mode), Save/Create
		No-project / detached mode: Application tab active (add/switch); other tabs show "No app configured" + Open Guide; config tabs read-only
Nav: Agents
	Click opens Node > Processes
Nav: Command Palette
	Trigger: Ctrl/Cmd+K, or nav button (shows shortcut)
	Modal: input (fuzzy search, "Type a command or parameters..."), results list (up to 12; title, description, group, disabled/parse-error states)
	Navigation: Arrow up/down, Enter execute, Escape close; empty state "No commands found."
	Commands
		Nav: Dashboard, Goals, Changes, Logs, node/settings surfaces & tabs
		Create: New Goal, Import
		AI: Plan (open the managed Plan terminal), Generate target-app config with AI
		Toolbar: Toggle Toolbar, Maximize Toolbar, Files open, Files search
		Goals: clear filters, select page, bulk status/priority/reporter/feature/transfer node/delete, move all by status, move failed back one step
		Changes: clear filters
		Logs: clear filters
		System: Pause/unpause agents, Hard reset worktree (danger), Rebuild projection cache, Clean up old activity logs (days)
		Application: target app start/stop/build/test/sync/check status
		Quality: configure candidate-evaluation context and plain-text tests
		Runtime: re-check auth
		Settings: copy application/runtime settings from node
		Support: Request refine feature/bugfix
Nav: Report Bug
	Click opens Request refine feature/bugfix Modal
Nav: New Goal
	Click opens New Goal Modal
Nav: New Goal Drop-down
	New Feature Modal
	Plan Mode
	Import Goals
	Request refine feature/bugfix (GitHub issue link)
Nav: Toolbar (bottom dock)
	Dock chrome
		Open/Collapse (click active tab toggles), chevron rotates
		Toggle full-screen (fills viewport below topbar, implies open)
		Resize (drag handle, 120px–85vh), persists height
		Persists tabs/active tab/open/height/fullscreen across reload & project switch reset
	Persistent Tabs (order: Supervisor, System, Files, Terminal, Standalone) + dynamic Goal terminal / Goal log / Plan terminal tabs
		Supervisor terminal
			Start/Stop/Restart the configured agent CLI in the target app checkout with workflow-monitoring context
			Managed process metadata includes profile, role, provider, and terminal session id
		System
			Recent system operations log (time, message, color by status)
			Filters: All, Info, Started, Queued, Completed, Errors (persisted)
			Visible/total + 250-item limit; empty / no-match states
		Files
			Path bar: Path input (repo-relative), Go, Copy path, Clear path, Refresh
			Search files (debounced; arrow up/down navigate, Enter open)
			Tree: directories/files, Expand all, Collapse all, Clear tree (max depth 3, 200 entries, limit messages)
			Content panel: status line, Copy contents
				Image preview (lightbox); text preview with line numbers + syntax highlighting; non-previewable message
				Scroll-to-load more (128KB chunks)
		Terminal
			Start/Stop/Restart a standard interactive shell with no agent launched
		Standalone terminal (default, no Goal)
			Start/Stop/Restart the configured agent CLI in a Refine-owned Git worktree
			Stopping preserves the worktree; restart validates and reuses the same branch and path
		Goal terminal (opened via Open Agent on a Goal)
			Tab labeled "Goal {id}…", link to goal, Close tab
			Attaches to the workflow-owned agent already implementing that Goal; never launches a duplicate
			Configured agent CLI receives fresh durable Goal and Round context from workflow launch
			Shows explicit needs-input state while the same process and workflow claim remain active
			Silence remains ordinary autonomous work and never creates needs-input state
			Uses the same PTY input, output, resize, and lifecycle behavior as every terminal profile
		Goal log tail (opened via Watch Logs on a Goal)
			Tab labeled "Logs {id}…"; live indicator, Goal link, Open full logs, Refresh, Close tab
			Loads the newest 200 Goal activity entries, defaults to chronological Tail order, and appends deduplicated Goal logs from SSE
			Compact text stream shows time, severity, category, actor, safely formatted message links/actions, and expandable details
			Search filters the recent trail; Head shows newest first and Tail shows newest last; empty/loading/error states remain visible
		Plan terminal / Plan Mode (toolbar tab, profile "plan")
			Tab labeled "Plan"; Start/Stop/Restart; optional initial prompt
			Configured agent CLI receives planning and Refine CLI persistence guidance at launch
		Shared terminal behavior
			One xterm renderer and PTY API for Terminal, Supervisor, Plan, Goal, and Standalone profiles
			Selecting a stopped Terminal, Supervisor, Plan, or Standalone tab starts it automatically; Goal resolves its active workflow session; selecting an already-running active tab retains the dock toggle behavior
			Supervisor, Plan, and Standalone sessions are role singletons; Goal Agent sessions are keyed by Goal and may run in parallel
			Agent profiles pass the provider's background-agent-equivalent full-access flag while retaining native interactive mode
			Each session registers as `interactive_session` in the daemon process registry and appears in Processes
			Terminal output uses per-session SSE; input and resize use the terminal API
			Terminal typography defaults to 15px; the terminal fills the dock width and a ResizeObserver refits xterm plus the backend PTY when its rendered box changes
			Session/process/provider/cwd/worktree state is tab-specific; reload checks daemon status and reattaches to a live session without treating an SSE interruption as process exit
		Tab management: active-session dot, close (non-standard tabs), reorder
Modals
	Goal Modal (#/goals/:id)
		Header: name, status pill, priority pill, workflow back/forward buttons, Open Agent
		Metadata: goal ID, created, updated, node owner, branch
		Feature association: Feature link + order, or "Standalone"
		Banners: failure (error from logs), governance (warn/error)
		Governance summary: rules/product/constitution/meta pills, message, details, rule actions
		Quality summary: status pill, checked time, message, details
		More Goal actions: Watch Logs, Reporter, Rename, Change Priority, Move to / Assign Feature, Remove from Feature, Cancel, Delete (confirmations)
		Rounds: count, per-round collapsible (round #, latest pill, governance/quality pills, reporter, created, prompt)
		Edit latest round (backlog/todo): prompt, reporter, Save changes (draft + cursor preserved)
		Submit follow-up / recovery round (review/failed): prompt, Submit new round
		Notes (collapsible, count): per note preview/author/time, Edit, Delete; Add note composer, Save note
		Workflow transitions per status (system-owned states have no buttons)
		Close, Escape / click-outside
	Feature Modal (#/features/:id)
		Header: title, status pill, progress (X/Y done), metadata (ID, created, updated, node owner)
		Actions: ← Backlog, Todo →, Cancel Feature, Delete Feature (confirmations)
		Fields: Name, Description (autosaved, name required)
		Ordered Goals: list (drag handle + order #, name link, status, priority, reporter, updated)
			Reorder via drag-and-drop or move up/down; Delete goal; New Goal (feature pre-filled); pagination (25/page)
		Create flow (new): Name (required), Description, Create
		Close, Escape / click-outside
	New Goal Modal (#/goals/new)
		Reporter ("Submitting as …")
		Fields: Prompt, Priority (low/medium/high); auto-named
		Create Goal, Cancel; validation (prompt, reporter)
		Duplicate handling: matched Goal info + prompt; decisions (move original to backlog / create anyway / import original)
		Escape / click-outside, focus first field
	New Feature Modal (#/features/new)
	Plan Mode (#/goals/plan)
		Plan text input; AI extracts Goal drafts; per-Goal edit (prompt/priority/reporter)
		Feature destination: standalone / new (name + description) / existing (dropdown); bulk save; duplicate detection
	Import Modal (#/goals/import)
		Tabs/sources: Import Feature (paste long spec), Import Goals (paste text), Import Goals (.csv) (paste CSV), Upload Goals (.csv) (Choose CSV file)
			Distribute across nodes checkbox; Extract Feature / Extract drafts / Parse CSV / Parse upload queue background preparation, close the modal, then reopen review when drafts are ready
		Review (drafts, 25/page)
			Per draft: checkbox, order, name (+ error), reporter, priority, node, prompt; possible-duplicate info
			Duplicate decision per row: move original to backlog / ignore / import
			Bulk: select page/all/duplicates, dismiss duplicates, import selected, move originals to backlog, update originals (field), needs-resolution filter
			Feature destination: standalone / new / existing; summary
			Save (N) goals [to Feature]; unresolved drafts must be resolved
		Saving: progress, Cancel (rollback), Hide (background); session persisted + recovered
		Result: created/duplicates toast; partial-failure drafts re-shown for retry
	Bulk Action Modals (from Goals bulk actions)
		Set Status / Priority / Reporter: value control, filter + selection context, help text, result toast
		Assign Feature: feature dropdown (name/status/progress), skips already-assigned / other-node goals
		Transfer node: active-node dropdown, skips in-progress/qa/ready-merge/build
		Delete: danger confirmation (cancels subprocesses, removes worktrees/branches, erases goal.json), partial-failure handling
		Background operation support (progress/result)
	Request refine feature/bugfix Modal (Report Bug)
		Title (Short summary), Description (What should change?)
		Cancel, Open GitHub (pre-fills new-issue URL); validation; popup-blocked error
	Shared: Escape closes, Enter submits (non-textarea), click-outside, focus management, toasts, danger confirmations, background-operation polling

Implementation Internals (for e2e testing)
	Purpose: contract details a test needs to drive the UI, wait correctly, and assert outcomes. Frontend is a hash-routed SPA served from one index.html; all data via JSON over /api; live updates via SSE.
	Testing contract (read first; full integration-test plan in docs/spec/rust-integration-spec.md)
		Determinism — tag every flow before testing it
			[crud] deterministic, assert directly: create/edit goals·features·rounds·notes; filters/search/sort/pagination; bulk status/priority/reporter/feature/transfer/delete; manual workflow buttons (backlog↔todo, review→done via Approve, done↔review); reporter/node/cluster mgmt; settings edits; Undo
			[agent] drives a real provider — run the smoke-ai fixture via REFINE_SMOKE_AI_PATH, then wait on the outcome: chat reply (standalone/goal/plan); Draft Goal / Draft Feature / Draft Round / import AI extract; governance + quality evaluation; Generate rules; Generate target-app config; and the Workflow Engine-driven chain todo→in-progress→qa→ready-merge→build→review (incl. auto-promote backlog→todo)
		Preconditions — gated features; build the state first
			Verify / Verify selected: a review goal assigned to the currently selected reporter
			←QA / ←Merge buttons: only on failed goals in quality-retry / merge-retry context
			Bulk transfer/assign: skip in-progress·qa·ready-merge·build and other-node goals
			Generate rules: product + constitution both filled; QA: target-app test command configured
			Node / Governance surfaces: an attached project; Application controls: supervisor/registry enabled
		Oracles — non-obvious success states to assert
			Approve → reviewed candidate integrates and status becomes done; Cancel Feature keeps done goals, cancels non-terminal ones
			Duplicate detection (New Goal / Import) matches on prompt; decisions → action keys move_original_to_backlog / create-anyway / import-original
			Undo → revert commit pushed (if upstream) and goal moved to cancelled
			Reporter throughput: completion_rate % is server-computed (shown beside Done/Reported) — assert the value from /api/dashboard, not a recomputed formula
		Selectors — no data-testid exists anywhere; the #ids below are the contract
			Prefer ARIA role/label/text for controls without an id; address dynamic rows (goals, drafts, rounds) by row text or the goal/feature link href — there is no per-row id or stable index
		Timing — SSE-driven; wait on the resulting DOM change, never a fixed sleep
			[agent] transitions with smoke-ai resolve within a few seconds; cap waits at ~30s and fail loudly rather than poll forever
	Routing (hash-based, location.hash)
		Parser strips ?query before path parsing; views read query off location.hash directly
		#/ → dashboard (#/?node=all = all-node scope)
		#/features, #/features/new, #/features/:id (detail modal over list)
		#/goals, #/goals/new, #/goals/plan, #/goals/import, #/goals/:id (detail modal over list)
		#/changes, #/logs
		#/node[/:tab] (tabs: application, reporters, processes, performance, target-app, runtime); legacy #/system and #/settings → node/processes; #/project/application → node/application
		#/project[/:tab] (tabs: governance, quality, guidance); legacy #/governance → project/governance
		#/chat[?goal=] → legacy redirect: opens toolbar dock, bounces to #/
		Unknown → dashboard
		Detail routes render a modal over the underlay screen; underlayHash preserved so closing restores list + scroll
		Settings/node/project route changes within same surface swap tab content without full re-render
	Live updates (SSE, EventSource "/api/sse")
		Event "activity_added": invalidates screen-data cache; feeds System ops log (recordSystemOperation); refreshes dashboard / logs / changes / agent-status if on that route (silent refresh*, not render* — no "Loading…" blink)
		Event "status_change": invalidates cache; refreshes agent status, target-app toggle, dashboard, goals table (table only, not filters — preserves in-flight search keystroke), logs, current settings surface
		Tests should wait on resulting DOM change, not a fixed delay; SSE de-dupes repeat events (sseEventChanged)
	Data fetching & caching
		api(method, path, body, options) — fetch wrapper; GET responses cached per-path
		Error envelope: { error: { message, code, details } }; non-OK throws with message; code "background_operation_active" surfaces "Active operation: …"
		Screen-data GET cache TTL 5000ms (SCREEN_DATA_CACHE_TTL_MS); pass {cache:false} to bypass; invalidated on every SSE event
		Background prefetch: delay 2000ms after navigation, 50ms between requests, 30000ms per-screen cooldown
	Polling / timers
		Terminal output is event-driven per live session; no toolbar chat polling
		Running-cell elapsed tickers: 1000ms (process/subprocess elapsed)
		Dashboard refresh timeout: 6000ms
		Toast auto-dismiss: 4000ms
		Path-preview refresh: 120ms
	localStorage keys
		refine_chat_tabs (legacy storage-key name retained for compatibility) — toolbar tabs, activeTabId, open, bodyHeight, fullscreen, and per-tab terminal session/process/provider/cwd/worktree state
		refine_system_tab / refine_node_tab / refine_project_tab — last active settings tab per surface
		refine_guide_state / refine_guide_checklist / refine_guide_width — guide panel mode, checklist status, panel width
		refine_last_reporter — global reporter selection
		refine_import_session_v — import wizard session (mode/phase/source/drafts/destination/operationId), for recovery
		refine_checkout / refine_port — runtime/desktop wiring
	Constants & limits (assert truncation/pagination against these)
		Default list limit 50; entries options 50/100/250/500/1000 (goals, changes, logs, features, performance)
		Import draft page size 25 (IMPORT_DRAFT_PAGE_SIZE); Feature-modal goals page size 25
		System operation log limit 250 (SYSTEM_OPERATION_LOG_LIMIT), 5s dedupe window
		Files: tree max depth 3, max 200 entries/dir, search max 20 results, search debounce 250ms, text chunk 128000 bytes (scroll-to-load)
		Terminal scrollback is maintained by each xterm instance while the page is open; the backend event backlog supports live-session reattachment
		List search debounce ~120ms (status_change keeps keystroke alive via table-only refresh)
		Guide panel width clamp 280–560px (GUIDE_MAX_WIDTH 560)
		Toolbar dock height clamp 120px–85vh (default 20vh)
	Goal workflow state machine (GOAL_WORKFLOW; user buttons only where listed)
		backlog → Todo → (forward: todo)
		todo → ← Backlog (back: backlog) — agent then drives todo → in-progress → qa → ready-merge → build → review automatically
			in-progress: Workflow Engine-owned, no user buttons
		qa: Quality-owned, no user buttons
		ready-merge: merger-owned, no user buttons
		build: target-app-build-owned, no user buttons
		review → ← Todo (back: todo) | Approve → (forward: done, POST /api/goals/:id/approve)
		done → ← Review (back: review)
		failed → ← Todo (back: todo); if QA-retry context: ← QA (POST /api/goals/:id/retry-quality); if merge-retry context: ← Merge (POST /api/goals/:id/retry-merge)
		cancelled → ← Todo (back: todo)
		Status enum: backlog, todo, in-progress, qa, ready-merge, build, review, done, failed, cancelled
		Priority enum: low, medium, high
	Interactive terminal sessions
		POST /api/terminal/session with body { profile:"terminal"|"supervisor"|"plan"|"goal"|"standalone", goal_id?, feature_id?, initial_prompt?, worktree?, cols?, rows? }; profile "goal" resolves the running workflow session rather than starting one
		GET /api/terminal/:id/status reports authoritative session liveness and metadata; GET /api/terminal/:id/events streams terminal_output, terminal_error, and terminal_exit events; POST /api/terminal/:id/input sends bytes; POST /api/terminal/:id/resize changes PTY size; POST /api/terminal/:id/stop terminates it
		Start response includes terminal id, managed process id, profile, provider, cwd, and optional worktree; the process can also be stopped through POST /api/processes/:process_id/stop
	Backend chat sessions
		/api/chat remains a backend capability for workflow automation and non-browser adapters; toolbar agent profiles do not use it as their interaction renderer
	Key element IDs / selectors
		Topbar: brand[data-route=dashboard], nav a[data-route=dashboard|features|goals|changes|logs], #nav-context-menu, #global-reporter, #target-app-indicator, #agent-status-indicator, #btn-command-palette, #btn-refine-issue, #btn-new-goal, #nav-create-menu, #btn-new-feature, #btn-plan, #btn-import; #active-node-label
		Layout regions: #main (active screen), #toolbar-dock, #guide-panel, #banners, template#t-banner
		Toolbar dock: #btn-dock-toggle, #btn-dock-fullscreen, .toolbar-dock-resize, .toolbar-tabs, [data-close-tab], #goal-log-tail, #btn-goal-log-refresh
		Terminal profiles: [data-testid=toolbar-terminal-panel], [data-testid=terminal-start], [data-testid=terminal-stop], [data-testid=terminal-status], [data-testid=terminal-profile], [data-testid=terminal-worktree], [data-testid=terminal-output]
		Goal modal: #btn-state-back, #btn-state-forward, #btn-open-agent, #goal-action-menu, #goal-feature-blocking-banner, #btn-watch-logs, #btn-reporter, #btn-rename, #btn-priority, #btn-goal-feature-assign, #btn-goal-feature-remove, #btn-cancel, #btn-delete, #btn-add-note, #goal-notes-status
		Goals list: #goal-select-page, #goal-select-all (+ row checkboxes), #bulk-export-jira, table header sort controls
		Import: #import-tabs, #import-title, #import-feature-text, #import-text, #import-csv-text, #import-csv-file, #import-csv-file-button, #import-csv-file-name, #import-csv-distribute, #import-upload-distribute, #import-drafts, #btn-extract, #btn-persist
		Settings inputs prefixed #s- (e.g. #s-cap, #s-idle, #s-hard, #s-chat-idle, #s-backlog-promote, #s-cli, #s-agent-limit-pause, #s-file-browser-ignore, #s-governance-add-rule, #s-governance-generate, #s-application-copy-node, #s-project-select)
	API surface (grouped; :id = path param)
		Project/app: /api/project/status|attach|path|directories|sync, /api/apps, /api/apps/status, /api/target-app/:id, /api/target-app/generate-instructions
		Nodes/cluster: /api/nodes(/activate|/copy-settings|/transfer-goals), /api/cluster, /api/cluster/nodes, /api/reporters, /api/reporters/:id/merge
		Goals: /api/goals, /api/goals/:id, /api/goals/:id/rounds(/latest), /api/goals/:id/approve|cancel|retry-quality|retry-merge, /api/goals/bulk, /api/goals/bulk/delete, /api/goals/export/jira(/:operation_id/retry)
		Features: /api/features, /api/features/:id(/cancel|/workflow), /api/features/:id/goals/:id(/reorder), /api/features/:id/goals/bulk
		Dashboard/lists: /api/dashboard, /api/changes(/undo), /api/activity(/cleanup|/ui-error), /api/performance(/cleanup), /api/diagnostics
		Governance/quality/guidance: /api/governance(/generate-rules), /api/quality(/checks|/screenshots), /api/guidance
		Import/operations: /api/import/extract|csv/parse|dedup|persist, /api/operations/:id(/cancel)
		Processes/runner: /api/processes(/agents|/background), /api/agents, /api/runner-workers/merger/hard-reset-worktree, /api/runner-workers/target-app-builder/build, /api/cache/rebuild
		Files: /api/files/tree|read|search
		Settings/runtime: /api/settings, /api/settings/recheck-auth
		Streaming: /api/sse (SSE), /api/chat/* (see Chat sessions)
	Testing notes
		Prefer waiting on SSE-driven DOM updates or button busy-state clearing over fixed sleeps
		Destructive actions (Delete, Hard reset, Undo, bulk delete) route through danger modalConfirm — assert the confirm dialog, then the okLabel button
		Long ops (import persist, bulk, Jira export, cache rebuild) return an operation; poll /api/operations/:id; UI shows progress and supports Cancel/Hide; Jira export resumes after reload and retries interrupted work
		Per repo guidance: do not run mutating endpoints against a real refine clone in tests — use a temp/throwaway project
