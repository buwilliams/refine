# Prepare and publish a semantic release

Use this runbook for a normal Refine release. Preparation is local and
reviewable; publication mutates Git and GitHub and always requires explicit
confirmation.

## Preconditions

- The target app is a Git checkout with a clean base branch.
- The package version is a three-part semantic version.
- Completed Goals and commits intended for the release have landed.
- Publication credentials and the `origin` remote are configured before the
  publish phase.

## Prepare

1. Open **Node > Releases** (the System release surface).
2. Select major, minor, or patch and choose **Preview**.
3. Review the current/proposed versions, previous tag, commits, breaking-change
   findings, affected files, and deterministic gates.
4. Choose **Prepare release**. Watch the persisted stage and agent activity.
5. Review the resulting `release/vX.Y.Z` branch and candidate commit. The
   preparation worktree is under the runtime root; the main checkout is not
   switched.
6. Review and merge the candidate normally.

CLI equivalents:

```text
refine system release-plan --bump patch --repo-root .
refine system release-prepare --bump patch --repo-root .
```

Repository automation can run the same deterministic preflight:

```text
cargo run --manifest-path xtask/Cargo.toml -- release-plan patch
cargo run --manifest-path xtask/Cargo.toml -- release-check
```

## Publish

Return to Releases after the candidate is reviewed and merged. Choose
**Publish release…** and explicitly confirm. Refine rejects publication unless:

- the current branch is clean `main`;
- local `main`, `origin/main`, and the reviewed candidate commit match;
- the package version and proposed semantic tag align;
- the tag does not already exist; and
- GitHub credentials work.

Publication creates and pushes the annotated tag, creates the GitHub release
from the prepared notes, and verifies that the release has a published URL.
Repository workflows remain responsible for deployment and package delivery;
their work begins from the pushed tag.

For CLI publication, save the `candidate` object returned by preparation as
JSON, then run:

```text
refine system release-publish --candidate candidate.json --confirm --repo-root .
```

If an operation fails or is interrupted, use **Retry / resume**. Retrying a
publish operation asks for confirmation again.
