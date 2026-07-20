# Migrate a Refine v2 Project to v4

This runbook is for an agent migrating a target application that was managed
by Refine v2.x. It is a semantic migration: do not replace it with a
deterministic field-renaming script. Refine v4 intentionally refuses to attach
to incompatible state until an agent has preserved and verified its meaning.

Product version and project schema version are different. Refine v2.3 and
Refine v4 can both report project schema version `2`, while their durable
layouts differ. Inspect the files; do not infer compatibility from the number
alone.

## Outcome

- Durable project state remains at `<app>/.refine/`, represented in the v4
  Goal, Feature, node, governance, guidance, and reporter shapes.
- Local process state is recreated in Refine's port-scoped runtime root and is
  not copied into durable state.
- The migrated `.refine` tree is published on `origin/refine/state` through
  Refine's isolated state worktree.
- The application branch, application files, index, and HEAD are unchanged by
  the migration.
- Every upgraded node can attach, synchronize, and show equivalent work and
  configuration.

## Stop conditions

Stop and request project-owner judgment if any of these are true:

- A v2 or v4 daemon is still running against the project.
- An agent, merge, rebase, or application deployment is in progress.
- `origin/refine/state` already exists and does not represent this same
  migration.
- The application worktree has changes that cannot be attributed and safely
  left untouched.
- A legacy record, setting, ownership reference, or status has no clear v4
  meaning.
- Verification changes the application branch or any non-Refine source file.

Never force-push, rewrite application history, or delete the legacy
application-branch copy of `.refine` during this migration. Refine v4 makes
legacy tracked state inert locally while `refine/state` becomes authoritative.

## Preconditions and evidence

1. Install the v4 release by following `docs/runbooks/install.md`, but do not
   start workflow automation for this project.
2. Stop every old and new Refine node that can access the project. Record how
   each node was stopped.
3. Record the application repository's current branch, exact HEAD, remotes,
   worktree status, and whether `.refine` is tracked.
4. Inspect `.refine/refine.toml`, `.refine/config.json`, `.refine/gaps/`,
   `.refine/features/`, `.refine/nodes.json`, `.refine/nodes/`, and all other
   JSON or JSONL evidence. Count records by type and record their ids.
5. Check whether `origin/refine/state` exists. If it does, fetch and inspect it
   before changing local state; do not assume the local v2 tree wins.
6. Make a byte-for-byte backup outside the application repository and outside
   `.refine`. Include a manifest with file paths and checksums. Do not put the
   backup under `.refine/backups`, because durable-state synchronization would
   publish it.

## Layout translation

Use this as a routing map, then inspect the current v4 models and services for
the exact destination shape.

| Refine v2 state | Refine v4 destination | Agent responsibility |
| --- | --- | --- |
| `.refine/refine.toml` | installation/runtime configuration, not durable project state | Preserve its target-app and port intent in the installed v4 configuration; do not copy the TOML into the new schema. |
| `.refine/config.json` | `.refine/refine.json` plus current governance and node settings | Create valid v4 project metadata with `schema_version: 2` and Refine `4.0.0`; translate settings into their current owners instead of copying obsolete keys blindly. |
| `.refine/gaps/<shard>/<id>/gap.json` | `.refine/goals/<shard>/<id>/goal.json` | Preserve ids, order, ownership, status, priority, branch, timestamps, notes, and evidence. Follow `migrate-gap-state.md` for round prompt synthesis. |
| Gap sibling logs and evidence | Goal sibling logs and evidence | Copy without rewriting content; repair only paths or references that changed from Gap to Goal. |
| `.refine/features/**/feature.json` | `.refine/features/**/feature.json` | Preserve feature ids and ordering; update Gap-named membership fields or references to the corresponding Goal ids. |
| `.refine/nodes.json` and `.refine/nodes/<id>/{application,runtime,target-app}.json` | `.refine/nodes.json` | Preserve node identity and metadata, merge supported per-node settings into each node's `settings` object, and retain unknown meaningful values for explicit review. Apply v4 defaults for new transport fields without inventing credentials. |
| `.refine/nodes/<id>/reporters.json` | `.refine/reporters.json` | Merge reporters by stable identity/name, resolve collisions deliberately, and verify every Goal and Feature reporter reference. |
| v2 project governance settings | `.refine/governance.json` | Preserve product, constitution, and rules semantically. Do not discard requirements that no longer have a one-to-one setting key. |
| `.refine/guidance.json` | `.refine/guidance.json` | Normalize to the v4 guidance list while preserving enabled state and instructions. |
| `target_app_rebuild_*` and `target_app_auto_rebuild*` settings | `target_app_build_*` and `target_app_auto_build*` | Use the current names while preserving commands, instructions, timeouts, and cadence. |
| SQLite indexes, caches, PID/socket files, process logs, maintenance flags, and `.refine/run/` | v4 port-scoped runtime root | Do not migrate. These are local, derived, or stale process state and v4 recreates them. |

Do not migrate provider credentials, API keys, SSH private keys, environment
secrets, or host authentication material into `.refine` or Git.

## Agent procedure

1. Work from the external backup or another disposable copy while composing
   the migration. Keep the live v2 tree unchanged until the destination passes
   structural and semantic review.
2. Create `refine.json`, the normalized node registry, and the current
   governance, guidance, and reporter files.
3. Migrate Features and Goals. For each legacy round, synthesize one `prompt`
   from `actual`, `target`, surrounding evidence, and project context. Never
   concatenate those fields mechanically.
4. Preserve sidecar logs and cross-record references. Confirm that every Goal
   owner exists, every Feature member exists, and every reporter reference can
   be resolved.
5. Remove runtime-only artifacts from the destination. Preserve them only in
   the external backup when they are useful for audit evidence.
6. Replace the live `.refine` tree only after the destination is internally
   consistent. Keep the backup untouched.
7. Attach the application with v4 and run `refine project status`. If Refine
   still reports a migration requirement, inspect and correct the state; do
   not invoke deterministic migration code to bypass the check.
8. Activate or confirm the intended node, then review settings through the
   shared Refine surface. Correct unsupported or renamed values explicitly.
9. Run `refine project sync`. This initializes or reconciles the isolated
   `refine/state` branch without moving the application branch.
10. Inspect `origin/refine/state`, then bring up one v4 node. Add remaining
    nodes one at a time, synchronizing and verifying each before enabling work.

## Verification

- `refine project status` reports `compatible: true`, schema version `2`, and
  no migration requirement.
- The application branch and HEAD equal the values recorded before migration;
  no application source file changed.
- Source and destination counts match for Goals, Features, rounds, notes, and
  evidence files. Any intentional count difference is explained in the
  migration report.
- Every Goal and Feature opens with correct ownership, reporter, ordering,
  workflow status, and history.
- Governance, guidance, quality behavior, provider choice, target-app
  lifecycle instructions, and test commands retain their intended behavior.
- `origin/refine/state` contains durable `.refine` files and excludes runtime
  directories, caches, logs, credentials, and the external backup.
- A second v4 node can synchronize the state and reports the same durable
  records without an application-branch commit or checkout.
- A no-op `refine project sync` creates no additional commit or push.

Write a migration report containing the backup location and checksum manifest,
the before/after counts, settings that required judgment, commands run,
verification evidence, and the resulting `refine/state` commit.

## Rollback

Stop all v4 nodes. Restore the external backup atomically to the application
only after preserving the failed migrated tree for diagnosis. Do not delete or
force-push `refine/state`; correct the migration in a new attempt and publish a
normal follow-up state commit. Restore the original Refine version only if the
project owner explicitly chooses to resume v2 operation.
