"""Background dispatcher: scans for `todo` Gaps and launches subprocesses
up to the parallel-run cap.

Reasons we may not launch even when work is available:
- Pool is paused (settings.paused = 1)
- Cap is already at the limit
- Pre-flight failed and Gap's last failure was auth-related (Retry pre-flight rule)
"""
from __future__ import annotations

import sqlite3
import threading
import time
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone

from refine_shared import activity, db
from refine_shared.gaps import now_iso, read_gap_json

from . import gap_writer, git_ops, preflight, subprocess_mgr
from .friendly_outcome import classify_outcome


@dataclass
class Dispatcher:
    """Polls SQLite for ready Gaps and launches subprocesses."""

    get_conn: callable  # type: ignore[type-arg]
    sub_mgr: subprocess_mgr.SubprocessManager
    # Called when an agent run finishes successfully — the merger
    # uses this signal to scan promptly for the new "awaiting merge"
    # Gap rather than waiting on its 10s poll. Optional so tests can
    # instantiate a Dispatcher without one.
    on_run_finished: callable | None = None  # type: ignore[type-arg]
    poll_interval: float = 2.0

    _stop: threading.Event = None  # type: ignore[assignment]
    _thread: threading.Thread = None  # type: ignore[assignment]

    def start(self) -> None:
        self._stop = threading.Event()
        self._thread = threading.Thread(
            target=self._loop, name="refine-dispatcher", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        if self._stop:
            self._stop.set()

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                self._tick()
            except Exception as e:
                conn = self.get_conn()
                activity.append(
                    conn,
                    message=f"Dispatcher error: {e!r}",
                    severity="error", category="cli", actor="runner",
                )
            self._stop.wait(self.poll_interval)

    def _tick(self) -> None:
        conn = self.get_conn()
        paused = db.get_setting_int(conn, "paused", 0)
        if paused:
            return
        self._promote_backlog(conn)
        cap = db.get_setting_int(conn, "parallel_run_cap", 3)
        running = len(self.sub_mgr.running_snapshot())
        if running >= cap:
            return
        # High → Medium → Low, then oldest-updated first within a tier.
        # SQLite sorts text alphabetically, which would put high < low < medium —
        # wrong order — so we explicitly map priority to an integer rank.
        rows = conn.execute(
            "SELECT id, name, branch_name FROM gaps_index "
            "WHERE status = 'todo' "
            "ORDER BY CASE priority "
            "  WHEN 'high'   THEN 0 "
            "  WHEN 'medium' THEN 1 "
            "  ELSE 2 "
            "END, updated ASC LIMIT ?",
            (cap - running,),
        ).fetchall()
        for row in rows:
            gid = row["id"]
            # already running? (race)
            if self.sub_mgr.is_running(gid):
                continue
            self._launch_one(conn, gid, row["branch_name"])

    def _promote_backlog(self, conn: sqlite3.Connection) -> None:
        """Auto-promote `backlog` Gaps to `todo` once they've sat idle for
        the configured interval. -1 = never (disabled), 0 = instant,
        otherwise seconds since the row's `updated` timestamp."""
        n = db.get_setting_int(conn, "backlog_promote_after_seconds", 3600)
        if n < 0:
            return
        if n == 0:
            rows = conn.execute(
                "SELECT id FROM gaps_index WHERE status = 'backlog'"
            ).fetchall()
        else:
            cutoff = (datetime.now(timezone.utc) - timedelta(seconds=n)).strftime(
                "%Y-%m-%dT%H:%M:%SZ"
            )
            rows = conn.execute(
                "SELECT id FROM gaps_index "
                "WHERE status = 'backlog' AND updated <= ?",
                (cutoff,),
            ).fetchall()
        if not rows:
            return
        ts = now_iso()
        for row in rows:
            gid = row["id"]
            with db.transaction(conn):
                cur = conn.execute(
                    "UPDATE gaps_index SET status = 'todo', updated = ? "
                    "WHERE id = ? AND status = 'backlog'",
                    (ts, gid),
                )
            if cur.rowcount:
                activity.append(
                    conn,
                    message="Auto-promoted from backlog to todo",
                    severity="info", category="state",
                    gap_id=gid, actor="runner",
                )

    def _launch_one(self, conn: sqlite3.Connection, gap_id: str,
                    existing_branch: str | None) -> None:
        # Retry pre-flight: if last failure was auth, re-check first. This is
        # a "soft" abort — leave the Gap in todo so a successful re-check
        # picks it up automatically.
        last = conn.execute(
            "SELECT failure_category FROM runs WHERE gap_id = ? "
            "ORDER BY id DESC LIMIT 1",
            (gap_id,),
        ).fetchone()
        if last and last["failure_category"] == "auth":
            ok, msg = preflight.check(conn)
            if not ok:
                activity.append(
                    conn,
                    message="Retry blocked — auth pre-flight still failing",
                    severity="warn", category="auth",
                    gap_id=gap_id, actor="runner", details=msg or "",
                )
                return

        # Pre-checks: target branch + upstream. The agent's worktree is
        # based off the same branch `verify` will merge back into — by
        # default that's the host's checked-out branch, but the operator
        # can pin it via the `merge_target_branch` setting (e.g. on a
        # monorepo where you want all Gaps to merge to `main` regardless
        # of what the host happens to be on).
        target = (db.get_setting(conn, "merge_target_branch") or "").strip()
        if target:
            if not git_ops.local_branch_exists(target):
                self._abort_to_failed(
                    conn, gap_id,
                    f"Configured merge_target_branch `{target}` does not "
                    f"exist locally — create/track it first or clear the setting",
                    category="git",
                )
                return
        else:
            host_branch = git_ops.current_branch()
            if host_branch is None:
                self._abort_to_failed(
                    conn, gap_id,
                    "Client repo is in detached-HEAD state and no "
                    "merge_target_branch is configured — pickup aborted",
                    category="git",
                )
                return
            target = host_branch
        # An upstream is nice-to-have, not required. Without one we
        # operate in local-only mode: skip the fetch, base the worktree
        # off the local branch's HEAD, and (later) skip the push at
        # verify time. The Gap still ships locally.
        has_upstream = git_ops.upstream_branch(target) is not None
        if has_upstream:
            r = git_ops.fetch()
            if not r.ok:
                self._abort_to_failed(
                    conn, gap_id, "git fetch failed",
                    category="git", details=r.stderr[:2000],
                )
                return
            base_ref = f"origin/{target}"
        else:
            activity.append(
                conn,
                message=(f"Branch `{target}` has no upstream — running "
                         f"in local-only mode (skipping fetch / push)."),
                severity="info", category="git",
                gap_id=gap_id, actor="runner",
            )
            base_ref = target

        # Pre-launch validation: has this round's work already been
        # merged into target? One `Refine Gap: <gap_id>` trailered
        # merge commit lands on target per completed round, so the
        # count divides cleanly per round. If existing merges already
        # cover the latest round, skip the agent run — the work is
        # done, just queue the Gap for human approval.
        if self._maybe_skip_already_implemented(conn, gap_id, target):
            return

        # Compute the branch name + worktree.
        pattern = db.get_setting(conn, "branch_name_pattern", "refine/{gap_id}") or "refine/{gap_id}"
        branch_name = existing_branch or pattern.format(gap_id=gap_id)

        wt = git_ops.create_worktree(gap_id, base_ref, branch_name)
        if not wt.ok:
            self._abort_to_failed(
                conn, gap_id, "git worktree create failed",
                category="git", details=wt.stderr[:2000],
            )
            return

        # Read the Gap and compute the prompt from the latest round.
        gap = read_gap_json(gap_id)
        if not gap or not gap.get("rounds"):
            self._abort_to_failed(
                conn, gap_id, "Gap has no rounds — cannot launch",
                category="state",
            )
            return
        round_idx = len(gap["rounds"]) - 1
        latest = gap["rounds"][round_idx]
        prompt = _format_prompt(latest)

        # Capture base for "no commits produced" detection.
        rev = git_ops._run(
            ["rev-parse", "HEAD"], cwd=git_ops.gap_worktree_path(gap_id),
        )
        base_commit = rev.stdout.strip() if rev.ok else base_ref

        # Transition: todo → in-progress
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'in-progress', updated = ?, branch_name = ? "
                "WHERE id = ? AND status = 'todo'",
                (now_iso(), branch_name, gap_id),
            )
        activity.append(
            conn,
            message="Agent run started",
            severity="info", category="cli",
            gap_id=gap_id, actor="runner",
        )

        idle = db.get_setting_int(conn, "agent_idle_timeout_seconds", 900)
        cap = db.get_setting_int(conn, "agent_hard_cap_seconds", 86400)
        # The agent runs inside the operator-configured sub-project when set
        # (e.g. a monorepo's `apps/web`). Worktree creation + base_ref + on-
        # finished git plumbing all stay at the worktree root above; only
        # the Claude subprocess cwd changes.
        worktree_root = git_ops.gap_worktree_path(gap_id)
        agent_subpath = db.get_setting(conn, "agent_subpath") or ""
        agent_cwd = git_ops.apply_agent_subpath(
            worktree_root, agent_subpath,
            log=lambda msg: activity.append(
                conn, message=f"agent_subpath: {msg}",
                severity="warn", category="state",
                gap_id=gap_id, actor="runner",
            ),
        )

        self.sub_mgr.launch(
            gap_id=gap_id,
            round_idx=round_idx,
            prompt=prompt,
            cwd=agent_cwd,
            base_ref=base_commit,
            idle_window=idle,
            hard_cap=cap,
            on_finished=lambda gid, code, reason, agent_ok: self._on_finished(
                gid, round_idx, code, reason, base_commit,
                agent_reported_success=agent_ok,
            ),
        )

    def _maybe_skip_already_implemented(self, conn: sqlite3.Connection,
                                          gap_id: str, target: str) -> bool:
        """Pre-launch idempotency guard. Returns True if we skipped
        the launch because the latest round's work is already on the
        target branch.

        The signal: count merge commits on `target` whose body carries
        `Refine Gap: <gap_id>`. Each completed round produces exactly
        one such commit, so:

            n_merges >= n_rounds  ⇒  the latest round is already done

        Round 1 with no prior merges: 0 merges, 1 round → 0 < 1 → run.
        Round 2 after round 1 merged: 1 merge, 2 rounds → 1 < 2 → run.
        Round 1 with a leftover merge (e.g., runner crashed after
        verify): 1 merge, 1 round → 1 >= 1 → skip.

        On skip we send the Gap to `review` directly (todo → review,
        skipping in-progress) since the work is already on target and
        the human just needs to approve it. No worktree created, no
        agent spawned.
        """
        gap = read_gap_json(gap_id)
        if not gap:
            return False
        rounds = gap.get("rounds") or []
        n_rounds = len(rounds)
        if n_rounds == 0:
            return False
        n_merges = git_ops.count_refine_merges_for_gap(gap_id, target)
        if n_merges < n_rounds:
            return False

        round_idx = n_rounds - 1
        msg = (f"Skipped agent run — this round's work is already on "
               f"`{target}` ({n_merges} merge commit"
               f"{'' if n_merges == 1 else 's'} for this Gap). "
               f"Moved straight to review for human approval.")
        # Log to the latest round's logs[] so the audit trail shows
        # why we bypassed the agent for this specific round.
        try:
            gap_writer.append_round_log(
                gap_id=gap_id, round_idx=round_idx,
                severity="info", category="state", actor="runner",
                message=msg,
            )
        except Exception:
            pass
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'review', updated = ? "
                "WHERE id = ? AND status = 'todo'",
                (now_iso(), gap_id),
            )
        activity.append(
            conn, message=msg,
            severity="info", category="state",
            gap_id=gap_id, actor="runner",
        )
        return True

    def _abort_to_failed(self, conn: sqlite3.Connection, gap_id: str,
                         message: str, *, category: str,
                         details: str | None = None) -> None:
        """Log a pre-launch failure and move the Gap to `failed` so the
        dispatcher stops re-attempting it every tick. The user can Reopen the
        Gap once the underlying environment issue is resolved."""
        activity.append(
            conn, message=message,
            severity="error", category=category,
            gap_id=gap_id, actor="runner", details=details or "",
        )
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed', updated = ? "
                "WHERE id = ? AND status = 'todo'",
                (now_iso(), gap_id),
            )

    def _on_finished(self, gap_id: str, round_idx: int, exit_code: int,
                     killed_reason: str | None, base_commit: str,
                     *, agent_reported_success: bool | None = None) -> None:
        conn = self.get_conn()
        cwd = git_ops.gap_worktree_path(gap_id)
        new_commits = git_ops.commits_on_branch_since(base_commit, cwd)
        no_new_commits = new_commits == 0

        outcome = classify_outcome(
            exit_code=exit_code,
            killed_reason=killed_reason,
            no_new_commits=no_new_commits,
            agent_reported_success=agent_reported_success,
        )

        success = outcome.kind == "success"

        # Failure path: move straight to `failed` and we're done.
        # Success path: transition to `ready-merge`. That's the
        # system-owned status — picked up by the Merger, never set or
        # cleared by the user. The dispatcher owns the
        # `in-progress -> ready-merge` flip; the merger owns
        # `ready-merge -> done` (clean verify) and
        # `ready-merge -> review` (verify needed human attention).
        next_status = "ready-merge" if success else "failed"
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = ?, updated = ? WHERE id = ?",
                (next_status, now_iso(), gap_id),
            )

        try:
            gap_writer.append_round_log(
                gap_id=gap_id,
                round_idx=round_idx,
                severity="info" if success else outcome.severity,
                category=outcome.category,
                message=outcome.message,
                details=outcome.details,
                actor="runner",
            )
        except Exception:
            pass

        activity.append(
            conn,
            message=outcome.message,
            severity="info" if success else outcome.severity,
            category=outcome.category,
            gap_id=gap_id, actor="runner",
            details=outcome.details,
        )

        # Auto-verify is owned by the Merger now — a single-threaded
        # worker that serializes every operation on the host worktree.
        # The dispatcher just flipped the Gap to `ready-merge`; tell
        # the merger to scan promptly instead of waiting on its
        # periodic tick. The Gap stays in `ready-merge` until the
        # merger lands it in `done` (clean verify) or punts it to
        # `review` (verify needed human attention).
        if success and self.on_run_finished is not None:
            try:
                self.on_run_finished(gap_id)
            except Exception:
                pass


def _format_prompt(round_obj: dict) -> str:
    return (
        f"You are working on a software change.\n\n"
        f"Current behavior (actual):\n{round_obj.get('actual','').strip()}\n\n"
        f"Desired behavior (target):\n{round_obj.get('target','').strip()}\n\n"
        f"Make the necessary code changes in this worktree. Commit your changes "
        f"with clear messages. Run any relevant tests. When you're satisfied, exit."
    )
