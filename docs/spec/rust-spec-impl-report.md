# Refine Rust Port — Spec Conformance Report

**Scope:** `docs/spec/rust-spec.md` against all of `rust/src/`, `rust/xtask/`, `rust/desktop/`.
**Method:** full re-read after the refactor; I re-verified each prior finding against source. This pass I also
built and ran the suite: `cargo test` is green — **159 passed, 0 failed**, and the test binaries compile clean.
**Status vs previous report:** the two headline structural findings are now substantially resolved, along with
most of the secondary gaps. What remains is a smaller, well-understood residual tier.

---

## Verdict

The port is now a coherent daemon-centric implementation, ~34k lines, and it builds and tests clean. The
spec's central thesis — *one daemon as the sole local authority, with a projection cache as the only query
path* — is now actually wired, not just modeled. The CLI talks to the daemon over HTTP by default, the daemon
owns the projection cache, node ownership is enforced at the mutation boundary, the daemon process is really
spawned, secret storage and command authorization exist, the desktop package is a real (feature-gated) Tauri 2
app, and the web server is no longer single-threaded.

The remaining deviations are narrower: an in-process CLI escape hatch that still bypasses the daemon when a
caller passes `--durable-root`, OS-service *activation* (as opposed to file generation) that installation does
not perform, and cluster SSH that still shells out to the system `ssh` binary. None of these contradict the
architecture the way the prior two findings did.

---

## What changed since the last review

| Prior finding | Prior status | Now | Evidence |
|---|---|---|---|
| **#1 CLI bypasses daemon, mutates durable state in-process** | Largest deviation | **Resolved (default path)** | `dispatch.rs:46-48` routes `Project` to the daemon; `dispatch.rs:1620-1624` routes `Gap/Feature/Workflow/Node/Cluster` to `dispatch_*_daemon`; `daemon_json` (`dispatch.rs:2255`) is a real HTTP client with session-token auth, `Idempotency-Key`, and `X-Refine-API-Version: 1`. In-process service construction now only fires when an explicit `--durable-root` is supplied (`skipped_durable_root`, `dispatch.rs:2400`). |
| **#2 Projection cache built but never read in production** | Dead code | **Resolved (daemon path)** | Daemon wires `FileWorkItemService::with_projection_cache(..., runtime_root/cache)` (`web_server/runtime.rs:55`); `projection_snapshot()` prefers `load_or_refresh_projection` when a cache dir is set (`work_items/service.rs:75-82`); scheduling and chat now call `load_or_refresh_projection` (`scheduling/mod.rs:305-307`, `chat/mod.rs:564`); cache is refreshed after each mutation (`http.rs:241`). |
| Node ownership not enforced in `work_items` | Asymmetric | **Resolved** | `ensure_gap_owned` / `ensure_feature_owned` return `RefineError::Conflict` when `gap.node_id != active_node_id` (`work_items/service.rs:90-123`). |
| Daemon lifecycle never spawns the process | State-only | **Resolved** | `start_background_daemon` spawns `current_exe`, `try_wait`s for readiness, and uses `setsid` to detach (`lifecycle/mod.rs:67-122,208`). The spawn now lives in core, not the CLI surface. |
| `project_registry` `clone` missing | Gap | **Resolved** | `clone_app` trait + impl runs `git clone` (`project_registry/mod.rs:18,216-224,313`). |
| Security: no secret storage, ACL unused | Partial | **Resolved** | `SecretStore` trait + `NativeSecretStore`, `SecretStoreBackend::{MacosKeychain, WindowsCredentialManager}` (`security/mod.rs:29-89`); `allowed_commands` loaded from settings and enforced (`security/mod.rs:453-503`). |
| `supervisor::testing` is a bare stub | Stub | **Resolved** | `TestRuntimeFixture`, `FakeProcessSupervisor`, `FakeProvider`, `assert_json_contract`, `process_spec` (`testing/mod.rs:14-158`). |
| Desktop/Tauri package empty | Stub | **Resolved** | `real-tauri` feature pulls `tauri 2.11.2` + `tauri-build`; gated `real_tauri` module builds a genuine `WebviewWindowBuilder` / `TrayIconBuilder` / `Menu` app with `#[tauri::command]`s; default build runs the bridge shell (`desktop/src-tauri/src/main.rs`, `Cargo.toml:6-22`). |
| Web server single-threaded | Limitation | **Resolved** | `serve_next_concurrent` spawns a thread per connection (`http.rs:111-119`, used at `http.rs:309`). |
| `model::log` `actions` not optional | Cosmetic | **Resolved** | `#[serde(default, skip_serializing_if = "Vec::is_empty")]` on both `LogEntry.actions` and `ActivityEntry.actions` (`model/log/mod.rs`). |
| Quality QA depth unconfirmed | Unknown | **Improved** | `run_playwright` invokes a real `playwright test <spec>` and captures `screenshot.png` (`quality/service.rs:304-378`). |

---

## Remaining gaps and residual inconsistencies

1. **The in-process CLI escape hatch still bypasses the daemon.** When a command is given an explicit
   `--durable-root`, the CLI constructs `FileWorkItemService::new(durable_root)` and mutates `.refine` files
   directly — no daemon auth, no idempotency, no SSE notification, and (because it uses `::new`, not
   `::with_projection_cache`) a full-scan `rebuild_projection()` rather than the cache. This is now opt-in
   rather than the default, and it's a reasonable scripting/bootstrap affordance, but if a user passes
   `--durable-root` while the daemon is also running, you again have two uncoordinated writers on the same
   files. Worth either documenting as an intentional offline/bootstrap-only path or guarding against use while
   a daemon holds the port.

