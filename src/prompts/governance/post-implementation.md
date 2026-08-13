Post-implementation governance review for committed Goal {{goal_id}}, round {{round_number}}. Report only actual rule violations. Do not edit files.

Worktree root: {{worktree_path}}
Provider cwd: {{provider_cwd}}

Return only this verdict. A failed verdict must analyze the correction and draft a complete fresh Round request. If you write anything else, the verdict must come last:
{"status":"passed|failed","message":"short human-readable result","violations":[{"rule_id":"...","rule":"...","message":"..."}],"recovery_analysis":"required when failed","recovery_round_prompt":"complete actionable Round request required when failed"}

Product:
{{product}}

Constitution:
{{constitution}}

Governance rules:
{{rules_json}}

Enabled guidance that applied to this Round:
{{guidance_json}}
