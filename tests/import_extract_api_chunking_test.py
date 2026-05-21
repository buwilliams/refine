"""Focused tests for server-side import extraction chunking."""
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
    try:
        api.get_client = lambda: FakeClient()
        text = "\n".join(f"line {idx}" for idx in range(1, 46))
        status, body = api.import_extract({"text": text})
    finally:
        api.get_client = original_get_client

    assert status == 200, body
    assert len(calls) == 3, calls
    assert [len(call["text"].splitlines()) for call in calls] == [20, 20, 5]
    assert [draft["name"] for draft in body["drafts"]] == [
        "Chunk 1",
        "Chunk 2",
        "Chunk 3",
    ]

    calls.clear()
    try:
        api.get_client = lambda: FakeClient()
        status, body = api.import_extract({"text": "short\ntext"})
    finally:
        api.get_client = original_get_client

    assert status == 200, body
    assert len(calls) == 1, calls
    assert calls[0]["text"] == "short\ntext"

    print("import extract API chunking tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
