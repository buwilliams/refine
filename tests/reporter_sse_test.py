"""Reporter-list SSE notification tests."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-reporter-sse-")
    conn = init_refine(client)
    try:
        from refine_server import reporters
        from refine_ui import sse
        from refine_ui.poller import SqlitePoller

        events: list[tuple[str, dict]] = []
        original_publish = sse.publish

        def fake_publish(event_type: str, data: dict) -> None:
            events.append((event_type, data))

        try:
            sse.publish = fake_publish
            p = SqlitePoller()

            p._poll_reporter_changes(conn)  # noqa: SLF001
            assert events == [], events

            reporters.add(conn, "Imported Reporter")
            p._poll_reporter_changes(conn)  # noqa: SLF001
            assert events == [("reporters_changed", {"count": 1})], events

            p._poll_reporter_changes(conn)  # noqa: SLF001
            assert events == [("reporters_changed", {"count": 1})], events
        finally:
            sse.publish = original_publish
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("reporter SSE tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
