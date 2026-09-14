# Quality

## Purpose

Quality assesses and corrects the work using the Goal, current context, and applicable user-defined Skills. Blocking failures, including pre-existing ones, are work to fix and verify before reporting success. An unsuccessful outcome means an unresolved blocker, not merely that tests initially failed. Governance reviews the overall result. The AI decides which investigation, edits, or checks are useful and when to stop. Users need not supply acceptance criteria or test lists.

## Behavior

Refine runs the enabled blocking Skills attached to Quality and follows their decisions. Skills may describe particular outcomes or checks for the AI to interpret. Supporting reports are optional context: Refine does not grade their completeness, execute commands found in them, or override the decision by scoring their contents.

If no Quality Skill applies, Quality succeeds without launching an agent. An absent Skills configuration never invokes the retired Quality evaluator. Old settings remain available for configuration migration or history; they do not independently authorize work.

Skill instructions and current user authorization determine permitted edits and supported workflow changes. Workflow Quality can correct work in the admitted Goal checkout. Verification of a finalized candidate remains bound to that exact candidate; a decision about one version does not authorize another version.

## Coordination and History

Manual and workflow Quality share operation ownership, capacity limits, process cancellation, and candidate identity checks. The recorded Goal, Round, node, workspace, and candidate determine where results belong. A superseding workflow decision invalidates late results. Restart retains completed decisions, failed attempts, logs, and worktrees without silently retrying failed work.

Missing or mismatched candidate infrastructure is reported separately from an AI finding. Cancellation prevents new work and waits for owned processes to stop before settlement. Persistence failures remain visible and recoverable without pretending a result was saved.

For already-integrated work, reuse a retained decision only for the exact source candidate. If a fresh evaluation is needed, inspect that candidate in an isolated checkout; a descendant on the shared target is not a substitute. Retain the first failed evaluation and require an explicit workflow decision before another attempt.

Failures preserve the AI's report and execution history. An authorized agent or user chooses whether to retry the current Round, redirect, or create a new Round. Refine does not mandate a recovery report, append a Round automatically, or impose a recovery budget based on the number of findings.

## Configuration

Skills use the shared revisioned definition service across browser, API, and CLI. Migration preserves old instructions and settings before converting them to Skills. Existing user-authored instructions remain user-owned.
