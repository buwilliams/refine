"""refine CLI entry point.

Provides `refine init`, `refine install`, `refine uninstall`, `refine start`,
`refine stop`, `refine status`, `refine server`, `refine ui`, and `refine doctor` — launchable via
`uv run refine <subcommand>`.

Components live in the sibling packages:
- refine_server: storage + backend method names + config + friendly summaries
- refine_ui:    host-native UI backend
"""
