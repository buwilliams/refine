Prepare the strongest complete semantic release candidate supported by this ReleasePlan and repository evidence.

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

Treat the plan as a map and inspect the actual commits, files, and documentation for blind spots. Update every affected version and lockfile, explain user-visible changes, preserve established documentation formats, and add migration guidance for real breaking changes. Run `cargo run --manifest-path xtask/Cargo.toml -- release-check` and report its exact outcome. Do not tag, push, create a GitHub release, or publish externally. Leave the worktree ready for review.
