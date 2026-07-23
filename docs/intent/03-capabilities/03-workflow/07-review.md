# Review

## Key Ideas

- **Judgment Boundary**: review is where evidence, risk, and intent are evaluated.
- **Human Or Agent Review**: review may involve people, agents, or both.
- **Ordering Boundary**: review can unblock later ordered Feature work when appropriate.

## Purpose

Review exists because not every decision can be reduced to command output. A change may build and pass checks while still needing judgment about product fit, governance, risk, maintainability, or user impact.

## Expected Role

Review should preserve the Goal's intent, implementation evidence, Ready Merge target commit and push evidence, rebuild outcome, quality output, governance concerns, diffs, and conversation. It should support accepting the integrated result, requesting more work from the current target state, creating follow-up rounds, failing the Goal, or moving toward completion.

Review is a meaningful workflow boundary. For ordered Features, review can represent enough completion to let later work proceed without pretending the entire process is done.

## What Happens

When a Goal is in review:

- Refine presents the work's intent, changed files, diffs, logs, quality output, governance concerns, and agent notes for judgment.
- A user, agent, or future review policy evaluates whether the work satisfies the Goal.
- Review approval marks the already-integrated result done and must not merge or push it again. Declining acceptance uses a new auditable round or supported recovery path from the current target state without rewriting integration history.
- Ordered Feature work may be allowed to proceed once review represents enough completion for the next Goal to start.
- Review decisions should be preserved as evidence, not treated as transient UI state.

## Future Direction

Future review should support agent reviewers, structured review evidence, risk summaries, dependency impact, approval policies, and generated follow-up Goals. It should remain the state where judgment is explicit.
