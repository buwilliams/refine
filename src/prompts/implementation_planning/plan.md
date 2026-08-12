{{spec}}

# Current Workflow Phase: Plan

Inspect the real repository and the complete pinned scenario above. This phase is observational: do not edit files, create commits, change branches, or mutate repository state. Propose one actionable implementation plan. Use stable checklist IDs, name affected behavior and surfaces, explain Governance relevance, and name intended verification evidence. Complete this phase by putting JSON matching the following schema in the completion signal's planning_result field:

{"summary":"...","checklist":[{"id":"P1","description":"...","affected_behavior":["..."],"governance_rationale":"... or null","verification":["exact evidence"]}],"criticism_resolutions":[]}
