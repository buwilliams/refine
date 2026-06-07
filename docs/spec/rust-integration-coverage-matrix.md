# Rust Integration Coverage Matrix

Baseline commit: `1a884e0`.

Inputs:

- `docs/spec/rust-integration-feature-index.md`
- `docs/spec/rust-integration-spec.md`
- Live CLI help from `refine --help` and each top-level command help.
- Baseline harness files: `tests/cli_surface.rs`, `tests/ui/*.spec.ts`, `tests/smoke_ai_contract.rs`, `tests/support/integration.rs`, and `xtask/src/main.rs`.

Legend:

- `CLI`: native Rust CLI integration test through the daemon.
- `UI`: Playwright browser test through the Rust daemon.
- `AI`: Smoke AI-backed provider path, never a real provider.
- `Journey`: combined UI/CLI/AI workflow where the user outcome crosses surfaces.
- `Route/unit`: lower-level Rust route/service coverage exists, but it is not surface integration coverage.
- `Manual`: extended/manual only because the workflow is host-dependent or external-service dependent.
- `Baseline`: covered by `1a884e0`.
- `Missing`: feasible but not covered by `1a884e0`.
- `Blocked`: missing Rust behavior or missing harness capability that must be fixed before surface coverage.

## Current Baseline Evidence

| Surface | Baseline evidence at `1a884e0` | Coverage |
| --- | --- | --- |
| xtask runners | `test-smoke-ai`, `test-cli`, `test-ui`, `test-surface`, `check` in `xtask/src/main.rs` | Baseline harness only |
| Isolated runtime | `tests/support/integration.rs`, `tests/ui/global-setup.ts` use `target/refine-integration`, `REFINE_TEST_PORT`, `REFINE_DAEMON_PORT`, disposable git app | Baseline fixture |
| Smoke AI contract | `tests/smoke_ai_contract.rs`, `tests/fixtures/smoke-ai` | Baseline AI fixture |
| CLI smoke | `tests/cli_surface.rs` covers `system status`, `project status/doctor`, core gap CRUD, simple workflow transition, feature create/list/add/remove/delete, node list/create/activate/archive | Baseline partial CLI |
| UI smoke | `tests/ui/app_shell.spec.ts`, `navigation.spec.ts`, `gaps.spec.ts`, `features.spec.ts`, `settings_provider.spec.ts`, `plan_chat.spec.ts` | Baseline partial UI/AI |
| Route/unit contracts | `src/surfaces/web_server/tests.rs` covers many API routes | Not a replacement for surface integration |

## Post-Baseline Coverage Added In This Work

| Surface | Evidence | Coverage added |
| --- | --- | --- |
| CLI system | `tests/cli_surface.rs` | `system doctor`, `system api-groups` through isolated runtime/app paths. |
| CLI gaps | `tests/cli_surface.rs` | `gap assign-feature`, `gap remove-feature`, and `gap round --edit-latest`. |
| CLI workflow | `tests/cli_surface.rs`; `src/surfaces/cli/dispatch.rs` | `workflow bulk-transition`, `workflow restore`, `workflow schedule`, `workflow pause`, `workflow resume`, `workflow enforce`; fixed daemon bulk update request body. |
| CLI features/import | `tests/cli_surface.rs` | `feature show`, `feature edit`, `feature reorder-gap`, `feature move`, `feature cancel`, `feature import --csv --text`. |
| CLI nodes | `tests/cli_surface.rs`; `src/surfaces/cli/dispatch.rs` | `node show`, `node rename`, `node settings`, `node transfer`; added daemon support for `node settings`. |
| CLI cluster | `tests/cli_surface.rs` | Local registry commands: `cluster list`, `add-node`, `edit-node`, `show`, `disable-node`, `enable-node`, `sync`, `maintenance`, `transfer`, `remove-node`, plus deterministic missing-node `cluster run` error shape. |
| CLI logs | `tests/cli_surface.rs`; `tests/support/integration.rs`; `src/surfaces/web_server/job_routes.rs` | `log list`, `tail`, `show`, `query`, `export`, and daemon-backed `bundle` using public activity setup plus the diagnostics support-bundle route. |
| CLI Smoke AI | `tests/cli_surface.rs` | `agent detect`, `configure`, `auth`, `diagnose`, and `invoke --provider smoke-ai`, asserting deterministic Smoke AI output plus unsupported `resume` error shape for Smoke AI. |
| UI test IDs | `src/surfaces/web/static/**`; `tests/ui/*.spec.ts` | Existing Playwright smoke coverage now uses `data-testid` as the primary locator for topbar nav/create/reporter controls, New Gap/Feature modals, Gap detail actions/notes, runtime provider preflight, chat controls, Import modal controls, and shared import review/destination controls. |
| UI command palette | `tests/ui/command_palette.spec.ts`; `src/surfaces/web/static/js/command-palette.js` | Open by button and Ctrl/Cmd+K, focus input, fuzzy search, 12-result cap, Enter navigation execution, empty state, Escape close, and ArrowDown selection state. |
| CLI project lifecycle | `tests/cli_surface.rs`; `tests/support/integration.rs` | `project register`, `switch`, `detach`, `attach`, `migrate`, `sync`, `clone --make-current`, and `remove` through the daemon using disposable git-backed target apps. |
| UI detached state | `tests/ui/detached_state.spec.ts` | Detaches through public HTTP, verifies Node Application app selection remains available, non-Application tabs show no-project/Open Guide, runtime config is read-only, then reattaches the disposable app. |
| UI toolbar Files/System | `tests/ui/toolbar.spec.ts`; `src/surfaces/web/static/js/features/toolbar.js` | Files tab open/search/path navigation/tree controls/text preview/line numbers/refresh/clear path, dock fullscreen/collapse, and System tab log count/filter/no-match behavior after a public file API error. |
| UI Smoke AI chat/import/governance/target app | `tests/ui/chat.spec.ts`; `tests/ui/plan_chat.spec.ts`; `tests/ui/import.spec.ts`; `tests/ui/governance.spec.ts`; `tests/ui/target_app.spec.ts`; `src/surfaces/web/static/js/features/toolbar.js`; `src/surfaces/web/static/js/features/gaps-detail.js`; `src/surfaces/web/static/js/features/settings_governance.js`; `src/surfaces/web/static/js/features/settings_application.js`; `src/surfaces/web_server/work_routes.rs`; `src/surfaces/web_server/project_routes.rs` | Standalone chat explicit start/send/output/activity collapse/stop/clear; Gap chat Open Chat/send/output/link/stop through Smoke AI; Draft Round opens the extraction modal and submits a new round; Plan Draft Feature saves extracted drafts to a new Feature; AI Import extracts and saves Smoke AI drafts; Governance Generate rules invokes Smoke AI and saves generated rules; Target App Config Generate with AI invokes Smoke AI, applies generated command fields, and persists settings. `/api/import/extract` invokes the configured provider with purpose-specific prompts (`import`, `plan`, `round`), `/api/governance/generate-rules` invokes the configured provider with a static fallback when no provider is available, and `/api/target-app/generate-instructions` invokes the configured provider with local project-file fallback. |

