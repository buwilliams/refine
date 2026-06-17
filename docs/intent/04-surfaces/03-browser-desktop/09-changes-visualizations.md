# Changes Visualizations

## Key Ideas

- **Git As Evidence**: changes should connect product work to Git history.
- **Visual Summary**: users should see change activity patterns without reading every commit.
- **Undo Awareness**: reversible operations should explain their Git consequences.
- **Period Controls**: day, week, month, and year views help users understand tempo.

## Purpose

Changes visualizations exist to show how Refine work lands in the codebase. They connect merge commits back to Gaps and help users understand recent software movement.

This surface should answer: what changed, when did it change, which Gap caused it, and can it be undone safely?

## Expected Role

The Changes screen should combine visualization, filtering, table inspection, and undo controls. It should make Git history product-readable.

Current implementation details that matter to intent:

- Changes list refine merge commits on the configured merge target branch;
- rows link commits to Gaps through Gap trailers or projection data;
- Undo runs a revert-style operation and moves the Gap to cancelled when appropriate;
- visualization buckets changes by selected period;
- filters and pagination align with the shared list pattern.

Changes should not replace Git. They should translate relevant Git history into Refine's work language.

## Future Direction

Future visualizations should show dependency chains, risk clusters, review evidence, rollout state, and composition progress across repositories.

As agent fleets produce more changes, visualizations should help people and agents understand not just volume, but consequence.
