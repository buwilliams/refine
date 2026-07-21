# Prepare and publish a semantic release

Use this runbook for a normal Refine release. Preparation is local and
reviewable; publication mutates Git and GitHub and always requires explicit
confirmation.

## Preconditions

- The target app is a Git checkout with a clean base branch.
- The package version is a three-part semantic version.
- Completed Goals and commits intended for the release have landed.
- Publication credentials and the target branch's configured upstream remote
  are available before the publish phase.

## Prepare

1. Open **Node > Releases** (the System release surface).
2. Select major, minor, or patch and choose **Preview**.
3. Review the current/proposed versions, previous tag, commits, breaking-change
   findings, affected files, and deterministic gates.
4. Choose **Prepare release**. Refine creates and queues a normal visible Goal
   whose prompt contains the trusted release plan.
5. Follow the linked Goal's real workflow state and agent logs. Its worktree is
   managed by the normal Goal workflow under `.git/refine-worktrees`.
6. Review and approve the Goal normally. Preparation never tags or publishes.

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
- local `main` and its configured upstream branch are synchronized;
- the approved preparation commit is an ancestor of `main` (the normal
  no-fast-forward merge commit may be `main` HEAD);
- the package version and proposed semantic tag align;
- any existing local tag, remote tag, or GitHub release resolves to the
  expected synchronized `main` commit; and
- GitHub credentials work.

Publication tags synchronized `main` HEAD, creates or validates the tag and
GitHub release stage by stage, waits for relevant workflow runs to finish, and
verifies the final remote tag and release URL. If no deployment or package
workflows are configured, the operation records that explicitly.

For CLI publication, retain the persisted preparation operation id returned by
`release-prepare`, then run:

```text
refine system release-publish --preparation-id <operation-id> --confirm --repo-root .
```

If preparation fails, retry its linked Goal without discarding review edits. If
publication fails or is interrupted, use **Retry / resume**; Refine validates
completed external stages and continues from the first missing stage. Every
publish attempt asks for confirmation again.
