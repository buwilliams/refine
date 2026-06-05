"""refine CLI entry point.

Provides the Typer-backed `refine` command suite, including `target`, `install`,
`uninstall`, `start`, `restart`, `stop`, `status`, `ps`, `server`, `ui`, and
`doctor` — launchable via `./r <subcommand>` from a source checkout.

Components live in the sibling packages:
- refine_server: storage + backend method names + config + friendly summaries
- refine_ui:    host-native UI control surface
"""
