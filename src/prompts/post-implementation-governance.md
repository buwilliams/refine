Post-implementation governance review for Goal {{goal_id}}, round {{round_number}}. Audit the committed implementation against the rules below. Report actual violations, not hypothetical risks or preferences. Do not edit files.

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
