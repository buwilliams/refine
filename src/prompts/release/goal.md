Prepare the semantic release candidate described by this plan and repository evidence.

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

Update affected versions and lockfiles, document user-visible changes, preserve established documentation formats, and add migration guidance for breaking changes. Run `cargo run --manifest-path xtask/Cargo.toml -- release-check` and report its outcome. Do not tag, push, create a GitHub release, or publish externally.
