# Plan

## Key Ideas

- **Pinned Context**: planning uses the Goal, current Round, repository state, parameters, and attached Skills selected for the occurrence.
- **Skill-Owned Method**: the default Plan Skill chooses how to investigate and challenge its solution before finalizing it.
- **Independent Results**: multiple Plan Skills contribute separate checklists with stable namespaced item IDs.
- **No Repository Mutation**: Plan produces durable execution evidence before implementation begins.

## Purpose

Plan turns an actionable Round into an inspectable implementation strategy. It makes assumptions and verification visible before an implementation agent changes files.

## Expected Role

After Todo admission, Workflow pins the Goal, Round, authorized step occurrence, and Git base. `workflow.plan.enter` launches its applicable Skills. Refine supplies each Plan Skill's result contract and collects every accepted plan; no fixed proposal, criticism, revision, or advisory Governance agent sequence is imposed.

A Plan Skill must leave the repository unchanged and return an actionable checklist. Invalid responses emit Plan Error with raw output, diagnostics, and process receipts retained. Refine does not repair the report or repeat the failed work. A blocked or invalid Plan result cannot advance Implement. Infrastructure, authority, and contract failures remain distinct from a Governance finding.

The finalized plan remains in the existing Round evidence model, allowing historical proposal, criticism, and revision artifacts to remain readable. Implement consumes the collected checklist and reports actual change and verification evidence for each item. An explicitly requested recovery Round invokes the Plan Skill with the reviewed recovery request and retained candidate, then continues through the full Quality and Governance gates.

## Future Direction

Let projects improve planning through their Skills and Event composition while maintaining concise, reviewable outcomes and durable evidence.
