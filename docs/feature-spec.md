# Feature Organization Spec

## Summary

Add a new organizational concept called **Feature**. A Feature is an ordered
collection of Gaps used to plan and execute larger bodies of work without
replacing the existing Gap workflow.

Refine's current Gap model remains the primary executable work unit. Gaps may
continue to exist independently. A Gap may optionally belong to one Feature, and
when it does, its position inside that Feature determines the order in which
agents should work through the Feature.

## Goals

- Support large planned work as Features > Gaps.
- Preserve all existing Gap behavior for standalone Gaps.
- Allow multiple Features to be worked in parallel.
- Serialize work within a single Feature so agents do not implement multiple
  Gaps from the same Feature at the same time.
- Make Feature progress visible without adding Features to the Dashboard
  workflow.
- Keep migration invisible for existing installations.
- Make Feature behavior available through both UI and CLI surfaces.
- Implement Feature behavior through shared backend operations, following the
  existing Refine architecture.
- Let imported Gaps optionally be saved as a Feature, whether the import comes
  from AI extraction, pasted CSV, or an uploaded CSV file.

## Non-Goals

- Replace Gaps.
- Add a third Agile layer such as Epic > Feature > Gap.
- Show Features as workflow items on the Dashboard.
- Allow cross-node Feature or associated Gap mutation.
- Duplicate business logic separately in the UI and CLI.

## Terminology

- **Feature**: An ordered group of Gaps for planning and serialized execution.
- **Associated Gap**: A Gap with a `feature_id`. This is still just a Gap, not
  a separate work type.
- **Standalone Gap**: A Gap with no `feature_id`.
- **Feature order**: The ordering of Gaps inside a Feature, stored on the Gap.

Use `Feature` consistently in UI, CLI, API, and storage. Do not persist this
concept under `epic` or mix Agile terminology in the schema.

## Data Model

Add a new `features` record type.

Feature fields:

- `id`: generated identifier.
- `name`: required display name.
- `description`: optional planning context.
- `reporter`: optional, following existing Gap reporter conventions where
  appropriate.
- `node_id`: required; Features are node-bound like Gaps.
- `created`: timestamp.
- `updated`: timestamp.
- `json_path`: relative path to the durable Feature JSON record, if mirrored in
  SQLite.

Add optional fields to Gaps:

- `feature_id`: nullable Feature id.
- `feature_order`: nullable integer used to order Gaps within a Feature.

A Gap with no `feature_id` remains a standalone Gap.

Recommended SQLite additions:

- `features` table or `features_index` cache table mirroring durable Feature
  JSON.
- `gaps_index.feature_id`.
- `gaps_index.feature_order`.
- Index on `features.node_id`.
- Index on `gaps_index.feature_id`.
- Index on `(feature_id, feature_order)`.

## Durable Storage

Features must use the same durable project-state model as Gaps. SQLite should
act as an index/cache for list performance, not as the only place where Feature
membership lives.

Durable storage requirements:

- Store each Feature as JSON under the Refine volume root, using a sharded path
  convention comparable to Gaps, for example
  `features/<first 2 chars>/<remaining id>/feature.json`.
- Store Feature metadata in the Feature JSON record.
- Store `feature_id` and `feature_order` on the Gap JSON record as top-level Gap
  fields.
- Mirror Feature metadata into the Feature SQLite index/cache.
- Mirror `feature_id` and `feature_order` into `gaps_index`.
- Rebuild SQLite indexes from durable Feature and Gap JSON without losing
  Feature associations.
- Write Feature JSON and associated Gap JSON through shared path helpers and
  atomic writes.
- Route Feature mutations through the same project sync path used for other
  durable Refine workflow state.

## Ordering Invariants

Feature order must be deterministic and safe under mutation:

- A standalone Gap has `feature_id = null` and `feature_order = null`.
- A Gap associated with a Feature has both `feature_id` and `feature_order`.
- `feature_order` is unique within a Feature.
- New Gaps appended to a Feature receive the next order after the current last
  Gap.
- Reorder operations are transactional across all affected Gaps.
- Removing a Gap from a Feature nulls both `feature_id` and `feature_order`.
- Moving a Gap between Features updates both Features transactionally.
- After remove or move, Refine should compact the remaining order values so UI
  order and persisted order stay easy to reason about.

## Shared Implementation Surface

Features must be implemented as a shared backend capability, not as UI-only
behavior.

