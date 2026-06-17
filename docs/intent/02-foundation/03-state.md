# State

## Key Ideas

- **Durable State**: product state should survive process restarts, surface changes, and agent failures.
- **Runtime State**: process, daemon, cache, and operation state should be recoverable and observable.
- **Workflow State**: a Gap's state should describe what can happen next.
- **Derived State**: projections and caches are acceleration layers, not the source of truth.
- **Local Authority**: the daemon should coordinate runtime state so surfaces do not compete for ownership.

## Purpose

State lets Refine coordinate long-running software work across people, agents, processes, and restarts. Without durable state, agentic work becomes chat transcript memory and shell side effects. Refine should instead make work explicit and recoverable.

State is split by purpose. Product state describes work and project configuration. Runtime state describes the local daemon, active app, processes, operations, workflow claims, and caches. Derived state makes the product fast to query but should be rebuildable.

## Expected Role

State should make Refine reliable under interruption. If an agent fails, a process dies, the browser refreshes, or the daemon restarts, the system should be able to explain what happened and what can happen next.

The current implementation uses:

- `.refine` under the target app for durable product state.
- port-scoped runtime roots under `run/` for daemon and process state.
- projection snapshots for fast list, dashboard, activity, Gap, and Feature queries.
- workflow automation state for claims, policy, pause controls, and active work.
- process state files for managed subprocesses and recoverable visibility.
- app registry state for attach, switch, detach, and multi-app operation.

Surfaces should read and mutate state through shared services and daemon routes. A browser tab, CLI command, or future agent surface should not invent its own state authority.

## Future Direction

Future AI systems will need state that supports many agents acting at once. Refine should evolve toward stronger state semantics for claims, dependencies, reservations, provenance, conflicts, evidence, and handoffs.

The desired future is not a hidden database that only the application can understand. The desired future is durable, inspectable state with enough structure for fleets of agents to coordinate safely and enough simplicity for humans to debug the system when something goes wrong.
