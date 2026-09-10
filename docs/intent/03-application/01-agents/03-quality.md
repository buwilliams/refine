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

Quality should sit between implementation and trust. Every committed Goal candidate receives a Quality evaluation. Quality instructions and expected outcomes are authored in Skills bound to `workflow.quality.enter`. The default Quality Skill is seeded from existing instructions and tests. Target-app lifecycle commands retain their separate operational purpose.

Current implementation details that matter to intent:

- all blocking Quality Skills contribute independent results; every accepted check command receives an observed pass or fail result;
- the Quality agent should choose one non-interactive command for each test whose final status is `0` if and only if that test passes; expected-empty predicates must invert or compare tools such as `grep` so a successful no-match cannot surface as exit `1`, and a pass without a correlated observed execution should fail;
- the provider and test commands should be correlated with one durable operation ID, and process registration should share the cancellation barrier so no work can launch after cancellation wins;
- manual and workflow evaluation of the same Goal candidate should share one exclusive operation owner and identical Goal-round evidence;
- manual evaluation should validate active Node ownership and reserve the same Node, provider, target-app, and global agent capacity used by workflow;
- each operation should retain its originating target-app and Refine-state identity so restart recovery cannot write evidence into a subsequently selected app;
- evaluation should persist an exact identity commitment and revalidate it before and after supervised checks: `isolated_candidate` requires the current Goal Round, candidate branch, registered worktree path, clean checkout, and candidate HEAD. A sibling Goal advancing a shared target does not change that isolated evaluation identity. Passed evidence includes a versioned proof bound to Goal ID, zero-based Round, evaluation scope, operation ID, checked candidate, source candidate, state, timestamp, and recorded results;
- an unreadable Quality response receives at most two diagnostic repair invocations. Every invalid raw response, diagnostic, and provider process receipt remains in the durable Event invocation; Quality diagnostics link to that evidence. Repair exhaustion links the Round to that operation as an output-contract fault, provider launch failure remains a provider fault, and neither is converted into a failed implementation finding;
- already-merged reconciliation accepts retained exact-candidate proof or normalizes legacy fields only when they reconstruct the complete identity above. Otherwise it creates a clean managed checkout at the exact candidate and runs isolated Quality again; ancestry or a shared-target descendant never supplies Quality approval;
- the first valid failed isolated evaluation from that already-merged regeneration is terminal and fail-closed. Refine durably retains its exact candidate and source identity, operation, timestamp, results, diagnostics, and provider attempts, and restart settlement reuses that artifact without launching another evaluation or allowing a later result to replace it;
- implementation worktree creation should register a node-local candidate handoff under the repository maintenance lock. That cleanup-visible owner remains active without a gap through implementation, Quality, and Governance, and cleanup cannot reclaim the current candidate checkout until Governance successfully integrates it. A newer recovery Round may supersede the retained handoff only after its successor checkout has acquired ownership;
- missing or mismatched candidate infrastructure should be distinct from a candidate-test failure and from `quality_command_harness_fault`. If no check ran, Quality remains unclassified and has no fabricated result; the operation records `quality_candidate_infrastructure_fault`, retains any observed process logs, and preserves the branch and checkout without cleanup, reset, or ad-hoc recreation;
- workflow should use quality evidence before moving work toward merge or done;
- failed Quality evidence should project one bounded, deterministic human-readable summary from
  supervised structured results and diagnostics, naming the failed test and observed cause when
  available while retaining the complete structured evidence for inspection;
- failures should be visible in logs, System, Goal evidence, or review surfaces;
- a valid failed verdict should trigger a separate read-only investigation that records an evidence-based cause, drafts a complete next-Round request, and returns the Goal to Todo; Quality and Governance share the configured five-Round automatic recovery budget, after which a remaining finding moves the Goal to Failed;
- provider, parsing, harness, candidate-identity, authority, and infrastructure failures should fail visibly without creating or consuming an automatic recovery Round;
- quality settings should be shared project context, not hidden UI state;
- Quality success requires nonempty supervised evidence; missing blocking Quality Skills or check commands are configuration or contract errors.

`automation/config.json` holds the authoritative Event and Skill definitions. Goal Quality evaluates the isolated candidate before Governance. Migration archives existing Quality instructions and test policy, including deduplicated enabled legacy commands from every node. Previously enforced commands remain supervised until the imported default Quality Skill is deliberately edited or replaced. Legacy files remain migration evidence and do not regain configuration authority.

Browser Settings, API Skills, and `refine skills` use the same revision-fenced definition service. Instructions are multiline plain text. Invalid definitions and stale revisions fail without erasing the current configuration.

Quality should be strict enough to reveal risk and flexible enough to fit different projects. Refine should not assume every app has the same test command, build step, or verification style.

## Future Direction

Future quality should become more evidence-aware. Agents may generate targeted tests, infer missing checks, summarize failures, compare screenshots, validate performance, inspect security risk, and attach proof to Goals.

As AI improves, quality should become one of the main ways Refine earns trust: every autonomous action should leave enough evidence for people and other agents to understand why the system believes the work is ready.
