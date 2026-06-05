"""Failed Gap recovery round support tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


class NoopDispatcher:
    def enforce_now(self) -> None:
        pass


class NoopGovernance:
    def wake(self) -> None:
        pass


def main() -> int:
    tmp, client = make_client_repo("refine-failed-round-")
    conn = init_refine(client)
    try:
        from refine_server import gap_ops, gap_writer
        from refine_server.backend_protocol import M_APPEND_ROUND
        from refine_server.runner import Runner

        runner = Runner()
        runner.dispatcher = NoopDispatcher()  # type: ignore[assignment]
        runner.governance_agent = NoopGovernance()  # type: ignore[assignment]

        def runner_call(method: str, params: dict, _timeout: float) -> dict:
            assert method == M_APPEND_ROUND
            return runner._h_append_round(params)

        failed_gap = "01FAILEDROUNDRECOVERYAAAA"
        create_indexed_gap(conn, failed_gap, status="failed")

        status, body = gap_ops.append_round(conn, runner_call, failed_gap, {
            "reporter": "Reviewer",
            "actual": "Agent hit a merge conflict in the generated wrapper.",
            "target": "Resolve the wrapper conflict, rerun checks, and resubmit.",
        })
        assert status == 201, body

        row = conn.execute(
            "SELECT status, reporter, round_count FROM gaps_index WHERE id = ?",
            (failed_gap,),
        ).fetchone()
        assert dict(row) == {
            "status": "todo",
            "reporter": "Reviewer",
            "round_count": 2,
        }, dict(row)
        gap = gap_writer.shared_gaps.read_gap_json(failed_gap)
        assert gap["status"] == "todo", gap
        assert len(gap["rounds"]) == 2, gap["rounds"]
        assert gap["rounds"][-1]["reporter"] == "Reviewer"
        assert "merge conflict" in gap["rounds"][-1]["actual"]
        assert any(
            "Workflow status changed: failed \u2192 todo; new round submitted" in log.get("message", "")
            for log in gap["rounds"][-1].get("logs", [])
        ), gap["rounds"][-1].get("logs", [])

        todo_gap = "01FAILEDROUNDTODOREJECTAA"
        create_indexed_gap(conn, todo_gap, status="todo")
        status, body = gap_ops.append_round(conn, runner_call, todo_gap, {
            "reporter": "Reviewer",
            "actual": "Still wrong",
            "target": "Fix it",
        })
        assert status == 409, body
        assert "review` or `failed" in body["error"]["message"], body

        root = Path(__file__).resolve().parents[1]
        gaps_detail_js = (root / "refine_ui/static/js/features/gaps-detail.js").read_text(encoding="utf-8")
        toolbar_js = (root / "refine_ui/static/js/features/toolbar.js").read_text(encoding="utf-8")
        gap_ops_py = (root / "refine_server/gap_ops.py").read_text(encoding="utf-8")

        assert 'gap.status === "failed"' in gaps_detail_js
        assert "const canSubmitNewRound" in gaps_detail_js
        assert "Submit recovery round" in gaps_detail_js
        editable_body = gaps_detail_js.split("const isLatestEditable", 1)[1].split("const canSubmitNewRound", 1)[0]
        assert '"failed"' not in editable_body
        assert '"failed"' in toolbar_js.split("const GAP_CHAT_ROUND_STATUSES", 1)[1].split("]);", 1)[0]
        assert 'row["status"] not in ("review", "failed")' in gap_ops_py

    finally:
        conn.close()
        cleanup_tmp(tmp)
    print("failed round support tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
