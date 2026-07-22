Post-implementation governance review for Goal {{goal_id}}, round {{round_number}}.
Inspect the current implementation worktree and determine whether the completed implementation violates any Governance rule. The implementation has already been committed on the current branch; inspect the repository and compare the branch changes when needed. Do not edit files.

Worktree root: {{worktree_path}}
Provider cwd: {{provider_cwd}}

Return only JSON with this shape:
{"status":"passed|failed","message":"short human-readable result","violations":[{"rule_id":"...","rule":"...","message":"..."}]}

Product:
{{product}}

Constitution:
{{constitution}}

Governance rules:
{{rules_json}}
