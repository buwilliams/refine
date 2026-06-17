# Build

## Key Ideas

- **Integration Check**: build verifies that the target app can still be assembled or prepared.
- **Target-App Specific**: build behavior should use target-app commands and settings.
- **Evidence Producing**: build output should become part of the Gap's evidence.

## Purpose

Build exists to test whether a change still fits the target app's operational reality. It is not enough for an agent to edit files; the changed app should still build, compile, bundle, or otherwise prepare successfully when that applies.

## Expected Role

Build should run through shared process capability and target-app configuration. It should expose logs, exit status, command context, and failure details.

Build should not be hardcoded to one ecosystem. Different target apps may define different build commands, or no build command at all.

## What Happens

When a Gap is in build:

- Refine uses target-app configuration to determine whether a build or equivalent preparation command exists.
- If a command exists, it runs through the shared Process capability.
- Build logs, command context, output paths, exit status, and timing become evidence.
- A successful build moves the Gap toward QA or review.
- A failed build should preserve actionable output and route the Gap to failed, retry, or a new implementation round.
- If no build step applies, workflow should make that explicit rather than inventing one.

## Future Direction

Future build behavior should become more inferential and adaptive. Agents may infer build commands, detect missing configuration, summarize failures, and propose fixes while preserving the build evidence that drove those actions.