## Cross-Cutting Harness Requirements

| Requirement | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Runtime/cache/process/artifacts isolated under `target/refine-integration/` | Harness | Baseline | Keep as invariant for all new suites. |
| Disposable git-backed target apps | Harness | Baseline | Reuse fixture for all new tests; add helper methods instead of per-test app setup. |
| `REFINE_DAEMON_PORT = REFINE_TEST_PORT` for daemon-backed CLI tests | Harness | Baseline | Keep as invariant. |
| Teardown on success/failure, retain diagnostics | Harness | Baseline | New tests must not bypass fixture teardown. |
| Avoid private `.refine` inspection except teardown diagnostics | Harness | Baseline | New tests should assert CLI JSON, DOM, public HTTP, or Git state when Git is the product outcome. |
| All default AI tests use Smoke AI only | AI/Harness | Baseline for existing AI tests | Missing coverage for most AI paths; first fix any path that cannot route through Smoke AI. |
| UI selectors use `data-testid` as primary locator | UI/Harness | Partial post-baseline | Existing smoke-tested controls have `data-testid` attributes and tests use them. New UI coverage must add test IDs before interacting with new controls. |
| Wait on DOM/API state, not fixed sleeps | UI/Harness | Baseline mostly complies | Preserve. |

## Nav And Primary Content

