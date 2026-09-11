# Implement

## Key Ideas

- **Plan-Guided Change**: a fresh managed agent receives the finalized plan and pinned Round context.
- **Isolated Candidate**: implementation occurs on the Goal branch and worktree.
- **Explicit Handoff**: the agent knows that independent Quality and Governance stages follow.

## Purpose

Implement converts the finalized plan into a committed, reviewable candidate without assigning quality or governance judgment to the implementation agent.

## Expected Role

The implementation AI changes the isolated worktree, follows the Goal and applicable Skill instructions, and judges when to stop. Refine follows its reported decision without requiring checklist IDs, per-item evidence, or verification commands. Optional planning and implementation reports remain available as context and history. Refine records the exact candidate commit, branch, target branch, and base commit before advancing to Quality.

The implementation agent may run useful checks, but it does not approve, merge, push, or advance Goal state. Failures preserve the candidate and evidence and move the Goal to Failed.
