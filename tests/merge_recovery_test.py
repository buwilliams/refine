"""Git merge and runner-recovery tests for user-visible Gap outcomes."""
from __future__ import annotations

import sqlite3
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, git, init_refine, make_client_repo


class FakeSubprocessManager:
    def running_snapshot(self) -> list[dict]:
        return []

    def is_running(self, _gap_id: str) -> bool:
        return False

    def cancel(self, _gap_id: str, reason: str = "cancel") -> bool:
        return False


def db_status(conn, gap_id: str) -> str:
    row = conn.execute(
        "SELECT status FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    return row["status"]


def make_ready_branch(conn, gap_id: str, branch: str, filename: str,
                      contents: str) -> None:
    from refine_server import git_ops

    create_indexed_gap(conn, gap_id, status="ready-merge", branch=branch)
    result = git_ops.create_worktree(gap_id, "main", branch)
    assert result.ok, result.stderr
    wt = git_ops.gap_worktree_path(gap_id)
    (wt / filename).write_text(contents, encoding="utf-8")
    git(wt, "add", filename)
    git(wt, "commit", "-m", f"gap {gap_id}")


def latest_messages(gap_id: str) -> list[str]:
    from refine_server import gaps

    gap = gaps.read_gap_json(gap_id)
    assert gap is not None
    return [log["message"] for log in gap["rounds"][-1]["logs"]]


def main() -> int:
    tmp, client = make_client_repo("refine-merge-", with_remote=True)
    conn = init_refine(client)
    conn.close()
    try:
        from refine_server import conflict_resolver, db, git_ops, recovery
        from refine_server.gaps import now_iso
        from refine_server.merger import Merger
        from refine_server.paths import sqlite_path

        conn = sqlite3.connect(
            str(sqlite_path()),
            isolation_level=None,
            check_same_thread=False,
            timeout=5.0,
        )
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode = WAL")
        conn.execute("PRAGMA synchronous = NORMAL")
        conn.execute("PRAGMA foreign_keys = ON")

        rebuild_queue: list[str] = []
        merger = Merger(
            get_conn=lambda: conn,
            sub_mgr=FakeSubprocessManager(),
            on_worktree_merged=rebuild_queue.append,
        )

        # Successful Merge-agent path: ready-merge -> awaiting-rebuild,
        # branch/worktree cleanup, merge commit pushed to the bare remote.
        gid_success = "01MERGESUCCESSAAAAAAAAAAA"
        branch_success = "refine/merge-success"
        make_ready_branch(
            conn, gid_success, branch_success, "feature-success.txt", "ok\n",
        )
        merger._merge_one(gid_success)
        assert db_status(conn, gid_success) == "awaiting-rebuild"
        assert rebuild_queue == [gid_success]
        assert not git_ops.local_branch_exists(branch_success)
        assert not git_ops.worktree_exists(gid_success)
        assert "feature-success.txt" in git(client, "ls-tree", "-r", "--name-only", "origin/main").stdout

        # Merge conflict path: unresolved conflict is recoverable user work, so
        # the Gap fails instead of becoming review-ready or blocking queue.
        gid_conflict = "01MERGECONFLICTAAAAAAAAAA"
        branch_conflict = "refine/merge-conflict"
        make_ready_branch(
            conn, gid_conflict, branch_conflict, "app.txt", "branch change\n",
        )
        (client / "app.txt").write_text("main change\n", encoding="utf-8")
        git(client, "add", "app.txt")
        git(client, "commit", "-m", "main conflicting change")
        git(client, "push")
        old_resolver = conflict_resolver.attempt_auto_resolve
        try:
            conflict_resolver.attempt_auto_resolve = lambda *a, **k: {
                "ok": False,
                "message": "auto-resolve failed",
                "details": "conflict remains",
            }
            merger._merge_one(gid_conflict)
        finally:
            conflict_resolver.attempt_auto_resolve = old_resolver
        assert db_status(conn, gid_conflict) == "failed"
        assert any("Merge conflict" in msg for msg in latest_messages(gid_conflict))

        # Push failure path: the merge may have landed locally, but the Gap
        # is not considered review-ready because it was not deployed remotely.
        gid_push = "01MERGEPUSHFAILAAAAAAAAAA"
        branch_push = "refine/merge-push-fail"
        make_ready_branch(conn, gid_push, branch_push, "push-fail.txt", "push\n")
        hook = tmp / "origin.git" / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\necho rejected by test >&2\nexit 1\n", encoding="utf-8")
        hook.chmod(0o755)
        merger._merge_one(gid_push)
        assert db_status(conn, gid_push) == "failed"
        assert any("Push failed" in msg for msg in latest_messages(gid_push))

        # Recovery: finished in-progress run is promoted to ready-merge; orphan
        # in-progress run is failed with its run row marked killed.
        gid_finished = "01RECOVERYFINISHEDAAAAAAA"
        create_indexed_gap(conn, gid_finished, status="in-progress")
        conn.execute(
            "INSERT INTO runs "
            "(gap_id, round_idx, started_at, finished_at, status, failure_category) "
            "VALUES (?, 0, ?, ?, 'finished', NULL)",
            (gid_finished, now_iso(), now_iso()),
        )
        gid_orphan = "01RECOVERYORPHANAAAAAAAAA"
        create_indexed_gap(conn, gid_orphan, status="in-progress")
        conn.execute(
            "INSERT INTO runs "
            "(gap_id, round_idx, started_at, pid, status, failure_category) "
            "VALUES (?, 0, ?, 999999, 'running', NULL)",
            (gid_orphan, now_iso()),
        )
        moved = recovery.reconcile_on_start(conn)
        assert moved == 1
        assert db_status(conn, gid_finished) == "ready-merge"
        assert db_status(conn, gid_orphan) == "failed"
        run = conn.execute(
            "SELECT status, failure_category, finished_at FROM runs "
            "WHERE gap_id = ? ORDER BY id DESC LIMIT 1",
            (gid_orphan,),
        ).fetchone()
        assert run["status"] == "killed"
        assert run["failure_category"] == "runner_restart"
        assert run["finished_at"]
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("merge and recovery tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
