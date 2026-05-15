"""refine CLI entry point.

Provides `refine init`, `refine start`, `refine stop`, `refine status`,
`refine server`, `refine ui`, and `refine doctor` — launchable via
`uv run refine <subcommand>`.

Components live in the sibling packages:
- refine_shared: storage + backend method names + config + friendly summaries
- refine_server: host-native backend server component
- refine_ui:    host-native UI backend
"""
