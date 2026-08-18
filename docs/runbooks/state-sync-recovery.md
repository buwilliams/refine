# Recover A State-Sync Conflict

Use this runbook when `refine sync` or `refine fleet sync` reports that state
sync needs a decision. Do not edit the live store, managed state worktree,
retained refs, resolve refs, or `refine/state` by hand.

The whole surface is one command family: `refine sync` converges, `refine sync
--preview` shows the divergence read-only, and `refine sync --authority`
answers the question a conflict asks. There is no separate recovery command
and no preview file — the preview is never written to disk and is never handed
to another command.

## How Divergence Is Decided

Synchronization resolves almost everything without an operator, in order:

1. **Ancestry** — equal or ancestor-related heads never merge: they finish or
   fast-forward. A node that is merely ahead publishes; a node that is merely
   behind hydrates.
2. **Merge from the real merge base** — genuinely diverged heads merge as one
   commit with both heads as parents. A record changed on only one side is
   taken from that side; a record changed on both goes to the structural
   driver, which merges only what it can prove disjoint (one-sided member
   changes, keyed Notes, the node registry). It is a merge driver, not a
   judge, and no textual line merge ever decides a contested record.
3. **Agent resolution** — anything still contested goes to an agent call-out
   (default on; set `state_sync_agent_resolution: off` to keep the fail-closed
   report instead). The agent never runs under the repository lock: a short
   hold pins base, ours, and theirs under `refs/refine/resolve/<id>` and
   materializes an isolated workspace, the agent resolves unlocked with both
   records and the ownership doctrine as guidance, and the rerun's short hold
   publishes the gated result as the merge commit. Output must be marker-free,
   parse, and schema-validate; a rejected output re-prompts the agent. An
   agent that cannot choose says so instead of editing, and the conflict
   escalates carrying its question verbatim; a budget of two attempts spent
   without one escalates naming the contested records instead. A
   crash anywhere is answered by rerunning — a surviving gated result for the
   same divergence publishes without invoking anything. (The workflow side has
   the analogous `workflow_conflict_resolution` setting for conflicted
   candidate refreshes.)
4. **Conflict report** — anything resolution cannot settle fails closed with a
   conflict report whose id is derived from the merge base, both heads, and
   the contested paths — the same divergence keeps the same id across
   attempts. `refine sync` exits nonzero carrying the report id, the
   node-local report path, and a domain-terms summary per contested path;
   when resolution escalated, the report records the `decision_question` and
   the summary leads with it, which `--authority` answers. A later pass over
   the same contention — the same contested records against the same remote
   head — keeps that question rather than re-asking the agent, so a standing
   escalation costs nothing until someone answers. It is the contention, not
   the divergence, because this node snapshots live state every pass and so
   mints a new divergence constantly; the carried question therefore states
   what an agent could not decide when it was authored, while the per-path
   summaries printed beside it are always the current sides.

   Under sustained contention a pass can report the conflict having invoked
   no agent at all, and that is deliberate rather than a failure to engage:
   each contested record may buy a bounded number of resolution engagements
   against one remote head before this node holds, because an agent asked
   again on the same evidence answers the same way and each call costs real
   money. Holding is not fencing — the report stands, `--authority` settles
   it in one command, automatic recovery still runs, a record contested for
   the first time is engaged at once even while another is held, and any
   publication on the remote side re-engages the resolver on the next pass.
   `git for-each-ref refs/refine/contention` shows what a node has spent, one
   ref per attempt, each targeting the remote head it was bought against.
5. **Automatic recovery** — once resolution has escalated, is unavailable, or
   is disabled, the daemon runs the terminal recovery itself with the
   ownership policy read from the merge base: remote authority by default,
   and a live override for each contested Goal record whose merge-base bytes
   name this node as `node_id` (no side votes for itself). It records a
   `sync_auto_recovered` activity entry and settles health. Set
   `state_sync_auto_recovery: off` on a node doing deliberate divergence work
   to keep the fail-closed behavior instead.
6. **Nothing is silently destroyed** — both pre-merge heads are parents of
   the recovery merge commit, so every displaced version stays reachable. The
   only state not already reachable as a commit — a joining node's live store
   displaced by remote authority — is retained under `refs/refine/retained/`
   before anything overwrites it.

Users never run CLI commands for ordinary syncing.

A mixed-version fleet needs nothing from this runbook. While nodes are being
upgraded one at a time, a node still on the previous build rebases and pushes
linear commits and an upgraded node publishes merges; each is an ordinary
remote head to the other, so the ladder above decides them exactly as it
decides any other divergence. A node reported `pending_upgrade` by
`refine fleet sync` is an API-contract fact about that node, never a state-sync
conflict, and it is not answered with `--authority`.

## Preview (read-only)

```text
refine sync --preview
```

Prints one JSON document and writes nothing: the classification (`converged`,
`local_ahead`, `remote_ahead`, `diverged`, `unrelated` for two independent
bootstraps of the same branch, `join`, or `remote_missing`), both
heads and the merge base, per-path sides (changed locally, changed remotely,
provable by the driver), domain-terms summaries for each contested path, any
live records not yet committed to the local branch, and — when a recorded
conflict report names the same contention, meaning the same remote head and
the same contested records — the escalated
`decision_question`. The preview is not a
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
everything deterministic, lets the agent resolve genuine overlaps, and fails
closed with the stable report id — and the agent's question, when it asked
one — only when resolution needs a decision.

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
