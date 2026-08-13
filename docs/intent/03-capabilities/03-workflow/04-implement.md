# Implement

## Key Ideas

- **Plan-Guided Change**: a fresh managed agent receives the finalized plan and pinned Round context.
- **Isolated Candidate**: implementation occurs on the Goal branch and worktree.
- **Explicit Handoff**: the agent knows that independent Quality and Governance stages follow.

## Purpose

Implement converts the finalized plan into a committed, reviewable candidate without assigning quality or governance judgment to the implementation agent.

## Expected Role

The implementation agent changes the isolated worktree, applies relevant guidance, and reports checklist outcomes and verification evidence. Refine records changed files, the implementation report, exact candidate commit, branch, target branch, and base commit. It then advances the Goal to Quality.

The implementation agent may run useful checks, but it does not approve, merge, push, or advance Goal state. Failures preserve the candidate and evidence and move the Goal to Failed.
