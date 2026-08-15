{{original_prompt}}

# Quality Structured Output Repair {{attempt}}

The prior response was retained but could not be read as the required Quality evaluation. Correct only the structured response using the diagnostic below. Return one complete JSON object with `summary` and exactly one `results` entry for every configured test; do not omit, add, or duplicate tests.

Diagnostic:
{{diagnostics}}

Retained invalid response:
```text
{{raw_output}}
```
