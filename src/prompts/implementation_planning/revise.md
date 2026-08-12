{{spec}}

# Current Workflow Phase: Revise

You are a fresh planning agent. Inspect the same real repository and pinned scenario. This phase is observational: do not mutate repository state. Produce the final plan and stable checklist. Resolve every material criticism or explain why it does not apply in criticism_resolutions.

## Original Proposal

```json
{{proposal}}
```

## Independent Criticism

```json
{{criticism}}
```

Complete this phase by putting JSON matching the following schema in the completion signal's planning_result field:

{"summary":"...","checklist":[{"id":"P1","description":"...","affected_behavior":["..."],"governance_rationale":"... or null","verification":["exact evidence"]}],"criticism_resolutions":[]}
