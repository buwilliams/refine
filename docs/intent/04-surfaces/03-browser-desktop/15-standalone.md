# Standalone

## Key Ideas

- **Unattached Exploration**: standalone lets users and agents discuss or experiment before a Gap exists.
- **Worktree Backed**: implementation experiments should happen in an isolated Git worktree when possible.
- **Draftable Output**: useful standalone conversations should be convertible into concrete Gaps.
- **Ready-Merge Handoff**: standalone work can become ready-merge work without losing its worktree evidence.
- **Not A Dead-End Chat**: standalone should feed the Refine workflow when work becomes concrete.

## Purpose

The Standalone surface exists for broad agent collaboration that is not yet attached to a specific Gap. It supports exploration, planning, debugging, and experiments that may later become work items.

This matters because not all useful work starts as a clean Gap. Refine should let ideas form while preserving a path back into durable workflow.

## Expected Role

Standalone should be a bridge between conversation and structured work. It should support agent turns, transcript capture, Gap drafting, worktree lifecycle, and ready-merge submission.

Current implementation details that matter to intent:

- standalone is a permanent toolbar tab;
- standalone chat sessions can create attached Git worktrees;
- provider turns can run in the attached standalone worktree;
- users can draft a standalone transcript into a Gap;
- users can submit standalone worktree output as a ready-merge Gap;
- submitted worktrees are preserved for merge handoff rather than cleaned immediately.

Standalone should not become a separate product mode disconnected from Gaps. Its successful output should become durable work.

## Future Direction

Future standalone may become the primary place for agents to explore large design changes, propose Feature decompositions, run experiments, and then convert the useful parts into structured workflow.

As AI becomes stronger, standalone should preserve the creative space for discovery while still enforcing the product's need for durable state, evidence, and handoff.
