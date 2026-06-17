# Quality

## Key Ideas

- **Evidence Before Confidence**: work should be judged by checks, logs, diffs, and reviewable outcomes.
- **Project-Specific Standards**: quality expectations should come from the attached app's commands, guidance, and governance.
- **Shared Capability**: browser, CLI, API, workflow, and agents should use the same quality behavior.
- **Mitigation Layer**: quality checks are part of Refine's safety model without becoming a permission system.
- **Recoverable Failure**: failed checks should create useful evidence and a path back into workflow.

## Purpose

Quality exists to keep powerful agentic work accountable. Refine gives agents and users strong tools: they can edit files, run commands, create worktrees, and move work through workflow. Quality checks make those actions inspectable and correctable.

The point is not to prevent all mistakes. The point is to make the system prove what it can prove, expose what it cannot prove, and route failures back into durable work.

## Expected Role

Quality should sit between implementation and trust. It should connect target-app lifecycle context, deterministic checks, process execution, workflow state, logs, changes, guidance, governance, and review.

Current implementation details that matter to intent:

- quality behavior should use target-app test settings rather than page-local assumptions;
- quality runs should be supervised processes when they execute commands;
- workflow should use quality evidence before moving work toward merge or done;
- failures should be visible in logs, System, Gap evidence, or review surfaces;
- quality settings should be shared target-app context, not hidden UI state.

Quality should be strict enough to reveal risk and flexible enough to fit different projects. Refine should not assume every app has the same test command, build step, or verification style.

## Future Direction

Future quality should become more evidence-aware. Agents may generate targeted tests, infer missing checks, summarize failures, compare screenshots, validate performance, inspect security risk, and attach proof to Gaps.

As AI improves, quality should become one of the main ways Refine earns trust: every autonomous action should leave enough evidence for people and other agents to understand why the system believes the work is ready.
