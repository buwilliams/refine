# Refine Rust Port — Spec Conformance Report

**Scope:** `docs/spec/rust-spec.md` against all of `rust/src/`, `rust/xtask/`, `rust/desktop/`.
**Method:** full re-read after the second round of conformance commits (`16f7d21`, `d087ea5`); I re-verified each
prior finding and residual against source. Built and ran the suite: `cargo test` is green — **161 passed, 0
failed**, and the test binaries compile clean.
**Status vs previous report:** the two headline structural findings remain resolved, and three of the prior
residuals (installation activation, support-bundle depth, layout) are now closed too. What's left is a single
architectural residual plus dependency-bounded integration depth.

---

## Verdict

The port is now a coherent daemon-centric implementation, ~34k lines, and it builds and tests clean. The
spec's central thesis — *one daemon as the sole local authority, with a projection cache as the only query
path* — is now actually wired, not just modeled. The CLI talks to the daemon over HTTP by default, the daemon
owns the projection cache, node ownership is enforced at the mutation boundary, the daemon process is really
spawned, secret storage and command authorization exist, the desktop package is a real (feature-gated) Tauri 2
app, and the web server is no longer single-threaded.

The latest commits closed most of what was left. Installation now *activates* services (real `systemctl` /
`launchctl` calls), the support bundle redacts secrets by default, cluster SSH gained real preflight guards, and
the spec itself was updated to list `imports/` and `nodes/` in the layout. The one architectural residual that
persists is the in-process CLI escape hatch: when a caller passes `--durable-root`, the CLI still mutates
durable state directly instead of going through the daemon. The other open items are dependency-bounded
integration depth (cluster SSH is a guarded shell-out rather than a native crate; no code signing/notarization),
not architectural contradictions.

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
| Installation writes service files but never activates | Residual #2 | **Resolved** | `activate_os_backend` executes the commands via `run_service_command` — `systemctl --user daemon-reload`+`enable` (`installation/mod.rs:640-648`), `launchctl bootstrap`+`enable` (`:661-667`); `activated`/`activation_error` reflect the real outcome; `deactivate_os_backend` mirrors it. |
| Cluster SSH silently depends on external `ssh` | Residual #3 | **Narrowed** | Still a shell-out, but now guarded: `validate_ssh_prerequisites` → `ensure_ssh_binary_available` (`cluster/mod.rs:513-530`) probes `ssh` and reports a clear error; identity-file preflight + `user@host` destination handling; tested (`:714-725`). No native SSH crate yet. |
| Support-bundle depth unconfirmed | Residual #6 | **Resolved** | `export` redacts by default via `redact_json`/`should_redact_key` covering secret/token/password/key (`support_bundle/mod.rs:151-179`); tested (`:191-198`). |
| `imports/`/`nodes/` missing from spec layout | Residual #5 | **Resolved (spec)** | Spec now lists both under `core/product/` in the suggested layout. |

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

2. **Cluster SSH is a guarded shell-out, not a native integration** (`cluster/mod.rs:503`, `Command::new("ssh")`).
   The latest commit added the right guardrails — `ensure_ssh_binary_available` probes for `ssh` and surfaces a
   clear error, and identity-file preflight is tested — so it no longer fails silently. But it still depends on
   a working external `ssh` and the user's `~/.ssh` setup; there is no SSH crate. Fine for now; worth revisiting
   if remote execution becomes load-bearing.

3. **Installation activates services but still can't sign or notarize.** `activate_os_backend` now really runs
   `systemctl`/`launchctl`, so daemon auto-start works on Linux/macOS where those managers are present. What's
   left is genuinely installer-territory: code signing, notarization, and the Windows service path are deferred
   to "the platform installer" (the note emitted when no activation commands apply). Not a gap in the daemon
   architecture, just packaging work that lives outside this crate.

4. **Core `Cargo.toml` is still deliberately minimal** — `clap, chrono, serde, serde_json, thiserror, uuid`;
   no async runtime, HTTP, SSH, or OS-service crates. The hand-rolled thread-per-connection HTTP server is fine
   for a local daemon, and avoiding heavy deps is a defensible choice, but it's the root reason cluster SSH is a
   shell-out rather than a native integration. Flagging it as the shared cause, not a bug.

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
| `core::host::cluster` | ✅ (shell-dep) | Real remote run; now preflight-guarded (`ensure_ssh_binary_available`, identity check); shells out to system `ssh`, no native crate. |
| `core::host::installation` | ✅ (no signing) | Generates **and activates** systemd/launchd services via real `systemctl`/`launchctl`; code signing/notarization deferred to platform installer. |
| `core::supervisor::lifecycle` | ✅ | **Really spawns** the daemon (`current_exe` + `setsid`), status/health/recover. |
| `core::supervisor::security` | ✅ | Token auth, redaction, audit, **secret store**, **command ACL** enforced. |
| `core::supervisor::{sessions,jobs,runtime,config,errors}` | ✅ | Auth + registry, file-backed jobs, OS path layout, settings/governance, categorized errors. |
| `core::supervisor::testing` | ✅ | Real fixtures + fakes + contract helper. |
| `core::observability::{logs,activity,diagnostics}` | ✅ | JSONL sidecars, multi-filter query + retention, 9-category doctor. |
| `core::observability::{metrics,support_bundle}` | ✅ | Support bundle redacts secrets by default (`redact_json`); metrics present. |
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

The remaining shortfalls are integration-depth rather than architecture: cluster SSH is a guarded shell-out
bounded by the minimal dependency set (residuals #2, #4), and installer-grade signing/notarization is deferred
to packaging (residual #3). The single architectural residual is the in-process CLI `--durable-root` escape
hatch (residual #1) — opt-in, but still a way to mutate durable state off-daemon. None of these undercut the
daemon-centric design the spec asks for.
