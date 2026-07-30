# Migrate a Node to the Scale and Reliability Changes

This runbook is for an agent upgrading an existing Refine node to the build
described by [scale-reliability-performance.spec.md](scale-reliability-performance.spec.md).

The changes are deliberately not backwards compatible. Two on-disk locations
moved and two are now obsolete. Nothing here rewrites durable project state:
every Goal record, Feature record, and setting stays exactly where it is. What
moves is node-local evidence, and what is deleted is derived data that rebuilds
itself.

Read the whole runbook before starting. Verify each step rather than assuming
it worked — several steps are silent when they do nothing.

## Outcome

- Goal log sidecars live under `runtime/` inside the live state directory, where
  synchronization excludes them structurally rather than by filename.
- Per-claim projection caches are gone. These were full copies of project state,
  one per claim ever created, and nothing pruned them.
- The projection snapshot has been rebuilt, so resident Goal text is the bounded
  form rather than every note body and Round prompt.
- The retired Goal mutation lock file is gone.
- The node starts, attaches, and schedules work, and the first synchronization
  retires the Goal logs previously published to `refine/state`.

## Stop conditions

Stop and ask the project owner if any of these are true:

- The node is not on the upgraded build. Check first: a node still running the
  previous binary will recreate what you remove and write logs back to the old
  path.
- `git status` in the target application worktree shows uncommitted work you did
  not expect. This runbook does not touch the application branch, but a dirty
  worktree usually means something else is in progress.
- The live state directory does not exist at the path derived below, or contains
  no `goals/` directory. Do not guess at another location.
- Any move or deletion below fails for a reason other than the path being
  absent.

## Locate the two roots

Everything depends on these. Derive them; do not assume.

**Live state directory** — durable project state, outside the application
worktree:

```bash
TARGET_APP=/path/to/the/target/application
LIVE_STATE="$(git -C "$TARGET_APP" rev-parse --git-common-dir)/refine-live-state"
ls "$LIVE_STATE/goals" >/dev/null || echo "STOP: live state not found"
```

**Runtime root** — node-local host state, port-scoped. Its location depends on
how Refine was installed:

| Install | Runtime root |
| --- | --- |
| Checkout-local | `<refine checkout>/run` |
| Linux user install | `${XDG_STATE_HOME:-$HOME/.local/state}/<app id>/run` |
| macOS user install | `$HOME/Library/Application Support/<app id>/run` |

Confirm by looking for port-numbered directories containing `daemon-status.json`:

```bash
RUNTIME_ROOT=/path/from/the/table
ls "$RUNTIME_ROOT"/*/daemon-status.json
```

## 1. Stop the daemon

Migration moves files the daemon writes. A running daemon will recreate them at
the old paths and leave the node in a mixed state.

```bash
./r daemon stop    # or the equivalent for this install
```

Confirm it is down before continuing. Do not proceed on the assumption that the
stop command succeeded.

## 2. Move Goal log sidecars

Logs moved from beside the Goal record to `runtime/` inside live state. The
readers follow the new path, so logs left behind are not deleted — they become
invisible, which is worse than either outcome. Move them to keep the history.

```bash
cd "$LIVE_STATE"
find goals -type f -name logs.jsonl | while read -r sidecar; do
  destination="runtime/$sidecar"
  mkdir -p "$(dirname "$destination")"
  mv "$sidecar" "$destination"
done
```

Verify nothing remains at the old location and the new tree is populated:

```bash
find "$LIVE_STATE/goals" -name logs.jsonl | wc -l          # expect 0
find "$LIVE_STATE/runtime/goals" -name logs.jsonl | wc -l  # expect the previous count
```

The relative shard structure is preserved by construction:
`goals/GO/AL1/logs.jsonl` becomes `runtime/goals/GO/AL1/logs.jsonl`, which is
what the reader derives from a Goal identifier.

## 3. Remove per-claim projection caches

Each workflow claim used to receive its own directory holding a full copy of
project state, and nothing ever removed them. On a long-running node this is
usually the largest single reclaim in this migration — measure before deleting
so you can report what it recovered.

```bash
du -sh "$RUNTIME_ROOT"/*/cache/workflow 2>/dev/null
rm -rf "$RUNTIME_ROOT"/*/cache/workflow
```

## 4. Rebuild the projection snapshot