| Indexed workflow | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Dashboard route and nav link | UI | Covered post-baseline | Existing nav smoke uses `data-testid` for the attached Dashboard route. Detached-state Playwright coverage now detaches the disposable app, navigates back to Dashboard through the nav link, and asserts the no-project empty state plus Open Guide action. |
| Dashboard node scope switcher current/all | UI | Covered post-baseline | Playwright covers `#/` and `#/?node=all`, clicks Current/All scope buttons, waits for `/api/dashboard?node=current|all`, asserts `node_filter`, URL, and pressed state. |
| Global banners and Re-check auth action when runtime unreachable | UI/Journey | Missing | Need deterministic unreachable/auth-failure setup in isolated daemon. |
| Workflow Visualization status cards for all Gap states | UI/CLI | Covered post-baseline | Playwright seeds one current-node Gap per workflow status through public HTTP routes/actions, asserts dashboard count deltas for every status card, asserts AI badges for agent-managed states, and clicks QA through to filtered Gaps. |
| Awaiting your review section expand/collapse persistence | UI | Covered post-baseline | Playwright seeds review Gaps for a unique reporter, selects that reporter, collapses the review panel, reloads, and asserts localStorage-backed closed state before reopening. |
| Awaiting your review row actions Verify and Add round | UI/CLI | Covered post-baseline UI | Dashboard review rows use `data-testid`; Playwright opens Add round, submits public `/api/gaps/:id/rounds`, verifies the new round count, and verifies a row to done. CLI review-row equivalent is not a separate surface. |
| Verify selected bulk review action | UI | Covered post-baseline | Playwright covers row selection, select-all, indeterminate select-all state, selected-count button text, confirmation modal, and bulk verify through public `/api/gaps/:id/verify`. |
| Reporter throughput expand/collapse and server-computed completion rate | UI/Journey | Covered post-baseline | Playwright opens reporter throughput, asserts the unique reporter's completion rate moves from `0.0%` to `100.0%` after verification, and clicks the row through to reporter-filtered Gaps. |
| Features route and URL query filters | UI/CLI | Covered post-baseline | Playwright seeds Features through public HTTP, loads `#/features` with q/status/reporter/node/sort/dir and q/limit/page variants, asserts filter controls, count text, Filtered pill, clear-to-base route, pagination URL state, and opens filtered/paged row modals. CLI list/show/edit coverage exists for the model equivalent. |
| Features filter card expand/collapse and Filtered pill | UI | Covered post-baseline | Playwright toggles the Features filter shell open/closed and asserts the Filtered pill under a URL-backed search filter. |
| Features table columns, sort arrows, row opens modal | UI | Covered post-baseline | Feature rows, sort headers, and pagination controls use `data-testid`; Playwright asserts filtered row content, toggles name sort direction, pages through 52 seeded rows, and opens a row/detail modal from the paged result. |
| Gaps route and URL query filters | UI/CLI | Covered post-baseline | Playwright seeds Gaps through public HTTP, assigns the filtered Gap to a Feature, records Gap-linked activity, loads `#/gaps` with q/status/reporter/node/feature/rounds/severity/category/actor/sort/dir plus q/node/limit/page variants, asserts filter controls, current-page count text, Filtered pill, clears filters back to the base route, pages through seeded rows, and opens filtered/paged row detail. |
| Gaps workflow visualization scoped to filters | UI | Covered post-baseline | Playwright seeds filtered backlog/todo Gaps, verifies status-card counts and the AI badge on agent-managed todo, clicks the todo workflow card, and asserts URL/status filtering plus row narrowing. |
| Gaps filter/bulk card expand/collapse and Filtered pill | UI | Covered post-baseline | Playwright toggles the Gaps filter/bulk shell open and asserts the Filtered pill under URL-backed filters. |
| Gaps table selection, select page, all matching, exclusions, indeterminate | UI | Covered post-baseline | Playwright opens the bulk shell, asserts default all-matching selection, row exclusion indeterminate state, Select page explicit selection, and cross-page unchecked rows with indeterminate master state. |
| Gaps table columns and sorting | UI/CLI | Covered post-baseline | Gap rows and sortable headers use `data-testid`; Playwright asserts filtered row content, toggles priority sort direction, pages through 52 seeded rows, and opens filtered/paged row detail. |
| Gaps pagination | UI | Covered post-baseline | Playwright seeds 52 Gaps, asserts `1-50 gaps`, Previous/Next disabled states, `page=2` URL state, second-page row count, and row detail navigation. |
| Changes route and URL filters | UI/CLI | Covered post-baseline | Playwright seeds 52 git commits in the disposable target app, links them to a high-priority Gap through commit subjects, syncs `/api/project/sync`, loads `#/changes` with q/status/priority/limit filters, asserts filter controls/table rows/status/priority cells, toggles commit sorting through URL-backed headers, paginates to page 2, and clears filters. CLI has no dedicated changes command. |
| Git Visualization by day/week/month/year | UI | Missing | Seed merge/undo history or use deterministic route setup. |
| Branch info and unresolved empty state | UI | Missing | Add attached app with known merge target and detached/no-branch variant. |
| Changes undo confirmation and result | UI/Journey | Covered post-baseline | Rust `/api/changes/undo` now resolves the linked Gap from the Changes projection, reverts the selected commit, cancels the linked Gap after a successful revert, and refreshes the projection cache. Playwright seeds a linked git commit in the disposable target app, opens the Changes undo confirmation, submits it, asserts the undo response, verifies the committed file was reverted, verifies the filtered row disappears, and verifies `/api/gaps/{id}` reports `cancelled`. |
| Logs route and URL filters | UI/CLI | Covered post-baseline | Playwright seeds activity through public `/api/activity/ui-error`, loads URL-backed q/severity/category/actor filters, asserts controls, table rows, details, clear filters, sort header URL state, and pagination across 52 seeded entries. CLI log list/tail/show/query/export/bundle coverage exists in `tests/cli_surface.rs`. |
| Log Visualization by severity and period | UI | Covered post-baseline | Playwright seeds activity through public `/api/activity/ui-error`, filters Logs by q/severity/category/actor, asserts day/week/month visualization buckets and error counts, and clears URL-backed filters. |
| Logs row Show details expand/collapse | UI | Covered post-baseline | Logs pretty-print structured details; Playwright expands a seeded row's Show details control and asserts JSON detail content. |

## Manage Drop-Down, Guide, Settings, Nodes, Project

