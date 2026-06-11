# Rust Integration Test Plan

## Summary

Refine's Rust integration tests live with the Rust implementation and exercise the
same product services through the public CLI. The CLI is the integration-test
surface because normal product commands route through the daemon, and the daemon
is shared by the CLI, web UI, workflow engine, and background workers.

The recommended shape is layered:

- Fast Rust unit and contract tests for in-process behavior.
- Cargo integration tests for production CLI guardrails.
- Daemon-backed CLI integration tests under `tests/cli_surface.rs`.
- Focused Docker-backed suites for install/uninstall and cluster SSH behavior.
- A deterministic `smoke-ai` fixture launched as a real executable through
  `REFINE_SMOKE_AI_PATH`.
- `xtask` commands that run the suites with isolated setup and teardown.

There is no repo-local browser integration scaffold. Do not add `tests/ui`,
npm manifests, or browser-test runner commands for first-party integration
coverage.

## Goals

- Validate Rust Refine through public CLI behavior, not private source imports.
- Exercise daemon-backed product commands so CLI and web behavior share the same
  backend services.
- Keep each run isolated by port, runtime root, cache, process registry, and
  disposable target app.
- Keep deterministic AI workflows testable without real provider credentials.
- Make failures inspectable with CLI stdout/stderr, daemon logs, runtime roots,
  Docker logs, and Smoke AI debug output.
- Keep the suite easy to invoke from a fresh checkout with stable commands.

## Non-Goals

- Replace unit tests in `src/**/tests.rs`.
- Replace route and model contract tests that can run faster in process.
- Reintroduce browser automation for first-party integration coverage.
- Depend on Node, npm, Python, or `uv` for the Rust product test path.
- Exercise private `.refine/` storage layout directly from integration tests
  except where a fixture needs setup or cleanup diagnostics.
- Test code signing, notarization, or real external provider authentication in
  the default local suite.

## Test Layers

### Unit And Contract Tests

Keep the existing Rust unit and module-level tests as the fast correctness
layer. These tests should continue to cover pure model behavior, workflow
transition rules, route request and response shaping, projection cache behavior,
core services with fake providers, security, redaction, diagnostics, and
settings contracts.

### Production CLI Guardrails

Production CLI guardrail tests should use the compiled binary via
`env!("CARGO_BIN_EXE_refine")`. They prove that normal builds reject internal
test-only escape hatches and route product commands through the daemon.

The current suite is `tests/cli_target_root.rs`.

### Daemon-Backed CLI Integration Tests

CLI integration tests should be Cargo integration tests under `tests/`, using a
shared fixture from `tests/support/integration.rs`. Daemon-backed tests should be
ignored by default so plain `cargo test` stays fast; `xtask test-cli` is the
public command that opts into starting the daemon and running them.

The tests should:

- Start a real daemon on an allocated or configured test port.
- Use an isolated runtime root.
- Attach a disposable git-backed target app.
- Invoke public CLI commands exactly as a user would.
- Assert structured JSON stdout where available.
- Assert user-visible stderr for failures.
- Prefer CLI-visible state over direct private-state reads.

Coverage should keep expanding around the shared daemon services used by all
surfaces:

- `system status`, `system doctor`, and `system api-groups`.
- `project status`, `attach`, `switch`, `detach`, `register`, `clone`,
  `remove`, `migrate`, and `sync`.
- Gap lifecycle commands: create, list, show, edit, note, round, workflow
  movement, retry, verify, merge, undo, and delete.
- Feature lifecycle and membership commands.
- Node registry and cluster-operation commands.
- Activity log list, tail, query, show, export, and support bundle commands.
- Agent detect/configure/auth/diagnose/invoke/resume through Smoke AI.

When a user journey has both CLI and web UI entrypoints, prefer testing the
backend semantics once through the CLI and keeping web-server/unit tests focused
on request/response and rendering contracts.

## Fixture Contract

The shared integration fixture owns:

- Temporary runtime root.
- Temporary cache/artifact root.
- Disposable target app root.
- Git initialization for target apps that need real branch or worktree behavior.
- Daemon startup and teardown.
- Smoke AI binary path.
- CLI command execution helpers.
- JSON stdout parsing and assertion helpers.

Environment variables used by the fixture:

```text
REFINE_TEST_PORT
REFINE_TEST_RUNTIME_ROOT
REFINE_TEST_APP_ROOT
REFINE_DAEMON_PORT
REFINE_SMOKE_AI_PATH
```

All CLI invocations in the integration suite must target this daemon. If a
command infers a daemon port from environment, the fixture must set
`REFINE_DAEMON_PORT` to the same value as `REFINE_TEST_PORT`; this is the live
CLI routing contract for daemon-backed product commands. If a command accepts an
explicit `--port`, the tests should pass it.

## Public Commands

Expose stable runner commands through `xtask`:

```sh
cargo run --manifest-path xtask/Cargo.toml -- test-cli
cargo run --manifest-path xtask/Cargo.toml -- test-smoke-ai
cargo run --manifest-path xtask/Cargo.toml -- test-cluster-ssh
cargo run --manifest-path xtask/Cargo.toml -- test-install-uninstall
cargo run --manifest-path xtask/Cargo.toml -- test-full-workflow
cargo run --manifest-path xtask/Cargo.toml -- test-multi-instance-sync
cargo run --manifest-path xtask/Cargo.toml -- test-integration
```

Expected behavior:

- `test-cli` runs daemon-backed CLI integration tests.
- `test-smoke-ai` validates the deterministic provider contract.
- `test-cluster-ssh` runs Docker/SSH-backed cluster CLI tests.
- `test-install-uninstall` runs Docker-backed installer tests.
- `test-full-workflow` runs the daemon-backed full workflow test.
- `test-multi-instance-sync` runs multi-instance sync tests.
- `test-integration` runs the opt-in integration suite without browser tests.

The existing `cargo run --manifest-path xtask/Cargo.toml -- check` remains
focused on static and contract checks. Integration suites stay opt-in unless CI
explicitly calls them.

## Repository Layout

Current integration-test layout:

```text
tests/
  cli_surface.rs
  cli_target_root.rs
  cluster_ssh_cli.rs
  full_workflow.rs
  install_uninstall_docker.rs
  multi_instance_sync.rs
  production_binary_install.rs
  smoke_ai_contract.rs
  support/
    integration.rs
  fixtures/
    smoke-ai/
```

Do not add repo-local browser test directories, browser runner config,
`package.json`, or `package-lock.json` for integration testing.

## CI Strategy

Run three tiers:

- Fast default: `cargo test` and `cargo run --manifest-path xtask/Cargo.toml -- check`.
- CLI integration: `cargo run --manifest-path xtask/Cargo.toml -- test-cli`.
- Extended/manual: Docker install checks, SSH cluster checks,
  update/rollback packaging, multi-instance sync, full workflow, and real
  provider authentication.

Artifacts to retain on CI failure:

- CLI command transcript.
- Daemon stdout/stderr.
- Runtime root logs.
- Docker logs when the suite uses Docker.
- Smoke AI debug stderr when enabled.

## Acceptance

- `test-smoke-ai` passes without external provider credentials.
- `test-cli` starts the daemon, attaches the app, and exercises shared product
  services through public CLI commands.
- Production CLI guardrails prove internal target-root escape hatches are not
  available in normal builds.
- No first-party browser scaffold, npm manifest, browser test directory, or
  browser runner command exists in the repo.
