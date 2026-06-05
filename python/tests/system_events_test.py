"""System operation SSE event contract checks."""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from refine_ui import api, system_events


def test_system_events_publish() -> None:
    published: list[tuple[str, dict]] = []
    old_publish = system_events.sse.publish
    system_events.sse.publish = lambda event_type, data: published.append((event_type, data))
    try:
        system_events.publish("  Test message  ", status="complete", category="test")
        system_events.publish("")
    finally:
        system_events.sse.publish = old_publish

    assert len(published) == 1
    event_type, payload = published[0]
    assert event_type == "system_operation"
    assert payload["message"] == "Test message"
    assert payload["status"] == "complete"
    assert payload["category"] == "test"
    assert payload["timestamp"].endswith("Z")


def test_system_operation_decorator() -> None:
    events: list[tuple[str, dict]] = []
    old_publish = api.system_events.publish
    api.system_events.publish = lambda message, **payload: events.append((message, payload))
    try:
        @api._system_operation("Example operation")
        def successful() -> tuple[int, dict]:
            return 200, {"ok": True}

        @api._system_operation("Queued operation")
        def queued() -> tuple[int, dict]:
            return 202, {"queued": True}

        @api._system_operation("Failing operation")
        def failed() -> tuple[int, dict]:
            return 409, {"error": {"message": "Already busy"}}

        assert successful() == (200, {"ok": True})
        assert queued() == (202, {"queued": True})
        assert failed() == (409, {"error": {"message": "Already busy"}})
    finally:
        api.system_events.publish = old_publish

    assert events == [
        ("Example operation started", {"status": "start", "category": "operation"}),
        (
            "Example operation completed",
            {"status": "complete", "category": "operation", "http_status": 200},
        ),
        ("Queued operation started", {"status": "start", "category": "operation"}),
        (
            "Queued operation queued",
            {"status": "queued", "category": "operation", "http_status": 202},
        ),
        ("Failing operation started", {"status": "start", "category": "operation"}),
        (
            "Failing operation failed: Already busy",
            {"status": "error", "category": "operation", "http_status": 409},
        ),
    ]


def main() -> int:
    test_system_events_publish()
    test_system_operation_decorator()
    print("system event tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
