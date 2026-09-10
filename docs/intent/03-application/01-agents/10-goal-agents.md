# Goal Agents

## Key Ideas

- **Workflow Owned**: Event agents perform work for an exact Goal Round and workflow occurrence.
- **Reusable Instructions**: Skills supply plain text; Refine supplies the role's completion contract.
- **Managed Execution**: the configured provider runs through the shared supervised invocation capability.
- **Pinned Context**: each invocation retains its definitions, parameters, Goal, candidate, and node identity.
- **Replaceable Process**: loss of a process does not erase accepted results or grant new workflow authority.

## Purpose

Goal agents turn the current Round's intent into a plan, implementation, Quality evidence, and Governance findings. Refine owns scheduling, process lifecycle, checkout coordination, workflow state, and integration. Skills describe the work and its method.

## Expected Role

Workflow Enter Events select the agents for Plan, Implement, Quality, and Governance. The default configuration binds one Skill for each role. Ordered bindings launch independently with the same occurrence context and applicable context attachments. Multiple Plan Skills contribute separate namespaced checklists; Implement receives every accepted plan. Refine validates result identity and role-specific artifacts, retains invalid responses and process references, and allows at most two diagnostic repairs.

Plan and Governance agents must leave the checkout unchanged. Quality can correct the implementation during its workflow phase; exact-candidate verification is observational and Refine supervises the proposed check commands. Implement and corrective Quality use the existing Goal worktree. Checkout serialization, runtime capacity, pause controls, completion and idle timeouts, process cancellation, and current Goal authority apply to Event agents.

Normal Exit gates finish before workflow advances. Cancellation and failure remain immediate. Missing automatic parameters produce visible execution errors. Provider, contract, authority, and candidate-infrastructure faults do not become implementation findings or consume a finding-recovery Round.

Invocation history retains results, attempts, and process references. Restart can reuse settled bindings and accepted plans. A fresh claim must revalidate Goal ownership, Round, candidate, and occurrence before it launches or accepts work. Historical proposal, criticism, revision, and provider-session artifacts remain readable; they do not impose the retired fixed planning sequence on new work.

Interactive Toolbar, Plan Mode, and standalone sessions retain their own attachment and terminal contracts. Event execution is inspected and cancelled through invocation history and Processes. Opening an interactive session does not create another workflow occurrence or change an Event result.

Every automated Goal agent, including Skill bindings, diagnostic repairs, corrections, and conflict recovery, receives the admitted Round workspace explicitly. The configured `agent_subpath` must resolve to an existing directory inside that workspace and the same Git checkout; absolute paths, parent traversal, symlink escapes, and nested repositories are rejected. Recovered invocations retain their pinned workspace and revalidate its repository, Goal, Round, branch or candidate, and registration before launching or accepting results. A process resume or fresh-launch fallback cannot choose a different cwd. Workspace faults retain edits and process evidence for recovery.

## Future Direction

Improve continuity, process visibility, and fleet-aware inspection while preserving synchronized Goal authority and the evidence of the agents that actually performed the work.
