# System

## Key Ideas

- **Canonical Local Notices**: user-visible UI notices and errors should land in System, not only transient toasts.
- **Operational Memory**: System should show recent starts, queues, completions, errors, and local actions.
- **Immediate Context**: System is for what the user needs to know now, while Logs are for deeper audit.
- **Shared Event Bridge**: early UI events should queue until the System panel is ready.

## Purpose

The System surface exists to make local Refine activity visible while the user works. It is the place to see immediate operational notices without leaving the current page.

It should prevent silent failure. If an import queues, a draft finishes, a UI error occurs, a blocking notice is produced, or a background operation changes state, System should be a natural destination.

## Expected Role

System should be the short-term operational log inside the toolbar. It should complement durable activity logs and process views.

Current implementation details that matter to intent:

- `recordUiNotice` and `recordUiError` bridge UI events into System behavior;
- pending System operations queue before toolbar initialization;
- System filters distinguish info, started, queued, completed, and errors;
- each operation identifies its status and source and preserves concrete diagnostic metadata such as Goal, Feature, operation, and error-code identifiers;
- diagnostic values and full error details remain visible and copyable so a user or agent can correlate the notice with deeper logs;
- failed blocking Goal notices and other important UI messages should be visible here.
- Refine (dev) exposes source/dogfood status separately from published-release status: controller checkout, current and fetched commits, upstream, blockers, check, and promote controls;
- source promotion persists stage-by-stage state outside the daemon so the UI can reconnect and continue polling through a deliberate restart, including failure and recovery guidance.
- when the attached target app is itself a Refine Git checkout, the main navigation exposes a compact source-refresh control; it stays disabled when source is current or promotion is blocked, enables for a safe fetched update, and queues the same shared source-promotion handoff as Refine (dev).
- Refine (dev) makes semantic delivery UI-first: major, minor, and patch previews lead to an agent-operated preparation with persisted stages and a normal reviewable candidate. Publication remains a separate, explicitly confirmed action after merge, with clean-main, version/tag, credentials, remote, deployment, and GitHub-release verification.

System should not be only a toast sink. It should make local operations inspectable and reduce surprise.

## Future Direction

Future System views should summarize agent fleet activity, risk signals, interrupted work, required approvals, and recovery recommendations.

As automation grows, System should become the user's immediate situational-awareness layer.
