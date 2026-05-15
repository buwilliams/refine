"""refine CLI entry point.

Provides `refine init`, `refine start`, `refine stop`, `refine status`,
`refine runner`, `refine web`, and `refine doctor` — launchable via
`uv run refine <subcommand>`.

Components live in the sibling packages:
- refine_shared: storage + backend method names + config + friendly summaries
- refine_runner: host-native backend runner
- refine_web:    host-native web backend
"""