Core Feature operations should live in shared server modules following the
existing Gap architecture, so the web UI, CLI, backend runner, and tests all use
the same behavior.

Required shared operations:

- Create Feature.
- List Features with filters, sorting, pagination, derived status, and progress
  counts.
- Get Feature detail with ordered Gaps.
- Update Feature metadata.
- Delete Feature with cascade behavior.
- Cancel Feature with cascade behavior.
- Assign Gap to Feature.
- Remove Gap from Feature.
- Reorder Gaps within a Feature.
- List candidate Gaps that can be assigned to a Feature.
- Persist imported Gaps as standalone Gaps, as a new Feature, or into an
  existing Feature.
- Enforce Feature scheduling constraints during agent dispatch.

Expose those operations through both UI and CLI surfaces:

- UI API: REST-style endpoints for Feature list/detail/mutations and Gap
  assignment/reordering.
- CLI: matching commands for create/list/show/update/delete/cancel/assign/remove
  and reorder.
- Import UI and CLI: extend the existing import persist path with Feature
  destination options instead of creating a separate import implementation.

The CLI and UI must not duplicate Feature business logic. Both should call the
same shared Feature operations, including validation, node ownership checks,
derived status calculation, cascade behavior, and ordering rules.

Representative CLI shape:

```bash
refine features list
refine features show <feature-id>
refine features create --name "Settings redesign"
refine features update <feature-id> --name "..."
refine features delete <feature-id>
refine features cancel <feature-id>
refine features add-gap <feature-id> <gap-id>
refine features remove-gap <feature-id> <gap-id>
refine features reorder <feature-id> <gap-id> --before <other-gap-id>
refine import persist reviewed-import.json --new-feature-name "Settings redesign"
refine import persist reviewed-import.json --feature <feature-id>
```

CLI output should support human-readable output and JSON output where consistent
with existing Refine CLI conventions. Feature import should remain under
`refine import`, because import already has shared extract, parse, dedup, and
persist stages.

## Derived Feature Status

A Feature has the same workflow status vocabulary as a Gap, but its status is
derived from its Gaps and is not stored as mutable workflow state on the Feature
record.

Suggested rollup:

- No Gaps: `backlog`.
- All Gaps `done`: `done`.
- All Gaps terminal and at least one Gap `cancelled`: `cancelled`.
- Otherwise, use the status of the first non-terminal Gap in `feature_order`.

This keeps the Feature status aligned with the next ordered unit of work. For
example, if the first unfinished Gap is `failed`, the Feature is `failed` and
later ordered Gaps remain blocked. If the first unfinished Gap is `in-progress`,
the Feature is `in-progress` even if later Gaps are still `todo`.

Also expose progress counts:

- `gap_count`.
- `done_count`.
- `active_count`.
- `failed_count`.
- `cancelled_count`.
- `blocked_count`, for later ordered Gaps blocked by an earlier non-terminal or
  failed Gap.
- `next_gap`, the first non-terminal Gap in Feature order.

Feature status is for planning, list display, and filtering. Features should not
appear in Dashboard workflow columns.

## Agent Scheduling

Standalone Gaps continue to schedule exactly as they do today.

For Gaps associated with a Feature:

- Only one Gap from a given Feature may be actively worked at a time.
- The scheduler may start only the first eligible non-terminal Gap in
  `feature_order`.
- Later Gaps in the same Feature must wait until all earlier ordered Gaps are
  terminal or explicitly skipped.
- Terminal states are `done` and `cancelled`.
- A failed earlier Gap blocks later Gaps until retried, cancelled, or otherwise
  resolved by the user.
- Multiple different Features may run in parallel, subject to existing global
  and node concurrency limits.
- Node ownership must continue to be enforced: a node may only schedule or
  mutate Features and Gaps owned by that node.
- Backlog auto-promotion must respect Feature order. Refine should not promote a
  later associated Gap to `todo` while an earlier associated Gap is non-terminal.
- Manual status changes must not allow a later associated Gap to bypass an
  earlier non-terminal Gap. If a user manually moves a later Gap to `todo`, the
  scheduler must still refuse to reserve it until ordering allows it.

Feature ordering affects implementation dispatch. It should not remove existing
manual Gap status controls, but the scheduler must respect Feature order when
deciding which `todo` Gap to reserve.

## UI

Add a new main nav screen: **Features**.

The Features screen should mirror the existing Gaps screen patterns:

