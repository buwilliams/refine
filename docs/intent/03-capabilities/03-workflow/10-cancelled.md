# Cancelled

## Key Ideas

- **Intentionally Stopped**: cancelled means work should not continue unless reopened or replaced.
- **Preserved Context**: cancellation should explain why the work stopped.
- **Safe Exit**: cancellation should release claims, stop relevant execution, and preserve evidence.

## Purpose

Cancelled exists so Refine can intentionally stop work without pretending it succeeded or failed accidentally. A Gap may become irrelevant, duplicate, unsafe, out of scope, or superseded by a different plan.

## Expected Role

Cancelled should remove work from active automation while preserving the reason, history, logs, and relationships needed for future understanding.

Cancellation should be visible to workflow, Features, agents, and surfaces. If cancelled work affected ordered Features or active processes, those effects should be handled explicitly.

## What Happens

When a Gap is cancelled:

- Refine removes it from active workflow consideration.
- Active claims, relevant processes, and pending automation should be stopped or released where appropriate.
- The cancellation reason, history, logs, and relationships should remain inspectable.
- Feature rollups and ordered work should account for the cancellation explicitly.
- Future work may replace or supersede the cancelled Gap, but cancellation should not silently delete the record.

## Future Direction

Future cancellation behavior may support replacement links, superseded-by relationships, automatic cleanup, and agent-suggested cancellation when work becomes obsolete. The state should remain a deliberate stop, not silent disappearance.
