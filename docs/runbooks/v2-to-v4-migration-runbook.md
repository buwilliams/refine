# Migrate a Refine v2 Project to the Current v4 Release

This runbook is for an agent migrating a target application managed by Refine
v2.x directly to the installed Refine v4 release. It is a semantic migration:
preserve the meaning of every durable record instead of treating the work as a
field rename. Refine intentionally refuses to attach to incompatible state
until an agent has resolved that meaning.

Read this whole runbook before changing the project. The current node-local
layout and cleanup rules come from
[`scale-reliability-migration.md`](scale-reliability-migration.md).
Its changes are incorporated here so a v2 project migrates directly to the
current v4 layout rather than passing through an obsolete 4.0 layout.

Product version and project schema version are different. Refine v2.3 and the
current v4 release can both report project schema version `2` while using
incompatible durable layouts. Inspect the files; do not infer compatibility
from the schema number alone.

## Outcome

- The application branch no longer tracks or contains `<target-app>/.refine`.
- The live project-state projection is
  `<git-common-dir>/refine-live-state/`. The committed `.refine/` tree is checked
  out at `<git-common-dir>/refine-state-worktree/` on `refine/state`.
- `refine.json` records schema version `2` and the installed Refine version; the
  migration never hard-codes an older v4 product version.
- Durable Goals, Features, settings, governance, guidance, reporters, and other
  supported project records are published through the configured `git_remote`
  (default `origin`) on `refine/state`.
- Goal log sidecars are preserved only as node-local evidence under
  `runtime/goals/<shard>/<id>/logs.jsonl` in the live-state directory. They are
  readable on the migration node but are not published to `refine/state`.
- Process state, caches, scheduler indexes, locks, credentials, and other
  derived or host-local artifacts are recreated rather than migrated.
- Each additional v4 node can attach and synchronize the same durable project
  records without changing the application branch.

## Stop conditions

Stop and request project-owner judgment if any of these are true:

- The installed v4 binary and exact version have not been identified.
- A v2 or v4 daemon is still running against the project.
- An agent, merge, rebase, deployment, state sync, or application-branch change
  is in progress.
- The application worktree has changes that cannot be attributed and preserved.
- `<git_remote>/refine/state` already exists and is not unquestionably state for
  this same project and migration.
- Both legacy `<target-app>/.refine` and an existing
  `<git-common-dir>/refine-live-state` contain state that has not been
  reconciled.
- A legacy record, setting, ownership reference, credential reference, or
  workflow status has no unambiguous current-v4 meaning.
- Source and destination records disagree after translation.
- Verification changes any non-Refine application file.

Never force-push or rewrite application or `refine/state` history. Preserve a
byte-for-byte backup before changing legacy state. Remove tracked legacy
`.refine` from the application branch with a normal reviewed commit only after
the staged destination is internally consistent.

## 1. Record the source and derive the current paths

1. Install the intended v4 release using [`install.md`](install.md), but do not
   start workflow automation for this project. Record `refine --version` (or
   `./r --version` inside a source checkout) and use that exact product version
   in the migrated `refine.json`.
2. Stop every Refine node that can access the project and verify each process is
   actually down. For a current installation, use the supported system
   lifecycle surface, for example `refine system stop --port <port>`.
3. Record the application repository's current branch, exact HEAD, remotes,
   worktree status, and tracked legacy state:

   ```bash
   TARGET_APP=/path/to/the/target/application
   git -C "$TARGET_APP" branch --show-current
   git -C "$TARGET_APP" rev-parse HEAD
   git -C "$TARGET_APP" remote -v
   git -C "$TARGET_APP" status --short
   git -C "$TARGET_APP" ls-files -- .refine
   ```

4. Derive the shared Git directory. Do not assume it is
   `<target-app>/.git`; linked worktrees use a different Git common directory.

   ```bash
   GIT_COMMON_DIR="$(git -C "$TARGET_APP" rev-parse --path-format=absolute --git-common-dir)"
   LIVE_STATE="$GIT_COMMON_DIR/refine-live-state"
   STATE_WORKTREE="$GIT_COMMON_DIR/refine-state-worktree"
   printf '%s\n' "$GIT_COMMON_DIR" "$LIVE_STATE" "$STATE_WORKTREE"
   ```

5. Inspect `.refine/refine.toml`, `.refine/config.json`, `.refine/gaps/`,
   `.refine/features/`, `.refine/nodes.json`, `.refine/nodes/`, and every other
   JSON or JSONL record. Record counts and stable identifiers by record type.
   Separately count Goal or Gap `logs.jsonl` sidecars.