| Indexed workflow | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Manage drop-down app status/app name/reporter selector | UI | Covered post-baseline | Playwright derives the active app label from `/api/project/status`, asserts the context app name and target-app status state, selects `refine-smoke`, verifies the summary and `localStorage` persistence across reload, then clears the reporter back to `No reporter`. |
| Guide open/close/resizable tabs | UI | Covered post-baseline | Playwright opens Guide from the Manage dropdown, verifies Get Started tab selection, resizes the panel through the drag handle, asserts persisted width, switches to Reference, and closes the panel. |
| Guide Get Started checklist cards/status cycle/default/skip/complete/prev | UI | Covered post-baseline | Playwright resets Guide state, opens Get Started, asserts the first action-only item has no default button and disabled Prev, completes Add app, navigates back with Prev, skips Create node, advances to the next card, and verifies checked/skipped state persists after reload. |
| Guide Reference search/category/field navigation/field links | UI | Covered post-baseline | Playwright searches Reference for AI provider, verifies unrelated items are filtered out, opens the field item, asserts navigation/highlight on Runtime Config, then closes Guide and reopens the same Reference item through the Runtime provider field info icon. |
| Node route tabs | UI | Covered post-baseline | Playwright clicks the Node settings tab strip through Application, Reporters, Processes, Performance, Target App Config, and Refine Runtime Config, asserting URL, active tab, and active pane. Command-palette route coverage also exercises every Node tab. |
| Application target-app select/add/switch/remove/copy/template | UI/CLI | Covered post-baseline for supported app-registry paths | Playwright drives the Application tab known-apps select, Add app modal, switch confirmation, active-app remove confirmation/detach behavior, and empty template lookup against a disposable app path, then test cleanup restores the original app. CLI project register/clone/switch/remove/detach coverage exists in `tests/cli_surface.rs`. Copy-from-node is covered separately through the command palette as the current native no-op contract. |
| Application Generate with AI / target-app config generation | AI/UI/Journey | Covered post-baseline | Smoke AI-backed Playwright coverage drives the Target App Config Generate with AI control, asserts `provider: smoke-ai` and raw Smoke AI output, verifies generated start/stop/rebuild/status/check fields, and confirms settings persistence. `/api/target-app/generate-instructions` routes through the configured provider with local project-file fallback. |
| Application node table activate/rename/archive/create | UI/CLI | CLI create/activate/archive baseline only | Add CLI show/rename/settings/transfer; add UI node table coverage. |
| Cluster list/configure/register/enable/disable | CLI/UI | Covered post-baseline | Native CLI cluster registry commands are covered, and Playwright covers Application-tab cluster list, register, configure, disable, and enable through the public `/api/cluster` routes with API cleanup. |
| Cluster bootstrap over SSH and remote run | Manual | Partial post-baseline | Manual/extended: requires SSH host and remote Refine checkout. CLI missing-node `cluster run` error shape is covered; real SSH run remains manual. |
| Reporters add/rename/merge/remove | UI/CLI | Covered post-baseline UI | Playwright manages reporters from Node settings: Add reporter, Rename, Merge into another reporter, Remove, and verifies `/api/reporters` after each mutation. No CLI reporter command exists today. |
| Processes table parent/child expand and actions | UI/Journey | Partial post-baseline | Playwright now covers the Processes tab managed-process table, background-process row, agent-scheduler row, statuses, and action availability. Supervisor parent/child expansion with real UI/runner children still needs deterministic process fixtures. |
| Process actions: pause/unpause agents, background start/stop, hard reset, cancel agent, stop chat, target app start/stop/rebuild/sync/check | UI/CLI/Journey | Partial post-baseline | Processes-tab Playwright coverage drives Stop/Start Background and Pause/Unpause agents through `/api/processes/background` and `/api/processes/agents`, including destructive confirmations and restored pause state. Command-palette target-app coverage covers start/stop/rebuild/check, and target-app settings coverage covers sync/autosave-adjacent state. Hard reset is covered through the command palette and Rust `/api/runner-workers/merger/hard-reset-worktree`, resetting tracked changes and deleting untracked target-app files while preserving `.refine`. Cancel-agent and stop-chat rows remain missing. |
| Subprocess table rebuild/generate/cleanup | UI/Journey | Missing | Add no-op deterministic subprocess setup or mark host-dependent if requiring real generated app commands. |
| Projection cache rebuild progress | UI/CLI | Covered post-baseline for direct rebuild | Command-palette Playwright coverage runs `rebuild-cache`, confirms the modal, waits for `/api/cache/rebuild`, asserts the response indexes at least the seeded Gap and writes under `target/refine-integration/run`, and verifies the completion toast. The command's background-job progress branch remains dormant because the Rust route currently returns a direct rebuild result. |
| Performance summary/events filters/refresh/prune/clear | UI | Covered post-baseline | Playwright clears and seeds real HTTP request metrics through public APIs, verifies summary and event rows, applies operation/outcome/limit filters plus clear filters, refreshes, prunes, and confirms destructive clear. Server regression coverage refreshes the cached default Performance projection after cleanup. |
| Target App Config fields and autosave | UI/Journey | Covered post-baseline | Playwright fills URL, command, cwd, environment, timeout, log, HTTP, TCP, and process-check fields through `data-testid` selectors, then verifies persisted `/api/settings` values. |
| Runtime Config fields and autosave | UI/CLI | Covered post-baseline | Playwright fills every Runtime Config field and select through `data-testid` selectors, verifies persisted `/api/settings` values, reloads the Runtime tab, and asserts the saved values render again. Provider select/recheck is covered separately with Smoke AI; CLI settings equivalent absent except node settings read. |
| Runtime AI provider selector and preflight | UI/AI/CLI | Covered post-baseline | Playwright selects Smoke AI in Runtime settings and drives Re-check auth to `Auth OK`. Native CLI coverage asserts Smoke AI-safe `agent detect`, `configure`, `auth`, `diagnose`, `invoke`, and unsupported `resume` without calling real providers. |
| Runtime upgrade banner/copy command | UI | Covered post-baseline | Rust route coverage keeps the default local-development/current-version payload. Playwright runs the isolated UI daemon with `REFINE_TEST_UPGRADE_LATEST_VERSION`, verifies `/api/upgrade` reports an upgrade, asserts the reachable settings banner, clicks the copy button, stubs clipboard write, and verifies `./r update` plus copied toast. |
| Governance Product/Constitution/Rules autosave | UI/CLI | Covered post-baseline | Playwright covers Product and Constitution markdown autosave without generation, manual Add rule, manual Remove rule, persisted `/api/governance` state, generated rules through Smoke AI, and Governance/Quality/Guidance tab-strip navigation. No CLI equivalent today. |
| Generate rules | AI/UI | Covered post-baseline | Smoke AI-backed Playwright coverage seeds product/constitution through UI, calls Generate rules, asserts `provider: smoke-ai` and raw Smoke AI output, checks generated rule inputs, and verifies saved public governance state. |
| Quality gate/timing/regressions CRUD/run | UI/Journey | Covered post-baseline | Playwright asserts Quality gate/timing/regression toggles render from saved state, then command-palette Quality coverage creates a managed regression through the browser, runs current-checkout regressions through `/api/quality/regressions/run`, verifies the latest run passed with a screenshot-backed Playwright result, toggles the regression disabled/enabled, and deletes it. Rust fixes make generated specs Playwright-discoverable as `.spec.cjs`, use `domcontentloaded` instead of `networkidle`, and run quality subprocesses under the daemon runtime root. |
| Quality requirements/instructions warning | UI | Covered post-baseline | Playwright forces incomplete quality settings through public `/api/quality`, asserts the warning, fills Business requirements and Instructions through markdown controls, verifies `/api/quality.configured`, reloads, and asserts the warning is gone. |
| Guidance table and modal CRUD/status/delete | UI | Covered post-baseline | Playwright creates Guidance from the Governance > Guidance tab, edits name/rule/instructions, disables it, verifies `/api/guidance`, deletes through the danger confirmation, and restores the original guidance list. |
| No-project/detached mode Application tab/read-only tabs/Open Guide | UI/Journey | Covered post-baseline for Node settings | Application tab, no-project tabs, Open Guide button, read-only runtime config, and reattach cleanup covered. Detached dashboard/project surfaces still useful later. |

