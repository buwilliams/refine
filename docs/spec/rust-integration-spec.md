# Rust Integration Test Plan

## Summary

Refine's Rust integration tests should live with the Rust implementation and exercise the same public surfaces a user uses: CLI and browser UI. The tests should preserve the useful black-box discipline from the standalone `refine-test` project while using Rust-native infrastructure where it actually helps: process orchestration, temp runtime roots, compiled CLI assertions, and deterministic provider fixtures.

The recommended shape is layered:

- Cargo integration tests for the CLI surface and external provider contract.
- Playwright TypeScript tests for the browser UI surface.
- `xtask` commands that run the surface suites with isolated setup and
  teardown.
- A deterministic `smoke-ai` fixture launched as a real executable through
  `REFINE_SMOKE_AI_PATH`.

The goal is not to re-create the Python `refine-test` runner inside Rust. The goal is to port the best ideas into a native test harness that is close to the Rust tree, cheap to run locally, stable in CI, and explicit about which product surface failed.

This document is the single source for integration testing: it carries both the harness/layering plan and the **UI testing contract** (determinism, preconditions, oracles, selectors, timing — see [UI Surface Tests](#ui-surface-tests)) that a Playwright author needs. The companion `docs/spec/3.x.x/rust-integration-feature-index.md` is the UI **surface inventory** — every screen, modal, route, `#id`, endpoint, storage key, and the Gap workflow state machine. Use the feature index to learn *what exists and how to address it*; use this document to learn *how to test it*.

## Goals

- Validate Rust Refine through public CLI and UI behavior, not source imports
  or private file inspection.
- Keep each run isolated by port, runtime root, cache, process registry, and
  disposable target app.
- Exercise the real daemon path for normal product commands.
- Keep deterministic AI workflows testable without real provider credentials.
- Make failures inspectable with CLI stdout/stderr, daemon logs, Playwright
  traces, screenshots, and fixture paths.
- Keep the suite easy to invoke from a fresh checkout with one stable command.
- Let the surface suite grow from the UI feature index without forcing every
  feature into one slow end-to-end journey.

## Non-Goals

- Replace unit tests in `src/**/tests.rs`.
- Replace model and route contract tests that can run faster in process.
- Use Rust browser automation only because the application is written in Rust.
- Depend on Python or `uv` for the Rust product test path.
- Exercise private `.refine/` storage layout directly from integration tests.
- Test remote SSH, system service installation, code signing, notarization, or
  real external provider authentication in the default local suite.

## Current Baseline

The standalone `../refine-test` project already has the right product testing ideas:

- A public runner split by surface.
- Port-scoped setup and teardown.
- A disposable git-backed target app.
- CLI tests that run the real command.
- Playwright tests against the served web UI.
- A deterministic `smoke-ai` executable plus contract tests.

It also has an early Rust path:

- `tests/rust_cli` drives the `rr` command against an isolated daemon.
- `tests/rust_ui` runs Playwright against a Rust daemon on a separate port.
- `tests/support/rust.py` starts the daemon, attaches `rust-test-app`, and
  tears down the runtime root.

This should be treated as the prototype, not the long-term home. Rust-specific surface tests should move into this repo so CLI shape, daemon flags, static asset paths, provider behavior, and test helpers evolve together.

## Test Layers

### Unit And Contract Tests

Keep the existing Rust unit and module-level tests as the fast correctness layer. These tests should continue to cover:

- Pure model behavior.
- Workflow transition rules.
- Route request and response shaping.
- Projection cache behavior.
- Core services with fake providers and fake process supervisors.
- Security, redaction, diagnostics, and settings contracts.

These tests can inspect internal types because they are not the surface suite. They are the first line of defense and should stay fast enough for ordinary `cargo test`.

### CLI Surface Tests

CLI surface tests should be Cargo integration tests under `tests/`, using the compiled binary via `env!("CARGO_BIN_EXE_refine")` or a small shared helper.

The tests should:

- Start a real daemon on an allocated or configured test port.
- Use an isolated runtime root.
- Attach a disposable git-backed target app.
- Invoke public CLI commands exactly as a user would.
- Assert structured JSON stdout where available.
- Assert user-visible stderr for failures.
- Avoid direct reads from private durable state except for cleanup diagnostics.

Initial coverage should port the existing `refine-test` Rust CLI cases:

- `system status` reports a healthy daemon.
- `project status` reports the disposable app.
- `project doctor` runs.
- `gap create/list/show/edit/note/round/delete`.
- `workflow allowed` and user-driven transitions.
- `feature create/list/add-gap/remove-gap/rollup/delete`.
- `node list/create/activate/archive`.
- The production CLI rejects internal durable-root escape hatches.

As the UI feature index matures, CLI coverage should expand to every user journey that has a CLI equivalent. The CLI suite should be the preferred place to test workflow semantics that do not require browser interaction.

### UI Surface Tests

Browser tests should use Playwright with TypeScript specs. Rust WebDriver crates are available, but they do not replace Playwright's strongest features: auto-waiting, locators, traces, screenshots, HTML reports, browser projects, and mature debugging workflow.

The UI tests should live in a repo-local directory such as `tests/ui/` or `tests/playwright/`, with `playwright.config.ts` checked into this repo.

The tests should:

- Start the Rust daemon through repo-local setup.
- Use the daemon's static assets from `src/surfaces/web/static`.
- Attach the disposable target app before the first browser test.
- Navigate and interact through visible UI controls.
- Use HTTP only for setup checks or cleanup plumbing, not as a replacement for
  browser assertions.
- Prefer resilient role, label, and text selectors; for controls without those,
  use the documented `#id`s as the selector contract. There are no `data-testid`
  attributes in the UI today — see the Selectors contract below before
  introducing brittle CSS selectors.
- Retain traces and screenshots on failure.

Initial coverage should include:

- App shell loads and primary navigation renders.
- Dashboard, Gaps, Features, Changes, Logs, and Settings navigation.
- Attached and detached project states.
- Gap creation, edit, status movement, note or round entry, and deletion.
- Feature creation and Gap membership.
- Runtime/provider settings can select Smoke AI.
- Import or planning chat paths that use Smoke AI deterministically.
- Logs or system notices show user-visible errors in the expected surface.

The UI suite should map to the product feature index, but not every indexed
feature needs a full e2e test. Prefer one browser test per critical user
journey, with CLI and route/unit tests covering lower-level permutations.

#### UI Testing Contract

Every UI spec is written against these five rules. The surface inventory they
reference (screens, modals, routes, `#id`s, endpoints, the Gap workflow state
machine, SSE channels, storage keys, timing constants) lives in
`docs/spec/3.x.x/rust-integration-feature-index.md`; this contract is how to use it.

1. **Determinism — classify the flow first.**
   - `[crud]` flows are deterministic; drive them and assert synchronously on
     the resulting DOM. These include: create/edit gaps, features, rounds, and
     notes; filters, search, sort, and pagination; bulk status/priority/
     reporter/feature/transfer/delete; the manual workflow buttons (backlog ↔
     todo, review → done via **Verify**, done ↔ review); reporter, node, and
     cluster management; settings edits; and **Undo** (a real `git revert`).
   - `[agent]` flows run a real provider and must use the deterministic
     `smoke-ai` fixture (via `REFINE_SMOKE_AI_PATH`), then wait on the outcome.
     These include: chat replies (standalone/gap/plan), Draft Feature, Draft
     Round, import AI extraction, governance and quality evaluation, Generate
     rules, Generate target-app config, and the dispatcher-driven status chain
     `todo → in-progress → qa → ready-merge → awaiting-rebuild → review`
     (including auto-promote `backlog → todo`). Never assert these against a
     real external provider.

2. **Preconditions — gated features need state built first.**
   - **Verify / Verify selected**: a `review` Gap assigned to the currently
     selected reporter.
   - **← QA / ← Merge** buttons: only on `failed` Gaps in quality-retry /
     merge-retry context.
   - **Bulk transfer / assign**: skip `in-progress`, `qa`, `ready-merge`,
     `awaiting-rebuild`, and other-node Gaps — seed eligible Gaps.
   - **Generate rules**: product and constitution both filled.
   - **Run regressions**: at least one regression defined.
   - **Node / Governance** surfaces: an attached project; **Application**
     controls additionally need supervisor/registry mode enabled.

3. **Oracles — assert these non-obvious success states.**
   - **Verify** moves a Gap to `done`; **Cancel Feature** keeps `done` Gaps and
     cancels only non-terminal ones.
   - Duplicate detection (New Gap / Import) matches on actual/target text; the
     three decisions map to action keys `move_original_to_backlog`,
     create-anyway, and import-original.
   - **Undo** produces a revert commit (pushed if an upstream exists) and moves
     the Gap to `cancelled`.
   - Reporter throughput `completion_rate` is **server-computed** and shown
     beside Done/Reported — assert the value returned by `/api/dashboard`, never
     a formula recomputed in the test.

4. **Selectors — the `#id`s in the feature index are the contract.**
   There are no `data-testid` attributes today. Prefer ARIA role/label/text for
   anything without an `#id`. Dynamic rows (Gaps, import drafts, rounds) have no
   per-row id and no stable index — address a row by its visible text or by the
   Gap/Feature link `href`, not by position.

5. **Timing — wait on state, not the clock.**
   The UI is SSE-driven; always wait on the resulting DOM change (Playwright
   auto-waiting / `expect`), never a fixed sleep. `[agent]` transitions backed
   by `smoke-ai` resolve within a few seconds — cap waits at roughly 30s and
   fail loudly rather than poll indefinitely.

### Smoke AI Provider Tests

The deterministic provider should be a real executable fixture, not only an in-process fake. Refine should discover it through `REFINE_SMOKE_AI_PATH` and launch it with the same provider execution path used for real CLIs.

The fixture should support:

- Prompt matching.
- Stdin matching.
- Stable fallback output.
- JSON and JSONL templates for structured workflows.
- A preflight prompt that returns exactly `hello`.
- Debug mode that writes to stderr without changing stdout.
- Exit status `0` for unknown flags to mimic tolerant provider CLIs.

Implementation options:

- Preferred: a tiny Rust fixture binary under `tests/fixtures/smoke-ai/` or
  `xtask` that embeds templates with `include_str!`.
- Acceptable: a small script fixture if keeping exact parity with the previous
  provider is more valuable during the first migration.

The default Rust suite should not require real Claude, Codex, Gemini, Copilot,
or other provider credentials.

## Harness Design

### Runtime Fixture

Add a shared integration fixture for the surface suites. It should own:

- Test port allocation or a stable default with override.
- Runtime root creation.
- Static root resolution.
- Disposable target app creation.
- Daemon start and readiness polling.
- Project attachment.
- Provider fixture path export.
- Teardown and failure diagnostics.

Default paths should be outside durable product state, for example:

```text
target/refine-integration/
  run/<port>/
  apps/rust-test-app/
  smoke-ai/
  logs/
```

The fixture should never write to the checkout's normal `run/` directory unless
explicitly configured to do so.

### Port Policy

Use a dedicated default port for local surface tests, with environment
overrides for CI and parallel runs.

Suggested variables:

```text
REFINE_TEST_PORT
REFINE_TEST_BASE_URL
REFINE_TEST_RUNTIME_ROOT
REFINE_TEST_APP_ROOT
REFINE_SMOKE_AI_PATH
```

All CLI invocations in the integration suite must target this daemon. If a
command can infer a daemon port from environment, the fixture should set it. If
a command accepts an explicit `--port`, the tests should pass it.

### Public Commands

Expose stable runner commands through `xtask`:

```sh
cargo run --manifest-path xtask/Cargo.toml -- test-surface
cargo run --manifest-path xtask/Cargo.toml -- test-cli
cargo run --manifest-path xtask/Cargo.toml -- test-ui
cargo run --manifest-path xtask/Cargo.toml -- test-smoke-ai
```

Expected behavior:

- `test-cli` runs Cargo integration tests for the CLI surface.
- `test-ui` installs or checks Playwright dependencies, starts the daemon, runs
  Playwright, and tears down.
- `test-smoke-ai` validates the deterministic provider contract.
- `test-surface` runs smoke-ai, CLI, then UI, stopping on the first failure.

The existing `cargo run --manifest-path xtask/Cargo.toml -- check` should
remain focused on static and contract checks. Surface tests can be slower and
should be opt-in unless CI explicitly calls them.

## Repository Layout

Recommended additions:

```text
tests/
  cli_surface.rs
  fixtures/
    mod.rs
    smoke_ai.rs
  smoke_ai_contract.rs
  support/
    integration.rs

tests/ui/
  app_shell.spec.ts
  gaps.spec.ts
  features.spec.ts
  settings_provider.spec.ts
  helpers.ts
  global-setup.ts
  global-teardown.ts

playwright.config.ts
package.json
package-lock.json
```

If the TypeScript UI tests become large, move them under
`tests/playwright/`. Keep the directory name product-oriented rather than
language-oriented if there is only one browser harness.

## CI Strategy

Run three tiers:

- Fast default: `cargo test` and `cargo run --manifest-path xtask/Cargo.toml -- check`.
- Surface smoke: `cargo run --manifest-path xtask/Cargo.toml -- test-surface`.
- Extended/manual: multi-browser Playwright, service install checks, SSH
  cluster checks, update/rollback packaging, and real provider authentication.

The surface smoke tier should use Chromium only at first. Cross-browser checks
are useful later, but the first priority is stable coverage of product
journeys.

Artifacts to retain on CI failure:

- Playwright report.
- Playwright traces.
- Screenshots.
- Daemon stdout/stderr.
- Runtime root logs.
- CLI command transcript.
- Smoke AI debug stderr when enabled.

## Implementation Plan

### Phase 1: Establish The Harness

- Add `xtask` commands for `test-cli`, `test-ui`, `test-smoke-ai`, and
  `test-surface`.
- Add the shared runtime fixture.
- Add the deterministic Smoke AI fixture and contract tests.
- Add a disposable git-backed target app helper.
- Confirm the daemon starts with an isolated runtime root and static root.

Acceptance:

- `test-smoke-ai` passes without external provider credentials.
- `test-cli` can start the daemon, attach the app, and run `system status`.
- `test-ui` can open the app shell and produce failure artifacts.

### Phase 2: Port Existing Rust Surface Coverage

- Port `../refine-test/tests/rust_cli` into Cargo integration tests.
- Port `../refine-test/tests/rust_ui` into repo-local Playwright specs.
- Remove Python and `uv` from the Rust surface path.
- Keep the old sibling tests until the new commands cover the same behavior.

Acceptance:

- CLI coverage includes system, project, gap, feature, workflow, and node.
- UI coverage includes app shell, navigation, project status, and Gaps query.
- The sibling Rust suite is no longer the authoritative Rust test path.

### Phase 3: Expand From The UI Feature Index

- Map indexed UI features to one of: unit/contract, CLI surface, UI surface, or
  manual/extended.
- Add browser journeys for high-value user workflows.
- Add CLI tests for model-equivalent workflows.
- Add smoke-ai-backed tests for import, planning chat, provider diagnostics,
  target-app command generation, and governance/quality text generation.

Acceptance:

- Every high-priority UI feature has at least one test owner.
- Critical workflows have a browser test and lower-level coverage.
- AI-backed flows are deterministic in default CI.

### Phase 4: CI And Release Gate

- Add surface smoke to the release or pre-merge gate once stable.
- Persist artifacts on failure.
- Track runtime duration and flake rate.
- Split extended host-dependent tests into explicit manual jobs.

Acceptance:

- Surface smoke is stable enough to run unattended.
- Failures point to a product surface and retain enough evidence to debug.
- Host-dependent tests do not block ordinary local development.

## Open Questions

- Should Playwright dependencies live at repo root or under a dedicated   `tests/ui` package directory? -> repo root
- Should the Smoke AI fixture be a separate tiny Rust binary, an `xtask`   subcommand, or an executable script during the first migration? -> separate tiny binary
- Should CLI integration tests allocate a random port by default, or use a stable default with an environment override? -> Use a stable default port, 18080
- Should `cargo test` include the CLI surface tests by default, or should those be gated behind an ignored test or `xtask` command because they start a daemon? -> Include CLI surface tests by default
- Which CI job should own Playwright browser installation and cache priming?

## Recommendation

Implement the harness in this repo and keep the surface boundary strict:

- Rust owns CLI integration tests and fixture orchestration.
- Playwright TypeScript owns browser UI tests.
- Smoke AI remains an external executable provider launched through the real
  provider path.
- `xtask` is the public runner for slower integration suites.

This gives Refine a native Rust testing path without giving up Playwright's UI
testing strengths or the black-box discipline that made `refine-test` useful.
