# Runbook: Distribute and Converge Work

Outcome: eligible Gaps spread across healthy fleet nodes for parallel
implementation, and reviewable results brought back to the node where your
user reviews and merges.

## Preconditions

- More than one enabled node (`refine cluster list`), each healthy (`health`
  absent or `ready`).
- Gaps exist in `backlog` or `todo`. Gaps with active claims stay where they
  are; Gaps inside a Feature move only when the Feature is transferred, so
  ordering survives.

## Distribute

```bash
refine next                              # will suggest distribution when it applies
refine cluster distribute --dry-run      # show the plan: who gets what, what's skipped
refine cluster distribute                # spread across enabled healthy nodes
refine cluster distribute --to worker-1  # or: fill one specific node
```

Distribution reassigns node ownership of unclaimed work — that is the entire
mechanism. There is no background scheduler; work moved because this command
was invoked. State reaches other nodes when `.refine/` changes are committed
and pushed to the shared remote (`POST /api/project/sync` or the Project Sync
button pulls on the other side).

## Converge for review

When workers finish, their Gaps sit in `review` status owned by the worker
node. Bring them home to the review node (usually where your user is):

```bash
refine cluster distribute --converge --to default --dry-run
refine cluster distribute --converge --to default
refine gap list        # reviewable gaps now owned by the review node
```

Convergence is the same distribute operation pointed home — review and merge
happen once, where the human judgment lives.

## Verify

- `refine next` no longer suggests distribution or convergence.
- `refine cluster list` + `refine gap list` show the expected ownership.
- The dry-run plan (`moves`, `skipped_details`) matches what happened.

## Undo

Distribution is ownership reassignment, so undoing is distributing again:
`refine cluster distribute --to <node>` to pull work back to one node, or a
targeted `refine cluster transfer <node-id> <gap-id>` for a single item.
