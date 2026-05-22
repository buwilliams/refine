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
        from refine_server import (
            conflict_resolver, db, gap_writer, git_ops, recovery,
            project_state, target_app_rebuilder, verify_op,
        )
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

        rebuild_runs: list[str] = []
        rebuilder = target_app_rebuilder.TargetAppRebuilder(
            get_conn=lambda: conn,
            run_rebuild=lambda reason: rebuild_runs.append(reason) or {"ok": True},
        )
        db.set_setting(conn, "target_app_auto_rebuild", "never")
        merger = Merger(
            get_conn=lambda: conn,
            sub_mgr=FakeSubprocessManager(),
            on_worktree_merged=rebuilder.queue_for_worktree_merge,
        )

        # Successful Merge-agent path: ready-merge -> awaiting-rebuild,
        # branch/worktree cleanup, merge commit pushed to the bare remote.
        # Automatic rebuild mode `never` must not bypass the rebuild gate.
        gid_success = "01MERGESUCCESSAAAAAAAAAAA"
        branch_success = "refine/merge-success"
        make_ready_branch(
            conn, gid_success, branch_success, "feature-success.txt", "ok\n",
        )
        merger._merge_one(gid_success)
        assert db_status(conn, gid_success) == "awaiting-rebuild"
        assert rebuilder.snapshot()["queued"] is False
        assert rebuild_runs == []
        assert not git_ops.local_branch_exists(branch_success)
        assert not git_ops.worktree_exists(gid_success)
        assert "feature-success.txt" in git(client, "ls-tree", "-r", "--name-only", "origin/main").stdout
        success_messages = latest_messages(gid_success)
        assert any("Merge started" in msg for msg in success_messages), success_messages
        assert any(
            "transitioned to `awaiting-rebuild`" in msg
            for msg in success_messages
        ), success_messages

        # On-worktree-merge rebuild mode gates the host-worktree merge queue
        # while already-merged work is waiting to be rebuilt. Agent work can
        # still finish into ready-merge, but the merger must not land more
        # branches on the clean host branch until rebuild promotion clears
        # awaiting-rebuild.
        gid_blocked = "01MERGEBLOCKEDREBUILDAAA"
        branch_blocked = "refine/merge-blocked-rebuild"
        make_ready_branch(
            conn, gid_blocked, branch_blocked, "blocked-rebuild.txt", "wait\n",
        )
        queued_rebuilds: list[str] = []
        db.set_setting(conn, "target_app_auto_rebuild", "on_worktree_merge")
        gated_merger = Merger(
            get_conn=lambda: conn,
            sub_mgr=FakeSubprocessManager(),
            queue_rebuild_for_pending=(
                lambda: queued_rebuilds.append("queued") or True
            ),
        )
        gated_merger._tick()  # noqa: SLF001
        assert queued_rebuilds == ["queued"], queued_rebuilds
        assert db_status(conn, gid_blocked) == "ready-merge"
        assert git_ops.local_branch_exists(branch_blocked)
        assert "blocked-rebuild.txt" not in git(
            client, "ls-tree", "-r", "--name-only", "origin/main",
        ).stdout
        conn.execute(
            "UPDATE gaps_index SET status = 'review' WHERE id = ?",
            (gid_success,),
        )
        gap_writer.update_fields(gid_success, status="review")
        gated_merger._tick()  # noqa: SLF001
        assert db_status(conn, gid_blocked) == "awaiting-rebuild"
        assert "blocked-rebuild.txt" in git(
            client, "ls-tree", "-r", "--name-only", "origin/main",
        ).stdout
        db.set_setting(conn, "target_app_auto_rebuild", "never")

        # Safety default: a direct merge operation also parks at
        # awaiting-rebuild when the caller omits final_status.
        gid_default = "01MERGEDEFAULTSTATUSAAAAA"
        branch_default = "refine/merge-default-status"
        make_ready_branch(
            conn, gid_default, branch_default, "feature-default.txt", "ok\n",
        )
        result = verify_op.perform_verify(conn, gid_default, actor="test")
        assert result["ok"], result
        assert result["final_status"] == "awaiting-rebuild", result
        assert db_status(conn, gid_default) == "awaiting-rebuild"

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
        other_instance = project_state.create_instance("Remote Recovery Host")
        gid_other_instance = "01RECOVERYREMOTEINSTANCEAA"
        create_indexed_gap(
            conn,
            gid_other_instance,
            status="in-progress",
            instance_id=other_instance["id"],
        )
        conn.execute(
            "INSERT INTO runs "
            "(gap_id, round_idx, started_at, pid, status, failure_category) "
            "VALUES (?, 0, ?, 999998, 'running', NULL)",
            (gid_other_instance, now_iso()),
        )
        moved = recovery.reconcile_on_start(conn)
        assert moved == 1
        assert db_status(conn, gid_finished) == "ready-merge"
        assert db_status(conn, gid_orphan) == "failed"
        assert db_status(conn, gid_other_instance) == "in-progress"
        assert any(
            "in-progress → ready-merge" in msg
            for msg in latest_messages(gid_finished)
        ), latest_messages(gid_finished)
        run = conn.execute(
            "SELECT status, failure_category, finished_at FROM runs "
            "WHERE gap_id = ? ORDER BY id DESC LIMIT 1",
            (gid_orphan,),
        ).fetchone()
        assert run["status"] == "killed"
        assert run["failure_category"] == "runner_restart"
        assert run["finished_at"]
        other_run = conn.execute(
            "SELECT status, failure_category, finished_at FROM runs "
            "WHERE gap_id = ? ORDER BY id DESC LIMIT 1",
            (gid_other_instance,),
        ).fetchone()
        assert other_run["status"] == "running"
        assert other_run["failure_category"] is None
        assert other_run["finished_at"] is None

        gid_runtime_local = "01RECOVERYRUNTIMELOCALAA"
        create_indexed_gap(conn, gid_runtime_local, status="in-progress")
        conn.execute(
            "INSERT INTO runs "
            "(gap_id, round_idx, started_at, pid, status, failure_category) "
            "VALUES (?, 0, ?, 999997, 'running', NULL)",
            (gid_runtime_local, now_iso()),
        )
        gid_runtime_remote = "01RECOVERYRUNTIMEREMOTEA"
        create_indexed_gap(
            conn,
            gid_runtime_remote,
            status="in-progress",
            instance_id=other_instance["id"],
        )
        conn.execute(
            "INSERT INTO runs "
            "(gap_id, round_idx, started_at, pid, status, failure_category) "
            "VALUES (?, 0, ?, 999996, 'running', NULL)",
            (gid_runtime_remote, now_iso()),
        )
        moved = recovery.reconcile_runtime_in_progress(conn, live_gap_ids=set())
        assert moved == 1
        assert db_status(conn, gid_runtime_local) == "failed"
        assert db_status(conn, gid_runtime_remote) == "in-progress"
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
