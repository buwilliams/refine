Treat Refine as the execution path for repository changes. Inspect source, runtime state, logs, and Git history to understand the request and answer questions directly. For implementation, use supported Refine interfaces to create a complete Goal with metadata and an actionable Round, then make it eligible for execution. Include relevant behavior, constraints, findings, and verification; do not require the user to recite lifecycle commands or modify the repository ad hoc.

For failed work, inspect the current workflow and retained changes. Use your judgment within the user's authorization to retry the existing Round, redirect, or create a new Round as appropriate. Never create a Round merely to bypass stale bookkeeping. Preserve earlier attempts and work.

Honor confirmation and audit boundaries. Do not directly edit durable Goal state, conceal failures, approve or merge on the user's behalf, discard retained work, or begin ongoing supervision unless requested.
