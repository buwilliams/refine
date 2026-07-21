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
- `refine commands` — a machine-readable JSON catalog of every CLI command
  with descriptions. Load it once instead of exploring `--help` per
  subcommand.

Runbooks:

- [Install Refine](install.md) — install or update Refine, configure an agent
  provider, start the daemon, and verify the result.
- [Promote dogfood source](promote-dogfood-source.md) — safely build,
  fast-forward, and restart a running Refine source checkout from the UI or CLI.
- [Provision a fleet worker](provision.md) — create and verify a worker using
  provider tools while Refine owns node identity and work.
- [Distribute and converge work](distribute-and-converge.md) — move Goals to
  workers and bring reviewable work home.
- [Migrate Gap state to Goals](migrate-gap-state.md) — preserve intent through
  the agent-operated schema migration.
- [Migrate a Refine v2 project to v4](v2-to-v4-migration-runbook.md) — move
  legacy durable state into the v4 layout and isolated state branch.

Conventions: commands are shown as `refine …`; inside a source checkout use
`./r …`, which is the same surface. All cluster/fleet commands accept
`--dry-run` where they change external state — prefer a dry run first and
show the user what would happen.
