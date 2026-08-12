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

- command groups include project, goal, feature, Todo, workflow, node, fleet, log, agent, and system.
- `fleet manage "<request>"` opens an agent session seeded with the manage-fleet runbook so fleet changes are conversational; `fleet "<request>"` and `fleet distribute "<instructions>"` reach the same session. The other fleet commands remain the deterministic primitives the agent acts through.
- `refine commands` emits the current command tree as machine-readable JSON,
  while `refine next` recommends commands from current state. These live
  surfaces are authoritative instead of a committed generated command snapshot.
- Todo commands require an explicit Reporter and route list and item operations through the shared daemon API and `FileTodoService`, returning the same machine-usable JSON as other Todo surfaces.
- `goal draft` turns Plan text into exactly one reviewable, unpersisted Goal draft through the shared import-extraction API.
- `agent open` starts a general Agent by default. `--profile goal <goal-id>`
  attaches the current terminal to the workflow-owned Goal Agent, while
  `--profile plan` and `--profile standalone` open those role sessions. Ctrl-]
  detaches without stopping the agent.
- normal target-state mutations are routed to the daemon instead of directly writing files in normal operation.
- system commands handle lifecycle, install, repair, update, rollback, uninstall, doctor, and API group discovery. They are thin callers of the same port-scoped host lifecycle and installation capabilities used by HTTP/API, update, and maintenance paths. The shared lifecycle authority selects activated systemd or launchd control versus direct-process fallback, reconciles durable state with fresh post-control reachability, keeps command failures visible without replacing a still-reachable daemon's healthy state, fails closed on unreachable or ambiguous observations with partial recovery evidence, and preserves restart-specific evidence. It reports stopped only after shutdown is confirmed. Explicit foreground and one-request starts remain direct bootstrap paths. launchd labels are installation-port scoped, while a recorded legacy registration is migrated or controlled only when exact parsed arguments prove that it belongs to the selected installation; adjacent textual ports never count as ownership.
- CLI tests verify daemon routing and shared service behavior.

The CLI should avoid becoming a second implementation of Refine. It should remain a reliable adapter to the same daemon, model, workflow, process, and tool capabilities.

## Future Direction

The CLI should become increasingly useful to agents. Future agents may prefer structured CLI calls for discoverability, reproducibility, and low visual overhead.

As AI systems improve, the CLI should expose high-signal operations and machine-readable output without requiring a human to click through the browser. It should remain conservative in surface area: add commands when they express real capabilities, not when they duplicate a page.
