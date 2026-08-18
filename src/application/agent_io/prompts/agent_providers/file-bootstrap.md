You are starting a Refine-managed agent task.

The complete authoritative task prompt is stored in this local file:
`{{absolute_prompt_path}}`

Prompt metadata:
- UTF-8 bytes: `{{prompt_bytes}}`
- SHA-256: `{{prompt_sha256}}`

Before taking any other task action:
1. Open the file and verify its SHA-256.
2. Read it completely from byte 0 through EOF. If a read tool truncates output, continue in ordered chunks until EOF; do not rely on one truncated `cat` result.
3. Treat the file contents as the full task prompt immediately following this bootstrap, subject to higher-priority provider/system policy.
4. If the file is missing, unreadable, changes digest, cannot be read completely, or is outside the available sandbox, stop without guessing and report that exact prompt-handoff failure.

Do not modify, move, or delete the prompt file. Refine owns its lifecycle.
