"""CLI parity for project settings operations."""
from __future__ import annotations

import json
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo
from refine_server.backend_protocol import M_PREFLIGHT


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
    tmp, client = make_client_repo("refine-cli-settings-")
    conn = init_refine(client)
    conn.close()
    cfg = str(client / ".refine" / "refine.toml")
    prefix = ["--config", cfg]
    try:
        rc, out, err = _run_cli([*prefix, "settings", "get"])
        assert rc == 0, err
        assert _json(out)["settings"]["parallel_run_cap"] == "5"

        payload = {
            "parallel_run_cap": 7,
            "agent_cli": "codex",
            "file_browser_ignore_patterns": " node_modules, .git, vendor_cache ",
            "target_app_env_json": '{"PORT":3001}',
            "target_app_auto_rebuild": "daily",
            "target_app_auto_rebuild_hour_utc": "19",
        }
        rc, out, err = _run_cli([*prefix, "settings", "save", json.dumps(payload)])
        assert rc == 0, err
        assert _json(out)["ok"] is True

        rc, out, err = _run_cli([*prefix, "settings", "get"])
        assert rc == 0, err
        settings = _json(out)["settings"]
        assert settings["parallel_run_cap"] == "7"
        assert settings["agent_cli"] == "codex"
        assert settings["file_browser_ignore_patterns"] == "node_modules, .git, vendor_cache"
        assert settings["target_app_env_json"] == '{"PORT": "3001"}'
        assert settings["target_app_auto_rebuild"] == "daily"
        assert settings["target_app_auto_rebuild_hour_utc"] == "19"

        rc, out, err = _run_cli([*prefix, "settings", "set", "target_app_cwd", "apps/web"])
        assert rc == 0, err
        assert _json(out)["ok"] is True

        rc, out, err = _run_cli([*prefix, "settings", "get"])
        assert rc == 0, err
        assert _json(out)["settings"]["target_app_cwd"] == "apps/web"

        rc, out, err = _run_cli([*prefix, "settings", "set", "parallel_run_cap", "0"])
        assert rc == 1
        assert "parallel_run_cap must be between 1 and 100" in err

        rc, out, err = _run_cli([
            *prefix,
            "settings",
            "set",
            "target_app_auto_rebuild_hour_utc",
            "24",
        ])
        assert rc == 1
        assert "target_app_auto_rebuild_hour_utc must be between 0 and 23" in err

        rc, out, err = _run_cli([
            *prefix,
            "settings",
            "set",
            "target_app_auto_rebuild",
            "nightly",
        ])
        assert rc == 0, err
        assert _json(out)["ok"] is True

        rc, out, err = _run_cli([*prefix, "settings", "get"])
        assert rc == 0, err
        assert _json(out)["settings"]["target_app_auto_rebuild"] == "daily"

        from refine_cli import cli

        calls: list[tuple[str, dict[str, object], float]] = []

        def fake_runner(method: str, params: dict[str, object], timeout: float) -> dict:
            calls.append((method, params, timeout))
            return {"ok": True, "message": "auth ok"}

        old_runner_for_cli = cli._backend_runner_for_cli
        cli._backend_runner_for_cli = lambda _ctx, _port: (cli.config.get(reload=True), fake_runner)
        try:
            rc, out, err = _run_cli([*prefix, "settings", "recheck-auth"])
        finally:
            cli._backend_runner_for_cli = old_runner_for_cli
        assert rc == 0, err
        assert _json(out)["message"] == "auth ok"
        assert calls == [(M_PREFLIGHT, {}, 30.0)], calls
    finally:
        cleanup_tmp(tmp)

    print("CLI settings tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
