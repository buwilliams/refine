# Migrate Gap State to Goals

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
- Confirm that `.refine/gaps/` exists and `.refine/goals/` does not.
- Create a recoverable copy of `.refine/` outside the application repository
  and outside `.refine`, with a path and checksum manifest recorded in the
  migration report.
- Do not modify application source files or Git history as part of migration.

## Agent procedure

1. Read each `gap.json` together with its sibling logs, chats, feature context,
   and round evidence. Treat the content as product intent, not a field-renaming
   exercise.
2. Create the corresponding `goal.json` under the same relative hierarchy in
   `.refine/goals/`. Preserve stable ids, ordering, ownership, status, branch,
   timestamps, and evidence references.
3. For every round, write one `prompt` that faithfully communicates the desired
   outcome and relevant current behavior. Use `actual` and `target` as evidence,
   but compose the prompt in context; do not concatenate them mechanically.
4. Copy sibling evidence files without rewriting their contents. Check every
   cross-reference after the move.
5. Compare source and destination counts and inspect every migrated Goal. If any
   intent is ambiguous, stop and request project-owner judgment.
6. Remove `.refine/gaps/` only after all checks pass. Update
   `.refine/refine.json` to `schema_version: 2` while preserving its other
   settings and metadata, then invalidate runtime projection caches.

## Verify

- `refine project status` reports a compatible schema and no migration needed.
- Goal and Feature counts match the legacy records.
- Each Goal opens with its rounds, logs, ownership, and workflow status intact.
- No application files changed.

If verification fails, restore the backed-up `.refine/` tree before resuming
any node.
