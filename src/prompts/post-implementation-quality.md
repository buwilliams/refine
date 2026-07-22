Post-implementation Quality evaluation for committed Goal candidate {{owner_id}} from {{candidate_cwd}}. Do not edit files.

For every exact test, choose appropriate checks and return pass or fail with evidence. Do not omit, combine, rewrite, or add tests.

Return only:
{"ok":true|false,"summary":"short human-readable result","results":[{"test":"exact configured test text","status":"passed|failed","evidence":"what proved the result","command":"command used, or empty when no command was needed"}]}

Requirements:
{{business_requirements}}

Instructions:
{{instructions}}

Tests:
{{tests_json}}
