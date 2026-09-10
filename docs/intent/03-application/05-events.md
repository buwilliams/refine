# Events

## Key Ideas

- **Activity Selects Work**: Events connect system activity or an explicit user request to reusable Skills.
- **One Shared Capability**: workflow, CLI, browser, API, and MCP use the same definitions, parameter resolution, execution, and evidence.
- **Explicit Scope**: project definitions apply across nodes; node definitions and binding overrides identify their node explicitly.
- **Durable Occurrences**: a workflow transition or manual request identifies one occurrence. Restart and transport retries reuse its evidence.
- **Bounded Execution**: managed process admission, cancellation, timeouts, and checkout serialization apply to Event agents.

## Purpose

Events make when work happens visible and configurable. They replace separate Governance, Quality, and Guidance configuration with a common way to connect instructions to workflow and lifecycle activity. A custom Event makes project operations available wherever users operate Refine.

## Expected Role

The initial system catalog contains Enter and Exit for Backlog, Todo, Plan, Implement, Quality, Governance, Review, Done, Failed, and Cancelled, plus `node.startup.ready`. Startup means the node daemon has become ready. State synchronization is deliberately absent from the trigger catalog.

An Event contains ordered Skill bindings. Blocking results gate normal progression, background results provide nonblocking evidence, and context attachments supply Skill instructions without launching another agent. Each agent receives the same pinned occurrence context, its parameters, and applicable attachments. A prior binding's response is not injected into the next binding. A failed blocking finding fails the collective gate; provider and contract faults remain execution errors.

Normal transitions wait for applicable gates. A user-requested transition retains its intent and source state while Exit gates run. Cancel and failure transitions proceed immediately and cannot be vetoed by a hook. Entry generations distinguish a later retry from a transport retry. A supported success action may explicitly request Start, Accept, Retry, or Reopen; the default is no additional lifecycle action. Actions retain receipts, reuse the existing transition capability, and stop automatic action chains after sixteen steps until a new manual trigger.

Custom Events declare text, number, boolean, or choice parameters, optional requirements, and defaults. Skill inputs can refer to `goal.*`, `system.*`, or `event.*` fields. The browser displays the launch form when parameters are needed; interactive CLI calls collect missing required values. Automatic missing input records a visible execution error rather than waiting for user input. Unknown inputs, invalid types, and inconsistent definitions fail before agent launch.

Configuration lives in the project state as one revision-fenced Events/Skills document and synchronizes through refine-state. Node overrides replace a named project binding; disabling an override suppresses that binding on that node without hiding unrelated bindings. Disjoint configuration edits merge structurally with a new revision; contested edits and broken references stay conflicted. Invocation snapshots retain the configuration and inputs that actually launched work. Editing a definition affects future invocations.

Agents use the existing Goal worktree, or the selected project checkout for work without a Goal. Managed checkout access is serialized. Background means its verdict does not gate progression; it does not grant concurrent mutation of a shared checkout. The existing worker admits queued work against runtime capacity and pause policy. Invocation evidence retains results, invalid attempts, process references, and cancellation. Process registration uses the same cancellation fence as managed operations, and callbacks revalidate Goal, Round, candidate, and node authority.

The capability routes are `/event-definitions`, `/skills`, and `/event-invocations`. `/events` retains its existing streaming meaning. The CLI exposes `events` and `skills`; Controls and the command palette discover custom Events dynamically.

## Future Direction

Add system sources only when they represent a useful, bounded semantic occurrence. Future delivery policies and richer parameter sources should compose with the existing worker, ownership, and evidence contracts rather than introduce another scheduler.
