# Quality Recovery Investigation

Investigate failed post-implementation Quality for Goal {{goal_id}} Round {{round_number}} in {{worktree_path}}. Read only: inspect the candidate, finalized context, agent report, failed commands, diagnostics, and repository evidence; do not modify files, commit, push, merge, or change Goal state.

Determine the implementation or test correction needed in a fresh Round. Return only one JSON object:
{"recovery_analysis":"concise evidence-based cause and required correction","recovery_round_prompt":"complete actionable request for the next implementation Round"}

Pinned context:
```json
{{context_json}}
```

Quality agent report:
{{quality_agent_report}}

Quality result:
```json
{{quality_json}}
```
