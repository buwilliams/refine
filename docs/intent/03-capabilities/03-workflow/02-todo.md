# Todo

## Key Ideas

- **Ready For Work**: todo means a Goal is eligible to be claimed and advanced.
- **Shared Queue**: todo is the main pool from which nodes and agents select work.
- **Ordering Aware**: todo work should respect Feature ordering and dependency constraints.

## Purpose

Todo exists to separate captured work from actionable work. It tells Refine that a Goal has enough context to be considered for implementation, recovery, or agent action.

## Expected Role

Todo should be the stable entry point for active automation. Workflow policy, node capacity, provider limits, target-app context, and Feature ordering should determine when a todo Goal can be claimed.

Todo should not mean "someone promised to do this manually." It means the work is ready for the system to evaluate and potentially assign.

## What Happens

When a Goal is in todo:

- Refine treats the Goal as eligible for active work.
- Workflow checks pause state, policy, node capacity, provider availability, target-app context, and Feature ordering.
- If the Goal can proceed, a node claims it so ownership is explicit.
- The assigned agent or process receives the Goal context, guidance, governance, and target-app information it needs.
- The Goal moves to in-progress when the active attempt begins.
- If it cannot proceed yet, it remains visible as actionable but unclaimed work.

## Future Direction

Future todo behavior should support smarter selection: dependencies, risk, agent capability, node capacity, target-app health, and expected impact. The state should remain understandable as the point where work becomes actionable.