## Command Palette And Top-Level Nav Actions

| Indexed workflow | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Command palette open via button and Ctrl/Cmd+K | UI | Covered post-baseline | Button and keyboard trigger covered. |
| Command palette input, fuzzy search, result metadata, disabled/parse-error states | UI | Covered post-baseline | Input, fuzzy search, group/title rendering, empty state, disabled row handling, and parse-error disabled state are covered; parse-error coverage uses a test-only command registered through `window.RefineCommands` to exercise the registry behavior without adding product-only commands. |
| Keyboard navigation Arrow/Enter/Escape and empty state | UI | Covered post-baseline | ArrowDown, ArrowUp, Enter execution, Escape close, and empty state are covered. |
| Palette navigation commands | UI | Covered post-baseline | Command-palette Playwright coverage navigates Dashboard, Features, Gaps, Changes, Logs, every Node tab, and every Governance/Quality/Guidance tab, asserting URL and route heading. |
| Palette create New Gap / Import gaps | UI | Covered post-baseline | Command-palette Playwright coverage selects a reporter, opens New Gap from the palette, dismisses it, opens Import gaps from the palette, and verifies the AI import tab. Deeper create/import flows are covered in modal-specific tests. |
| Palette AI Plan / Draft Gaps / Generate target-app config | UI/AI | Covered post-baseline | Command-palette Playwright coverage opens Plan with a prompt, waits for a Smoke AI plan turn, runs Draft Gaps from the palette through provider-backed extraction, then runs Generate target-app config from the palette and verifies Smoke AI-generated command fields. |
| Palette toolbar commands | UI | Covered post-baseline | Command-palette Playwright coverage runs toolbar toggle, fullscreen toggle, `files README.md`, and `search-files app.py`, asserting dock state, file preview, and search results. |
| Palette Gaps filter/bulk/status commands | UI | Covered post-baseline | Command-palette Playwright coverage clears Gaps filters, selects the current filtered page, opens Status/Priority/Reporter/Feature/Node/Delete bulk modals from palette commands, and cancels each modal after asserting the seeded Feature/node choices. |
| Palette Changes/Logs clear filters | UI | Covered post-baseline | Command-palette Playwright coverage starts from filtered Changes and Logs URLs, runs `clear-changes` and `clear-logs`, and verifies each route resets to its unfiltered hash. |
| Palette System pause/hard reset/cache rebuild/cleanup | UI/Journey | Covered post-baseline | Cache rebuild is covered through command palette confirmation, `/api/cache/rebuild`, integration runtime cache-path assertion, and completion toast. Activity-log cleanup is covered through public activity seeding, `cleanup-logs 0`, destructive confirmation, `/api/activity/cleanup`, and completion toast. Pause/unpause is covered by command-palette `pause-agents` and `unpause-agents` with `/api/processes/agents` response assertions; the command now reads live `runtime.agents_paused` from `/api/settings`. Hard reset coverage dirties tracked and untracked disposable-app files, confirms the command, waits for `/api/runner-workers/merger/hard-reset-worktree`, and asserts tracked reset plus guarded untracked cleanup. |
| Palette target app actions | UI/Journey | Covered post-baseline | Command-palette Playwright coverage configures disposable `printf` target-app commands, runs `check-app`, `app-start`, `app-rebuild`, and `app-stop`, confirms the destructive/action modals, waits on `/api/target-app/health`, `/api/target-app/start`, `/api/target-app/rebuild`, and `/api/target-app/stop`, and asserts status messages, state transitions, and operation stdout. |
| Palette Quality actions | UI/Journey | Covered post-baseline | Command-palette Playwright coverage runs `new-regression` with a seeded scenario, verifies the Quality regression modal and created row, then runs `run-regressions` and asserts the `/api/quality/regressions/run` result and latest-run UI before toggling and deleting the regression. |
| Palette Runtime re-check auth | UI/AI | Covered post-baseline | Command-palette Playwright coverage runs `recheck-auth`, waits on `/api/settings/recheck-auth`, and asserts the successful `Auth OK` UI result. Direct settings button coverage remains in Runtime settings with Smoke AI selected. |
| Palette Settings copy from node | UI | Covered post-baseline for current native no-op contract | Command-palette Playwright coverage seeds a secondary node, runs application and runtime copy-from-node commands, selects the source node in the modal, asserts `/api/nodes/copy-settings` receives the source/section, and verifies the `Copied 0 settings.` toast. The Rust backend currently has no per-node settings persistence to copy real values. |
| Palette Support request Refine feature/bugfix | UI | Covered post-baseline | Command-palette Playwright coverage opens the request modal and asserts popup-blocked feedback through a deterministic `window.open` stub. |
| Nav Report Bug | UI | Covered post-baseline | Topbar Playwright coverage opens the request modal, verifies empty-submit validation, stubs `window.open`, and asserts the generated GitHub issue URL contains the encoded title/body. |
| Nav New Gap | UI | Covered post-baseline | Direct New Gap flow covers `data-testid` navigation, focus, validation, priority, auto-name, Escape and backdrop dismissal, and duplicate decision handling. |
| New Gap drop-down New Feature / Plan Mode / Import / Request issue | UI/AI | Covered post-baseline | New Feature, Plan route, Import gaps modal, and Request refine feature/bugfix are covered through `data-testid` navigation. Playwright also verifies dropdown visible actions, Escape close, menu close after selection, and the request modal from the dropdown entry. |

## Toolbar

