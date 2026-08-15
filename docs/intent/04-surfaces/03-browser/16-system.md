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
- Refine (dev) exposes source upgrade status separately from published-release status: controller checkout, current and fetched commits, upstream, blockers, check, and promote controls;
- source promotion is authoritative in the revisioned operation registry; `source-promotion.json` is a repairable redacted projection. Its bounded helper uses reserve, submit or atomic claim, durable receipt, then active-handoff phases. Systemd unit, launchd label, or detached PID/start identity and argument fingerprint remain attempt-scoped. Stage text alone is never liveness. The candidate is atomically installed at the checkout's stable `bin/refine`, and daemon registration continues to point there. A failed candidate identity check restores and verifies the exact prior binary and source, then fresh-probes that prior live identity before rollback can be called successful; partial or indeterminate registration, command, reachability, identity, cancellation, and workflow-admission restoration evidence remains visible with recovery guidance. Promotion completion likewise requires live executable identity rather than checkout state or reachability alone. The UI can reconnect after a deliberate restart, reconcile the same attempt, and resume SSE-driven updates.
- browser install, lifecycle, update, source, and provider actions use the immutable product-home and port authority captured by daemon bootstrap. A request handler cannot redirect them through its process CWD or a user-global runtime.
- the main navigation exposes a target-app-independent compact source control whenever the running checkout is discoverable. Cached status reads never fetch; stale automatic and manual checks queue one hourly, coalesced supervised fetch. An available update launches exactly one configured installed update Agent and requires no confirmation dialog.
- Refine (dev) makes semantic delivery UI-first: major, minor, and patch previews lead to an agent-operated preparation with persisted stages and a normal reviewable candidate. Publication remains a separate, explicitly confirmed action after merge, with clean-main, version/tag, credentials, remote, deployment, and GitHub-release verification.

System should not be only a toast sink. It should make local operations inspectable and reduce surprise.

## Future Direction

Future System views should summarize agent fleet activity, risk signals, interrupted work, required approvals, and recovery recommendations.

As automation grows, System should become the user's immediate situational-awareness layer.
