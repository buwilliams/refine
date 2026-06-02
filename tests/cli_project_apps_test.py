"""CLI parity for project app setup helpers."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo
from refine_server.backend_protocol import M_PROJECT_SYNC


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
    tmp, client = make_client_repo("refine-cli-project-apps-")
    conn = init_refine(client)
    try:
        from refine_cli import cli
        from refine_server import gaps as shared_gaps

        cfg = str(client / ".refine" / "refine.toml")
        prefix = ["--config", cfg]

        rc, out, err = _run_cli(["app", "templates"])
        assert rc == 0, err
        payload = _json(out)
        template_ids = {template["id"] for template in payload["templates"]}
        assert "nodejs-webapp" in template_ids, payload

        sync_calls: list[str] = []
        old_sync = cli._sync_cli_refine_state
        cli._sync_cli_refine_state = (
            lambda _cfg, *, message, rebuild_cache=True:
            sync_calls.append(message) or {"ok": True, "message": message}
        )
        try:
            rc, out, err = _run_cli([
                *prefix,
                "app",
                "scaffold",
                "nodejs-webapp",
                "--reporter",
                "CLI Refine",
            ])
        finally:
            cli._sync_cli_refine_state = old_sync

        assert rc == 0, err
        payload = _json(out)
        assert payload["ok"] is True, payload
        assert payload["sync"]["ok"] is True, payload
        assert payload["template"]["id"] == "nodejs-webapp", payload
        gap = payload["gap"]
        assert gap["name"] == "Scaffold Node.js WebApp", gap
        assert gap["priority"] == "high", gap
        assert gap["rounds"][-1]["reporter"] == "CLI Refine", gap
        assert "Vite" in gap["rounds"][-1]["target"], gap
        assert shared_gaps.read_gap_json(gap["id"])["name"] == "Scaffold Node.js WebApp"
        assert sync_calls == ["refine: create scaffold gap"], sync_calls

        row = conn.execute(
            "SELECT priority, reporter FROM gaps_index WHERE id = ?",
            (gap["id"],),
        ).fetchone()
        assert dict(row) == {"priority": "high", "reporter": "CLI Refine"}

        runner_calls: list[tuple[str, dict[str, object], float]] = []
        old_runner_for_cli = cli._backend_runner_for_cli

        def fake_runner(method: str, params: dict[str, object], timeout: float) -> dict:
            runner_calls.append((method, params, timeout))
            return {"ok": True, "stage": "synced"}

        cli._backend_runner_for_cli = lambda _ctx, _port: (cli.config.get(reload=True), fake_runner)
        try:
            rc, out, err = _run_cli([*prefix, "app", "sync"])
        finally:
            cli._backend_runner_for_cli = old_runner_for_cli
        assert rc == 0, err
        assert _json(out)["stage"] == "synced"
        assert runner_calls == [(M_PROJECT_SYNC, {}, 120.0)], runner_calls

        refine_source = tmp / "refine-source"
        (refine_source / "refine_cli").mkdir(parents=True)
        (refine_source / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (refine_source / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
        new_app = tmp / "attached-from-cli"
        old_cwd = Path.cwd()
        os.chdir(refine_source)
        try:
            rc, out, err = _run_cli(["app", "attach", str(new_app)])
        finally:
            os.chdir(old_cwd)
        assert rc == 0, err
        payload = _json(out)
        assert payload["attached"] is True, payload
        assert payload["client_repo"] == str(new_app.resolve()), payload
        assert payload["git_initialized"] is True, payload
        assert payload["config_created"] is True, payload
        assert payload["runner"]["started"] is False, payload
        assert (new_app / ".git").exists()
        assert (new_app / ".refine" / "refine.toml").exists()

        subprocess.run(["git", "config", "user.email", "t@x"], cwd=new_app, check=True)
        subprocess.run(["git", "config", "user.name", "t"], cwd=new_app, check=True)
        second_app = tmp / "second-cli-app"
        os.chdir(refine_source)
        try:
            rc, out, err = _run_cli(["app", "attach", str(second_app)])
        finally:
            os.chdir(old_cwd)
        assert rc == 0, err
        subprocess.run(["git", "config", "user.email", "t@x"], cwd=second_app, check=True)
        subprocess.run(["git", "config", "user.name", "t"], cwd=second_app, check=True)

        os.chdir(refine_source)
        try:
            rc, out, err = _run_cli(["app", "remove", str(second_app)])
        finally:
            os.chdir(old_cwd)
        assert rc == 0, err
        payload = _json(out)
        assert payload["auto_attached"] is True, payload
        assert payload["client_repo"] == str(new_app.resolve()), payload
        assert payload["removed_path"] == str(second_app.resolve()), payload

        old_env_port = os.environ.get("REFINE_UI_PORT")
        old_env_scope = os.environ.get("REFINE_UI_SCOPE")
        old_env_run_dir = os.environ.get("REFINE_RUN_DIR")
        old_env_config = os.environ.get("REFINE_CONFIG_PATH")
        try:
            os.environ["REFINE_UI_PORT"] = "18123"
            os.environ.pop("REFINE_UI_SCOPE", None)
            os.environ.pop("REFINE_RUN_DIR", None)
            os.environ.pop("REFINE_CONFIG_PATH", None)

            env_app = tmp / "env-port-app"
            os.chdir(refine_source)
            try:
                rc, out, err = _run_cli(["app", "attach", str(env_app)])
                assert rc == 0, err
                payload = _json(out)
                assert payload["client_repo"] == str(env_app.resolve()), payload
                assert str(refine_source / "run" / "18123" / "apps.json") == payload["registry_path"]

                rc, out, err = _run_cli(["app", "status"])
                assert rc == 0, err
                payload = _json(out)
                assert payload["client_repo"] == str(env_app.resolve()), payload

                rc, out, err = _run_cli(["app", "list"])
                assert rc == 0, err
                payload = _json(out)
                assert payload["current"] == str(env_app.resolve()), payload

                switched_app = tmp / "env-port-switched"
                switched_app.mkdir()
                subprocess.run(["git", "init", "-q"], cwd=switched_app, check=True)
                subprocess.run(["git", "config", "user.email", "t@x"], cwd=switched_app, check=True)
                subprocess.run(["git", "config", "user.name", "t"], cwd=switched_app, check=True)
                rc, out, err = _run_cli(["app", "switch", str(switched_app), "--force"])
                assert rc == 0, err
                text = (switched_app / ".refine" / "refine.toml").read_text(encoding="utf-8")
                assert "port = 18123" in text
                assert "port = 8080" not in text
            finally:
                os.chdir(old_cwd)
        finally:
            if old_env_port is None:
                os.environ.pop("REFINE_UI_PORT", None)
            else:
                os.environ["REFINE_UI_PORT"] = old_env_port
            if old_env_scope is None:
                os.environ.pop("REFINE_UI_SCOPE", None)
            else:
                os.environ["REFINE_UI_SCOPE"] = old_env_scope
            if old_env_run_dir is None:
                os.environ.pop("REFINE_RUN_DIR", None)
            else:
                os.environ["REFINE_RUN_DIR"] = old_env_run_dir
            if old_env_config is None:
                os.environ.pop("REFINE_CONFIG_PATH", None)
            else:
                os.environ["REFINE_CONFIG_PATH"] = old_env_config
    finally:
        conn.close()
        cleanup_tmp(tmp)

    print("CLI project app tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
