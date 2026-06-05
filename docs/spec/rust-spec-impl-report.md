Refine Rust Port — Spec Conformance Report

Scope reviewed: docs/spec/rust-spec.md against all of rust/src/, rust/xtask/, rust/desktop/.
Method: full read of every module; I personally re-verified the projection-cache and CLI-authority findings below against
source. No changes made. I did not run cargo build/cargo test (143 #[test] functions exist; I did not execute them).

Verdict

The port is far more complete than a scaffold — roughly 30k lines of real, working Rust. The model layer is essentially a
faithful translation, the daemon web server is a genuine hand-rolled HTTP/1.1 server, the CLI command tree matches the
spec noun-by-noun, and host integrations (git, process spawn, provider invocation, ssh) actually shell out and do work.

But there are two structural inconsistencies that contradict the spec's central thesis, plus a tier of OS-integration and
cache work that is modeled-but-not-wired. The spec's whole reason for existing is "one daemon as the sole local
authority, with a projection cache as the only query path." Both of those load-bearing claims are violated in the code as
written.

---
The two findings that matter

1. The CLI bypasses the daemon and mutates durable state in-process — contradicts the core architecture

The spec is unambiguous (lines 175-179, 1008-1011): the daemon is "the single local authority," "Surfaces do not directly
mutate durable state," and "The CLI should normally talk to the same daemon web server using structured HTTP/JSON
contracts… [bootstrap] when the daemon is not available."

In practice, surfaces/cli/dispatch.rs constructs core services directly and mutates durable files in the CLI's own
process:

- dispatch.rs:64 — FileWorkItemService::new(durable_root).transition_gap_status(...)
- dispatch.rs:95,140,305 — bulk update / transfer gaps, in-process
- dispatch.rs:245-333 — node and cluster registry mutations, in-process

The CLI can spawn the daemon (start_background_daemon, dispatch.rs:1380, a real current_exe spawn) and has an
InProcessWebServer/LocalHttpDaemon import — but only system-group commands use it. Every product command (gap, feature,
workflow, node, cluster) writes durable state directly. This means two writers (CLI process + daemon) can mutate the same
.refine files concurrently with no shared authority, no auth, no idempotency, and no SSE notification — exactly the
condition the daemon was introduced to eliminate. This is the single largest deviation in the codebase.

2. The projection cache is built and persisted but never read in production — the spec's required cache abstraction is
dead code

The spec devotes its entire Storage Model section to this and lists it as an acceptance criterion: load the snapshot from
<port>/cache/, validate fingerprints, rescan only changed records, and make "the projection API the only supported query
abstraction" (lines 1073-1098, 1339-1341).

The mechanism exists and is correct in isolation — load_or_refresh_projection (project_state/store.rs:89) does
fingerprint-validated incremental refresh with full-scan fallback. But the only callers of it are tests. Every production
path calls the full-scan rebuild_projection() instead:

- work_items/service.rs:109, 117, 171, 234, 617, 637, 667
- scheduling/mod.rs:347, 415
- chat/mod.rs:564

So every list/count/mutation re-scans all gap.json/feature.json from disk — O(n) per operation — and the persisted
<port>/cache/ snapshot is never loaded except in load_or_refresh_projection, which nothing in production calls. The cache
is implemented, tested, and bypassed. (This is downstream of finding #1: because the CLI and daemon both call core
directly with no shared in-memory daemon state, there's no long-lived process to hold an in-memory index, so everyone
full-scans.)

---
Layer-by-layer conformance

Layer: model::{project,gap,feature,workflow,cluster,node}
Status: ✅ Faithful
Notes: All spec'd structs/fields/enum variants present; workflow transition tables match spec exactly; pure (no I/O); has

fixtures + tests
────────────────────────────────────────
Layer: model::log
Status: ⚠️ Minor
Notes: LogEntry.actions / ActivityEntry.actions are Vec<LogAction>, spec says optional (log/mod.rs:12,27). Cosmetic.
────────────────────────────────────────
Layer: core::product::work_items
Status: ✅ Broad / ⚠️ ownership
Notes: Full CRUD/transition/bulk/assign/reorder. But node-ownership is not enforced before mutation (spec lines 460, 612)

— enforced in scheduling but not work_items
────────────────────────────────────────
Layer: core::product::project_state
Status: ⚠️ Split brain
Notes: Query + index layer fully built and correct; cache layer orphaned (finding #2)
────────────────────────────────────────
Layer: core::product::scheduling
Status: ✅ Real
Notes: promote/reserve/dispatch/pause/resume/cancel/retry; concurrency limits (global/node/provider/app); reservations
persisted + restart-recovered. Gap: feature-ordering enforced only in promote, not reserve/dispatch
────────────────────────────────────────
Layer: core::product::chat
Status: ✅ Strong
Notes: Durable sessions, interrupted-turn recovery, provider-resume, gap/feature context rebuild — closely matches the
detailed chat spec
────────────────────────────────────────
Layer: core::product::project_registry
Status: ✅ / ⚠️
Notes: register/attach/switch/detach/remove/inspect all real; clone not implemented (spec line 560)
────────────────────────────────────────
Layer: core::host::{git_worktrees, process_supervision, target_apps, agent_providers}
Status: ✅ Real
Notes: Genuine subprocess work — git CLI with audit log, OS process spawn/signal/stream with typed ProcessOwner, provider

CLI detection + JSON-event parsing
────────────────────────────────────────
Layer: core::host::cluster
Status: ⚠️ Fragile
Notes: Real, but shells out to system ssh (no ssh crate in Cargo.toml); no key/identity management; silently depends on
external binary
────────────────────────────────────────
Layer: core::host::installation
Status: ⚠️ Modeled only
Notes: Tracks install state but performs no real OS registration — backend.registered = true is hardcoded; no
launchd/systemd/Windows-service/signing/notarization (and no deps to do it)
────────────────────────────────────────
Layer: core::host::quality
Status: ◐ Present
Notes: service/types/tests scaffolded; depth of real browser-QA execution not fully confirmed
────────────────────────────────────────
Layer: core::supervisor::lifecycle
Status: ⚠️ State-only
Notes: start/stop write a status file; they do not spawn/kill the daemon. The real spawn lives in the CLI
(start_background_daemon), not in core lifecycle
────────────────────────────────────────
Layer: core::supervisor::{sessions,jobs,runtime,config,errors}
Status: ✅ Real
Notes: Token auth + registry, file-backed job registry w/ states & log pagination, OS-specific path layout,
settings/governance/reporters, categorized error enum
────────────────────────────────────────
Layer: core::supervisor::security
Status: ⚠️ Partial
Notes: Token auth + log redaction + audit trail real; no secret storage (keychain/cred-mgr), allowed_commands ACL defined

but unused
────────────────────────────────────────
Layer: core::supervisor::testing
Status: ❌ Stub
Notes: Only not_implemented_fixture() (testing/mod.rs:3). Spec requires fake supervisor/providers/process handles +
contract helpers
────────────────────────────────────────
Layer: core::observability::{logs,activity,diagnostics}
Status: ✅ Real
Notes: JSONL sidecars, multi-filter query + retention, 9-category doctor
────────────────────────────────────────
Layer: core::observability::{metrics,support_bundle}
Status: ◐ Partial
Notes: Types/constants present; full service depth unconfirmed
────────────────────────────────────────
Layer: surfaces::web_server
Status: ✅ Real
Notes: Hand-rolled std::net::TcpListener HTTP/1.1; static serving w/ traversal guards; SSE (/events, /api/sse); auth +
local-origin checks; idempotency keys; API-version negotiation; all 9 API groups; thin handlers delegating to core.
Single-threaded (one request per accept)
────────────────────────────────────────
Layer: surfaces::web/static
Status: ✅ Complete
Notes: All 55 assets copied from python/refine_ui/static/; xtask verifies parity
────────────────────────────────────────
Layer: surfaces::cli
Status: ✅ Tree / ❌ wiring
Notes: All 9 model-oriented groups + every spec'd action present — but wired in-process (finding #1)
────────────────────────────────────────
Layer: surfaces::desktop + desktop/src-tauri
Status: ⚠️ Bridge only
Notes: FileDesktopShellBridge real (bootstrap/status/notify/tray/deep-link/event-stream); Tauri package is a stub —
src-tauri/Cargo.toml has no tauri dependency, main.rs just calls desktop_bridge_commands(). No window/webview/tray
actually rendered
────────────────────────────────────────
Layer: xtask
Status: ✅ Real
Notes: api-contract export, static-asset parity check, runtime-layout — real file I/O

---
Inconsistencies (beyond the two headline findings)

- Lifecycle authority is in the wrong layer. Spec puts daemon start/stop/recover in core::supervisor::lifecycle, but the
only real process spawn is in surfaces::cli::dispatch::start_background_daemon. Core lifecycle is reduced to status-file
bookkeeping. A surface owns the capability the spec assigns to core.
- Node-ownership enforcement is asymmetric. scheduling checks active-node ownership before acting; work_items does not.
So scheduled work respects ownership but direct CLI/API mutations can edit another node's gaps — violating spec lines
460-461.
- Two undocumented modules. core/product/imports/ and core/product/nodes/ exist and are sensible, but aren't in the
spec's suggested layout. Minor — the layout is explicitly "suggested" — but worth folding into the spec or noting as an
intentional decision.
- installation.registered = true hardcoded presents fake success for an OS integration that doesn't happen. This is the
kind of convincing-stub that will read as "done" on a status board while doing nothing — flag it explicitly rather than
leave it green.
- Dependency gaps that cap several capabilities at "shell-out or stub": no async runtime / HTTP-server crate (web server
is sync, single-threaded), no ssh crate (cluster depends on system binary), no OS-service/signing crates (installation
can't really register a daemon). These aren't bugs, but they bound how far installation, cluster, and concurrency can go
without new dependencies — and the spec's "native, no host Python" daemon-registration goal isn't reachable
as-dependency'd.