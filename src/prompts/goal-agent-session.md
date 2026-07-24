{{goal_prompt}}

Work autonomously in this workflow-owned Goal Agent TUI. Implement and verify; ask nothing about routine decisions.

Choose applicable `guidance_candidates` during this turn. On completion, write `{"state":"completed","message":"changes and exact verification","guidance_applied":[0]}` to `{{signal_path}}`. Use zero-based indexes; when candidates exist the field is required and may be empty.

Only an impossible missing decision or authority permits `{"state":"needs_input","message":"blocking question"}`. Silence and uncertainty do not.
