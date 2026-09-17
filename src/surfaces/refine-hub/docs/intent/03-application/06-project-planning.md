# Project Planning

Project Planning replaces Reporter-specific Todo Lists with boards shared by every node attached to the project. A personal board is an ordinary shared board named for its purpose or Reporter; it is not private storage. Reporter is attribution, not an access filter.

A card references one ordinary Goal. New cards create that Goal in **Draft**, before Backlog, with a name, description, optional Reporter and priority. They create no execution Round or implementation checkout. The Goal ID, notes and history survive acceptance, routing and execution. Existing Goals can be attached without copying them. A Goal has at most one board placement.

Boards own ordered lanes. Placements carry their own revision, order, archive state and optional routing override. Organizational edits do not change Goal workflow revisions. Goal edits carry a separate expected Goal revision. Board changes use the board revision; card changes use the placement revision. A conflict preserves the submitted action and requires refreshed intent. Contested synchronized planning records require explicit state recovery; an agent cannot pick a winner.

## Lane behavior

A lane explicitly selects one built-in action:

- **Organize only**: change placement. A personal Done lane leaves Goal status unchanged.
- **Accept into Backlog**: move a Draft Goal through its normal lifecycle gates into Backlog, without creating a Round or starting execution.
- **Release to execution**: accept Draft if necessary, resolve the execution node, satisfy source Backlog gates, create a first Round from the name and description if needed, hand off ownership while still in Backlog, and release through the ordinary Todo transition on the selected node.

Creating or attaching a card in a lane counts as entry. Moving within the same lane only reorders it. Changing lane settings never dispatches existing cards; **Apply lane action** explicitly requests that behavior. Moving an already executing or terminal Goal organizes it without restarting or resetting its workflow. Archive and detach retain the Goal and its evidence.

Routing resolves card, then lane, then board. An explicit node must be enabled, unarchived and not marked failed or deprovisioned. `auto` chooses the eligible node with the fewest Goals in executable stages, using node ID as a stable tie-break. Empty inherited routing waits for a decision; it never chooses an implicit node. A selected offline node remains pending. Routing a Feature Goal to another node requires a supported Feature transfer first.

## Durable actions and Skills

Every command has a stable request ID. Retrying identical input returns the same action; reusing that ID for different input conflicts. The current Goal owner processes card actions, even when another node submits them. New cards belong to their creating node. Waiting work is resumed by the existing workflow worker, respecting pause and normal Skill process admission. A whole resumable card action is serialized against subsequent actions on that Goal.

`planning.lane.exit` and `planning.lane.enter` support optional board and lane filters. Skills receive the action, actor, from/to placement, routing, behavior and ordinary Goal context. Applicable blocking Skills must succeed; context and background modes retain their existing meaning. Configuration and invocation IDs are pinned before execution. Draft lifecycle Skills and lane Skills use the existing managed lifecycle checkout boundary; they do not create an implementation candidate. No state-sync trigger is introduced.

The action records each phase and invocation. Source departure gates finish before ownership changes. A release receipt and new owner are written together on the Goal. For a different execution node, the target verifies and publishes the handoff through state synchronization before Todo. Retries retain one Goal and one initial Round. Cancel stops further action processing and requests cancellation of its Skills; completed placement or workflow changes remain visible. Failure retains evidence; correct the cause and submit a new action rather than rewriting the failed one.

## Surfaces

Project Planning is a main navigation destination with board selection, lane and card editors, drag-and-drop, a keyboard-accessible Move dialog, archive controls, pending actions and Goal links. It ignores the selected Reporter when reading boards. Skill configuration exposes Draft and lane triggers with optional board/lane filters.

CLI, API and MCP call the same application service:

- `refine planning list`, `action ID`, `cancel ID` and `apply OPERATION`.
- `GET /api/planning`, `/boards/:id`, `/cards/:id`, `/actions/:id`; `POST /api/planning/commands` and `/actions/:id/cancel`. The daemon accepts the same paths without `/api`.
- MCP: `refine_planning`, `refine_planning_command`, `refine_planning_action`, `refine_cancel_planning_action`.

