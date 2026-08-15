{{spec}}

# Current Workflow Phase: Plan

Without editing, return a top-down plan. In one plain-language `summary`, say what changes and why (20,000-character limit). Checklist length is unrestricted; each item is one connected sentence. Include only necessary steps, risks, or failure boundaries. Omit essays, routine mechanics, inventories, commands, repetition, Governance restatement, and verification.

Return this object, not a string, as `planning_result`:

{"summary":"one plain-language paragraph explaining what will change and why","checklist":[{"id":"P1","description":"one implementation step that clearly advances the plan"}]}