| Indexed workflow | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Dock open/collapse/fullscreen/resize/persistence/project reset | UI | Partial post-baseline | Open, collapse, fullscreen, drag resize, and localStorage-backed resize persistence are covered. Project-change/reset behavior remains missing. |
| Persistent tabs System/Files/Standalone and dynamic Gap/Plan tabs | UI | Partial post-baseline | System, Files, Standalone, Plan, and dynamic Gap tabs are covered. Gap tab active-session dot and close behavior are covered; tab reorder remains missing because no reorder interaction is exposed today. |
| System operations log filters/count/limit/empty/no-match | UI/Journey | Partial post-baseline | Public file API error records browser-visible system operations; All/Error/Queued filters, count, and no-match state are covered. Limit/persistence remain missing. |
| Files path bar Go/Copy/Clear/Refresh | UI | Covered post-baseline | Playwright covers Go, Copy via stubbed browser clipboard plus toast, Clear, and Refresh against the disposable app. |
| Files search keyboard navigation/open | UI | Covered post-baseline | Playwright searches for `app.py` and opens it with Enter, then seeds two unique disposable app files, searches by prefix, asserts the selected result moves with ArrowDown and ArrowUp, opens the selected result with Enter, and verifies the preview content. |
| Files tree expand/collapse/clear/depth/limit messages | UI | Covered post-baseline | Playwright covers expand all, collapse all, clear tree controls, recursive depth-limit messaging, and 200-entry limit messaging against seeded disposable target-app directories. |
| Files content image/text/non-preview/line numbers/chunk loading/copy | UI | Covered post-baseline | Playwright covers text preview, line numbers, content copy through browser clipboard, large-file scroll chunk loading, image preview through `/api/files/read` data URLs, and binary non-preview messaging. Rust route coverage asserts image and binary file responses. |
| Standalone chat start/stop/status/output/activity/input/send | AI/UI | Covered post-baseline | Smoke AI-backed Playwright coverage starts, sends, observes output/activity, stops, and clears standalone chat. |
| Standalone queued messages edit/save/remove/clear history | UI/AI | Covered post-baseline | Playwright uses a prompt-gated delayed Smoke AI turn to create a real server-side queued standalone message, edits it through PATCH, removes it through DELETE, and clears the chat history through the confirmation flow. |
| Gap chat open/link/close/status/input/output/activity | AI/UI | Partial post-baseline | Smoke AI-backed Playwright coverage opens Gap chat, asserts link/status, sends input, observes output, drafts a round, and stops. Activity collapse for Gap-specific tab remains useful later. |
| Gap chat Draft Round extract modal/add round | AI/UI | Covered post-baseline | Smoke AI-backed Playwright coverage drives a Gap chat response containing a round-shaped line, opens Draft Round, verifies provider-extracted actual/target fields, submits the modal, and observes `round_count` increment through `/api/gaps/:id`. `/api/import/extract` uses `purpose: "round"` to route extraction through the configured provider before parsing the response. |
| Plan chat start/stop/output | AI/UI | Covered post-baseline | Smoke AI-backed Playwright coverage opens Plan Mode, asserts active status and Stop plan, stops the auto-started session, verifies No active session and Start plan, starts a new Plan session explicitly, sends input, observes Smoke AI output, and drafts a Feature. |
| Plan Draft Feature modal/minimize toolbar | AI/UI | Covered post-baseline | Smoke AI-backed Playwright coverage opens Plan Mode, sends a plan-shaped prompt, opens Draft Feature, verifies provider-extracted draft rows and new-Feature destination controls, saves, and verifies the created Feature through public APIs. Plan Draft filters quoted user prompt lines before extraction and calls `/api/import/extract` with `purpose: "plan"`. |
| Active-session dot/activity pulse/close/reorder | UI | Partial post-baseline | Gap chat Playwright coverage asserts the active-session dot, activity panel behavior, and dynamic tab close. Tab reorder remains missing because no reorder interaction is exposed today. |

## Modals And Dialog Workflows

