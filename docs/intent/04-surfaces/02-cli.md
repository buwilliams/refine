# CLI

## Key Ideas

- **Reliable Surface**: the CLI should be dependable, scriptable, and low-state.
- **Daemon Routed**: normal product mutations should go through the local daemon so all surfaces share authority.
- **JSON Friendly**: command output should be inspectable by people and machines.
- **Operational Escape Hatch**: install, repair, update, diagnostics, lifecycle, and local system commands need a stable terminal surface.
- **Agent Compatible**: agents should be able to use the CLI when it is the most direct and robust interface.

## Purpose

The CLI exists for reliable operation. It should let users and agents start, stop, inspect, repair, install, update, attach projects, manage work, query diagnostics, and automate flows without depending on browser state.

The CLI is not meant to bypass the product model. It should expose the same shared capabilities as other surfaces.

## Expected Role

The CLI should be the most stable surface for automation and system control. Browser state can be refreshed or lost, and future agent surfaces may evolve quickly. The CLI should remain a compact way to operate Refine from the host environment.

Current implementation details that matter to intent:

- command groups include config, project, sync, goal, feature, Todo, workflow, node, fleet, log, agent, and system.
- `refine config` reads Settings, Quality, Governance, and Guidance together or by domain. Its domain subcommands accept concise scalar and multiline flags or one JSON object supplied inline, by file, or through stdin; successful writes print the daemon's authoritative pretty-JSON readback. Configuration does not absorb project attach/switch, workflow pause/resume, node or fleet management, Reporter/Todo lifecycle, or agent authentication, which remain in their named groups.
- normal config calls use the active checkout-owned daemon, API contract version, and mutation idempotency key. Detached apps, unreachable daemons, invalid payloads, missing entries, and stale collection revisions remain structured errors suitable for automation. Explicit target roots exist only as hidden test adapters.
- `fleet manage "<request>"` opens an agent session seeded with the manage-fleet runbook so fleet changes are conversational; `fleet "<request>"` and `fleet distribute "<instructions>"` reach the same session. The other fleet commands remain the deterministic primitives the agent acts through.
- `refine commands` emits the supported user-facing command tree as
  machine-readable JSON, while `refine next` recommends commands from current
  state. Hidden worker entry points and checkout-launcher-only operations are
  omitted from the catalog. These live surfaces are authoritative instead of a
  committed generated command snapshot.
- Todo commands require an explicit Reporter and route list and item operations through the shared daemon API and `FileTodoService`, returning the same machine-usable JSON as other Todo surfaces.
- `goal draft` turns Plan text into exactly one reviewable, unpersisted Goal draft through the shared import-extraction API.
- `goal approve` is the only CLI command for accepting a Goal after it reaches Review; obsolete verification and merge aliases are not retained.
- `goal resolve-merged <id>` is the supported operator recovery for a Quality Goal whose authoritative current-Round candidate was already integrated. It routes through the daemon to the same idempotent, fail-closed resolver used by workflow automation and does not weaken Review-only approval.
- `agent open` starts a general Agent by default. `--profile goal <goal-id>`
  attaches the current terminal to the workflow-owned Goal Agent, while
  `--profile plan` and `--profile standalone` open those role sessions. Ctrl-]
  detaches without stopping the agent.
- normal target-state mutations are routed to the daemon instead of directly writing files in normal operation.
- `sync` is the single top-level state-convergence command; it replaced
  `project sync` and the `project state-recovery` subtree with no aliases.
  Daemon-routed `sync` and `fleet sync` follow their durable operation
  through success, failure, cancellation, interruption, or timeout. Success
  prints the terminal structured result; every other terminal outcome is
  nonzero and retains the structured reconciler error, the stable conflict
  report id, the node-local report path, and per-path domain-terms summaries
  instead of returning an initial `running` receipt.
- `sync --preview` is a read-only divergence summary — classification, both
  heads and the merge base, per-path sides, and a domain-terms summary per
  contested path. It writes nothing, exits nonzero on error having written
  nothing, and is never a token handed to another command.
- `sync --authority live|remote` is terminal recovery — sync with a decision
  attached: every contested path takes the chosen side inside one merge
  commit, and repeated `--path` exceptions settle named contested paths on
  the opposite side. Rerunning after success finds converged heads and is a
  no-op. Bounded races against a moving remote head are retried inside the
  command; callers never wrap it in retry loops, and every non-race failure
  surfaces immediately as itself. The daemon runs the merge-base ownership
  policy automatically after a reported conflict unless the node sets
  `state_sync_auto_recovery: off`, so ordinary syncing needs no CLI.
- every stateful command derives its product home from the invoked checkout-owned binary and uses only that checkout's `run/<port>` tree. Running the command from another checkout cannot redirect ownership. Explicit absolute runtime paths fail closed unless they are the exact canonical checkout runtime; isolated tests use explicit test-only adapters rather than production fallbacks.
- system commands handle daemon lifecycle, port-scoped OS service registration and removal, repair, rollback, doctor, and API group discovery. They are thin callers of the same port-scoped host lifecycle and installation capabilities used by HTTP/API, update, and maintenance paths. `system service-install` and `system service-uninstall` name the service-manager effect directly; the retired `system install` and `system uninstall` spellings are neither parsed nor advertised. Checkout-only production-binary maintenance remains owned by `./r system build` and `./r system clean`, while source updates remain owned by `./r system update`; these are launcher operations rather than production-binary subcommands. The shared lifecycle authority selects activated systemd or launchd control versus direct-process fallback, reconciles durable state with fresh post-control reachability, keeps command failures visible without replacing a still-reachable daemon's healthy state, fails closed on unreachable or ambiguous observations with partial recovery evidence, and preserves restart-specific evidence. It reports stopped only after shutdown is confirmed. Explicit foreground and one-request starts remain direct bootstrap paths. launchd labels are installation-port scoped, while a recorded legacy registration is migrated or controlled only when exact parsed arguments prove that it belongs to the selected installation; adjacent textual ports never count as ownership.
- current installation targets are daemon-oriented (`macos_daemon`, `windows_daemon`, and `linux_cli_web`). Historical target spellings are deserialize-only migration aliases. When the production binary is missing, `./r system service-install` bootstraps the locked release from the invoked product home, atomically publishes the stable `bin/refine` executable and deployed marker, and only then performs ordinary service conflict detection and registration. An existing binary is never rebuilt as a side effect of service registration, and the command never fetches or updates source. A bootstrap or publication failure leaves service state untouched. `./r system service-uninstall` stops and removes only the selected port's service registration. Explicit repair backs up exact legacy registration bytes and parsed identity in a retained port-scoped journal before atomically publishing a checkout-local registration, and restores the original registration if activation or verification fails. External runtime and binary trees are never merged, overwritten, or deleted.
- source status reads the same hourly cached identity used by the browser and API; refresh queues one coalesced supervised fetch. Source upgrade launches the configured installed Agent outside Goal claims, and its hidden granular capability/helper commands carry the durable operation and handoff-attempt identifiers. The raw claim nonce is process fencing only and is never returned in command output or public status.
- CLI tests verify daemon routing and shared service behavior.

The CLI should avoid becoming a second implementation of Refine. It should remain a reliable adapter to the same daemon, model, workflow, process, and tool capabilities.

## Future Direction

The CLI should become increasingly useful to agents. Future agents may prefer structured CLI calls for discoverability, reproducibility, and low visual overhead.

As AI systems improve, the CLI should expose high-signal operations and machine-readable output without requiring a human to click through the browser. It should remain conservative in surface area: add commands when they express real capabilities, not when they duplicate a page.
