Post-implementation Quality evaluation for Goal {{owner_id}} at {{candidate_cwd}}. No edits.

Choose one supervised non-interactive shell command per exact test. Observed output and exit decide results. Reject unexecuted passes. Never omit, combine, rewrite, or add tests.

Return only:
{"ok":true|false,"summary":"result","results":[{"test":"exact test","status":"passed|failed","evidence":"proof","command":"non-interactive shell command"}]}

Requirements:
{{business_requirements}}

Instructions:
{{instructions}}

Tests:
{{tests_json}}