6. Record the configured `git_remote` (default `origin`). Fetch and inspect its
   `refine/state` branch if one exists; do not assume the local v2 tree wins.
7. Make a byte-for-byte backup outside the application repository and outside
   every `.refine` or Git-owned Refine directory. Include a path and checksum
   manifest. A backup beneath `.refine` can be published accidentally and is
   not acceptable.

## 2. Build a disposable destination

Compose the destination from the external backup or another disposable copy.
Do not edit the live v2 tree while translating it. The staged tree must contain
the contents that will eventually become `LIVE_STATE`.

Use this routing map, then validate each result against the current models and
services in the installed release:

| Refine v2 state | Current v4 destination | Required treatment |
| --- | --- | --- |
| `.refine/refine.toml` | installed runtime and target-app configuration | Preserve target-app, port, and lifecycle intent through supported installation/runtime settings. Do not copy the TOML into durable project state. |
| `.refine/config.json` | `refine.json`, current Node settings, and current project-owned settings | Create schema version `2`; set `refine.version` to the installed v4 version, not `4.0.0`; preserve timestamps where meaningful; translate only supported settings. |
| `.refine/gaps/<shard>/<id>/gap.json` | `goals/<shard>/<id>/goal.json` | Preserve stable ids, order, ownership, valid status, priority, branch, timestamps, notes, and durable evidence. Use [`migrate-gap-state.md`](migrate-gap-state.md) for semantic Round prompt synthesis. |
| Gap or Goal `logs.jsonl` | `runtime/goals/<shard>/<id>/logs.jsonl` | Preserve on the migration node as node-local evidence. Never place logs beside `goal.json` and never publish them on `refine/state`. |
| Other Gap sibling files | current durable Goal fields or the external backup | Move content into a current field only when the current model consumes it. Keep unrecognized evidence in the migration backup rather than inventing a synchronized sidecar format. |
| `.refine/features/**/feature.json` | `features/**/feature.json` | Preserve Feature identity, description, ownership, reporter, and ordering meaning. Update Gap references to Goal ids and verify every member. |
| `.refine/nodes.json` and `.refine/nodes/<id>/{application,runtime,target-app}.json` | `nodes.json` | Preserve node identity and supported metadata. Merge supported node-specific settings into each node's `settings`; retain unknown meaningful values in the migration report for explicit judgment. Do not invent transport credentials. |
| `.refine/nodes/<id>/reporters.json` | `reporters.json` | Merge reporters by stable identity or name, resolve collisions deliberately, and verify all Goal and Feature reporter references. |
| v2 governance configuration | `governance.json` | Preserve product, constitution, and rules semantically. Do not discard requirements merely because a current key has a different shape. |
| `.refine/guidance.json` | `guidance.json` | Normalize to the current guidance list while preserving enabled state and instructions. |
| v2 Quality checks and timing | `quality/settings.json` | Preserve enforced checks. Remove retired timing fields: Quality now always runs after Implement and before Governance. |
| `target_app_rebuild_*` and `target_app_auto_rebuild*` | `target_app_build_*` and `target_app_auto_build*` Node settings | Preserve commands, instructions, timeouts, and cadence under the current names. |
| Explicit concurrency limits | `parallel_run_cap`, `parallel_per_node_cap`, `parallel_per_provider_cap`, and `parallel_per_target_app_cap` Node settings | Preserve only limits that were deliberate operator decisions. Omit inherited or seeded defaults so the host-capacity governor remains reachable. |
| Other supported durable records | the path owned by the current service | Preserve chats, Todo lists, and similar product records only when their current service has a defined destination. Verify references rather than copying directories blindly. |
| SQLite files, caches, PIDs, sockets, process logs, maintenance flags, `.refine/run/`, support bundles, provider binaries, and temporary or lock files | nowhere | Do not migrate. They are derived, transient, secret-bearing, or node-local and current v4 recreates what it needs. |

For every legacy Round, synthesize one current `prompt` from `actual`, `target`,
surrounding evidence, and project context. Do not concatenate fields
mechanically. Preserve current durable Round evidence in the Round record when
the model has a corresponding field.

Do not migrate provider credentials, API keys, SSH private keys, environment
secrets, host authentication, or copied runtime state into the staged tree.

Do not create these derived node-local paths by hand:

- `runtime/active-goals.jsonl` — the scheduler reconstructs this index from Goal
  records and then maintains it as Goals change.