- Search.
- Status filter using derived status.
- Node filter where applicable.
- Sortable table.
- Pagination using the same shared list primitives as Gaps.
- Row click opens a Feature detail route.

Feature list columns:

- Name.
- Status.
- Progress, for example `3 / 7 done`.
- Current or next Gap.
- Reporter, if used.
- Node.
- Updated.

Feature detail should show:

- Feature name and description.
- Derived status and progress.
- Ordered Gap list.
- Action to create a new Gap in the Feature.
- Action to assign existing Gaps to the Feature.
- Reorder controls for Gaps associated with the Feature.
- Remove Gap from Feature.
- Cancel Feature.
- Delete Feature.

Add **New Feature** under the existing topbar create menu near **New Gap**,
**Plan**, and **Import gaps**.

Feature creation and editing should use a Feature modal similar to the New Gap
modal, but scoped to the Feature. The modal should include Feature metadata and
an ordered list of Gaps currently associated with the Feature. The list should
make the Feature's execution order visible and editable without requiring
navigation away from the modal.

The Gap list inside the Feature modal should support:

- Add a new Gap to the Feature.
- Assign an existing Gap to the Feature.
- Reorder Gaps associated with the Feature.
- Remove a Gap from the Feature without deleting the Gap.

Each listed Gap should show enough context for ordering decisions, such as name,
status, priority, reporter, and updated time. Creating a Gap from the Feature
modal should prefill `feature_id` and assign the next `feature_order`.

For large Features, the modal Gap list should use the same pagination or
incremental loading pattern as other Refine lists. The full Feature detail route
should expose the same ordered Gap management actions with more room for
filtering, sorting, and review. Creating a new Gap from the Feature modal should
avoid nested modal stacks; it should either switch the current modal into the
New Gap flow or close the Feature modal and reopen it after the Gap is saved.

The existing Import gaps flow should gain a Feature option rather than splitting
into a separate import surface. During import review, the user can choose one of:

- Save imported Gaps as standalone Gaps.
- Create a new Feature from the import and save all imported Gaps into it.
- Add imported Gaps to an existing Feature.

The Feature option must be available for each import source:

- AI extraction from pasted planning text or feedback.
- CSV paste.
- CSV file upload.

When an import creates a new Feature, the user can edit the Feature name and
description before saving. When an import targets an existing Feature, Refine
appends the imported Gaps after the current last `feature_order` unless the user
reorders them during review.

## Gap UI Changes

On the Gap list and Gap detail:

- Show Feature association when present.
- Add filter by Feature.
- Allow assigning a Gap to a Feature.
- Allow removing a Gap from its Feature.
- Allow changing order when viewing Gaps inside a Feature.

When creating a Gap from a Feature detail page, prefill `feature_id` and assign
the next `feature_order`.

## Plan Mode

Plan Mode should default to creating a Feature plus ordered Gaps.

Expected behavior:

- User enters a plan prompt.
- Refine drafts a Feature.
- Refine drafts ordered Gaps under that Feature.
- User can review and edit the Feature and its Gaps before saving.
- Existing Gap-only planning should remain available as an option if needed, but
  the default should be Feature-backed planning.

## Import Mode

Import should support Feature-backed saves across all import sources.

AI extraction:

- The current AI-assisted import flow may infer a Feature name and description
  from the submitted text.
- The user must be able to accept the inferred Feature, edit it, choose an
  existing Feature, or save the extracted Gaps as standalone Gaps.
- Imported Gaps should preserve the AI-proposed order as `feature_order` when
  saved into a Feature.

CSV paste and CSV file:

- CSV import should expose the same Feature destination choices as AI import.
- CSV rows should preserve file or paste order as `feature_order` when saved
  into a Feature.
- If CSV data includes a Feature name column in the future, that can be used as
  an enhancement, but the initial requirement is a single optional Feature
  destination for the whole import batch.

Shared import behavior:

- Import preview should show whether the batch will create a Feature, append to
  a Feature, or create standalone Gaps.
- Feature destination should be represented in the shared import persist request
  body so UI and CLI use the same persistence path.
- Duplicate detection and import rollback should continue to work for
  Feature-backed imports.
- If a Feature-backed import fails, Refine must not leave a partially created
  Feature with missing or unordered Gaps.
- Imported Gaps must inherit the owning node from the Feature destination or,
  for a new Feature, from the active node.
