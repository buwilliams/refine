# Recover A State-Sync Conflict

Use this runbook when `project sync` or `fleet sync` reports that Refine state
changed on multiple nodes. Do not edit the live store, baseline, managed state
worktree, recovery refs, or `refine/state` by hand.

## One-Shot Recovery (default)

When the winning side is already decided — for fleet convergence that is almost
always the remote — run recovery as a single command:

```text
refine project state-recovery run --authority remote
```

`run` synchronizes, and only when synchronization is rejected with recoverable
evidence (a missing baseline, or the semantic conflict it just recorded) does
it derive a preview and apply it under one repository lock hold, then verify
with a fresh synchronization. Because evidence is derived and consumed in the
same hold, the preview cannot go stale between the two steps; a race against a
remote head that moves mid-apply or a concurrent live write is retried inside
the command, bounded, before the last race surfaces as a `stale_preview`
error. Never wrap `run` in a retry loop of your own — the bounded retry is the
command's job, and a `run` that still fails is reporting a real condition.

`--authority remote` is the default and the safe fleet choice: it preserves the
node's pre-recovery live state on a recovery ref, then converges live state to
the remote. `--authority live` republishes this node's divergent state to the
fleet — use it only when this node is deliberately authoritative. Per-path
exceptions (`--live-path`, `--remote-path`) apply to reported conflicts exactly
as they do for `apply`.

The result is one JSON document: `recovered` (false when sync needed no
recovery), the attempt count, the recovery result with its manifest and
retained pre-recovery ref, and the verifying sync. The daemon API equivalent is
`POST /api/project/state-recovery/run`, which also settles state-sync health.

## Manual Review (preview → apply)

Use the two-step flow when an operator wants to read the comparison before
choosing authority.

1. Run `refine project sync`. The command waits for the durable operation and
   exits nonzero with the conflict report id and node-local path.
2. Read that JSON report. Confirm its target and repository identities, phase,
   baseline and snapshot identities, local and remote heads, and complete
   `unresolved_paths` list.
3. Save the output of `refine project state-recovery preview` as JSON. A valid
   preview has `baseline_status: valid_conflict`, the same report id, no
   truncated paths, and exact live, remote, and baseline identities.
4. Choose the side that should win most unresolved paths, then list only the
   exceptions. For example:

```text
refine project state-recovery apply \
  --authority remote \
  --preview-file preview.json \
  --live-path goals/00/example/goal.json
```

`--live-path` and `--remote-path` may repeat. Unknown paths, duplicates, stale
snapshots, changed heads, changed decisions, and fabricated reports fail before
authority is accepted. Compatible one-sided and schema-proven changes are
preserved automatically; Goal Rounds remain atomic.

The complete preview is the compare-and-swap token: anything that moves between
preview and apply — the remote head, live state, the conflict report — fails
closed as `stale_preview` and requires a fresh preview. On a busy fleet that
window loses races routinely; that is what `run` exists for.

## Verify

The result must report `baseline_created: true`, a manifest path, retained
pre-recovery ref, and exact local and remote state heads. Read the manifest and
confirm `stage: completed` and `outcome: succeeded`. Run `refine project sync`
again: it must complete, state-sync health must recover, and any live write that
arrived after apply began must publish as the next local delta. (`run` performs
this verifying sync itself and returns it as `sync`.)

If apply is interrupted, repeat the exact same preview and decision. Refine
resumes only matching manifest stages and owned refs. A different decision or
remote movement requires a fresh sync report and preview. An interrupted `run`
is simply rerun: every step resumes or re-derives from durable state.
