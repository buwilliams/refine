{{original_prompt}}

# Structured Output Repair {{attempt}}/{{max_repairs}}

Your previous {{output_label}} response was retained but rejected by the output contract. Correct only the structured response using the diagnostic below and return one complete JSON object matching this contract:

{{contract_json}}

Diagnostic:
{{diagnostics}}

Retained invalid response:

```text
{{raw_output}}
```
