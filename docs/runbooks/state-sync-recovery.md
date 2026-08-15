# Recover A State-Sync Conflict

Use this runbook when `project sync` or `fleet sync` reports that Refine state
changed on multiple nodes. Do not edit the live store, baseline, managed state
worktree, recovery refs, or `refine/state` by hand.

## Inspect

1. Run `refine project sync`. The command waits for the durable operation and
   exits nonzero with the conflict report id and node-local path.
2. Read that JSON report. Confirm its target and repository identities, phase,
   baseline and snapshot identities, local and remote heads, and complete
   `unresolved_paths` list.
3. Save the output of `refine project state-recovery preview` as JSON. A valid
   preview has `baseline_status: valid_conflict`, the same report id, no
   truncated paths, and exact live, remote, and baseline identities.

## Decide And Apply

Choose the side that should win most unresolved paths, then list only the
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

## Verify

The result must report `baseline_created: true`, a manifest path, retained
pre-recovery ref, and exact local and remote state heads. Read the manifest and
confirm `stage: completed` and `outcome: succeeded`. Run `refine project sync`
again: it must complete, state-sync health must recover, and any live write that
arrived after apply began must publish as the next local delta.

If apply is interrupted, repeat the exact same preview and decision. Refine
resumes only matching manifest stages and owned refs. A different decision or
remote movement requires a fresh sync report and preview.
