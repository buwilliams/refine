# Configure Events and Skills

Events select when reusable Skill instructions run. Use **Controls → Settings → Skills** to edit instructions and assign system or custom Events to each Skill. Use **Settings → Events** to create and edit custom Events and their launch parameters. Definitions can apply to the project or a named node and synchronize through refine-state.

Refine supplies Plan, Implement, Quality, and Governance Skills and binds them to the corresponding Workflow Enter Events. Each of those workflow steps requires an enabled blocking Skill with its matching result role. The default Plan Skill chooses its own planning method. Refine supplies output contracts and retains independent results.

## Discover and edit

```sh
refine events catalog
refine skills list
refine events list
refine skills show default-quality
```

Save a complete definition with its observed configuration revision. For example, after writing a Skill object with `name`, `prompt`, `role`, and any optional parameters to `skill.json`:

```sh
refine skills save release-check --revision 1 --file skill.json
```

Use the revision returned by the most recent read. A stale revision returns a conflict and preserves the current definitions. Refresh, review concurrent edits, and save again. A referenced Skill must be unbound before deletion.

In a Skill editor, **Add event** creates an assignment with an Event, order, mode, scope, optional overridden project assignment, and input mappings. Saving the Skill and all its assignments is atomic; assignments for other Skills are preserved. A node override disables or replaces only the named project binding. Additional bindings remain independent. Map parameters to paths such as `goal.rounds.0.prompt`, `system.node_id`, or `event.subject`. Defaults and explicit manual input complete the launch values.

- **Blocking**: every blocking result must pass before normal advancement.
- **Background**: its verdict does not gate progression. Checkout serialization can still delay other work.
- **Context**: attach these instructions to the Event's agents without launching another agent.

## Trigger and inspect

Enabled custom Events appear in Controls under Events and in the command palette. **Add event...** at the bottom of the Controls Events section opens the New Event modal. The Events tab lists custom Events; system Events remain available in each Skill’s Event selector. The launch form collects typed parameters and defaults. Select a Goal when the Event has a lifecycle success action; otherwise work without a Goal uses the selected project checkout.

```sh
refine events trigger release-check --goal-id GOAL1 --param subject=release --request-id release-check-001
refine events runs
refine events status INVOCATION_ID
refine events cancel INVOCATION_ID
```

Reuse a request ID only for a retry of the same Event request. Reusing it with different parameters, Goal, or node returns a conflict. Interactive CLI launch collects missing required values; unattended calls return an error. Automatic Events with missing parameters record an execution error.

The invocation retains the selected configuration, inputs, results, process references, and invalid response diagnostics. Quality checks retain their supervised command evidence and exact candidate proof. Cancellation fences later process launches and prevents a late result from settling cancelled work.

`on_success` can explicitly request `start`, `accept`, `retry`, or `reopen` through Refine's existing Goal actions. Omit it for no additional action. Action receipts survive retries; automatic chains stop after sixteen actions. Normal Exit gates can defer a requested transition while preserving its source state. Failure and cancellation cannot be vetoed by a hook.

## Migration and recovery

First use deterministically converts existing Governance and Quality policy into the default Skill prompts and Guidance into context Skills. Refine archives source configuration in `automation/migration-v1.json` before installing `automation/config.json`. Previously enforced Quality commands remain supervised until the imported default Quality Skill is deliberately edited or replaced. Historical Round and invocation evidence remains readable.

Use supported configuration surfaces after migration. Legacy configuration files no longer control new invocations. Pristine defaults on a freshly attached node can adopt the project's synchronized configuration; authored edits retain the normal conflict boundary. Never repair a conflict by deleting Goal history or rewriting an invocation to success.

The API exposes `/event-definitions`, `/skills`, and `/event-invocations` under contract version 4. `/events` keeps its streaming purpose. MCP exposes the catalog, definitions, and custom trigger capability, with generic requests for editing and invocation control.
