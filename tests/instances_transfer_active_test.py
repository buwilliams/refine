"""Settings instance transfer cancellation behavior tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


class FakeBackend:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict]] = []

    def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0) -> dict:  # noqa: ARG002
        params = params or {}
        self.calls.append((method, dict(params)))
        from refine_server import db, gap_writer
        from refine_server.backend_protocol import (
            M_CANCEL, M_CANCEL_ALL, M_CHAT_RESET_ALL, M_ENFORCE_SCHEDULING,
        )
        from refine_server.gaps import now_iso

        if method == M_CANCEL_ALL:
            assert params.get("reason") == "paused", params
            return {"killed_subprocesses": 3}
        if method == M_CANCEL:
            gap_id = params["gap_id"]
            conn = db.connect()
            try:
                conn.execute(
                    "UPDATE gaps_index SET status = 'cancelled', updated = ? WHERE id = ?",
                    (now_iso(), gap_id),
                )
                gap_writer.update_fields(gap_id, status="cancelled")
            finally:
                conn.close()
            return {"killed_subprocess": True}
        if method == M_ENFORCE_SCHEDULING:
            return {"ok": True}
        if method == M_CHAT_RESET_ALL:
            assert params.get("reason") == "instance activated", params
            return {"stopped": 2}
        raise AssertionError(f"unexpected backend call: {method}")


def main() -> int:
    tmp, client = make_client_repo("refine-instances-transfer-active-")
    conn = init_refine(client)
    original_get_client = None
    try:
        from refine_server import db, gap_writer, gaps, project_state
        from refine_server.backend_protocol import M_CANCEL, M_CANCEL_ALL, M_CHAT_RESET_ALL
        from refine_ui import api

        original_get_client = api.get_client
        fake = FakeBackend()
        api.get_client = lambda: fake  # type: ignore[assignment]

        source = project_state.active_instance_id()
        target = project_state.create_instance("Target")
        other = project_state.create_instance("Other")

        def create(gap_id: str, status: str, instance_id: str = source) -> None:
            gap_writer.create_gap(
                gap_id=gap_id,
                name=gap_id,
                initial_round=gaps.new_round("Jane", "Actual", "Target"),
                status=status,
                priority="medium",
                instance_id=instance_id,
            )

        todo = "01INSTANCEACTIVETODOAAAA"
        running = "01INSTANCEACTIVEINPROGAA"
        ready = "01INSTANCEACTIVEREADYAAA"
        unrelated = "01INSTANCEACTIVEOTHERAAA"
        create(todo, "todo")
        create(running, "in-progress")
        create(ready, "ready-merge")
        create(unrelated, "in-progress", other["id"])
        project_state.rebuild_sqlite_cache(conn)

        status, body = api.transfer_instance_gaps({
            "source_instance_id": source,
            "target_instance_id": target["id"],
            "cancel_active": True,
        })
        assert status == 200, body
        assert body["paused"] is True, body
        assert body["stopped_processes"] == 3, body
        assert body["cancelled"] == 2, body
        assert set(body["cancelled_ids"]) == {running, ready}, body
        assert set(body["ids"]) == {todo, running, ready}, body
        assert body["skipped"] == 0, body

        methods = [m for m, _ in fake.calls]
        assert methods[0] == M_CANCEL_ALL, methods
        assert methods.count(M_CANCEL) == 2, methods
        assert db.get_setting(conn, "paused") == "1"

        rows = {
            row["id"]: (row["status"], row["instance_id"])
            for row in conn.execute(
                "SELECT id, status, instance_id FROM gaps_index WHERE id IN (?, ?, ?, ?)",
                (todo, running, ready, unrelated),
            )
        }
        assert rows[todo] == ("todo", target["id"]), rows
        assert rows[running] == ("cancelled", target["id"]), rows
        assert rows[ready] == ("cancelled", target["id"]), rows
        assert rows[unrelated] == ("in-progress", other["id"]), rows

        fake.calls.clear()
        status, body = api.activate_instance({"instance_id": target["id"]})
        assert status == 200, body
        assert body["active_instance_id"] == target["id"], body
        methods = [m for m, _ in fake.calls]
        assert M_CHAT_RESET_ALL in methods, methods
        assert M_CANCEL_ALL not in methods, methods
    finally:
        if original_get_client is not None:
            from refine_ui import api

            api.get_client = original_get_client  # type: ignore[assignment]
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("instance transfer active cancellation tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
