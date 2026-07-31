# Runbook: Distribute and Converge Work

Outcome: eligible Goals spread across healthy fleet nodes for parallel
implementation, and reviewable results return to the node where the user makes
the final acceptance decision. Ready Merge has already integrated a Goal's
exact candidate before it reaches Review.

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

Convergence is the same distribute operation pointed home. It changes ownership
only; it does not merge or replay the candidate. Review happens once, where the
human judgment lives, and approval accepts the integration already recorded by
Ready Merge.

## Verify

- `refine next` no longer suggests distribution or convergence.
- `refine cluster list` + `refine goal list` show the expected ownership.
- The applied result matches the reviewed dry-run plan (`moves`,
  `skipped_details`).
- Each converged Review Goal still records the same candidate and Ready Merge
  integration evidence; convergence did not perform another Git operation.

## Undo

Distribution is ownership reassignment, so undoing is distributing again.
Preview both ordinary open-work moves and Review convergence when both classes
are present:

```bash
refine cluster distribute --to <node> --dry-run
refine cluster distribute --converge --to <node> --dry-run
```

Apply only the required plan, or use
`refine cluster transfer <node-id> <goal-or-feature-id>` for one item. Transfer
has no dry-run flag, so inspect the item and destination first.
