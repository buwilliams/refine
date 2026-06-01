"""Git merge and runner-recovery tests for user-visible Gap outcomes."""
from __future__ import annotations

import sqlite3
import subprocess
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
            run_rebuild=lambda reason, _cancel_event=None: rebuild_runs.append(reason) or {"ok": True},
        )
        db.set_setting(conn, "target_app_auto_rebuild", "never")
        merger = Merger(
            get_conn=lambda: conn,
            sub_mgr=FakeSubprocessManager(),
            on_worktree_merged=rebuilder.queue_for_worktree_merge,
        )
        other_instance = project_state.create_node("Remote Recovery Host")
        gid_remote_ready = "01MERGEREMOTEREADYAAAAAA"
        create_indexed_gap(
            conn,
            gid_remote_ready,
            status="ready-merge",
            branch="refine/remote-ready",
            node_id=other_instance["id"],
        )
        assert merger._find_one_ready() is None  # noqa: SLF001
        assert merger.snapshot()["queued"] == 0

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

        # A conflicted stash apply/pop leaves unmerged index entries without a
        # MERGE_HEAD sentinel. The merger must clear that once before picking
        # up ready Gaps; otherwise every queued Gap fails on the dirty tree.
        gid_stash_conflict = "01MERGESTASHCONFLICTAAAA"
        branch_stash_conflict = "refine/stash-conflict-cleanup"
        make_ready_branch(
            conn,
            gid_stash_conflict,
            branch_stash_conflict,
            "stash-conflict-gap.txt",
            "merged after cleanup\n",
        )
        (client / "stash-conflict.txt").write_text("base\n", encoding="utf-8")
        git(client, "add", "stash-conflict.txt")
        git(client, "commit", "-m", "stash conflict base")
        (client / "stash-conflict.txt").write_text("stashed\n", encoding="utf-8")
        git(client, "stash", "push", "-m", "test conflicting stash")
        (client / "stash-conflict.txt").write_text("upstream\n", encoding="utf-8")
        git(client, "add", "stash-conflict.txt")
        git(client, "commit", "-m", "stash conflict upstream")
        (client / "host-wip.txt").write_text("base\n", encoding="utf-8")
        (client / "host-staged.txt").write_text("base\n", encoding="utf-8")
        git(client, "add", "host-wip.txt", "host-staged.txt")
        git(client, "commit", "-m", "host wip base")
        (client / "host-wip.txt").write_text("dirty\n", encoding="utf-8")
        (client / "host-staged.txt").write_text("staged\n", encoding="utf-8")
        git(client, "add", "host-staged.txt")
        (client / "host-untracked.txt").write_text(
            "untracked\n",
            encoding="utf-8",
        )
        apply = subprocess.run(
            ["git", "stash", "apply"],
            cwd=client,
            capture_output=True,
            text=True,
        )
        assert apply.returncode != 0, apply.stdout + apply.stderr
        stuck = git_ops.in_progress_op()
        assert stuck and stuck[0] == "unmerged-index", stuck
        conn.execute(
            "UPDATE gaps_index SET status = 'review' "
            "WHERE status = 'awaiting-rebuild'"
        )
        merger._tick()  # noqa: SLF001
        assert git_ops.in_progress_op() is None
        assert git_ops.unmerged_paths() == []
        stash_list = git(client, "stash", "list").stdout
        assert "refine cleanup auto-stash" in stash_list, stash_list
        stash_files = git(
            client,
            "stash",
            "show",
            "--include-untracked",
            "--name-only",
            "stash@{0}",
        ).stdout.splitlines()
        assert "host-wip.txt" in stash_files, stash_files
        assert "host-staged.txt" in stash_files, stash_files
        assert "host-untracked.txt" in stash_files, stash_files
        assert db_status(conn, gid_stash_conflict) == "awaiting-rebuild"
        origin_files = git(
            client, "ls-tree", "-r", "--name-only", "origin/main",
        ).stdout
        assert "stash-conflict-gap.txt" in origin_files

        # A merge-stage failure should be recoverable without re-running the
        # implementation agent: failed -> ready-merge wakes the normal Merger.
        gid_retry_merge = "01MERGERETRYMERGEAAAAAAA"
        branch_retry_merge = "refine/retry-merge"
        make_ready_branch(
            conn, gid_retry_merge, branch_retry_merge,
            "retry-merge.txt", "retry\n",
        )
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed' WHERE id = ?",
                (gid_retry_merge,),
            )
        gap_writer.update_fields(gid_retry_merge, status="failed")
        gap_writer.append_latest_round_log(
            gap_id=gid_retry_merge,
            severity="warn",
            category="state",
            actor="runner",
            message=(
                "Workflow status changed: ready-merge → failed; "
                "Local branch diverged from remote"
            ),
        )
        gap_writer.append_latest_round_log(
            gap_id=gid_retry_merge,
            severity="info",
            category="git",
            actor="runner",
            message="Diagnostic log after merge failure",
        )
        from refine_server.runner import Runner

        runner = Runner()
        wakeups: list[str] = []

        class WakeOnlyMerger:
            def wake(self) -> None:
                wakeups.append("wake")

        runner.merger = WakeOnlyMerger()  # type: ignore[assignment]
        try:
            result = runner._h_retry_merge({"gap_id": gid_retry_merge})  # noqa: SLF001
        finally:
            runner._conn.close()  # noqa: SLF001
        assert result["ok"], result
        assert wakeups == ["wake"], wakeups
        assert db_status(conn, gid_retry_merge) == "ready-merge"
        retry_messages = latest_messages(gid_retry_merge)
        assert any("failed → ready-merge" in msg for msg in retry_messages), retry_messages

        gid_agent_failed = "01MERGEAGENTFAILEDRETRYAA"
        branch_agent_failed = "refine/agent-failed-retry"
        make_ready_branch(
            conn, gid_agent_failed, branch_agent_failed,
            "agent-failed-retry.txt", "agent\n",
        )
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET status = 'failed' WHERE id = ?",
                (gid_agent_failed,),
            )
        gap_writer.update_fields(gid_agent_failed, status="failed")
        gap_writer.append_latest_round_log(
            gap_id=gid_agent_failed,
            severity="warn",
            category="state",
            actor="runner",
            message="Workflow status changed: in-progress → failed; agent errored",
        )
        runner = Runner()
        try:
            result = runner._h_retry_merge({"gap_id": gid_agent_failed})  # noqa: SLF001
        finally:
            runner._conn.close()  # noqa: SLF001
        assert not result["ok"], result
        assert "failed merge attempt" in result["message"], result
        assert db_status(conn, gid_agent_failed) == "failed"

        # Remote target advances while a Gap branch is waiting, and the host has
        # dirty `.refine` state. The Merge agent must update target first; if it
        # commits `.refine` before pulling, `git pull --ff-only` sees a false
        # local/remote divergence and fails the Gap.
        gid_remote_advance = "01MERGEREMOTEADVANCEAAAA"
        branch_remote_advance = "refine/remote-advance"
        make_ready_branch(
            conn,
            gid_remote_advance,
            branch_remote_advance,
            "feature-remote-advance.txt",
            "gap\n",
        )
        peer = tmp / "peer"
        git(tmp, "clone", str(tmp / "origin.git"), "peer")
        git(peer, "config", "user.email", "t@x")
        git(peer, "config", "user.name", "t")
        (peer / "remote-advance.txt").write_text("remote\n", encoding="utf-8")
        git(peer, "add", "remote-advance.txt")
        git(peer, "commit", "-m", "remote advance")
        git(peer, "push")
        (client / ".refine" / "dirty-before-merge.txt").write_text(
            "local refine state\n",
            encoding="utf-8",
        )
        merger._merge_one(gid_remote_advance)
        assert db_status(conn, gid_remote_advance) == "awaiting-rebuild"
        origin_files = git(
            client, "ls-tree", "-r", "--name-only", "origin/main",
        ).stdout
        assert "remote-advance.txt" in origin_files
        assert "feature-remote-advance.txt" in origin_files
        assert ".refine/dirty-before-merge.txt" in origin_files

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

        # Recovery: finished in-progress run is promoted to ready-merge when
        # QA is disabled; orphan in-progress run is failed with its run row
        # marked killed.
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
        gid_other_instance = "01RECOVERYREMOTEINSTANCEAA"
        create_indexed_gap(
            conn,
            gid_other_instance,
            status="in-progress",
            node_id=other_instance["id"],
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
            node_id=other_instance["id"],
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
