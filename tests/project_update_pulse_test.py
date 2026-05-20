"""Target-repo update pulse poller tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-project-update-pulse-")
    conn = init_refine(client)
    try:
        from refine_server import db
        from refine_ui import poller as poller_mod, sse
        from refine_ui.poller import SqlitePoller

        calls: list[str] = []
        events: list[tuple[str, dict]] = []
        original_pulse = poller_mod.project_sync.pulse
        original_publish = sse.publish

        def fake_pulse(_conn, *, actor: str = "runner") -> dict:
            calls.append(actor)
            return {
                "ok": True,
                "changed": True,
                "stage": "refreshed",
                "branch": "main",
                "message": "changed",
            }

        def fake_publish(event_type: str, data: dict) -> None:
            events.append((event_type, data))

        try:
            poller_mod.project_sync.pulse = fake_pulse
            sse.publish = fake_publish

            db.set_setting(conn, "project_update_pulse_interval_seconds", "30")
            p = SqlitePoller()
            p._run_project_update_pulse(100.0)  # noqa: SLF001
            assert calls == ["runner"], calls
            assert events == [(
                "project_updated",
                {
                    "stage": "refreshed",
                    "branch": "main",
                    "upstream": None,
                    "message": "changed",
                },
            )], events

            p._run_project_update_pulse(110.0)  # noqa: SLF001
            assert calls == ["runner"], calls

            db.set_setting(conn, "project_update_pulse_interval_seconds", "-1")
            p._run_project_update_pulse(200.0)  # noqa: SLF001
            assert calls == ["runner"], calls
        finally:
            poller_mod.project_sync.pulse = original_pulse
            sse.publish = original_publish
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("project update pulse tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
