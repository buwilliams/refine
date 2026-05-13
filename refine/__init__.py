"""refine CLI entry point.

Provides `refine init`, `refine runner`, `refine web`, `refine doctor` —
launchable via `uv run refine <subcommand>`.

Components live in the sibling packages:
- refine_shared: storage + IPC types + config + friendly summaries
- refine_runner: host-native daemon
- refine_web:    Dockerized webapp
"""
