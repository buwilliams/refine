# Upgrade Refine Source

Outcome: a running Refine source checkout advances to its latest configured
upstream commit only after the candidate builds, then restarts and reports
healthy. Published-release updates remain unchanged.

## Preconditions

- The invoked `bin/refine` belongs to the same source checkout being inspected.
- The controller checkout is on a branch with a reachable configured remote.
- The controller checkout has no staged, unstaged, or untracked changes.
- The fetched commit is a fast-forward descendant of the current commit.
- The installed upgrade Agent can pause new workflow admission and observe or
  safely reconcile preserved active work on the selected port.

Do not stash, reset, merge, or discard work to satisfy these checks. Resolve a
dirty or divergent checkout explicitly before retrying.

This workflow is unavailable for a gitless published product home. Use the
published-release update workflow there; do not add Git metadata or infer a
source checkout from the caller's working directory.

## UI Workflow

When the running Refine checkout is discoverable, the source-update control in
the main navigation provides the same operation regardless of the attached
target app. Cached reads never run Git fetch. Automatic refresh runs at most
once per hour by default; duplicate manual clicks and clients share one
supervised fetch.

1. Use the enabled main-navigation refresh icon, or open **Node → Refine (dev)**
   and find **Upgrade** for the detailed status.
2. In the detailed view, select **Check for source updates**. Confirm the checkout, current commit,
   upstream remote/branch, and available commit.
3. If the panel reports a blocker, resolve it without overwriting work and
   check again.
4. Select **Upgrade Refine** once. This authorizes the installed maintenance
   Agent; there is no confirmation dialog.
5. Keep the page open or return to it later. The panel reconnects and polls the
   durable operation state through the daemon restart.
6. Require the final message `Latest source promoted and Refine is healthy`.

## CLI Parity

Inspect without fetching:

```sh
./r system source-status --port 8082 --runtime-root run
```

Fetch and re-evaluate availability:

```sh
./r system source-status --fetch --port 8082 --runtime-root run
```

Queue the same external handoff used by the UI:

```sh
./r system source-promote --port 8082 --runtime-root run
```

The command returns the durable operation id before the daemon stops. The
revisioned record under `<runtime-root>/<port>/operations/` is authoritative;
`source-promotion.json` is a redacted projection repaired from it after a
crash. `run/<port>/...` is only the checkout-local default.

More precisely, `run` resolves only to the owning checkout's canonical
`<checkout>/run`; arbitrary relative or external absolute runtime roots are
rejected before a check, build, helper launch, or source mutation.

## Restart-Safe Handoff Evidence

The operation reserves a unique attempt and nonce verifier before submission.
Reservation or `restart_safe_handoff_preparing` is not proof that a helper is
live. The helper receives the operation id, attempt id, and raw nonce, then
atomically claims before delay or mutation. The registry records the expected
systemd unit, launchd label, or detached process fingerprint and a bounded
claim deadline, followed by a structured receipt. Only receipt or claim plus
exact live identity can activate `restart_safe_handoff`.

The candidate is built in isolated storage and then atomically installed at
the stable `<checkout>/bin/refine` path. Service registration continues to
point at that path across the restart. The exact prior binary is backed up for
this attempt and restored together with prior source state if activation or
health verification fails; the existing attempt/receipt protocol remains the
authority for helper liveness.

After a daemon restart, Refine adopts one exact live claimant. No claimant,
identity mismatch, stale or late claim, or ambiguous evidence settles visibly
as interrupted or failed and retryable; it never remains running because of a
stage string. A retry receives a new attempt only after the old attempt is
terminal. Public API, SSE, browser, CLI, and MCP output include only the nonce
verifier and redacted receipt, never the raw nonce.

## Failure And Recovery

- `build_candidate`: the daemon and checkout were not changed. Fix the build
  failure and check again.
- `verify_idle`: work or source state changed while the candidate built. The
  daemon and checkout were not changed; restore quiescence and check again.
- `stop_daemon`: the checkout was not advanced. Inspect the port-scoped daemon
  process records and retry only after the runtime is idle.
- `activate_source`: the helper restarts the previous daemon when possible;
  inspect the reported Git precondition failure.
- `restart_daemon`: the helper attempts to restore the prior commit and restart
  the previous daemon. Follow the persisted `recovery` text and verify with:
- `restart_safe_handoff_preparing`: inspect the operation attempt. Before the
  claim deadline, a manager job may still claim. After the deadline, zero or
  ambiguous claimants must settle interrupted or failed before retry.
- `cancelling`: cancellation has fenced late receipts and claims but is still
  observing or terminating the exact helper, restoring registration/source as
  required, and restoring the recorded workflow-admission intent. Do not call
  it cancelled until the operation becomes terminal.

```sh
./r system status --port 8082 --runtime-root run
./r system source-status --port 8082 --runtime-root run
```

Never claim success from a branch change alone; daemon health verification is
part of the operation. Final evidence must include the healthy daemon, the live
executable identity, and source status at the promoted commit; if rollback was
required, report the restored commit and executable instead.
