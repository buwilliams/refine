Prepare the reviewable semantic release candidate described by this trusted ReleasePlan.

Current version: {{current_version}}
Proposed version: {{proposed_version}}
Proposed tag: {{proposed_tag}}
Previous tag: {{previous_tag}}
Version-bearing files detected: {{version_files}}
Documentation files detected: {{documentation_files}}

Completed Goals:
{{completed_goals}}

Commits since the prior release:
{{changes}}

Analyze the completed Goals and commits. Update every applicable version-bearing file and lockfile, write release notes, preserve established documentation formats, and update story, runbooks, migration, or other affected documentation only where the actual changes require it. Identify breaking changes and write migration guidance when needed. Run `cargo run --manifest-path xtask/Cargo.toml -- release-check` and report deterministic command outcomes. Do not tag, push a tag, create a GitHub release, or publish externally. Use this normal Goal worktree and leave the candidate ready for the standard review and approval workflow.
