"""Post-rebuild Quality gate workflow tests."""
from __future__ import annotations

import sys
import threading
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, git, init_refine, make_client_repo


class FakeSubprocessManager:
    def __init__(self) -> None:
        self.launches: list[dict] = []

    def is_running(self, _gap_id: str) -> bool:
        return False

    def launch(self, **kwargs) -> None:  # noqa: ANN003
        self.launches.append(kwargs)


def main() -> int:
    tmp, client = make_client_repo("refine-post-rebuild-quality-")
    conn = init_refine(client)
    try:
        from refine_server import db, gaps
        from refine_server.dispatcher import Dispatcher

        db.set_setting(conn, "quality_enabled", "1")
        db.set_setting(conn, "quality_timing", "post_rebuild")

        fake = FakeSubprocessManager()
        lock = threading.Lock()
        failures: list[tuple[str, str, str]] = []
        dispatcher = Dispatcher(
            get_conn=lambda: conn,
            sub_mgr=fake,
            target_app_lock=lock,
            on_post_rebuild_quality_failed=(
                lambda gid, message, details: failures.append(
                    (gid, message, details or ""),
                )
            ),
        )

        gid = "01POSTREBUILDQUALITYPASSAA"
        create_indexed_gap(conn, gid, status="qa", branch=None)
        assert dispatcher._launch_quality(conn, gid, None) is True  # noqa: SLF001
        assert lock.locked()
        assert fake.launches[-1]["cwd"] == client
        assert "post-rebuild Quality gate" in fake.launches[-1]["prompt"]
        assert "Do not modify, add, or commit files" in fake.launches[-1]["prompt"]
        dirty_file = client / "qa.tmp"
        dirty_file.write_text("discard me\n", encoding="utf-8")
        fake.launches[-1]["on_finished"](gid, 0, None, True, "")
        assert not lock.locked()
        row = conn.execute(
            "SELECT status, branch_name FROM gaps_index WHERE id = ?", (gid,),
        ).fetchone()
        assert row["status"] == "review", dict(row)
        assert row["branch_name"] is None, dict(row)
        assert not dirty_file.exists()
        gap = gaps.read_gap_json(gid)
        assert gap["status"] == "review"
        assert gap["rounds"][-1]["quality_state"] == "passed"

        gid_fail = "01POSTREBUILDQUALITYFAILAA"
        create_indexed_gap(conn, gid_fail, status="qa", branch=None)
        assert dispatcher._launch_quality(conn, gid_fail, None) is True  # noqa: SLF001
        fake.launches[-1]["on_finished"](
            gid_fail,
            1,
            None,
            False,
            "verification failed",
        )
        assert not lock.locked()
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid_fail,),
        ).fetchone()
        assert row["status"] == "failed", dict(row)
        assert failures and failures[-1][0] == gid_fail, failures
        assert "Agent errored" in failures[-1][1], failures

        gid_revert = "01POSTREBUILDREVERTMERGEAA"
        create_indexed_gap(conn, gid_revert, status="failed", branch=None)
        git(client, "checkout", "-B", f"refine/{gid_revert}")
        (client / "app.txt").write_text("merged change\n", encoding="utf-8")
        git(client, "add", "app.txt")
        git(client, "commit", "-m", "implement gap")
        git(client, "checkout", "main")
        git(
            client,
            "merge",
            "--no-ff",
            f"refine/{gid_revert}",
            "-m",
            f"Merge Gap {gid_revert}\n\nRefine Gap: {gid_revert}",
        )

        from refine_server import runner as runner_mod

        runner = runner_mod.Runner()
        try:
            result = runner._do_post_rebuild_quality_revert(  # noqa: SLF001
                gid_revert,
                "QA failed",
                "expected failure details",
            )
            assert result["ok"] is True, result
            assert result["pushed"] is False, result
        finally:
            runner._conn.close()  # noqa: SLF001
        assert (client / "app.txt").read_text(encoding="utf-8") == "base\n"
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = ?", (gid_revert,),
        ).fetchone()
        assert row["status"] == "failed", dict(row)
        activity_row = conn.execute(
            "SELECT message FROM activity WHERE gap_id = ? "
            "ORDER BY id DESC LIMIT 1",
            (gid_revert,),
        ).fetchone()
        assert "reverted Gap merge" in activity_row["message"], dict(activity_row)
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("post-rebuild quality tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
