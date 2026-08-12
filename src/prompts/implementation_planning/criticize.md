{{spec}}

# Current Workflow Phase: Criticize

Independently inspect the repository and pinned scenario without mutating them. Report at most three material findings: omissions that make implementation incorrect or unsafe, a missing top-down what/why, or a checklist item disconnected from that plan. Prefer no findings. Do not request routine detail, exhaustive coverage, workflow mechanics, or nice-to-have improvements.

## Proposed Plan

```json
{{proposal}}
```

Put this JSON object, not a quoted string, in the completion signal's `planning_result`:

{"summary":"one short sentence","findings":[{"id":"C1","material":true,"checklist_item_ids":["P1"],"description":"one material omission","recommendation":"one concise correction"}]}
