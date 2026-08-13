# Cancelled

## Key Ideas

- **Intentionally Stopped**: cancelled means work should not continue.
- **Monotonic Intent**: cancellation is committed on the Goal before local cleanup.
- **Explicit Reopening**: moving a cancelled Goal to todo supersedes the cancelled attempt and permits a replacement attempt.
- **Preserved Context**: history, evidence, branches, and worktrees remain inspectable.

## Purpose

Cancelled intentionally stops work without pretending it succeeded or failed accidentally. A Goal may be irrelevant, duplicate, unsafe, out of scope, or superseded.

## Expected Role

Single and bulk cancellation use the same Goal capability. Each Goal is changed to `cancelled` and read back independently. Refine then attempts to stop matching local processes as best-effort cleanup and reports any failures without rolling the Goal back.

A stale worker cannot transition a cancelled Goal. Governance cancellation before integration prevents its first Git side effect; cancellation after integration begins remains terminal while integration may finish and preserve exact evidence. Cancellation never silently deletes history, branches, or worktrees.

Undo and shared bulk movement may explicitly reopen a cancelled Goal to todo. Reopening clears the prior attempt's settlement authority; the next worker claims the existing latest Round or a newly submitted Round with a newer authority. Logs, process records, candidate branches, and worktrees from the cancelled attempt remain evidence, but its late failure cannot fail the reopened Goal or write failure metadata onto the replacement Round.

## Future Direction

Cancellation may gain replacement links, superseded-by relationships, and smarter cleanup while remaining deliberate synchronized Goal intent rather than process state.
