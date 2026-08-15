# API

## Key Ideas

- **Local Daemon Contract**: the API is primarily the contract between surfaces and the local Refine daemon.
- **Capability Groups**: routes should map to real system capabilities, not arbitrary page needs.
- **Surface Alignment**: browser, CLI, MCP, and agent integrations should share API behavior where appropriate.
- **Not A SaaS Boundary By Default**: the API should not imply that Refine must become a centralized hosted service.
- **Recoverable Mutations**: API writes should flow through shared services with idempotency, logging, and state repair where needed.

## Purpose

The API exists so surfaces can talk to the local daemon consistently. It gives browser JavaScript, CLI daemon routing, MCP tools, and future agent integrations a shared way to access project status, work items, workflow, processes, chat, settings, files, terminal sessions, diagnostics, and more.

The API should be treated as local capability plumbing. It is important, but it is not the product center.

## Expected Role

The API should expose system capability groups that match Refine's product design. Current route groups include system, apps, project, target app, work, workflow, activity, import, dashboard, agents, operations, runner workers, processes, events, quality, chat, settings, governance, guidance, reporters, Reporter-scoped todos, nodes, fleet, changes, cache, performance, files, terminal, diagnostics, and upgrade.

The daemon stores the checkout identity resolved at bootstrap and passes that
request-scoped authority through system, install, update, source, process, and
provider handlers. Handlers do not rediscover a checkout from their own CWD or
construct a user-global runtime service. Port status responses include the
canonical checkout-local runtime root so clients can diagnose ownership.

Those groups are useful because they map surfaces onto shared behavior. They should not drift into page-specific endpoints when a shared service would express the capability better.

Dashboard responses include typed node-local state-sync health, freshness timestamps, the configured stale threshold, an optional exceptional `recovery_kind`, and whether all-node counts are authoritative. `missing_baseline` is emitted only for the daemon's missing three-way baseline failure; other failed, stale, detached, bootstrap, and ineligible states do not acquire that eligibility by parsing error text. Nodes responses attach that evidence only to the serving daemon's active node and explicitly mark other nodes unknown, without changing fleet bootstrap health. The event stream publishes typed `state_sync_health` state whose semantic fingerprint changes on failure, recovery eligibility, the wall-clock stale boundary, and recovery; clients reconcile Dashboard, Nodes, and Logs from their authoritative read endpoints on those events and after reconnect.

State recovery remains a shared daemon capability: `GET /api/project/state-recovery/preview` returns the complete bounded, evidence-identified comparison and `POST /api/project/state-recovery/apply` accepts only an explicit authority plus that unchanged preview. Apply-time `409` errors carry stable `error.reason` values `git_busy` or `stale_preview` when those recovery actions apply; unrelated conflicts remain generic. A successful apply settles authoritative state-sync health and returns the resulting heads, recovery audit ref, manifest, counts, authority, detail, evidence-correlated result, and refreshed health. Browser code never performs the Git or durable-state mutation.

State-sync health also carries the latest monotonic attempt id and source, the
latest failed reconciliation identity, and the stable id and location of its
complete conflict report. `POST /project/sync` stays a sub-50ms queueing
adapter and returns a durable operation; its terminal error preserves those
fields and recovery guidance. State-recovery preview and apply use the shared
service: valid-baseline previews bind the exact local report and snapshots,
and apply accepts a default authority plus validated path overrides.

Settings, Quality, Governance, and Guidance routes are the shared configuration contract for browser and CLI. Ordinary Settings and Quality writes remain validated partial patches. Governance scalar patches preserve rules, while every rule replacement carries the observed `rules_revision`; Guidance entries have stable ids, item routes mutate one entry under the repository coordination lock, and both item and compatibility whole-list writes carry the observed `revision`. Stale revisions return `409` without overwriting unrelated state, missing ids return `404`, and successful writes return the normalized authoritative collection.

Browser mutations must present an `Origin` or `Referer` whose authority matches
the request `Host`. CLI and other non-browser clients may omit those headers.
The local daemon does not add a separate authorization-token boundary, so
binding it beyond loopback intentionally grants control to clients that can
reach the port.

Daemon lifecycle routes use the shared host authority. Start may return its
observed result directly. Stop and restart first persist and return a
port-scoped operation receipt, then a restart-safe helper performs service
control outside the daemon's systemd control group or launchd job. A caller can
read the durable lifecycle operation after shutdown or reconnection and obtain
the same observed status, lifecycle evidence, failure, and recovery guidance as
a synchronous CLI caller.

Source-update reads are cached and responsive. Automatic stale refresh and
manual refresh both return one durable, coalesced supervised-check receipt;
they do not fetch in an HTTP handler. Source promotion returns one installed
Agent operation outside Goal capacity. The operation projection includes the
same redacted attempt identifier and receipt evidence used by SSE and browser
reconnect, never the raw claim nonce. `POST /operations/{id}/cancel` dispatches
by owner: ordinary work uses supervised cancellation, while
`maintenance:source-upgrade` fences and reconciles its exact external helper,
rollback, executable identity, and prior workflow-admission intent before any
terminal cancellation result is exposed.

Plan-to-Goal drafting uses the shared `/import/extract` route with purpose `plan_goal`; it returns exactly one unpersisted Goal draft so browser, CLI, and agent adapters can preserve their own explicit review boundary.

The Goals screen sends its filter-scoped bulk selection to `POST /work/goals/export/jira`, which returns a durable operation immediately. The supervised runner records ownership, progress, logs, cancellation, failures, and the completed Jira CSV result; interrupted exports can be retried through `/work/goals/export/jira/{operation_id}/retry`. The result contains one row per selected Goal plus the filename, content type, selected Goal ids, and aggregate counts. Existing single-Goal CLI and agent adapters reuse the same row renderer and evidence rules rather than formatting separate reports. That renderer budgets every Description to Jira's Unicode-character limit, prioritizes audit identity and commit traceability before normalized round outcomes and narrative, and marks every shortened section or field explicitly. Raw provider payloads are not replayed, and verbose history in one Goal must not fail the rest of a valid bulk selection.

The API should remain local-first. It should be secure by context, constrained by local daemon ownership, and careful about which operations mutate target state.

## Future Direction

Future agent-native surfaces may use the API directly or through a higher-level protocol. The API should be structured enough for automated discovery and stable enough that agents can rely on it.

If Refine later supports distributed or hosted operation, the API may become a stronger remote contract. That should be an intentional scale step, not an accidental consequence of browser implementation.
