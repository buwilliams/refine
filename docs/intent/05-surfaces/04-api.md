# API

## Key Ideas

- **Local Daemon Contract**: the API is primarily the contract between surfaces and the local Refine daemon.
- **Application Groups**: routes should map to real Application responsibilities, not arbitrary page needs.
- **Surface Alignment**: browser, CLI, MCP, and agent integrations should share API behavior where appropriate.
- **Not A SaaS Boundary By Default**: the API should not imply that Refine must become a centralized hosted service.
- **Recoverable Mutations**: API writes should flow through shared services with idempotency, logging, and state repair where needed.

## Purpose

The API exists so surfaces can talk to the local daemon consistently. It gives browser JavaScript, CLI daemon routing, MCP tools, and future agent integrations a shared way to access project status, work items, workflow, processes, chat, settings, files, terminal sessions, diagnostics, and more.

The API should be treated as local Application transport. It is important, but it is not the product center.

## Expected Role

The API should expose groups that match Refine's Application design. Current route groups include system, apps, project, sync, target app, work, workflow, activity, import, dashboard, agents, operations, runner workers, processes, events, quality, chat, settings, governance, guidance, reporters, Reporter-scoped todos, nodes, fleet, changes, cache, performance, files, terminal, diagnostics, and upgrade.

The daemon stores the checkout identity resolved at bootstrap and passes that
request-scoped authority through system, install, update, source, process, and
provider handlers. Handlers do not rediscover a checkout from their own CWD or
construct a user-global runtime service. Port status responses include the
canonical checkout-local runtime root so clients can diagnose ownership.

Those groups are useful because they map Surfaces onto shared Application behavior. They should not drift into page-specific endpoints when an Application service would express the operation better.

Dashboard responses include typed node-local state-sync health, freshness timestamps, the configured stale threshold, and whether all-node counts are authoritative. Health carries no recovery-kind classification: the merge-base pipeline records every failure the same way, and a `recovery_kind` written by an older binary is simply an unknown member that reads back ignored. Nodes responses attach that evidence only to the serving daemon's active node and explicitly mark other nodes unknown, without changing fleet bootstrap health. The event stream publishes typed `state_sync_health` state whose semantic fingerprint changes on failure, recovery eligibility, the wall-clock stale boundary, and recovery; clients reconcile Dashboard, Nodes, and Logs from their authoritative read endpoints on those events and after reconnect.

State convergence is one shared daemon capability under `/api/sync`, mirroring the CLI's single `sync` command; the `/project/sync` and `/project/state-recovery/*` routes are deleted, not aliased — a deletion that lands on each node as that node is upgraded, since a node's daemon and CLI ship as one binary and no node ever serves a mixture. `GET /api/sync/preview` returns the read-only divergence summary — classification, both heads and the merge base, per-path sides, a domain-terms summary per contested path, and, when a recorded conflict report matches the previewed heads, the `decision_question` resolution escalated with — the resolving agent's own words when it declared it could not choose, the contested-records question a spent resolution budget produced, or the explicit transfer/sync-authority choice ambiguous ownership requires — and is never an apply token. `POST /api/sync` without a body stays a sub-50ms queueing adapter that returns a durable operation for the ordinary pipeline; its terminal error preserves the stable conflict report id, node-local report location, and recovery guidance, leading with that decision question when resolution escalated. `POST /api/sync` with `{"authority": "live"|"remote", "paths": [...]}` is terminal recovery — sync with a decision attached: contested paths take the chosen side inside one merge commit, named `paths` are exceptions settled on the opposite side, proven one-sided Goal ownership is preserved over that whole-record choice, bounded races are retried inside the daemon rather than by callers, and a recovered run settles authoritative state-sync health and returns the resulting heads, settled paths, preserved Goal owners, retained refs, authority, detail, and refreshed health. Conflict-time `409` errors carry the stable `error.reason` value `state_moved` when a terminal recovery lost its bounded race because the remote moved again while the pass was being verified — the answer is to rerun, and a reviewing client drops the divergence it reviewed for a fresh one; unrelated conflicts remain generic. Browser code never performs the Git or durable-state mutation. The sync attempt itself runs agent resolution first for ordinary conflicts, subject to the node's `state_sync_agent_resolution` opt-out; ambiguous ownership bypasses agent choice. The background sync worker preserves safe automatic recovery after resolution has escalated, is unavailable, or is disabled, records a `sync_auto_recovered` activity entry naming retained refs and preserved Goal owners, fails closed on ambiguous ownership, and honors the node's `state_sync_auto_recovery: off` opt-out.

