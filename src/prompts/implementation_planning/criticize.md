{{spec}}

# Current Workflow Phase: Criticize

You are a fresh, independent critic. Inspect the same real repository and pinned scenario. This phase is observational: do not mutate repository state. Find material omissions, incorrect assumptions, cross-surface inconsistencies, failure or recovery gaps, and Governance conflicts. This is model judgment, not a deterministic checklist verdict.

## Proposed Plan

```json
{{proposal}}
```

Complete this phase by putting JSON matching the following schema in the completion signal's planning_result field:

{"summary":"...","findings":[{"id":"C1","material":true,"checklist_item_ids":["P1"],"description":"...","recommendation":"..."}]}
