# Runbooks

Task-oriented guides for operating Refine, written to be followed by an AI
agent acting on a user's behalf (they work fine for people too). Each runbook
states its preconditions, the questions to ask the user before acting, the
commands to run, how to verify the outcome, and how to undo it.

Two commands make Refine self-navigating — reach for them before reading any
source code:

- `refine next` — inspects the current project and fleet state and recommends
  the next operations, each with the exact command to run. Call it whenever
  you are unsure what to do; call it again after acting.
- `refine commands` — a machine-readable JSON catalog of supported
  user-facing production-binary commands with descriptions. Hidden worker
  entry points and checkout-launcher operations are intentionally omitted.
  Load it once instead of exploring `--help` per subcommand.

Runbooks:

- [Install Refine](install.md) — install or update Refine, configure an agent
  provider, start the daemon, and verify the result.
- [Operate development-request email intake](development-request-email.md) —
  connect the Fastmail `goal@getrefine.dev` mailbox to the active project,
  verify queued intake, automatic approval, and threaded resolution replies.
- [Update Refine from source](update-refine-source.md) — safely build,
  fast-forward, and restart a running Refine source checkout from the UI or CLI.
- [Prepare and publish a release](semantic-release.md) — preview a semantic
  increment, prepare and review the candidate, then explicitly publish it.
- [Manage the fleet](manage-fleet.md) — inspect the fleet, add and bootstrap
  workers on any infrastructure, move work, and retire nodes; Refine owns node
  identity and work while the agent owns the machines.
- [Distribute and converge work](distribute-and-converge.md) — move Goals to
  workers and bring reviewable work home.
- [Accelerate Goal builds](accelerate-goal-builds.md) — share a compile cache
  across Round worktrees through the node's shell environment so fresh Rounds
  stop paying cold builds.
- [Recover a state-sync conflict](state-sync-recovery.md) — read the
  divergence preview and settle contested paths terminally with default and
  per-path authority, without editing synchronized state by hand.
- [Migrate Gap state to Goals](migrate-gap-state.md) — preserve intent through
  the agent-operated schema migration.
- [Migrate a Refine v2 project to current v4](v2-to-v4-migration-runbook.md) —
  preserve legacy durable state and node-local evidence in the current v4
  layout and isolated state branch.
- [Migrate a node to the scale and reliability layout](scale-reliability-migration.md)
  — relocate node-local logs, retire derived state, and restore host-governed
  concurrency after upgrading an existing v4 node.

Conventions: commands are shown as `refine …`; inside a source checkout use
`./r …`, which delegates to the same production-binary surface. The exceptions
are launcher-owned `./r system build` and `./r system clean`, documented in the
install runbook. Use `--dry-run` only when a command's CLI entry documents it.
Currently, use dry-run before `fleet distribute` and `fleet bootstrap`; do not
invent a dry-run flag for transfer, enable/disable, maintenance, or removal
commands.
