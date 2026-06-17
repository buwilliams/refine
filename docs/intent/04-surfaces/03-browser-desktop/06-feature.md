# Feature

## Key Ideas

- **Outcome Container**: a Feature groups Gaps that together produce a larger result.
- **Ordered When Needed**: Feature Gap order matters when work has dependencies.
- **Editable Membership**: users should be able to add, remove, reorder, and transfer Feature work safely.
- **Rollup Visibility**: Features should summarize progress without hiding individual Gaps.
- **Shared List Pattern**: Features should reuse the same dense table, filter, sort, and pagination patterns as other work views.

## Purpose

The Feature surface exists to help users manage larger software outcomes. It organizes related Gaps, preserves their order, shows progress, and gives users a place to inspect and edit the shape of the work.

Features prevent large ideas from becoming vague. They keep the bigger purpose visible while letting agents execute smaller Gaps.

## Expected Role

The Feature UI should show Feature identity, description, reporter, assignee, node, progress, current or next Gap, and state rollup. The detail modal should let users inspect the ordered Gap list and edit membership without losing context.

Current implementation details that matter to intent:

- Feature rows use shared table, filter, sort, bulk, and pagination behavior;
- Feature details include a rollup and Gap list;
- completed or protected Gaps should not be casually disrupted by Feature-level actions;
- large Feature-managed Gap collections need pagination or incremental loading;
- Feature work should reuse shared work item services rather than page-only rules.

The Feature surface should not become a parallel task system. It is a grouping and intent-preservation layer over Gaps.

## Future Direction

Future Features may become higher-level composition plans. Agents may propose Feature decomposition, dependency ordering, risk grouping, and staged rollout plans.

The surface should preserve the relationship between large product intent and small executable work so future AI systems can scale software composition without losing why the work exists.
