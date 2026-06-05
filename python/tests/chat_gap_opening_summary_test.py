"""Gap chat opening-summary prompt checks."""
from __future__ import annotations

from tests.helpers import cleanup_tmp, create_indexed_gap, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-gap-chat-summary-")
    conn = init_refine(client)
    try:
        from refine_server import activity, gap_writer
        from refine_server.runner import _build_gap_chat_preamble
        from refine_server.ulid import new_ulid

        gap_id = new_ulid()
        create_indexed_gap(conn, gap_id, status="in-progress", branch="gap/test")
        gap_writer.append_latest_round_log(
            gap_id=gap_id,
            message="Round log captured failing pytest output",
            severity="error",
            category="cli",
            actor="agent",
            details="pytest failed in tests/checkout_test.py",
        )
        activity.append(
            conn,
            message="Agent hit failing tests in checkout flow",
            severity="error",
            category="cli",
            gap_id=gap_id,
            actor="agent",
        )
        conn.commit()

        prompt, intro = _build_gap_chat_preamble(conn, gap_id)
        assert intro is not None
        assert prompt is not None
        assert "First, analyze the Gap logs and context below" in prompt
        assert "Do not wait for another user message" in prompt
        assert "Stay scoped to THIS GAP by default" in prompt
        assert "Do not enumerate `.refine/gaps`" in prompt
        assert "scan sibling Gap files" in prompt
        assert "## Latest round logs (oldest first)" in prompt
        assert "Round log captured failing pytest output" in prompt
        assert "pytest failed in tests/checkout_test.py" in prompt
        assert "## Recent Gap logs/activity (oldest first)" in prompt
        assert "Agent hit failing tests in checkout flow" in prompt
        assert "The user's first message follows." not in prompt
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("gap chat opening summary tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
