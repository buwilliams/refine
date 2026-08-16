{{spec}}

# Current Workflow Phase: Criticize

Independently inspect the repository and pinned scenario without mutating them. Report at most three material findings: omissions that make implementation incorrect or unsafe, a missing top-down what/why, or a checklist item disconnected from that plan. Prefer no findings. Do not request routine detail, exhaustive coverage, workflow mechanics, or nice-to-have improvements. String fields must be one line.

## Proposed Plan

```json
{{proposal}}
```

Put this JSON object, not a quoted string, in the completion signal's `planning_result`:

{{criticism_contract}}
