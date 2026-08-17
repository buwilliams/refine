# Migrate a Node to the Current Scale and Reliability Layout

This runbook is for an agent upgrading an existing Refine node to the current
node-local scale and reliability layout. The durable product contracts behind
the procedure live in [Target App](../intent/02-foundation/03-target-app.md) and
[Process](../intent/03-capabilities/01-process.md).

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
- Per-worker projection caches are gone. These were full copies of project state
  created for old workflow attempts, and nothing pruned them.
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
GIT_COMMON_DIR="$(git -C "$TARGET_APP" rev-parse --path-format=absolute --git-common-dir)"
LIVE_STATE="$GIT_COMMON_DIR/refine-live-state"
ls "$LIVE_STATE/goals" >/dev/null || echo "STOP: live state not found"
```

**Runtime root** — node-local host state owned by the invoked Refine product
home and port-scoped:

| Install | Runtime root |
| --- | --- |
| Source checkout or gitless deployed product | `<refine product home>/run` |

Older installations may still have state or service registration in HOME,
XDG, or platform support directories. Treat those locations as legacy
external evidence: do not merge, move, overwrite, or delete them during this
procedure. Inspect ordinary status first, then use explicit
`./r system repair --port "$PORT"` if the daemon registration must be migrated;
the repair journal is retained under the owning product's
`run/$PORT/installation-migrations/`.

Confirm by looking for port-numbered directories containing `daemon-status.json`:

```bash
RUNTIME_ROOT=/path/to/refine-product/run
PORT=8082
PORT_RUNTIME="$RUNTIME_ROOT/$PORT"
test -f "$PORT_RUNTIME/daemon-status.json" || echo "STOP: port runtime not found"
```

Run this procedure once per port. Do not use a wildcard to delete state for
other Refine installations that happen to share the runtime root.

## 1. Stop the daemon

Migration moves files the daemon writes. A running daemon will recreate them at
the old paths and leave the node in a mixed state.

```bash
REFINE_DAEMON_PORT="$PORT" ./r workflow pause
./r system stop --port "$PORT" --runtime-root "$RUNTIME_ROOT"
./r system status --port "$PORT" --runtime-root "$RUNTIME_ROOT"
```

Record whether workflow automation was already paused so it is resumed only if
appropriate at the end. Confirm the status readback reports the daemon stopped
before continuing; do not trust the stop command alone.

## 2. Move Goal log sidecars

Logs moved from beside the Goal record to `runtime/` inside live state. The
readers follow the new path, so logs left behind are not deleted — they become
invisible, which is worse than either outcome. Move them to keep the history.

Count the source files and make a recoverable copy outside `LIVE_STATE` before
moving them. Record that backup path and its checksum in the migration report.

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

## 3. Remove retired workflow projection caches

Each old workflow attempt used to receive its own directory holding a full copy of
project state, and nothing ever removed them. On a long-running node this is
usually the largest single reclaim in this migration — measure before deleting
so you can report what it recovered.

```bash
du -sh "$PORT_RUNTIME/cache/workflow" 2>/dev/null
rm -rf "$PORT_RUNTIME/cache/workflow"
```

## 4. Rebuild the projection snapshot

The cached snapshot still deserializes, so this is not a correctness
requirement. It is worth doing anyway: entries written by the previous build
carry every note body and Round prompt in resident text, and content hashes that
nothing consults any more. Deleting the snapshot forces one rebuild into the
bounded form.

```bash
rm -f "$PORT_RUNTIME/cache/projection-snapshot.json"
```

## 5. Remove the retired mutation lock

Goal mutations no longer take a single project-wide lock. The file is inert, not
harmful, but leaving it invites the next reader to assume it still means
something.

```bash
rm -f "$LIVE_STATE/.goal-mutations.lock"
```

## 6. Start the upgraded daemon while workflow remains paused

```bash
./r system start --port "$PORT" --runtime-root "$RUNTIME_ROOT"
./r system status --port "$PORT" --runtime-root "$RUNTIME_ROOT"
REFINE_DAEMON_PORT="$PORT" ./r project status
```

Require a healthy status from the upgraded executable before using its API.
Do not resume workflow yet.

## 7. Hand concurrency back to the host

Refine used to seed `parallel_run_cap` into every node, so a node upgraded from
an earlier build carries that value in its stored settings whether or not anyone
chose it. A stored cap always wins over the host-capacity governor, so such a
node keeps running at its seeded limit and the governor never engages.

The seeded value and a deliberate one are indistinguishable once stored, so
nothing clears it automatically — that would make the number impossible to
choose on purpose. Clear it only where the limit was never a decision:

With the upgraded daemon running and workflow still paused, clear every
non-deliberate inherited cap in web settings or one request to the API the
daemon serves. Omit any cap that was an intentional operator choice:

```bash
curl -s -X PATCH "http://127.0.0.1:$PORT/api/settings" \
  -H 'content-type: application/json' \
  -d '{"parallel_run_cap":"","parallel_per_node_cap":"","parallel_per_provider_cap":"","parallel_per_target_app_cap":""}'
```

There is no CLI for writing settings — `refine node settings <node-id>` only
prints them.

Confirm it took effect by reading back settings through the API or browser.

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

## 8. Verify and restore the prior workflow admission state

Confirm the four things this migration was for:

1. **Logs are readable.** Open a Goal that has history and confirm its round log
   still renders. An empty log on a Goal that had one means step 2 did not take
   effect for that shard.
2. **Work schedules.** If workflow was running before the migration, resume it
   with `REFINE_DAEMON_PORT="$PORT" ./r workflow resume`, then confirm a Todo
   Goal starts. The scheduler now reads `runtime/active-goals.jsonl`; if
   that file is absent after a promotion pass, the node is not on the upgraded
   build. If workflow was already paused, leave it paused and report that
   scheduling was not exercised.
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

Run `REFINE_DAEMON_PORT="$PORT" ./r sync`, inspect the resulting
`refine/state` commit, and verify it removes old Goal log paths without adding
`runtime/`.

History still carries the volume already committed. Reclaiming that requires
rewriting `refine/state`, which is a separate decision, is not part of this
migration, and must not be attempted without the project owner.

## If the node misbehaves after migration

Every file this runbook deletes is derived and rebuilds itself. If scheduling,
search, or the dashboard look wrong, delete the derived state and let the node
reconstruct it before investigating further:

```bash
rm -f "$LIVE_STATE/runtime/active-goals.jsonl"
rm -f "$PORT_RUNTIME/cache/projection-snapshot.json"
```

If that does not resolve it, report the exact symptom and the contents of
`daemon-status.json` rather than deleting anything under `goals/` or
`features/`. Those are the source of truth and nothing in this migration should
ever require touching them.
