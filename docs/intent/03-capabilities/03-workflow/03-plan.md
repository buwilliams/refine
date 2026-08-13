# Plan

## Key Ideas

- **Pinned Context**: planning uses the product, constitution, rules, guidance, prior Rounds, current Round, and repository state captured for this attempt.
- **Independent Critique**: proposal, critique, and finalization are distinct agent phases.
- **No Repository Mutation**: Plan produces durable execution evidence before implementation begins.

## Purpose

Plan turns an actionable Round into a governed implementation strategy. It makes assumptions and risks inspectable before an implementation agent changes files.

## Expected Role

After Todo admission, Workflow pins the exact Goal, Round, workflow revision, Git base, and project context. Fresh managed agents propose a plan, critique it independently, and finalize a concise checklist. Each artifact is persisted before the next phase begins. Planning agents must leave the repository unchanged.

A completed final plan advances the same Round to Implement. Provider, parsing, authority, or persistence failures preserve their evidence and move the Goal to Failed; they do not create automatic Governance recovery Rounds.
