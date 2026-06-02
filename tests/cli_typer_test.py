"""Focused checks for the Typer CLI dispatch layer."""
from __future__ import annotations

import os
import json
import shutil
import subprocess
import sys
import tempfile
from contextlib import redirect_stderr, redirect_stdout
from io import BytesIO, StringIO
from pathlib import Path
from urllib.error import HTTPError


def _run_cli(args: list[str]) -> tuple[int, str, str]:
    from refine_cli import cli

    stdout = StringIO()
    stderr = StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        rc = cli.main(args)
    return rc, stdout.getvalue(), stderr.getvalue()


def main() -> int:
    from refine_cli import cli

    rc, out, err = _run_cli(["--help"])
    assert rc == 0, err
    assert "Manage refine" in out
    assert "target" in out
    assert "init" not in out
    assert "install" in out
    assert "update" in out
    assert "migrate" in out
    assert "runner" not in out
    assert "web" not in out
    assert "supervisor" not in out

    rc, out, err = _run_cli(["migrate", "--help"])
    assert rc == 0, err
    assert "Manage Refine project-state migrations" in out
    assert "status" in out
    assert "run" in out

    rc, out, err = _run_cli(["runner", "--help"])
    assert rc == 0, err
    assert "Usage: refine runner" in out

    calls: list[object] = []
    old_target = cli.cmd_target
    try:
        cli.cmd_target = lambda args: calls.append(args) or 13
        rc, _out, err = _run_cli(["target", "/tmp/app", "--force"])
    finally:
        cli.cmd_target = old_target
    assert rc == 13, err
    assert len(calls) == 1
    assert getattr(calls[0], "path") == "/tmp/app"
    assert getattr(calls[0], "force") is True

    tmp = Path(tempfile.mkdtemp(prefix="refine-target-cli-"))
    try:
        clone = tmp / "refine-source"
        client = tmp / "target-app"
        (clone / "refine_cli").mkdir(parents=True)
        client.mkdir()
        (clone / "pyproject.toml").write_text(
            "[project]\nname = \"refine\"\n",
            encoding="utf-8",
        )
        (clone / "refine_cli" / "cli.py").write_text("# marker\n", encoding="utf-8")
        subprocess.run(["git", "init", "-q"], cwd=client, check=True)
        old_cwd = Path.cwd()
        os.chdir(clone)
        try:
            rc, _out, err = _run_cli(["target", str(client), "--force"])
        finally:
            os.chdir(old_cwd)
        assert rc == 0, err
        assert not (clone / ".refine-binding").exists()
        assert (client / ".refine" / "refine.toml").exists()
        other = tmp / "other-target-app"
        other.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=other, check=True)
        from refine_server import config, project_registry

        other_cfg = config.write_defaults(other / ".refine")
        other_cfg.write_text(
            other_cfg.read_text(encoding="utf-8") + "\n# sentinel: keep me\n",
            encoding="utf-8",
        )
        os.chdir(clone)
        try:
            rc, _out, err = _run_cli(["target", str(other)])
        finally:
            os.chdir(old_cwd)
        assert rc == 0, err
        assert "# sentinel: keep me" in other_cfg.read_text(encoding="utf-8")
        assert project_registry.active_app(clone, port=8080) == other.resolve()
        assert [app["path"] for app in project_registry.list_apps(clone, port=8080)] == [
            str(client.resolve()),
            str(other.resolve()),
        ]
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    tmp = Path(tempfile.mkdtemp(prefix="refine-migrate-cli-"))
    try:
        client = tmp / "target-app"
        client.mkdir()
        subprocess.run(["git", "init", "-q"], cwd=client, check=True)
        subprocess.run(["git", "config", "user.email", "t@x"], cwd=client, check=True)
        subprocess.run(["git", "config", "user.name", "t"], cwd=client, check=True)
        (client / "app.txt").write_text("base\n", encoding="utf-8")
        subprocess.run(["git", "add", "app.txt"], cwd=client, check=True)
        subprocess.run(
            ["git", "commit", "-m", "init"],
            cwd=client,
            check=True,
            capture_output=True,
            text=True,
        )

        from refine_server import config

        refine_root = client / ".refine"
        config.write_defaults(refine_root)
        (refine_root / "config.json").write_text(
            json.dumps({"schema_version": 1, "settings": {}}),
            encoding="utf-8",
        )
        (refine_root / "instances.json").write_text(
            json.dumps({"instances": [{"id": "default", "display_name": "Default"}]}),
            encoding="utf-8",
        )
        (refine_root / "instances" / "default").mkdir(parents=True)
        subprocess.run(["git", "add", ".refine"], cwd=client, check=True)
        subprocess.run(
            ["git", "commit", "-m", "legacy refine state"],
            cwd=client,
            check=True,
            capture_output=True,
            text=True,
        )

        cfg_path = refine_root / "refine.toml"
        rc, out, err = _run_cli(["--config", str(cfg_path), "migrate", "status"])
        assert rc == 0, err
        assert "instance_to_node_v2" in out
        rc, out, err = _run_cli(["--config", str(cfg_path), "migrate", "run"])
        assert rc == 0, err
        result = json.loads(out)
        assert result["schema"]["compatible"] is True
        assert result["lock_sync"]["committed_state"] is True
        assert result["migration_sync"]["committed_state"] is True
        assert (refine_root / "nodes.json").exists()
        assert not (refine_root / "instances.json").exists()
        subject = subprocess.run(
            ["git", "log", "-1", "--format=%s"],
            cwd=client,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        assert subject == "refine: migrate project state"
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    rc, _out, err = _run_cli(["init", "/tmp/app"])
    assert rc == 2
    assert "No such command 'init'" in err

    calls.clear()
    old_run = cli.subprocess.run
    try:
        cli.subprocess.run = lambda cmd, **_kwargs: calls.append(cmd) or type(
            "Result",
            (),
            {"returncode": 37},
        )()
        rc, out, err = _run_cli(["update"])
    finally:
        cli.subprocess.run = old_run
    assert rc == 37, err
    assert calls == [["bash", "-lc", cli.README_INSTALL_COMMAND]]
    assert (
        cli.README_INSTALL_COMMAND
        == "curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash"
    )
    assert f"Running: {cli.README_INSTALL_COMMAND}" in out

    calls.clear()
    old_start = cli.cmd_start
    try:
        cli.cmd_start = lambda args: calls.append(args) or 17
        rc, _out, err = _run_cli(["-c", "/tmp/refine.toml", "start", "18111"])
    finally:
        cli.cmd_start = old_start
    assert rc == 17, err
    assert len(calls) == 1
    assert getattr(calls[0], "config") == "/tmp/refine.toml"
    assert getattr(calls[0], "port") == 18111

    assert cli._normalize_argv(["ps", "--watch"]) == ["ps", "--watch", "2.0"]
    assert cli._normalize_argv(["ps", "18112", "--watch"]) == [
        "ps", "18112", "--watch", "2.0",
    ]
    assert cli._normalize_argv(["ps", "--watch", "3"]) == [
        "ps", "--watch", "3",
    ]
    assert cli._normalize_argv(["--config", "ps", "status", "--watch"]) == [
        "--config", "ps", "status", "--watch",
    ]

    calls.clear()
    old_ps = cli.cmd_ps
    try:
        cli.cmd_ps = lambda args: calls.append(args) or 23
        rc, _out, err = _run_cli(["ps", "--watch"])
    finally:
        cli.cmd_ps = old_ps
    assert rc == 23, err
    assert len(calls) == 1
    assert getattr(calls[0], "watch") == 2.0
    assert getattr(calls[0], "once") is False

    calls.clear()
    old_ps = cli.cmd_ps
    old_argv = sys.argv[:]
    try:
        cli.cmd_ps = lambda args: calls.append(args) or 29
        sys.argv = ["refine", "ps", "--watch"]
        stdout = StringIO()
        stderr = StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            rc = cli.main()
    finally:
        cli.cmd_ps = old_ps
        sys.argv = old_argv
    assert rc == 29, stderr.getvalue()
    assert len(calls) == 1
    assert getattr(calls[0], "watch") == 2.0

    old_ps = cli.cmd_ps
    try:
        cli.cmd_ps = lambda _args: (_ for _ in ()).throw(
            AssertionError("cmd_ps should not run for mutually exclusive options")
        )
        rc, _out, err = _run_cli(["ps", "--watch", "--once"])
    finally:
        cli.cmd_ps = old_ps
    assert rc == 2
    assert "mutually exclusive" in err

    class UpgradeInfo:
        current_version = "1.0.0"
        latest_version = "1.2.0"
        upgrade_available = True
        command = "curl install.sh | bash -s -- --yes"

    old_upgrade_status = cli.upgrade.status
    try:
        cli.upgrade.status = lambda _clone: UpgradeInfo()
        stdout = StringIO()
        with redirect_stdout(stdout):
            cli._print_upgrade_notice(Path("/tmp/refine"))
    finally:
        cli.upgrade.status = old_upgrade_status
    upgrade_notice = stdout.getvalue()
    assert "Upgrade available" in upgrade_notice
    assert "Refine 1.2.0 is available (current 1.0.0)." in upgrade_notice
    assert "curl install.sh | bash -s -- --yes" in upgrade_notice
    assert "--yes" in upgrade_notice
    assert "--upgrade" not in upgrade_notice

    cli_source = Path(cli.__file__).read_text(encoding="utf-8")

    def function_source(name: str) -> str:
        start = cli_source.index(f"def {name}(")
        end = cli_source.find("\ndef ", start + 1)
        return cli_source[start:] if end == -1 else cli_source[start:end]

    start_source = function_source("cmd_start")
    assert "_print_upgrade_notice(clone)" in start_source
    assert "_print_upgrade_notice(clone)" in function_source("_start_systemd_ui")
    assert "_print_upgrade_notice(clone)" in function_source("_restart_systemd_ui")
    assert "_print_upgrade_notice(clone)" in function_source("_restart_setup_systemd_ui")

    cleanup_source = function_source("_pause_agents_for_clean_shutdown")
    assert "/api/processes/background" in cleanup_source
    assert '{"stopped": True}' in cleanup_source
    assert 'method="POST"' in cleanup_source
    assert 'method="PATCH"' not in cleanup_source
    error = HTTPError(
        "http://127.0.0.1:8080/api/processes/background",
        500,
        "Internal Server Error",
        {},
        BytesIO(
            b'{"error":{"message":"cleanup failed","details":"target still dirty"}}',
        ),
    )
    assert cli._shutdown_cleanup_http_error_message(error) == (
        "cleanup failed: target still dirty"
    )

    print("[ok] Typer CLI dispatch preserves commands, aliases, and ps options")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
