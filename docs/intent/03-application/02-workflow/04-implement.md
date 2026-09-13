# Implement

## Key Ideas

- **Plan-Guided Change**: a fresh managed agent receives the finalized plan and pinned Round context.
- **Isolated Candidate**: implementation occurs on the Goal branch and worktree.
- **Explicit Handoff**: the agent knows that independent Quality and Governance stages follow.

## Purpose

Implement converts the finalized plan into a committed, reviewable candidate without assigning quality or governance judgment to the implementation agent.

## Expected Role

The implementation AI changes the isolated worktree, follows the Goal and applicable Skill instructions, and judges when to stop. Refine follows its reported decision without requiring checklist IDs, per-item evidence, or verification commands. Optional planning and implementation reports remain available as context and history. Refine records the exact candidate commit, branch, target branch, and base commit before advancing to Quality.

The implementation agent may run useful checks and use supported workflow controls when its Skill instructions and current intent call for a change. It does not integrate through an implementation result. A workflow decision supersedes that invocation and the daemon stops it; a late result cannot advance replacement work. Failed verdicts retain their evidence and follow Error handling.

Implement uses the same workspace preparation and evidence applicability rules as other executable steps. Missing or inconsistent generated inputs can record a recovery to Plan under the existing authored Round; retaining unusable implementation work is not required. See the [shared consistency contract](11-consistency-contract.md).