2. **Installation generates service files but does not activate them.** `register_os_backend` now writes a real
   systemd user unit and a launchd plist, and `registered`/`partial` reflect whether the write actually
   succeeded (`installation/mod.rs:215-263,360`) — a genuine improvement over the previously hardcoded
   `registered = true`. But it stops at writing the unit file; nothing runs `systemctl --user enable` /
   `launchctl load`, and there is no code signing or notarization. The daemon won't actually auto-start on
   login from this alone. This is honest now (the `partial` flag surfaces it), just incomplete.

3. **Cluster SSH still shells out to the system `ssh` binary** (`cluster/mod.rs:502`, `Command::new("ssh")`).
   No SSH crate, no key/identity management; remote execution silently depends on a working external `ssh` and
   `~/.ssh` setup. Functional but fragile, and unchanged from the prior review.

4. **Core `Cargo.toml` is still deliberately minimal** — `clap, chrono, serde, serde_json, thiserror, uuid`;
   no async runtime, HTTP, SSH, or OS-service crates. The hand-rolled thread-per-connection HTTP server is fine
   for a local daemon, and avoiding heavy deps is a defensible choice, but it's the root reason #2 and #3 above
   are shell-out / file-generation rather than native integrations. Flagging it as the shared cause, not a bug.

5. **Layout nit:** `core/product/imports/` and `core/product/nodes/` exist and are sensible but still aren't in
   the spec's "suggested layout." Fold them into the spec or note them as intentional.

6. **Not re-verified in depth this pass:** `observability::{metrics, support_bundle}` internals and the full
   chat transcript/resume surface. They compile and their tests pass; I did not re-read them line-by-line after
   the refactor.

---

## Layer-by-layer conformance

| Layer | Status | Notes |
|---|---|---|
| `model::{project,gap,feature,workflow,cluster,node}` | ✅ Faithful | Structs/fields/enum variants match; transition tables match spec; pure; fixtures + tests. |
| `model::log` | ✅ | `actions` now effectively optional on the wire (`skip_serializing_if`). |
| `core::product::work_items` | ✅ | Full CRUD/transition/bulk/assign/reorder; **node ownership now enforced** (`Conflict` on mismatch). |
| `core::product::project_state` | ✅ | Query + index layer intact; **cache now consumed in production** via `load_or_refresh_projection`. |
| `core::product::scheduling` | ✅ | promote/reserve/dispatch/pause/resume/cancel/retry; concurrency limits; restart-recovered reservations; now reads the projection cache. |
| `core::product::chat` | ✅ | Durable sessions, interrupted-turn recovery, provider-resume, context rebuild. |
| `core::product::project_registry` | ✅ | register/attach/switch/detach/remove/inspect/**clone** all present. |
| `core::host::{git_worktrees, process_supervision, target_apps, agent_providers}` | ✅ | Real subprocess work; git audit log; typed `ProcessOwner`; provider detect + JSON-event parse. |
| `core::host::quality` | ✅ (shell-dep) | Real `playwright test` invocation + screenshot capture; depends on external playwright. |
| `core::host::cluster` | ⚠️ Fragile | Real but shells out to system `ssh`; no key management. |
| `core::host::installation` | ⚠️ Writes, doesn't activate | Generates real systemd/launchd files; `partial` flag honest; no enable/load, no signing. |
| `core::supervisor::lifecycle` | ✅ | **Really spawns** the daemon (`current_exe` + `setsid`), status/health/recover. |
| `core::supervisor::security` | ✅ | Token auth, redaction, audit, **secret store**, **command ACL** enforced. |
| `core::supervisor::{sessions,jobs,runtime,config,errors}` | ✅ | Auth + registry, file-backed jobs, OS path layout, settings/governance, categorized errors. |
| `core::supervisor::testing` | ✅ | Real fixtures + fakes + contract helper. |
| `core::observability::{logs,activity,diagnostics}` | ✅ | JSONL sidecars, multi-filter query + retention, 9-category doctor. |
| `core::observability::{metrics,support_bundle}` | ◐ | Present; not re-verified in depth (tests pass). |
| `surfaces::web_server` | ✅ | std `TcpListener` HTTP/1.1, **thread-per-connection**; static serving with traversal guards; SSE; auth + local-origin; idempotency; version negotiation; serves every route the CLI calls. |
| `surfaces::web/static` | ✅ | 55 assets copied from `python/refine_ui/static/`; xtask verifies parity. |
| `surfaces::cli` | ✅ default / ⚠️ escape hatch | All 9 model-oriented groups + actions; **daemon-routed by default**; in-process only with explicit `--durable-root`. |
| `surfaces::desktop` + `desktop/src-tauri` | ✅ feature-gated | Bridge shell by default; real Tauri 2 webview/tray/menu under `--features real-tauri`. |
| `xtask` | ✅ | api-contract export, static-asset parity, runtime-layout. |

---

## Acceptance criteria

The two criteria that previously failed now pass as wired:

- *"The daemon is the sole local authority for process lifecycle and durable workflow mutations."* — The default
  CLI path, the web/desktop surfaces, lifecycle spawn, and the projection cache all now route through the
  daemon. The only way to mutate durable state off-daemon is the explicit `--durable-root` escape hatch
  (residual #1).
- *"durable model records plus a persisted projection snapshot … and in-memory indexes"* as the query path —
  The daemon loads/refreshes the `<port>/cache/` snapshot, builds indexes, and refreshes after mutation; CLI,
  scheduling, and chat consume it.

The remaining shortfalls against the spec are integration-depth rather than architecture: installation activates
nothing (residual #2), cluster SSH and the HTTP/SSH layers are shell-outs bounded by the minimal dependency set
(residuals #3–#4). These are reasonable next targets but don't undercut the daemon-centric design the spec asks
for.