| Indexed workflow | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Gap modal header/status/priority/workflow buttons/Open Chat | UI/CLI/AI | Partial UI forward and CLI transition | Add all user-visible states, Open Chat. |
| Gap metadata/feature association/banners/governance/quality summaries | UI/AI | Missing | Seed feature/governance/quality outcomes. |
| Gap More actions View Logs/Reporter/Rename/Priority/Assign/Remove Feature/Cancel/Delete | UI/CLI | Partial UI delete and CLI edit/delete/cancel | Add UI all actions; CLI assign/remove feature already untested. |
| Gap rounds collapse/edit latest/follow-up/recovery | UI/CLI | CLI round baseline only | Add UI edit latest and follow-up/recovery; add CLI edit-latest assertion. |
| Gap notes add/edit/delete | UI/CLI | UI add and CLI note add baseline | Add edit/delete if supported in UI; CLI has no edit/delete note command. |
| Gap workflow transitions per status including retry-quality/retry-merge/verify/done/back | UI/CLI/Journey | Partial post-baseline | CLI covers start, cancel, retry quality, retry merge, verify, merge, undo from done/review/cancelled, plus workflow transition/bulk-transition. UI per-status Gap modal buttons remain incomplete. |
| Feature modal header/actions/autosave/name validation | UI/CLI | Partial create/open | Add UI actions, CLI show/edit/move/cancel/delete assertions. |
| Feature ordered gaps reorder/delete/new gap/pagination | UI/CLI | Partial new gap | Add drag/move up/down and CLI reorder-gap. |
| New Gap modal validation/priority/auto-name/focus/Escape/click-outside | UI/CLI | Covered post-baseline | Playwright selects the reporter, verifies initial focus, asserts empty-submit validation, creates a high-priority auto-named Gap, closes the deep-link modal with Escape, and closes a nav-opened modal by clicking the backdrop. |
| New Gap duplicate handling decisions | UI/Journey | Covered post-baseline | Rust `/api/gaps` detects latest-round actual/target duplicates for generated-ID New Gap creates. Playwright seeds an original Gap, verifies the duplicate prompt, chooses ignore and confirms no new Gap is created, then chooses import/create-anyway and verifies the second Gap through public Gaps API state. |
| New Feature modal create validation | UI/CLI | Covered post-baseline | Playwright opens the New Feature modal from the create menu, verifies name-field focus and required-name validation, creates a Feature with description/reporter, asserts the public create response and detail URL, then cleans up through the public Feature API. CLI Feature create/list/show coverage covers the model equivalent. |
| Plan Mode draft extraction/edit/destination/bulk save/duplicate detection | AI/UI | Partial post-baseline | Smoke AI-backed Plan Draft Feature covers provider-backed extraction, review rows, row editing, duplicate detection and duplicate dismissal, new-Feature and existing-Feature destinations, save, and verification of the appended edited Gap. Bulk-save variants and import-as-original/update-original duplicate branches remain useful later. |
| Import AI text extraction | AI/UI/CLI | Partial post-baseline | Smoke AI-backed UI Import opens from the create menu, calls provider-backed `/api/import/extract`, verifies `provider: smoke-ai`, reviews extracted drafts, saves them, and verifies created Gaps through public APIs. CLI `feature import --csv --text` parser/persist path is covered; there is no separate CLI AI extraction command today. |
| Import CSV paste/upload parse/review/persist | UI/CLI | Covered post-baseline | CLI import CSV/text is covered. UI CSV paste and CSV upload both parse, dedup, review, persist through background jobs, verify created Gaps, and clean them up. |
| Import draft review selection/pagination/duplicate decisions/bulk actions | UI/Journey | Covered post-baseline | Playwright seeds one duplicate, parses 30 CSV drafts, verifies two-page review pagination, page/all/duplicate selection, bulk update-original decision state, duplicate dismissal, adjusted save count, persist, and cleanup. |
| Import saving progress/cancel/hide/recovery/result/retry | UI/Journey | Partial post-baseline | `/api/import/persist` now honors `background: true` with persisted job progress/result metadata. Playwright starts a 60-draft CSV save, hides the saving modal, verifies saved session recovery after reopening Import, waits for job result count, verifies modal/session cleanup, and deletes created Gaps. Cancel, failure retry, and failed-draft recovery remain missing. |
| Bulk Set Status/Priority/Reporter modals | UI/CLI | Covered post-baseline | Playwright seeds a filtered set of Gaps through public HTTP, selects the filtered page in the browser, drives the Status, Priority, and Reporter bulk modals, and verifies each updated Gap through `/api/gaps/{id}`. CLI bulk status remains covered through workflow operations; no separate CLI priority/reporter command exists. |
| Bulk Assign Feature / Transfer node / Delete modals and background jobs | UI/CLI/Journey | Covered post-baseline | Playwright seeds separate filtered Gap sets, assigns one set to a Feature through the bulk Feature modal, transfers another set to a created node through the bulk Node modal, deletes a third set through destructive confirmation, and verifies each result through public Gap/Feature/Node APIs. CLI node transfer is covered in `tests/cli_surface.rs`. |
| Request Refine feature/bugfix modal validation/GitHub URL/popup error | UI | Covered post-baseline | Covered through nav and command-palette entrypoints: validation for empty submit, generated GitHub URL, modal close after successful open, and popup-blocked error feedback. |
| Shared Escape/Enter/click-outside/focus/toasts/danger confirmations/jobs | UI | Partial delete confirm | Add common modal contract coverage across representative dialogs. |

## AI And Agent Workflows

| Indexed workflow | Owner | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| Provider preflight Smoke AI | AI/UI/CLI | Covered post-baseline for default provider path | UI Re-check Auth, Smoke AI contract, and CLI detect/configure/auth/diagnose/invoke are covered with Smoke AI. |
| Standalone chat | AI/UI | Covered post-baseline | Smoke AI-backed UI test covers start/send/output/activity/stop/clear. |
| Gap chat | AI/UI | Covered post-baseline | Smoke AI-backed UI test covers Open Chat/send/output/link/stop plus Draft Round modal/add-round using the public Gap projection. Gap-specific activity collapse remains useful later. |
| Plan chat | AI/UI | Covered post-baseline | Smoke AI-backed UI test covers Plan open, active/no-session status, explicit stop/start, send/output, and Draft Feature save. |
| Draft Round extraction | AI/UI | Covered post-baseline | Covered as a Smoke AI chat turn followed by provider-backed modal extraction and public add-round submission. |
| Draft Feature extraction | AI/UI | Covered post-baseline | Smoke AI-backed Plan Draft Feature modal/save flow is covered through provider-backed `/api/import/extract` after the Smoke AI Plan response. |
| Import extraction | AI/UI/CLI | Partial post-baseline | Smoke AI-backed UI Import extraction and save are covered; CLI parser-backed `feature import --text/--csv` is covered, but there is no separate CLI AI extraction command. |
| Governance/rules generation | AI/UI | Covered post-baseline | `/api/governance/generate-rules` routes through the configured provider; Playwright verifies Smoke AI generation and persisted generated rules. Static fallback remains only for no-provider environments. |
| Quality/regression AI | UI/Journey | Covered post-baseline | Managed regression coverage drives the Quality UI through command-palette actions and Rust `/api/quality/regressions/run`, executing a generated Playwright regression against a disposable target-app file URL. This path is browser-regression based, not an external provider call; no real provider is invoked. |
| Target-app config generation | AI/UI/Journey | Covered post-baseline | Covered by Smoke AI-backed Target App Config Generate with AI test and provider-backed `/api/target-app/generate-instructions` route. |
| Dispatcher/agent status chain todo -> review | AI/Journey | Missing | Add Smoke AI-backed daemon journey with disposable app and deterministic command outcomes. |
| Auto-promote backlog -> todo | AI/Journey | Missing | Add scheduler/daemon journey; CLI `workflow schedule` if exposed. |
| Real Claude/Codex/Gemini/Copilot/OpenAI/Anthropic provider calls | Manual | Not in default suite | Manual only; default tests must assert Smoke AI path when exercising AI behavior. |

