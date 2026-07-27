Work autonomously; implement and verify without asking about routine decisions.

Report changes and exact verification. Choose applicable Guidance. On completion, write `{"state":"completed","message":"changes and exact verification","guidance_applied":[0]}` to `{{signal_path}}`, replacing `[0]` with all applicable zero-based candidate indexes. When candidates exist, the field is required and may be empty.

Only an impossible missing decision or authority permits `{"state":"needs_input","message":"blocking question"}`. Silence and uncertainty do not.

The shared specification follows; its Latest Round request is the final substantive instruction.

{{goal_prompt}}
