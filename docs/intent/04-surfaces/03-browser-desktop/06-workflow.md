# Workflow

## Key Ideas

- **Visible State Movement**: users should see how work is distributed across workflow states.
- **Filtered Truth**: workflow summaries should reflect the current filter context when shown on list screens.
- **Agent-Managed Markers**: states managed by automation should be visually clear.
- **Shared Renderer**: workflow visualization should be one concept reused across screens.

## Purpose

The workflow UI exists to make Refine's automation legible. It shows where Gaps are in the lifecycle and gives users fast navigation into filtered work states.

It should answer: how much work is waiting, active, under QA, ready to merge, in review, done, failed, or cancelled?

## Expected Role

Workflow visualization should sit close to work lists and dashboards. On the Gaps screen, it should summarize the currently filtered work, not a misleading global slice. On the Dashboard, it should provide a high-level operational overview.

Current implementation details that matter to intent:

- `renderWorkflowVisualization` is a shared UI renderer;
- agent-managed states are marked in the visualization;
- Gaps use filtered `status_counts` from the API;
- workflow links preserve relevant filters and route back into the Gaps view.

The UI should not invent its own workflow states or counts. It should reflect the shared model and workflow capability.

## Future Direction

Future workflow UI should make multi-agent orchestration understandable. It should show dependencies, blocked paths, active claims, review gates, risk, and evidence.

As automation improves, the workflow surface should become less about manually clicking states and more about supervising state movement and investigating exceptions.
