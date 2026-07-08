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

- [Stand up a fleet worker](fleet-standup.md) — create, provision, and verify
  a cloud worker node.
- [Distribute and converge work](distribute-and-converge.md) — move Gaps to
  workers and bring reviewable work home.

Conventions: commands are shown as `refine …`; inside a source checkout use
`./r …`, which is the same surface. All cluster/fleet commands accept
`--dry-run` where they change external state — prefer a dry run first and
show the user what would happen.
