"""User-visible Gap lifecycle state tests."""
from __future__ import annotations

import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, git, init_refine, make_client_repo


class FakeSubprocessManager:
    def __init__(self) -> None:
        self.cancelled: list[str] = []

    def running_snapshot(self) -> list[dict]:
        return []

    def is_running(self, _gap_id: str) -> bool:
        return False

    def cancel(self, gap_id: str, reason: str = "cancel") -> bool:
        self.cancelled.append(f"{gap_id}:{reason}")
        return True


class NoopDispatcher:
    def enforce_now(self) -> None:
        pass


class NoopGovernance:
    def wake(self) -> None:
        pass


def status(conn, gap_id: str) -> str:
    row = conn.execute(
        "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    return row["status"]


def launchable_worktree(gap_id: str, branch: str) -> tuple[Path, str]:
    from refine_server import git_ops

    base = git(Path.cwd(), "rev-parse", "HEAD").stdout.strip()
    result = git_ops.create_worktree(gap_id, "main", branch)
    assert result.ok, result.stderr
    return git_ops.gap_worktree_path(gap_id), base


def commit_in_worktree(worktree: Path, filename: str, text: str) -> None:
    (worktree / filename).write_text(text, encoding="utf-8")
    git(worktree, "add", filename)
    git(worktree, "commit", "-m", f"change {filename}")


def main() -> int:
    tmp, client = make_client_repo("refine-lifecycle-")
    conn = init_refine(client)
    try:
        from refine_server import db, gap_writer, git_ops, verify_op
        from refine_server.dispatcher import Dispatcher
        from refine_server.runner import Runner

        fake_sub = FakeSubprocessManager()
        merger_wakeups: list[str] = []
        dispatcher = Dispatcher(
            get_conn=lambda: conn,
            sub_mgr=fake_sub,
            on_run_finished=lambda gid: merger_wakeups.append(gid),
        )

        # Agent success with real new commits moves in-progress -> ready-merge.
        gid_success = "01LIFECYCLESUCCESSAAAAAA"
        branch_success = "refine/lifecycle-success"
        create_indexed_gap(
            conn, gid_success, status="in-progress", branch=branch_success,
        )
        wt, base = launchable_worktree(gid_success, branch_success)
        commit_in_worktree(wt, "success.txt", "changed\n")
        dispatcher._on_finished(gid_success, 0, 0, None, base)
        assert status(conn, gid_success) == "ready-merge"
        assert merger_wakeups == [gid_success]

        # Clean exit with no commits is a user-visible failure unless the
        # provider explicitly reported success/no-op.
        gid_no_change = "01LIFECYCLENOCHANGEAAAA"
        branch_no_change = "refine/lifecycle-no-change"
        create_indexed_gap(
            conn, gid_no_change, status="in-progress", branch=branch_no_change,
        )
        wt_no_change, base_no_change = launchable_worktree(
            gid_no_change, branch_no_change,
        )
        dispatcher._on_finished(gid_no_change, 0, 0, None, base_no_change)
        assert status(conn, gid_no_change) == "failed"

        gid_noop_success = "01LIFECYCLENOOPPASSAAAA"
        branch_noop = "refine/lifecycle-noop"
        create_indexed_gap(
            conn, gid_noop_success, status="in-progress", branch=branch_noop,
        )
        _wt_noop, base_noop = launchable_worktree(gid_noop_success, branch_noop)
        dispatcher._on_finished(
            gid_noop_success,
            0,
            0,
            None,
            base_noop,
            agent_reported_success=True,
        )
        assert status(conn, gid_noop_success) == "ready-merge"

        for reason, expected_fragment in (
            (None, "exit 2"),
            ("idle", "stuck"),
            ("hard_cap", "wall-clock cap"),
        ):
            gid = f"01LIFECYCLEFAIL{(reason or 'EXIT').upper():0<8}"[:26]
            branch = f"refine/{gid.lower()}"
            create_indexed_gap(conn, gid, status="in-progress", branch=branch)
            _wt, base_ref = launchable_worktree(gid, branch)
            dispatcher._on_finished(gid, 0, 2, reason, base_ref)
            assert status(conn, gid) == "failed"
            gap = gap_writer.shared_gaps.read_gap_json(gid)
            latest_logs = gap["rounds"][-1]["logs"]
            assert any(expected_fragment in log["message"] for log in latest_logs)

        # Preemption/paused cancellation resets in-progress work to todo and
        # removes partial branch/worktree state.
        gid_preempt = "01LIFECYCLEPREEMPTAAAAAA"
        branch_preempt = "refine/lifecycle-preempt"
        create_indexed_gap(
            conn, gid_preempt, status="in-progress", branch=branch_preempt,
        )
        wt_preempt, base_preempt = launchable_worktree(gid_preempt, branch_preempt)
        commit_in_worktree(wt_preempt, "partial.txt", "partial\n")
        dispatcher._on_finished(
            gid_preempt, 0, -15, "priority_preempted", base_preempt,
        )
        row = conn.execute(
            "SELECT status, branch_name FROM gaps_index WHERE id = ?",
            (gid_preempt,),
        ).fetchone()
        assert row["status"] == "todo"
        assert row["branch_name"] is None
        assert not git_ops.worktree_exists(gid_preempt)
        assert not git_ops.local_branch_exists(branch_preempt)

        # Human follow-up from review appends a new round and returns to todo.
        runner = Runner()
        runner.dispatcher = NoopDispatcher()  # type: ignore[assignment]
        runner.governance_agent = NoopGovernance()  # type: ignore[assignment]
        gid_followup = "01LIFECYCLEFOLLOWUPAAAAA"
        create_indexed_gap(conn, gid_followup, status="review")
        runner._h_append_round({
            "gap_id": gid_followup,
            "reporter": "Reviewer",
            "actual": "Still not right",
            "target": "Handle the edge case",
        })
        assert status(conn, gid_followup) == "todo"
        followup = gap_writer.shared_gaps.read_gap_json(gid_followup)
        assert len(followup["rounds"]) == 2
        assert followup["rounds"][-1]["reporter"] == "Reviewer"

        # Human Verify approves only a parked review Gap with no live branch.
        result = verify_op.approve_review(conn, gid_followup)
        assert result["ok"] is False
        assert "not awaiting review" in result["message"]
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'review', branch_name = NULL "
                "WHERE id = ?",
                (gid_followup,),
            )
        result = verify_op.approve_review(conn, gid_followup)
        assert result["ok"] is True, result
        assert status(conn, gid_followup) == "done"

        # Cancel is available from non-terminal states and records the terminal
        # cancelled state while asking the subprocess manager to stop work.
        runner.sub_mgr = fake_sub  # type: ignore[assignment]
        gid_cancel = "01LIFECYCLECANCELAAAAAAA"
        create_indexed_gap(conn, gid_cancel, status="in-progress")
        runner._h_cancel({"gap_id": gid_cancel})
        assert status(conn, gid_cancel) == "cancelled"
        assert any(call.startswith(gid_cancel + ":") for call in fake_sub.cancelled)
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("lifecycle state tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
