# Process

## Key Ideas

- **Supervisor Ownership**: the system should know which processes it owns and why they exist.
- **Observable Execution**: long-running work should produce inspectable status, logs, and controls.
- **Recoverability**: process state should survive daemon restarts well enough to explain current reality.
- **Bounded Power**: Refine should run useful commands while keeping command ownership and authorization explicit.
- **Surface Independence**: process control should be available to CLI, browser, API, and agents through shared capability.

## Purpose

Process management exists because agentic software work is not just data mutation. Refine has to run target apps, agents, quality checks, import extraction, maintenance tasks, terminal sessions, and background operations.

Those processes need to be visible. A user or agent should be able to answer: what is running, who started it, what is it doing, where are its logs, can it be stopped, and did it succeed?

## Expected Role

The process capability should be the local operating substrate under workflow and tools. It should make process execution durable enough for real work without turning Refine into a general-purpose operating system.

Current implementation details that matter to intent:

- managed processes have owners such as daemon, runner, target app, agent, quality, import, maintenance, and user helper.
- process records include pid, state, label, details, output paths, limits, start time, and exit code.
- process metadata can attach workflow, Goal, session, mode, and runner context.
- workflow Goal Agents run as PTY-backed managed processes with shared transcript,
  input, resize, attention, and lifecycle state, so CLI and browser attachments
  observe the same process.
- the daemon remains a responsive control plane while supervised runners own workflow and Git synchronization waits.
- pause state can stop background processes or pause agents.
- stopping any managed, interactive, chat, or synthetic agent uses one shared
  capability that retains ownership evidence, confirms exact process exit, and
  only then atomically validates the exact claim, execution, round, Goal
  revision/status, and absence of a competing owner with cancellation of the
  linked Goal; missing registration-time PID identity, mismatched or stale
  workflow ownership, and failed or resisted stops leave the process supervised
  and the Goal active, while post-exit ownership failures retain a partial
  outcome receipt with exit, registry/identity cleanup, Goal, cause, and
  supported recovery evidence.
- the browser System and Processes surfaces read shared process state rather than inventing their own status.

Process management should favor visibility and recovery over hiding execution behind polished UI messages. If something is running, failing, or waiting, Refine should be able to show it.

## Future Direction

As agent fleets grow, process management should evolve toward orchestration: resource limits, queues, priorities, remote nodes, cancellation, isolation, provenance, and health checks.

The future process layer should support many agents and target apps without losing local debuggability. Superintelligent systems may automate most decisions, but they will still need an observable substrate that explains what was launched, what happened, and what changed.