- Importing into an existing Feature must reject cross-node destinations.

## Cancel and Delete Semantics

Cancel Feature:

- Is a system-owned cascade operation, not a normal user workflow transition.
- Stops running work for any active Gap in that Feature before finalizing the
  cascade.
- Marks all non-terminal Gaps in the Feature as `cancelled` through the shared
  runner/cancel path or an equivalent shared operation that records workflow
  logs.
- Leaves completed Gaps as `done`.
- Cancels `backlog`, `todo`, `in-progress`, `qa`, `ready-merge`,
  `awaiting-rebuild`, `review`, and `failed` Gaps unless a narrower state rule
  is explicitly added during implementation.
- Only affects Gaps owned by the same node as the Feature.

Delete Feature:

- Requires confirmation.
- Cascades to all Gaps in the Feature, including their normal Gap cleanup
  behavior.
- Uses the same safety expectations as bulk Gap deletion.
- Only affects Gaps owned by the same node as the Feature.

Consider offering "Delete Feature only" as a future option, but the initial
behavior can be cascade delete because that is the requested model.

## Migration

Migration must be automatic and invisible:

- Add durable Feature JSON storage if missing.
- Add the `features` or `features_index` table if missing.
- Add nullable `feature_id` and `feature_order` columns to `gaps_index`.
- Add nullable `feature_id` and `feature_order` fields to Gap JSON when a Gap is
  associated with a Feature.
- Backfill nothing; existing Gaps remain standalone.
- Add indexes for `feature_id`, `(feature_id, feature_order)`, and `node_id` as
  needed.
- Ensure persisted Feature and Gap JSON is updated lazily or through existing
  sync/cache rebuild paths without user-visible disruption.

## Node Ownership

Features are node-bound like Gaps.

- A Feature is owned by one `node_id`.
- A Gap may only be assigned to a Feature when both records have the same
  `node_id`.
- Feature metadata, ordering, cancellation, and deletion are only allowed from
  the owning active node.
- Feature scheduling only considers Features and Gaps owned by the local active
  node.

## Acceptance Criteria

- Existing Gaps screens and workflows still work for standalone Gaps.
- Existing databases start without manual migration.
- Feature records and Gap Feature associations survive SQLite cache rebuilds and
  project-state sync.
- Feature order remains unique and deterministic after add, remove, move, and
  reorder operations.
- A user can create or edit a Feature in a modal that includes the ordered Gap
  list.
- From the Feature modal, a user can add a new Gap, assign an existing Gap,
  reorder Gaps, and remove Gaps from the Feature.
- A user can create a Feature, add ordered Gaps, reorder them, remove them, and
  see derived progress.
- Plan Mode creates a Feature with ordered Gaps by default.
- AI import, CSV paste import, and CSV file import can optionally create a new
  Feature or append imported Gaps to an existing Feature.
- Feature import remains part of the existing import persist flow for UI and
  CLI.
- Feature-backed imports preserve reviewed import order as `feature_order`.
- Agents never work two Gaps from the same Feature at once.
- Agents may work Gaps from different Features in parallel.
- Features and their Gaps cannot be mutated across node ownership boundaries.
- Features do not appear as workflow cards or items on the Dashboard.
- Feature list uses the same interaction style as the Gaps list.
- Deleting or cancelling a Feature cascades predictably to its Gaps, with cancel
  handled as a system-owned workflow operation.
- Feature behavior is available from both UI and CLI.
- UI and CLI use the same shared backend operations.
- CLI output supports human-readable and JSON modes where consistent with
  existing Refine CLI conventions.
- Tests cover shared operations directly, plus representative UI/API and CLI
  paths.

## Test Coverage

Add focused coverage for:

- Feature CRUD shared operations.
- Durable Feature JSON, Gap JSON association fields, and SQLite cache rebuilds.
- Derived Feature status and progress rollups.
- Gap assignment, removal, and reorder validation.
- Feature modal Gap list behavior for add, assign, reorder, and remove actions.
- Feature-backed AI import, CSV paste import, and CSV file import.
- Import rollback and duplicate handling for Feature-backed imports.
- Node ownership enforcement for Feature and associated Gap mutations.
- Scheduler serialization within a Feature.
- Parallel scheduling across different Features.
- Cascade cancel and cascade delete behavior.
- UI API routes for list/detail/mutation paths.
- CLI commands and JSON output.
- Smooth migration from databases with no Feature schema.
