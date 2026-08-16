# Plan

## Key Ideas

- **Pinned Context**: planning uses the product, constitution, rules, guidance, prior Rounds, current Round, and repository state captured for this attempt.
- **Independent Critique**: proposal, critique, and finalization are distinct agent phases.
- **No Repository Mutation**: Plan produces durable execution evidence before implementation begins.

## Purpose

Plan turns an actionable Round into a governed implementation strategy. It makes assumptions and risks inspectable before an implementation agent changes files.

## Expected Role

After Todo admission, Workflow pins the exact Goal, Round, workflow revision, Git base, and project context. Fresh managed agents propose a plan, critique it independently, and finalize a concise checklist. Each artifact is persisted before the next phase begins. Planning agents must leave the repository unchanged. When proposal, criticism, or revision output cannot be parsed or validated, Workflow durably records the raw attempt and diagnostic, then makes at most two diagnostic repair invocations while retaining the last valid phase artifact.

On providers whose CLI accepts a caller-chosen interactive session identifier, the plan phase pins a provider-native session; revision and the later implementation launch resume it so accumulated repository context carries forward, while criticism always runs fresh to preserve its independent judgment. A lost provider session degrades to a fresh launch rather than failing the Round.

When governance is configured, an advisory plan-stage governance pre-check judges the finalized plan before any implementation spend. Violations are folded back into the criticize-and-revise contract as material findings for one additional revision; a plan that still fails settles the Round as a planning failure, and an unreadable pre-check verdict is recorded as inconclusive while the Round proceeds. The post-implementation Governance gate stays authoritative.

A Quality- or Governance-finding recovery Round skips the proposal, criticism, and revision phases entirely: the drafted recovery request is already a reviewed delta against a candidate both gates have judged, so Workflow synthesizes and persists its final plan deterministically and the Round spends its agent budget on the fix. Every later gate still runs unchanged.

A completed final plan advances the same Round to Implement. Provider, repair-exhausted output-contract, authority, or persistence failures preserve their distinct evidence and move the Goal to Failed; they do not create automatic Governance recovery Rounds.
