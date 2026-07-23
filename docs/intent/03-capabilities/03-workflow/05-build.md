# Rebuild

## Key Ideas

- **Post-Integration Check**: rebuild verifies the integrated target app can still be assembled or prepared.
- **Target-App Specific**: build behavior should use target-app lifecycle instructions and settings.
- **Evidence Producing**: build output should become part of the Goal's evidence.

## Purpose

Rebuild exists to test whether an integrated change still fits the target app's operational reality. It runs after Ready Merge against the configured target-branch checkout, never the isolated candidate worktree.

## Expected Role

Build should run through shared target-app lifecycle capability and target-app configuration. It should expose agent output, check evidence, and failure details.

Build should not be hardcoded to one ecosystem. Different target apps may need different build instructions, setup recovery, or no build step at all.

## What Happens

When a Goal is in build:

- Refine first requires durable Ready Merge integration evidence.
- Refine uses target-app configuration to determine whether rebuild instructions or equivalent preparation guidance exists.
- If build instructions exist, Refine asks the configured agent to perform the build work through the shared lifecycle capability.
- Agent output, check context, output paths, status, and timing become evidence.
- A successful build moves the Goal toward QA or review.
- A failed build should preserve actionable output and route the Goal to failed, retry, or a new implementation round.
- If no rebuild step applies, workflow records an explicit successful skip and continues according to the round's pinned Quality timing.

## Future Direction

Future build behavior should become more inferential and adaptive. Agents may infer build instructions, detect missing configuration, summarize failures, and propose fixes while preserving the build evidence that drove those actions.
