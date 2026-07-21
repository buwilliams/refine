# Promote Dogfood Source

Outcome: a running Refine source checkout advances to its latest configured
upstream commit only after the candidate builds, then restarts and reports
healthy. Published-release updates remain unchanged.

## Preconditions

- The controller checkout is on a branch with a reachable configured remote.
- The controller checkout has no staged, unstaged, or untracked changes.
- The fetched commit is a fast-forward descendant of the current commit.
- Refine automation and background processes are paused, with no active Goal
  claims or running non-daemon work on the selected port.

Do not stash, reset, merge, or discard work to satisfy these checks. Resolve a
dirty or divergent checkout explicitly before retrying.

## UI Workflow

1. Open **System → Runtime** and find **Dogfood source**.
2. Select **Check for source updates**. Confirm the checkout, current commit,
   upstream remote/branch, and available commit.
3. If the panel reports a blocker, resolve it without overwriting work and
   check again.
4. Select **Promote latest source** and confirm the restart handoff.
5. Keep the page open or return to it later. The panel reconnects and polls the
   durable operation state through the daemon restart.
6. Require the final message `Latest source promoted and Refine is healthy`.

## CLI Parity

Inspect without fetching:

```sh
./r system source-status --port 8082 --runtime-root run
```

Fetch and re-evaluate availability:

```sh
./r system source-status --fetch --port 8082 --runtime-root run
```

Queue the same external handoff used by the UI:

```sh
./r system source-promote --port 8082 --runtime-root run
```

The command returns the durable operation id before the daemon stops. State is
stored at `run/<port>/source-promotion.json`.

## Failure And Recovery

- `build_candidate`: the daemon and checkout were not changed. Fix the build
  failure and check again.
- `verify_idle`: work or source state changed while the candidate built. The
  daemon and checkout were not changed; restore quiescence and check again.
- `stop_daemon`: the checkout was not advanced. Inspect the port-scoped daemon
  process records and retry only after the runtime is idle.
- `activate_source`: the helper restarts the previous daemon when possible;
  inspect the reported Git precondition failure.
- `restart_daemon`: the helper attempts to restore the prior commit and restart
  the previous daemon. Follow the persisted `recovery` text and verify with:

```sh
./r system status --port 8082 --runtime-root run
./r system source-status --port 8082 --runtime-root run
```

Never claim success from a branch change alone; daemon health verification is
part of the operation.
