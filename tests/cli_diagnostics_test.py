"""CLI parity for backend diagnostics."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from refine_server.backend_protocol import M_DIAGNOSTICS


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
        return {"ok": True, "runner": "fake"}

    old_runner_for_cli = cli._backend_runner_for_cli
    cli._backend_runner_for_cli = lambda _ctx, _port: (
        type("Cfg", (), {
            "config_path": Path("/tmp/refine.toml"),
            "web_port": 18181,
        })(),
        fake_runner,
    )
    try:
        rc, out, err = _run_cli(["diagnostics", "backend"])
    finally:
        cli._backend_runner_for_cli = old_runner_for_cli
    assert rc == 0, err
    payload = _json(out)
    assert payload["reachable"] is True
    assert payload["backend"]["process_model"] == "supervisor"
    assert payload["runner"] == "fake"
    assert calls == [(M_DIAGNOSTICS, {}, 5.0)]

    print("CLI diagnostics tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