The API contract version is the one version fact nodes exchange with each
other, because a fleet upgrades node by node in any order and never at once.
Every mutation that can cross builds carries the caller's contract version — the
CLI to its own daemon, and one node's fleet request to another node's — and a
daemon that does not speak it answers `426` naming the version it does speak
instead of acting on a request it does not understand. The browser is not such a
caller: it is served by the daemon it talks to and upgrades with it. `POST /api/fleet/sync` is the fleet
fan-out around this node's own `/api/sync`: it asks every other enabled node's
daemon to synchronize and returns one status per node — `local` for the serving
daemon's own node, which the calling pass already synchronized in-process,
`queued` for a node
that accepted the request and queued its own pass, `pending_upgrade` for a node
whose daemon rejected the contract version, `unsupported_git` for a node
whose Git is too old to run the state merge at all, `unreachable`, `failed`, or
`disabled`. The statuses are that fan-out's own observation and are returned,
never written into the synchronized node registry: a node's recorded health is
its provisioning verdict and no sync answer may create or erase it, so
`GET /api/fleet` keeps reporting what bootstrap found. A node that has not been
upgraded yet is therefore a reported per-node status with both contract
versions attached, never a failed fan-out: the rest of the fleet still syncs
and the route still answers `200`.

State-sync health also carries the latest monotonic attempt id and source, the
latest failed reconciliation identity, and the stable id and location of its
complete conflict report.

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

`GET /upgrade` classifies the bootstrap-owned executable before projecting a
banner contract. A validated published release returns its version and upgrade
fields. A source runtime instead returns structured executable provenance,
owning checkout HEAD, cached local and upstream identities, cache freshness,
and a relationship of current, behind, ahead, diverged, or unknown. HEAD and
upstream relationships fail closed to a stable unknown reason unless the
embedded executable commit, live checkout, cached identities, and fresh cached
ancestry all agree; the handler never fetches or substitutes its CWD.

Plan-to-Goal drafting uses the shared `/import/extract` route with purpose `plan_goal`; it returns exactly one unpersisted Goal draft so browser, CLI, and agent adapters can preserve their own explicit review boundary.

The Goals screen sends its filter-scoped bulk selection to `POST /work/goals/export/jira`, which returns a durable operation immediately. The supervised runner records ownership, progress, logs, cancellation, failures, and the completed Jira CSV result; interrupted exports can be retried through `/work/goals/export/jira/{operation_id}/retry`. The result contains one row per selected Goal plus the filename, content type, selected Goal ids, and aggregate counts. Existing single-Goal CLI and agent adapters reuse the same row renderer and evidence rules rather than formatting separate reports. That renderer budgets every Description to Jira's Unicode-character limit, prioritizes audit identity and commit traceability before normalized round outcomes and narrative, and marks every shortened section or field explicitly. Raw provider payloads are not replayed, and verbose history in one Goal must not fail the rest of a valid bulk selection.

The API should remain local-first. It should be secure by context, constrained by local daemon ownership, and careful about which operations mutate target state.

## Future Direction

Future agent-native surfaces may use the API directly or through a higher-level protocol. The API should be structured enough for automated discovery and stable enough that agents can rely on it.

If Refine later supports distributed or hosted operation, the API may become a stronger remote contract. That should be an intentional scale step, not an accidental consequence of browser implementation.
