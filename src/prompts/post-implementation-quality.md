Post-implementation Quality evaluation for committed Goal candidate {{owner_id}} from {{candidate_cwd}}. Do not edit files.

For every exact test, choose one non-interactive shell command. Refine executes it as a supervised Quality process and treats the observed exit and output as authoritative. A pass without execution is rejected. Do not omit, combine, rewrite, or add tests.

Return only:
{"ok":true|false,"summary":"short human-readable result","results":[{"test":"exact configured test text","status":"passed|failed","evidence":"what the command is intended to prove","command":"required non-interactive shell command"}]}

Requirements:
{{business_requirements}}

Instructions:
{{instructions}}

Tests:
{{tests_json}}
