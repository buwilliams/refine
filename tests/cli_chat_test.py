"""CLI parity for chat session operations."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from refine_server.backend_protocol import M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START, M_CHAT_STOP


def _run_cli(args: list[str]) -> tuple[int, str, str]:
    from refine_cli import cli

    stdout = StringIO()
    stderr = StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        rc = cli.main(args)
    return rc, stdout.getvalue(), stderr.getvalue()


def _json(out: str) -> dict:
    return json.loads(out)


def main() -> int:
    from refine_cli import cli

    calls: list[tuple[str, dict[str, object], float]] = []

    def fake_runner(method: str, params: dict[str, object], timeout: float) -> dict:
        calls.append((method, params, timeout))
        return {"ok": True, "method": method, "params": params}

    old_runner_for_cli = cli._backend_runner_for_cli
    cli._backend_runner_for_cli = lambda _ctx, _port: (None, fake_runner)
    try:
        rc, out, err = _run_cli([
            "chat",
            "start",
            "--gap-id",
            "01HX0000000000000000000000",
            "--purpose",
            "plan",
        ])
        assert rc == 0, err
        payload = _json(out)
        assert payload["method"] == M_CHAT_START
        assert payload["params"]["gap_id"] == "01HX0000000000000000000000"
        assert payload["params"]["purpose"] == "plan"

        rc, out, err = _run_cli(["chat", "input", "sid123", "hello"])
        assert rc == 0, err
        payload = _json(out)
        assert payload["method"] == M_CHAT_INPUT
        assert payload["params"] == {"session_id": "sid123", "text": "hello"}

        rc, out, err = _run_cli(["chat", "read", "sid123"])
        assert rc == 0, err
        assert _json(out)["method"] == M_CHAT_READ

        rc, out, err = _run_cli(["chat", "stop", "sid123"])
        assert rc == 0, err
        assert _json(out)["method"] == M_CHAT_STOP
    finally:
        cli._backend_runner_for_cli = old_runner_for_cli

    assert [call[0] for call in calls] == [
        M_CHAT_START,
        M_CHAT_INPUT,
        M_CHAT_READ,
        M_CHAT_STOP,
    ]
    print("CLI chat tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
