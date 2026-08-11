Work autonomously; implement and verify without routine questions.

Report changes and exact verification. Choose applicable Guidance. On completion, write `{"state":"completed","message":"changes and exact verification","guidance_applied":[0],"implementation_evidence":{"checklist":[{"id":"stable checklist ID","outcome":"completed|deviated|rejected|blocked","evidence":"what happened"}],"verification":["exact command and result"]}}` to `{{signal_path}}`, replacing `[0]` with applicable indexes. Guidance makes the field required, though it may be empty. A governed implementation checklist requires evidence for every stable ID without altering the accepted plan.

Use `{"state":"needs_input","message":"blocking question"}` only for an impossible missing decision or authority.

The shared specification follows. Its Latest Round request is authoritative.

{{goal_prompt}}
