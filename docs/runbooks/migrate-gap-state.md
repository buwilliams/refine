# Migrate Gap State to Goals

In v4, every durable `.refine` path below is relative to a disposable migration
stage and then the authoritative live projection at
`<git-common-dir>/refine-live-state/`. Synchronization mirrors durable files to
the isolated `<git-common-dir>/refine-state-worktree/` checkout of
`refine/state`; do not edit that worktree directly and do not create
`<app>/.refine` in the primary target-app worktree except for the temporary
untracked attach handoff described by the complete v2-to-v4 runbook. Goal logs
are node-local under `<git-common-dir>/refine-live-state/runtime/goals/` and are
not synchronized.

Use this runbook only when Refine reports that an attached project requires the
`goals-prompt-1-to-2` migration. This is an agent-operated semantic migration,
not a user workflow and not a deterministic Refine application transform.
For a complete Refine v2 product upgrade, follow
`docs/runbooks/v2-to-v4-migration-runbook.md`; this document covers only its
Gap-to-Goal portion.

## Outcome

The project retains the meaning and evidence of every legacy Gap while using
the current Goal schema. Refine can attach to the project at schema version 2.

## Preconditions

- Stop workflow execution on every node that uses the project.
- Confirm that `gaps/` exists and `goals/` does not in the source state tree.
- Create a recoverable copy of the source state outside the application
  repository and every Refine state directory, with its path and checksum
  manifest recorded in the migration report.
- Do not modify application source files or Git history as part of migration.

## Agent procedure

1. Read each `gap.json` together with its logs, chats, feature context,
   and round evidence. Treat the content as product intent, not a field-renaming
   exercise.
2. Create the corresponding `goal.json` under the same relative hierarchy in
   staged `goals/`. Preserve stable ids, ordering, ownership, status, branch,
   timestamps, and evidence references.
3. For every round, write one `prompt` that faithfully communicates the desired
   outcome and relevant current behavior. Use `actual` and `target` as evidence,
   but compose the prompt in context; do not concatenate them mechanically.
4. Preserve supported durable evidence in current Goal fields. Move each legacy
   `logs.jsonl` to `runtime/goals/<shard>/<id>/logs.jsonl` in the live-state
   staging tree; never place it beside `goal.json` or publish it to
   `refine/state`. Keep unrecognized sidecars in the external backup rather
   than inventing a new durable format. Check every cross-reference.
5. Compare source and destination counts and inspect every migrated Goal. If any
   intent is ambiguous, stop and request project-owner judgment.
6. Remove staged `gaps/` only after all checks pass. Update staged
   `refine.json` to `schema_version: 2` while preserving its other settings and
   metadata, then complete the attach/sync handoff in the v2-to-v4 runbook.

## Verify

- `refine project status` reports a compatible schema and no migration needed.
- Goal and Feature counts match the legacy records.
- Each Goal opens with its rounds, ownership, and workflow status intact. On the
  migration node, its logs resolve from `runtime/goals/`; another node is not
  expected to receive node-local logs through `refine/state`.
- No application files changed.

If verification fails, preserve the failed staging tree for diagnosis and
restore from the external backup before resuming any node. Do not overwrite an
unreconciled live-state projection or state worktree.