- `runtime/record-locks/` — per-record locks are created on demand.
- port-scoped projection snapshots or `cache/workflow/` — current v4 rebuilds
  projections and no longer uses per-worker project copies.
- retired project-wide mutation locks such as `.goal-mutations.lock`.

## 3. Validate before handoff

Before touching the live tree:

1. Parse every staged JSON and JSONL file.
2. Confirm every Goal owner exists, every Feature member and order is valid,
   every reporter reference resolves, and every status has current-v4 meaning.
3. Compare source and destination counts for Goals, Features, rounds, notes,
   durable evidence, and node-local Goal logs. Explain every intentional
   difference in the migration report.
4. Confirm there are no `logs.jsonl` files beneath staged `goals/`; they must be
   beneath staged `runtime/goals/` with the same shard structure.
5. Confirm the staged durable namespace contains no credentials, backup,
   runtime cache, scheduler index, process registry, or lock file.
6. Review effective settings against the current Settings surface. Record every
   renamed, omitted, defaulted, or owner-resolved setting.

If any check is ambiguous, stop. Do not use deterministic migration code or a
schema-number edit to bypass semantic review.

## 4. Hand the state to v4

1. After the staged destination passes review, remove the legacy tracked
   `.refine` tree from the application branch and commit that removal normally.
   Verify `git -C "$TARGET_APP" ls-files -- .refine` prints nothing and no
   unrelated application file changed.
2. Verify `LIVE_STATE` does not already contain unreconciled state. Place the
   staged destination temporarily at `<target-app>/.refine` as an untracked
   handoff.
3. Attach with the installed v4 binary:

   ```bash
   refine project attach "$TARGET_APP"
   refine project status
   ```

   Refine atomically moves the temporary tree to `LIVE_STATE`. Attachment
   refuses to proceed while the application branch still tracks `.refine` or
   when both legacy and live-state trees exist. Verify the physical
   `<target-app>/.refine` path is gone.
4. Activate or confirm the intended node. Read settings through the shared
   Refine surface and correct unsupported or renamed values explicitly. Leave
   concurrency caps absent unless the recorded migration evidence proves they
   were intentional.
5. Run `refine project doctor`, then `refine sync`. Sync initializes or
   reconciles `STATE_WORKTREE` and `refine/state` without checking out or moving
   the application branch. If the configured remote is absent, verify the local
   state commit and configure the remote before expecting publication.
6. Inspect `<git_remote>/refine/state`, then start one v4 node. Add other nodes
   one at a time, synchronizing and verifying each before enabling workflow
   automation.

## 5. Verify the migration

- `refine project status` reports `compatible: true`, schema version `2`, the
  expected target root, and no migration requirement.
- `refine.json` records the installed Refine version rather than an older
  hard-coded v4 version.
- The application branch contains only the reviewed legacy `.refine` removal
  commit and expected pre-existing application changes.
- `<target-app>/.refine` does not exist. `LIVE_STATE` and `STATE_WORKTREE` exist,
  and the committed `.refine` tree exists only inside `STATE_WORKTREE`.
- Durable Goal, Feature, Round, note, reporter, governance, guidance, Quality,
  and other supported record counts match the reviewed translation.
- On the migration node, a Goal with legacy history renders its logs from
  `LIVE_STATE/runtime/goals/<shard>/<id>/logs.jsonl`; no Goal sibling
  `logs.jsonl` remains.
- `refine/state` contains durable `.refine` records and excludes `runtime/`,
  `run/`, caches, logs, locks, credentials, provider binaries, support bundles,
  and the external backup.
- The scheduler creates `runtime/active-goals.jsonl` itself when it first needs
  the index. It was not copied from v2 or built by the migration agent.
- Effective concurrency uses the host-capacity governor wherever no explicit
  operator cap was preserved.
- A second v4 node synchronizes the same durable records. Node-local Goal logs
  are not expected to appear on that node through `refine/state`.
- A no-op `refine sync` creates no additional commit or push.

Write a migration report containing the installed Refine version, derived Git
and runtime roots, backup location and checksum manifest, before/after counts,
settings judgments, node-local logs preserved, commands run, verification
evidence, application removal commit, and resulting `refine/state` commit.

## Rollback

Stop all v4 nodes. Preserve the failed `LIVE_STATE`, `STATE_WORKTREE`, and
node-local runtime roots for diagnosis. Restore the external backup to
`<target-app>/.refine` only if the project owner explicitly chooses to resume v2
operation, and revert the corresponding application-branch removal commit
normally. Do not delete or force-push `refine/state`; correct a v4 migration in
a new attempt and publish a normal follow-up state commit.
