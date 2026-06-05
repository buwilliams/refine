"""Focused tests for server-side whole-context import extraction."""
from __future__ import annotations


def main() -> int:
    from refine_server.backend_protocol import M_EXTRACT_GAPS
    from refine_ui import api

    calls: list[dict] = []

    class FakeClient:
        def call(self, method: str, params: dict | None = None, *, timeout: float = 30.0):
            assert method == M_EXTRACT_GAPS
            assert timeout == 200.0
            payload = params or {}
            calls.append(payload)
            return {
                "drafts": [{
                    "name": f"Chunk {len(calls)}",
                    "actual": payload["text"].splitlines()[0],
                    "target": "Target",
                }],
            }

    original_get_client = api.get_client
    original_stopped = api._background_processes_stopped_response
    try:
        api.get_client = lambda: FakeClient()
        api._background_processes_stopped_response = lambda: None
        text = "\n".join(
            [f"line {idx}" for idx in range(1, 24)]
            + [""]
            + [f"line {idx}" for idx in range(24, 46)]
        )
        status, body = api.import_extract({"text": text})
    finally:
        api.get_client = original_get_client
        api._background_processes_stopped_response = original_stopped

    assert status == 200, body
    assert len(calls) == 1, calls
    assert calls[0]["text"] == text
    assert body["drafts"][0]["name"] == "Chunk 1"

    calls.clear()
    try:
        api.get_client = lambda: FakeClient()
        api._background_processes_stopped_response = lambda: None
        status, body = api.import_extract({"text": "short\ntext"})
    finally:
        api.get_client = original_get_client
        api._background_processes_stopped_response = original_stopped

    assert status == 200, body
    assert len(calls) == 1, calls
    assert calls[0]["text"] == "short\ntext"

    print("import extract API whole-context tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
