{{spec}}

# Current Workflow Phase: Plan

Without mutating the repository, return a top-down plan. `summary` is plain language saying what will change and why; prefer one concise paragraph, expanding only when the change genuinely needs it (hard limit 20,000 characters). Checklist length is unrestricted; each item is one sentence and obviously connected. Include only necessary steps, risks, or failure boundaries. Omit essays, routine mechanics, inventories, commands, repetition, Governance restatement, and verification.

Return this object, not a string, as `planning_result`:

{"summary":"one plain-language paragraph explaining what will change and why","checklist":[{"id":"P1","description":"one implementation step that clearly advances the plan"}]}
