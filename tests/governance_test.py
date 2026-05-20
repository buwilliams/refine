"""Focused tests for Gap Governance data and scheduling behavior."""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


class FakeSubprocessManager:
    def running_snapshot(self) -> list[dict]:
        return []

    def is_running(self, _gap_id: str) -> bool:
        return False

    def cancel(self, _gap_id: str, reason: str = "cancel") -> bool:
        return False


def main() -> int:
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    tmp = Path(tempfile.mkdtemp(prefix="refine-governance-"))
    client = tmp / "client"
    client.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=client, check=True)
    subprocess.run(
        ["git", "-c", "user.email=t@x", "-c", "user.name=t",
         "commit", "--allow-empty", "-m", "init"],
        cwd=client,
        check=True,
    )
    os.chdir(client)

    try:
        from refine_server import config, db, gap_writer, gaps, governance
        from refine_server.dispatcher import Dispatcher
        from refine_server.governance_agent import GovernanceAgent

        config.write_defaults(client / ".refine")
        config.get(reload=True)
        db.init_db()
        conn = db.connect()
        db.set_setting(conn, "backlog_promote_after_seconds", "-1")

        loaded = governance.load_settings(conn)
        assert loaded["product"] == ""
        assert loaded["constitution"] == ""
        assert loaded["rules"] == []
        assert governance.is_configured(conn) is False

        saved = governance.save_settings(
            conn,
            product="Product",
            constitution="Constitution",
            rules=["Rule one", {"id": "r2", "text": "Rule two"}],
        )
        assert governance.is_configured(conn) is True
        assert [r["text"] for r in saved["rules"]] == ["Rule one", "Rule two"]

        def insert_gap(gid: str, status: str = "todo",
                       priority: str = "low") -> None:
            gap = gap_writer.create_gap(
                gap_id=gid,
                name=gid,
                initial_round=gaps.new_round("Reporter", "Actual", "Target"),
            )
            conn.execute(
                "INSERT INTO gaps_index "
                "(id, name, status, priority, reporter, created, updated, json_path) "
                "VALUES (?, ?, ?, ?, 'Reporter', ?, ?, ?)",
                (gid, gid, status, priority, gap["created"], gap["updated"],
                 f"gaps/{gid}.json"),
            )

        launched: list[str] = []
        dispatcher = Dispatcher(get_conn=lambda: conn, sub_mgr=FakeSubprocessManager())
        dispatcher._launch_one = lambda _c, gid, _b: launched.append(gid)  # type: ignore[method-assign]

        insert_gap("01AAAAAAAAAAAAAAAAAAAAAAAA", "todo", "high")
        dispatcher._tick()
        assert launched == [], launched

        gap_writer.set_latest_round_governance(
            "01AAAAAAAAAAAAAAAAAAAAAAAA",
            {
                "rule_state": "passed",
                "product_state": "pass",
                "constitution_state": "pass",
                "meta_rule_state": "none",
            },
        )
        dispatcher._tick()
        assert launched == ["01AAAAAAAAAAAAAAAAAAAAAAAA"], launched

        gap_writer.edit_latest_round(
            "01AAAAAAAAAAAAAAAAAAAAAAAA",
            actual="Changed actual",
        )
        reread = gaps.read_gap_json("01AAAAAAAAAAAAAAAAAAAAAAAA")
        latest = reread["rounds"][-1]
        assert latest["rule_state"] == "unclassified"
        assert latest["product_state"] == "unclassified"

        # Incomplete governance preserves current dispatch behavior.
        governance.save_settings(conn, product="", constitution="", rules=[])
        conn.execute("DELETE FROM gaps_index")
        shutil.rmtree(client / ".refine" / "gaps", ignore_errors=True)
        launched.clear()
        insert_gap("01BBBBBBBBBBBBBBBBBBBBBBBB", "todo", "low")
        dispatcher._tick()
        assert launched == ["01BBBBBBBBBBBBBBBBBBBBBBBB"], launched

        # Governance failure moves todo -> backlog and blocks auto-promotion.
        governance.save_settings(
            conn, product="Product", constitution="Constitution", rules=[],
        )
        conn.execute("DELETE FROM gaps_index")
        shutil.rmtree(client / ".refine" / "gaps", ignore_errors=True)
        insert_gap("01CCCCCCCCCCCCCCCCCCCCCCCC", "todo", "medium")

        old_classify = governance.classify_gap
        try:
            governance.classify_gap = lambda _conn, _gid, provider=None: governance.normalize_classification({
                "rule_state": "failed",
                "meta_rule_state": "none",
                "product_state": "fail",
                "constitution_state": "pass",
                "message": "Does not fit product direction.",
                "details": "Product mismatch.",
                "rule_actions": [],
            })
            agent = GovernanceAgent(get_conn=lambda: conn)
            agent._review_one("01CCCCCCCCCCCCCCCCCCCCCCCC")
        finally:
            governance.classify_gap = old_classify

        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01CCCCCCCCCCCCCCCCCCCCCCCC'"
        ).fetchone()
        assert row["status"] == "backlog", dict(row)
        failed_gap = gaps.read_gap_json("01CCCCCCCCCCCCCCCCCCCCCCCC")
        assert failed_gap["status"] == "backlog", failed_gap
        failed_messages = [
            log["message"] for log in failed_gap["rounds"][-1]["logs"]
        ]
        assert "Governance review started" in failed_messages, failed_messages
        assert "Does not fit product direction." in failed_messages, failed_messages
        assert any("todo → backlog" in msg for msg in failed_messages), failed_messages
        db.set_setting(conn, "backlog_promote_after_seconds", "0")
        dispatcher._promote_backlog(conn)
        row = conn.execute(
            "SELECT status FROM gaps_index WHERE id = '01CCCCCCCCCCCCCCCCCCCCCCCC'"
        ).fetchone()
        assert row["status"] == "backlog", dict(row)

        # Passing governance can auto-apply rule maintenance.
        conn.execute("DELETE FROM gaps_index")
        shutil.rmtree(client / ".refine" / "gaps", ignore_errors=True)
        governance.save_settings(conn, product="Product", constitution="Constitution", rules=[])
        insert_gap("01DDDDDDDDDDDDDDDDDDDDDDDD", "todo", "medium")
        try:
            governance.classify_gap = lambda _conn, _gid, provider=None: governance.normalize_classification({
                "rule_state": "passed",
                "meta_rule_state": "candidate_rule",
                "product_state": "pass",
                "constitution_state": "pass",
                "message": "Governance passed.",
                "rule_actions": [{"action": "add", "text": "New rule"}],
            })
            agent = GovernanceAgent(get_conn=lambda: conn)
            agent._review_one("01DDDDDDDDDDDDDDDDDDDDDDDD")
        finally:
            governance.classify_gap = old_classify
        settings = governance.load_settings(conn)
        assert [r["text"] for r in settings["rules"]] == ["New rule"], settings
        passed_gap = gaps.read_gap_json("01DDDDDDDDDDDDDDDDDDDDDDDD")
        passed_messages = [
            log["message"] for log in passed_gap["rounds"][-1]["logs"]
        ]
        assert "Governance review started" in passed_messages, passed_messages
        assert "Governance auto-applied 1 rule change" in passed_messages, passed_messages
        assert "Governance passed." in passed_messages, passed_messages
    finally:
        os.chdir(tempfile.gettempdir())
        shutil.rmtree(tmp, ignore_errors=True)

    print("governance tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
