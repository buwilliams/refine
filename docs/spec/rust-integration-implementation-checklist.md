# Rust Integration Implementation Checklist

This checklist tracks implementation status only. The authoritative contract is
`docs/spec/rust-integration-spec.md`; this file must not weaken, reinterpret, or
replace that spec. The UI surface inventory remains
`docs/spec/rust-integration-feature-index.md`.

## Phase 1: Baseline Discovery And Gap Map

- [x] Read `docs/spec/rust-integration-spec.md`.
- [x] Read `docs/spec/rust-integration-feature-index.md`.
- [x] Inspect current `Cargo.toml`, `xtask`, CLI actions/dispatch, web server,
  and existing tests.
- [x] Inspect `../refine-test` Rust CLI/UI harness and Smoke AI prototype.
- [x] Review final implementation against every hard requirement below.

## Phase 2: Public `xtask` Commands

- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-smoke-ai`
  builds/runs the deterministic provider contract.
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-cli`
  runs daemon-backed Cargo CLI surface tests.
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-ui`
  runs the repo-local Playwright UI suite.
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-surface`
  runs Smoke AI, CLI, then UI, stopping on first failure.
- [x] Existing `xtask check` remains focused on static/contract checks.

## Phase 3: Deterministic Smoke AI Fixture

- [x] Fixture is a real executable, not only an in-process fake.
- [x] Fixture is discoverable through `REFINE_SMOKE_AI_PATH`.
- [x] Supports prompt matching and stdin matching.
- [x] Emits stable fallback output.
- [x] Supports JSON and JSONL template output.
- [x] Preflight prompt returns exactly `hello`.
- [x] Debug mode writes to stderr without changing stdout.
- [x] Unknown flags exit with status `0`.
- [x] Default suite does not require real provider credentials.

## Phase 4: Shared Isolated Runtime Fixture

- [x] Uses `REFINE_TEST_PORT` with default port `18080`.
- [x] Sets `REFINE_DAEMON_PORT` to the same value as `REFINE_TEST_PORT`.
- [x] Keeps runtime roots, cache, process state, and artifacts isolated under
  `target/refine-integration/`.
- [x] Does not touch the checkout's normal `run/` state.
- [x] Uses a disposable git-backed target app.
- [x] Starts a real daemon and polls readiness through public HTTP.
- [x] Attaches the disposable app through public CLI/HTTP surfaces.
- [x] Teardown runs on success and failure.
- [x] Failure diagnostics retain CLI transcript, daemon stdout/stderr, runtime
  logs, Playwright traces/screenshots/report, and Smoke AI stderr when relevant.

## Phase 5: Cargo CLI Surface Tests

- [x] Plain `cargo test` stays fast; daemon-backed tests are ignored/gated.
- [x] Tests invoke the compiled public `refine` binary.
- [x] Tests avoid direct private `.refine` storage inspection except teardown
  diagnostics.
- [x] Coverage includes `system status` healthy daemon.
- [x] Coverage includes `project status` attached disposable app.
- [x] Coverage includes `project doctor`.
- [x] Coverage includes `gap create/list/show/edit/note/round/delete`.
- [x] Coverage includes `workflow allowed` and user-driven transitions.
- [x] Coverage includes `feature create/list/add-gap/remove-gap/delete`.
- [x] Coverage includes feature rollup assertions from public output.
- [x] Coverage includes `node list/create/activate/archive`.
- [x] Coverage includes rejection of internal durable-root escape hatches.

## Phase 6: Repo-Local Playwright TypeScript UI Harness

- [x] `package.json`, lockfile, and `playwright.config.ts` are repo-local.
- [x] Harness does not depend on Python or `uv`.
- [x] Global setup starts the Rust daemon through repo-local setup.
- [x] Global setup uses `src/surfaces/web/static` assets.
- [x] Global setup attaches the disposable target app before first test.
- [x] Tests use visible UI controls, roles/labels/text, or documented `#id`s.
- [x] Tests avoid fixed sleeps; waits are state/DOM based.
- [x] Traces/screenshots/report are retained on failure.
- [x] Initial UI coverage includes app shell and primary navigation.
- [x] Initial UI coverage includes attached project status and Gaps query.
- [x] Initial UI coverage includes Gap creation/deletion through the browser.
- [x] Initial UI coverage includes Feature creation/membership where feasible.
- [x] Initial UI coverage includes selecting Smoke AI in runtime settings.
- [x] Initial UI coverage includes a planning chat path that uses Smoke AI
  deterministically.

## Phase 7: `test-surface` Orchestration And Teardown

- [x] Runs suites in order: Smoke AI, CLI, UI.
- [x] Stops on first failure.
- [x] Teardown runs even when setup or tests fail.
- [x] Reports artifact paths and actionable missing-dependency instructions.

## Phase 8: CI, Artifacts, Failure Diagnostics

- [x] Chromium-only Playwright surface smoke is the default UI project.
- [x] Missing Playwright browser/dependency failures are actionable.
- [x] CLI transcript is retained.
- [x] Daemon stdout/stderr are retained.
- [x] Runtime logs are retained or copied before cleanup.
- [x] Smoke AI debug stderr is retained when relevant.
- [x] Artifacts live outside durable product state.

## Phase 9: Final Verification

- [x] `cargo test`
- [x] `cargo run --manifest-path xtask/Cargo.toml -- check`
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-smoke-ai`
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-cli`
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-ui`
- [x] `cargo run --manifest-path xtask/Cargo.toml -- test-surface`
- [x] `git diff --check`
- [x] Final review notes any remaining risks, skipped checks, or local
  dependency limitations.