## CLI Parity Matrix

The CLI audit below is from live help at `1a884e0`. Every command with a real model operation needs native Rust CLI integration coverage unless it is host-dependent or a browser-only interaction.

| CLI operation | Feature-index equivalent | Baseline status | Missing work / blocker |
| --- | --- | --- | --- |
| `system status` | Runtime health/status | Baseline | Expand failure/unreachable assertions. |
| `system start/stop/restart` | Runtime lifecycle | Harness uses start/stop | Add explicit CLI lifecycle test if safe with isolated port. |
| `system doctor` | Diagnostics/support | Covered post-baseline | Isolated CLI test covers runtime/app paths. |
| `system api-groups` | API contract | Covered post-baseline | CLI assertion checks the `/work` group. |
| `system install/repair/update/rollback/uninstall` | Service/install workflows | Manual | Host/service/package dependent; add no-op/temp metadata tests only if implementation is file-local. |
| `project status` | Attached/detached project state | Covered post-baseline | Attached baseline plus detached lifecycle assertions covered. |
| `project attach/switch/register/clone/remove/detach/migrate/sync/doctor` | Application management | Covered post-baseline | Registry lifecycle covered with disposable git apps; doctor covered. |
| `gap create/list/show/edit/delete` | Gap CRUD/list/detail | Baseline | Expand filters/priority/name/explicit id. |
| `gap note/round` | Notes/rounds | Partial post-baseline | Add note and edit-latest round covered; note edit/delete absent. |
| `gap start/cancel/retry/verify/merge/undo` | Workflow and changes undo | Covered post-baseline | Native CLI suite covers start, cancel, retry quality, retry merge, verify, merge, undo from done/review/cancelled, and cleanup through allowed workflow transitions. |
| `gap assign-feature/remove-feature` | Feature association | Covered post-baseline | CLI tests cover assign and remove through daemon. |
| `workflow allowed/transition` | Workflow state machine | Covered post-baseline | Native CLI suite asserts the full 10x10 allowed/no-op/blocked matrix, allowed user transitions, a no-op transition, and a denied transition that preserves status. |
| `workflow bulk-transition` | Bulk status | Covered post-baseline | Selected-id status update covered; filter-scoped update still missing. |
| `workflow schedule/pause/resume/restore/enforce` | Scheduler/background/agents | Covered post-baseline | Schedule, pause, resume, restore, and enforce are covered through daemon-backed CLI. |
| `feature create/list/add-gap/remove-gap/delete` | Feature CRUD/membership | Covered post-baseline for model ops | Existing plus extended CLI tests cover CRUD and membership. |
| `feature show/edit/reorder-gap/move/cancel/import` | Feature modal/import workflows | Covered post-baseline | CSV text import covered; non-CSV text/file variants still useful later. |
| `node list/create/activate/archive` | Node management | Covered post-baseline | Native CLI suite covers list, create, activate, archive, and cleanup through daemon-backed node commands. |
| `node show/rename/settings/transfer` | Node settings/transfer | Covered post-baseline | Basic transfer ownership covered; skip/error cases still missing. |
| `cluster list/show/add-node/edit-node/enable-node/disable-node/remove-node/sync/transfer/maintenance` | Cluster management | Covered post-baseline | Local registry commands and transfer are covered through daemon-backed CLI. |
| `cluster run` | Remote command | Manual | Missing-node error shape covered; real remote execution remains manual because it requires SSH host credentials and a remote Refine checkout. |
| `log list/tail/show/query/export` | Logs table/filter/export | Covered post-baseline | Uses public activity event setup. |
| `log bundle` | Support bundle | Covered post-baseline | Daemon dispatch uses `/diagnostics/support-bundle` and asserts returned redacted bundle metadata. |
| `agent detect/configure/auth/diagnose/invoke/resume` | AI provider operations | Covered post-baseline | Smoke AI-safe detect/configure/auth/diagnose/invoke covered; Smoke AI unsupported resume error covered. Real provider resume is tracked as manual/provider-specific because default tests cannot call Claude/Codex/Copilot. |

## Evidence-Backed Manual / Extended Items

| Item | Reason |
| --- | --- |
| Cluster SSH bootstrap and remote run | Requires reachable SSH host, credentials, remote Refine checkout, and remote target app. Default local suite should not depend on that host state. |
| System service install/update/rollback/uninstall | Mutates host service/package state and may require privileges or platform packaging. Default local suite should not install services. |
| Real external provider authentication and calls | Requires host credentials and would violate the Smoke AI-only default AI requirement. |
| Cross-browser Playwright, code signing, notarization, desktop packaging | Environment/package dependent and explicitly outside the current local default harness. |

## Immediate Expansion Order

1. Migrate existing UI tests and high-traffic controls to `data-testid` selectors.
2. Add native CLI tests for existing daemon-backed model operations: feature show/edit/reorder/move/cancel/import, gap assign/remove feature, workflow bulk/pause/resume/schedule/enforce, node show/rename/settings/transfer, cluster local registry, logs, and Smoke AI-safe agent invoke.
3. Add UI coverage for command palette, toolbar shell/files/system, filters/sort/pagination, settings tabs, Guide, and destructive confirmations.
4. Add Smoke AI-backed UI journeys for quality/regression and dispatcher chain.
5. Revisit blockers after root-cause fixes and move each item from `Missing` or `Blocked` to covered evidence.
