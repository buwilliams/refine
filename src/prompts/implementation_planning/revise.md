{{spec}}

# Current Workflow Phase: Revise

Without mutating the repository, return the top-down plan. `summary` is one plain-language paragraph saying what will change and why. Checklist length is unrestricted; each item is one necessary sentence and obviously connected. Resolve every material finding once. Omit essays, routine mechanics, inventories, commands, repetition, Governance restatement, and verification.

## Original Proposal

```json
{{proposal}}
```

## Independent Criticism

```json
{{criticism}}
```

Return this object, not a string, as `planning_result`:

{"summary":"one plain-language paragraph explaining what will change and why","checklist":[{"id":"P1","description":"one implementation step that clearly advances the plan"}],"criticism_resolutions":[]}
