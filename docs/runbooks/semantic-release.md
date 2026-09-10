# Prepare and publish a semantic release

Use this runbook for a normal Refine release. Preparation is local and
reviewable; publication mutates Git and GitHub and always requires explicit
confirmation.

## Preconditions

- The Refine release repository is a Git checkout with a clean trusted target
  branch (normally `main`).
- The package version is a three-part semantic version.
- Completed Goals and commits intended for the release have landed.
- Publication credentials and the target branch's configured upstream remote
  are available before the publish phase.

## Prepare

1. Configure the [Release Skill](skills/release.json) in **Settings → Skills**.
2. Open **Controls → Skills → Release** and enter the requested version change or operation.
3. The Skill uses the shared release commands to preview the version, commits, affected files, and gates. Ask it to prepare the release when ready.
4. Preparation creates a normal Goal with a managed worktree, visible workflow state, and agent logs.
5. Review and approve that Goal normally. Governance integrates the exact candidate before Review; preparation never tags or publishes.

The supported CLI acceptance command, after the Goal reaches Review, is:

```text
refine goal approve <goal-id>
```

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

After the candidate is integrated, reviewed, and approved, explicitly ask the Release Skill to publish the retained preparation ID. Refine rejects publication unless:

- the current branch is the clean target branch recorded by the trusted
  preparation (normally `main`);
- that local branch and its configured upstream branch are synchronized;
- the approved preparation commit is an ancestor of the synchronized target
  branch (its normal no-fast-forward merge commit may be branch HEAD);
- the package version and proposed semantic tag align;
- any existing local tag, remote tag, or GitHub release resolves to the
  expected synchronized target-branch commit; and
- GitHub credentials work.

Publication tags synchronized target-branch HEAD, creates or validates the tag
and GitHub release stage by stage, waits for relevant workflow runs to finish,
and verifies the final remote tag and release URL. If no deployment or package
workflows are configured, the operation records that explicitly. Deployment
success and GitHub Release publication are separate evidence and must both be
reported accurately.

For CLI publication, retain the persisted preparation operation id returned by
`release-prepare`, then run:

```text
refine system release-publish --preparation-id <operation-id> --confirm --repo-root .
```

If preparation fails, retry its linked Goal without discarding review edits. If
publication fails or is interrupted, ask the Skill to inspect the persisted operation and use `POST /api/system/releases/{operation_id}/retry` with `{"confirmed":true}` only after publication is authorized. Refine validates completed external stages and continues from the first missing stage.
