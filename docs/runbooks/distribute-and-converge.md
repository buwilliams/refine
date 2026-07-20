# Runbook: Distribute and Converge Work

Outcome: eligible Goals spread across healthy fleet nodes for parallel
implementation, and reviewable results brought back to the node where your
user reviews and merges.

## Preconditions

- More than one enabled node (`refine cluster list`), each healthy (`health`
  absent or `ready`).
- Goals exist in `backlog` or `todo`. Goals with active claims stay where they
  are; Goals inside a Feature move only when the Feature is transferred, so
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
was invoked. Refine's daemon observes the durable state change and publishes a
debounced batch through the shared `refine/state` branch. Other nodes do the
same. Use **Sync state now** only when an immediate handoff is required.

## Converge for review

When workers finish, their Goals sit in `review` status owned by the worker
node. Bring them home to the review node (usually where your user is):

```bash
refine cluster distribute --converge --to default --dry-run
refine cluster distribute --converge --to default
refine goal list        # reviewable goals now owned by the review node
```

Convergence is the same distribute operation pointed home. Review happens once,
where the human judgment lives; approving a reviewed Goal asks Refine to
integrate its isolated candidate.

## Verify

- `refine next` no longer suggests distribution or convergence.
- `refine cluster list` + `refine goal list` show the expected ownership.
- The dry-run plan (`moves`, `skipped_details`) matches what happened.

## Undo

Distribution is ownership reassignment, so undoing is distributing again:
`refine cluster distribute --to <node>` to pull work back to one node, or a
targeted `refine cluster transfer <node-id> <goal-id>` for a single item.
