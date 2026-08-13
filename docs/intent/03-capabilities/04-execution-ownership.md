# Execution Ownership

## Key Ideas

- **Semantic Ownership Is Synchronized**: Goal status and node assignment say which node may advance the work.
- **Worker Ownership Is Transient**: a node may track its current workers in memory and local process records.
- **At-Least-Once Is Acceptable**: restarting the same instructions is cheaper and safer than maintaining distributed execution locks.
- **Idempotence Over Reservation**: workers prove or reuse semantic results instead of reserving durable execution slots.
- **Cancellation Is Goal Intent**: cancellation changes the Goal first; stopping local execution is cleanup.

## Purpose

Execution ownership answers two different questions without combining them into one persisted concept:

- Which node is allowed to move this Goal forward?
- Which local worker, if any, is currently trying to do that work?

The synchronized Goal answers the first question. Node-local runtime state answers the second. Keeping them separate prevents process lifetimes, restarts, and partial synchronization from becoming part of the product's durable workflow model.

## Expected Role

A node schedules a Goal only when its status is actionable and its `node_id` names that node. The scheduler keeps an in-memory set to avoid launching the same Goal twice in one runner and observes live managed processes for soft capacity. These are efficiency controls, not authority.

Each worker receives the Goal, Round, selected context, and semantic instructions. Before a transition or consequential side effect it rereads the synchronized Goal. If status, node, or Round changed, it stops. If the daemon restarts, a replacement worker may receive the same instructions and continue from preserved planning, Git, quality, and integration evidence.

Two nodes may briefly believe work is available because synchronization is delayed. Durable state convergence decides the outcome. In the clear reassignment-versus-start race, an automated start by the previously authoritative node wins over a concurrent queued reassignment. Ambiguous lifecycle conflicts remain visible for resolution rather than being hidden by timestamps or local runtime records.

Stop and Cancel have different meanings. Stop targets a local process and conditionally requeues an unchanged Goal. Cancel writes terminal synchronized Goal intent and then attempts to stop matching local processes. Governance has an explicit point of no return: cancellation before Git integration blocks it; cancellation after integration begins does not trigger rollback, and exact integration evidence is still recorded.

Retired execution-coordination and cancellation-journal files are one-way cleanup inputs. They are removed during recovery and never imported into the new authority model.

## Future Direction

Refine should keep execution ownership proportional to the cost of duplicate work. If future non-idempotent operations require stronger coordination, that protection should be scoped to the side-effect boundary itself rather than recreating a general durable worker-lock system for every workflow step.