The cached snapshot still deserializes, so this is not a correctness
requirement. It is worth doing anyway: entries written by the previous build
carry every note body and Round prompt in resident text, and content hashes that
nothing consults any more. Deleting the snapshot forces one rebuild into the
bounded form.

```bash
rm -f "$RUNTIME_ROOT"/*/cache/projection-snapshot.json
```

## 5. Remove the retired mutation lock

Goal mutations no longer take a single project-wide lock. The file is inert, not
harmful, but leaving it invites the next reader to assume it still means
something.

```bash
rm -f "$LIVE_STATE/.goal-mutations.lock"
```

## 6. Hand concurrency back to the host

Refine used to seed `parallel_run_cap` into every node, so a node upgraded from
an earlier build carries that value in its stored settings whether or not anyone
chose it. A stored cap always wins over the host-capacity governor, so such a
node keeps running at its seeded limit and the governor never engages.

The seeded value and a deliberate one are indistinguishable once stored, so
nothing clears it automatically — that would make the number impossible to
choose on purpose. Clear it only where the limit was never a decision:

Clear the field in web settings, or through the API the daemon serves:

```bash
curl -s -X PATCH http://127.0.0.1:<port>/api/settings \
  -H 'content-type: application/json' \
  -d '{"parallel_run_cap": ""}'
```

There is no CLI for writing settings — `refine node settings` only prints them.

Do the same for `parallel_per_node_cap`, `parallel_per_provider_cap`, and
`parallel_per_target_app_cap` if they were never chosen deliberately; cleared,
they follow the global limit.

Confirm it took effect by reading back the effective policy:

```bash
python3 -c "import json;print(json.load(open('$RUNTIME_ROOT/<port>/workflow-automation-state.json'))['policy'])"
```

Leave a cap in place where it was chosen for a reason — a node sharing hardware
with something else, or one deliberately limited during an investigation. The
governor is a default, not a correction.

Expect the effective limit to move in either direction. On a capable host it
rises well above the seeded value. On a constrained one it can fall below it,
because the governor leaves a core for the daemon: a two-core node settles at 1.

## Do not create these by hand

Two node-local files are built on demand. Creating or copying them produces
state that does not match this node.

- `runtime/active-goals.jsonl` — the scheduler index. Reconstructed from Goal
  records on first use and maintained as Goals are written. If it is absent,
  wrong, or damaged, delete it and let it rebuild; a partial one would silently
  drop live work from scheduling.
- `runtime/record-locks/` — per-record lock files, created as needed.

Neither is synchronized, and neither should be copied between nodes.

## 7. Start the daemon and verify

```bash
./r daemon start
./r status
```

Then confirm the four things this migration was for:

1. **Logs are readable.** Open a Goal that has history and confirm its round log
   still renders. An empty log on a Goal that had one means step 2 did not take
   effect for that shard.
2. **Work schedules.** Confirm a Todo Goal is claimed. The scheduler now reads
   `runtime/active-goals.jsonl`; if that file is absent after a promotion pass,
   the node is not on the upgraded build.
3. **Caches stay bounded.** After some work has run, confirm no
   `cache/workflow/` directory has reappeared.
4. **Concurrency reflects the host.** If `parallel_run_cap` is unset, the limit
   is now derived from cores and available memory rather than a fixed default.
   On a constrained node it may legitimately settle at 1.

## What the first synchronization will do

Goal logs were previously published to `refine/state`. The first sync after this
upgrade commits their deletion — on the order of dozens of files. This is
expected, and it is the fix taking effect rather than data loss: the logs remain
on the node under `runtime/`.

History still carries the volume already committed. Reclaiming that requires
rewriting `refine/state`, which is a separate decision, is not part of this
migration, and must not be attempted without the project owner.

## If the node misbehaves after migration

Every file this runbook deletes is derived and rebuilds itself. If scheduling,
search, or the dashboard look wrong, delete the derived state and let the node
reconstruct it before investigating further:

```bash
rm -f "$LIVE_STATE/runtime/active-goals.jsonl"
rm -f "$RUNTIME_ROOT"/*/cache/projection-snapshot.json
```

If that does not resolve it, report the exact symptom and the contents of
`daemon-status.json` rather than deleting anything under `goals/` or
`features/`. Those are the source of truth and nothing in this migration should
ever require touching them.
