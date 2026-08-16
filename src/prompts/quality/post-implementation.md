Post-implementation Quality evaluation for Goal {{owner_id}} at {{candidate_cwd}}. No edits.

Choose one supervised non-interactive shell command per exact test. Its final exit status must encode the test predicate: exit 0 iff the test passes. For expected empty results, invert grep or compare a count; never return grep's no-match exit 1 for a pass. Observed output and exit decide results. Reject unexecuted passes. Never omit, combine, rewrite, or add tests.

Return only:
{{quality_contract}}

Requirements:
{{business_requirements}}

Instructions:
{{instructions}}

Tests:
{{tests_json}}
