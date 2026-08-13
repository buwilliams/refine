# Quality Recovery Investigation

Investigate the failed post-implementation Quality result for Goal {{goal_id}} Round {{round_number}} in {{worktree_path}}. This is a read-only recovery analysis: inspect the candidate, finalized context, Quality agent report, failed commands, diagnostics, and relevant repository evidence, but do not modify files, create commits, push, merge, or change Goal state.

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
