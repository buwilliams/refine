# Quality

## Key Ideas

- **Evidence Before Confidence**: work should be judged by checks, logs, diffs, and reviewable outcomes.
- **Plain-Text Tests**: projects describe observable outcomes without encoding a shell runner into Quality policy.
- **Agent Evaluation**: the configured agent proposes how to evaluate each test; Refine runs the proposed command and treats the observed supervised exit and output as authoritative evidence.
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
- the Quality agent should choose one non-interactive command for each test, and a pass without a correlated observed execution should fail;
- the provider and test commands should be correlated with one durable operation ID, and process registration should share the cancellation barrier so no work can launch after cancellation wins;
- manual and workflow evaluation of the same Goal candidate should share one exclusive operation owner and identical Goal-round evidence;
- manual evaluation should validate active Node ownership and reserve the same Node, provider, target-app, and global agent capacity used by workflow;
- each operation should retain its originating target-app and Refine-state identity so restart recovery cannot write evidence into a subsequently selected app;
- evaluation should pin the recorded isolated candidate commit for `pre_merge`, or the recorded integrated target commit for `post_build`, and require matching HEAD plus a clean index and checkout before and after checks, preserving any detected user changes;
- workflow should use quality evidence before moving work toward merge or done;
- failures should be visible in logs, System, Goal evidence, or review surfaces;
- quality settings should be shared project context, not hidden UI state;
- an empty Quality test list should be an explicit successful no-op, not a reason to skip durable Quality evidence.

`quality/settings.json` is the authoritative timing and test policy. Legacy `/settings` timing reads and writes are compatibility adapters to that file rather than independent Node settings. When a candidate enters validation, workflow durably commits the effective timing to that Goal round; Ready Merge, Rebuild, QA, and retries reuse the committed decision even if project settings later change. Before migration is marked complete, Refine inspects every Node and deduplicates all enabled legacy target-app QA commands; a failed migration remains retryable. Imported commands remain enforced as supervised Quality tests until a user saves a non-empty plain-text test set, so upgrade cannot silently convert an enforced gate into a no-op. `pre_merge` evaluates the isolated candidate before Ready Merge; `post_build` evaluates the integrated target after Ready Merge and optional rebuild.

Quality should be strict enough to reveal risk and flexible enough to fit different projects. Refine should not assume every app has the same test command, build step, or verification style.

## Future Direction

Future quality should become more evidence-aware. Agents may generate targeted tests, infer missing checks, summarize failures, compare screenshots, validate performance, inspect security risk, and attach proof to Goals.

As AI improves, quality should become one of the main ways Refine earns trust: every autonomous action should leave enough evidence for people and other agents to understand why the system believes the work is ready.