Command operations are `board.create`, `board.update`, `board.archive`, `lane.create`, `lane.update`, `lane.reorder`, `lane.delete`, `card.create`, `card.attach`, `card.update`, `card.move`, `card.archive`, `card.detach`, `card.delete`, `card.apply`, and `migrate`. A lane can be deleted only when empty, including archived placements. Board archive is reversible.

`lane.update` accepts an optional integer `position`, the lane's zero-based index in the board. Name, action, routing, and position are saved together under one board revision; an invalid position leaves the board unchanged.

```sh
refine planning apply board.create --request-id team-board --data '{"name":"Team"}'
refine planning list
# Use the returned board and lane IDs:
refine planning apply card.create --request-id first-idea \
  --board-id BOARD --lane-id LANE \
  --data '{"name":"Improve onboarding","description":"Explain the next action","reporter":"Alice"}'
refine planning action first-idea
refine planning apply card.move --request-id release-idea \
  --board-id BOARD --lane-id READY --goal-id GOAL --expected-revision 1
```

Card commands return queued receipts (HTTP 202); board/lane commands are processed immediately. HTTP 200 means the receipt was read successfully, so callers must inspect its `state` and `message`. Actions can be queued, running, waiting, complete, failed or cancelled. `expected_revision` is required for existing board/card mutations. Card metadata edits additionally supply `data.expected_goal_revision`. Routing-only edits leave Goal metadata untouched. Descriptions become authored Round content on first release; later execution edits use existing Round controls.

## Upgrade and migration

Upgrade every node sharing a project before resuming planning automation: older nodes do not understand Draft or the new automation schema. API contract 7 and automation schema 4 advertise the new capability. Existing Goals retain their status. Existing automation bindings survive schema upgrade; new lifecycle and lane events start without user bindings.

Run `refine planning apply migrate --request-id import-todos` once from the chosen owning node, or select **Import Todo Lists** in Project Planning. Pause writers using old binaries until the migration is synchronized. Migration preserves the source `todo-lists.json`, list/item identities, text, Reporter and timestamps, and uses deterministic destination IDs plus a completion receipt for safe restart. Each legacy list becomes a board with Open and Done lanes. Both unfinished and completed items become Draft Goals; completed items occupy Done without claiming delivered work. Historical import emits no Draft or lane Skills. Importing an empty legacy store is valid.

The old Todo toolbar is retired. Legacy HTTP routes and the hidden CLI remain compatible before migration; once its receipt exists, HTTP returns `410 Gone` and legacy CLI writes are rejected. No live migration runs merely because a new binary starts.

Draft cards belong exclusively to Project Planning in the browser. Workflow visualizations, creation controls, filters, Goal lists and searches, Feature progress, and bulk selections exclude Draft. The Goals query supports `exclude_draft=1` before pagination and facets; bulk filters use `exclude_draft: true`. API and CLI callers can still explicitly inspect planning Goals. Draft detail links return to the board.

`board.delete` requires the board revision and rejects pending actions involving that board. It records a durable deletion marker so synchronization cannot restore old placements. Goals and their workflow history remain intact; their old card placements are no longer visible and can be replaced by `card.attach` on another board. Card placement revisions remain monotonic across reattachment. Board locks serialize deletion with card processing, including moves between boards. Replaying the deletion request is safe after interruption.

`card.delete` deletes a Draft Goal and its placement on the owning node. It requires the placement `expected_revision` and `data.expected_goal_revision`; promoted or changed Goals are rejected. Retrying the same request resumes interrupted placement cleanup without repeating the deletion. Use `card.detach` to keep the Goal and only remove its board placement.

The API processes card creation during the request so ordinary Draft cards appear without a background-worker delay. Creation and deletion retain durable action records; actions waiting on Skills or another node continue through the worker.
