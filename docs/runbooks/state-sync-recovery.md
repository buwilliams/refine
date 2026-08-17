# Recover A State-Sync Conflict

Use this runbook when `refine sync` or `refine fleet sync` reports that Refine
state changed on multiple nodes. Do not edit the live store, managed state
worktree, retained refs, or `refine/state` by hand.

The whole surface is one command family: `refine sync` converges, `refine sync
--preview` shows the divergence read-only, and `refine sync --authority`
answers the question a conflict asks. There is no separate recovery command
and no preview file — the preview is never written to disk and is never handed
to another command.

## How Divergence Is Decided

Synchronization resolves almost everything deterministically, in order:

1. **Ancestry** — equal or ancestor-related heads never merge: they finish or
   fast-forward. A node that is merely ahead publishes; a node that is merely
   behind hydrates.
2. **Merge from the real merge base** — genuinely diverged heads merge as one
   commit with both heads as parents. Cleanly merging paths are taken from
   Git's own tree merge; contested state files go to the structural driver,
   which merges only what it can prove disjoint (one-sided member changes,
   keyed Notes, the node registry). It is a merge driver, not a judge.
3. **Conflict report** — anything still contested fails closed with a conflict
   report whose id is derived from the merge base, both heads, and the
   contested paths — the same divergence keeps the same id across attempts.
   `refine sync` exits nonzero carrying the report id, the node-local report
   path, and a domain-terms summary per contested path.
4. **Automatic recovery** — on a reported conflict the daemon runs the
   terminal recovery itself with the ownership policy read from the merge
   base: remote authority by default, and a live override for each contested
   Goal record whose merge-base bytes name this node as `node_id` (no side
   votes for itself). It records a `sync_auto_recovered` activity entry and
   settles health. Set `state_sync_auto_recovery: off` on a node doing
   deliberate divergence work to keep the fail-closed behavior instead.
5. **Nothing is silently destroyed** — both pre-merge heads are parents of
   the recovery merge commit, so every displaced version stays reachable. The
   only state not already reachable as a commit — a joining node's live store
   displaced by remote authority — is retained under `refs/refine/retained/`
   before anything overwrites it.

Users never run CLI commands for ordinary syncing.

## Preview (read-only)

```text
refine sync --preview
```

Prints one JSON document and writes nothing: the classification (`converged`,
`local_ahead`, `remote_ahead`, `diverged`, `join`, or `remote_missing`), both
heads and the merge base, per-path sides (changed locally, changed remotely,
provable by the driver), domain-terms summaries for each contested path, and
any live records not yet committed to the local branch. The preview is not a
token and is never handed to another command. The daemon API equivalent is
`GET /api/sync/preview`. On error it exits nonzero having written nothing.

## Terminal Recovery

Recovery is sync with a decision attached:

```text
refine sync --authority remote
# or: --authority live (republishes this node's divergent state — be deliberate)
```

With `--authority`, the run is the ordinary sync pass in which every path the
deterministic ladder cannot settle takes the chosen side. The result is one
merge commit carrying both heads as parents, published with force-with-lease,
hydrated into the live store record-by-record under preimage compare-and-swap
(a record the daemon advanced mid-run is left alone and becomes the next
pass's delta), and verified by classification — a read, never a re-merge.

Because the command never re-enters the merge it is clearing, rerunning it
finds converged heads and is a no-op; it cannot reproduce the conflict. A race
against a remote head that moves mid-run is retried inside the command,
bounded. Never wrap the command in a retry loop of your own — a run that still
fails is reporting a real condition. An interrupted run is simply rerun:
every step re-derives from durable state.

Per-path exceptions adjust the decision: `--path` may repeat, and each named
contested path is settled on the opposite side of the chosen authority:

```text
refine sync --authority remote --path goals/00/EXAMPLE/goal.json
```

Without `--authority`, `refine sync` is the ordinary pipeline: it converges
everything deterministic and fails closed on a genuine conflict with the
stable report id.

The result is one JSON document: `recovered` (false when sync needed no
decision), the attempt count, the settled paths, any retained refs, and the
sync result. The daemon API equivalent is `POST /api/sync` with an
`{"authority": "live"|"remote", "paths": [...]}` body, which also settles
state-sync health; without a body it queues the ordinary pipeline as a
supervised operation.

## Verify

A terminal run verifies convergence itself and returns the verifying heads in
`recovery.local_state_head` / `recovery.remote_state_head`. To confirm
independently, run `refine sync` again: it must complete without a new
conflict report, state-sync health must recover, and any live write that
arrived during recovery must publish as the next local delta. The displaced
side of every settled path is reachable from the merge commit's parents;
`git log refine/state` shows the recovery merge with both parents.
