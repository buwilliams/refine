# Quality

## Key Ideas

- **Evidence Before Confidence**: work should be judged by checks, logs, diffs, and reviewable outcomes.
- **Plain-Text Tests**: projects describe observable outcomes without encoding a shell runner into Quality policy.
- **Agent Evaluation**: the configured agent determines how to evaluate each test and reports pass or fail with evidence.
- **Shared Capability**: browser, CLI, API, workflow, and agents should use the same quality behavior.
- **Mitigation Layer**: quality checks are part of Refine's safety model without becoming a permission system.
- **Recoverable Failure**: failed checks should create useful evidence and a path back into workflow.

## Purpose

Quality exists to keep powerful agentic work accountable. Refine gives agents and users strong tools: they can edit files, run commands, create worktrees, and move work through workflow. Quality checks make those actions inspectable and correctable.

The point is not to prevent all mistakes. The point is to make the system prove what it can prove, expose what it cannot prove, and route failures back into durable work.

## Expected Role

Quality should sit between implementation and trust. Every committed Goal candidate receives a Quality evaluation. Quality uses its own project-wide plain-text tests, separate from Governance rules and target-app lifecycle commands.

Current implementation details that matter to intent:

- each configured plain-text test should receive exactly one pass or fail result;
- the Quality agent should choose the appropriate commands, inspection, or other evidence for each test;
- Quality agent runs and any commands they launch should remain supervised processes;
- workflow should use quality evidence before moving work toward merge or done;
- failures should be visible in logs, System, Goal evidence, or review surfaces;
- quality settings should be shared project context, not hidden UI state;
- an empty Quality test list should be an explicit successful no-op, not a reason to skip durable Quality evidence.

Quality should be strict enough to reveal risk and flexible enough to fit different projects. Refine should not assume every app has the same test command, build step, or verification style.

## Future Direction

Future quality should become more evidence-aware. Agents may generate targeted tests, infer missing checks, summarize failures, compare screenshots, validate performance, inspect security risk, and attach proof to Goals.

As AI improves, quality should become one of the main ways Refine earns trust: every autonomous action should leave enough evidence for people and other agents to understand why the system believes the work is ready.
