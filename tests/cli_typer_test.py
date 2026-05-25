"""Focused checks for the Typer CLI dispatch layer."""
from __future__ import annotations

import sys
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO


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
    assert "install" in out
    assert "runner" not in out
    assert "web" not in out
    assert "supervisor" not in out

    rc, out, err = _run_cli(["runner", "--help"])
    assert rc == 0, err
    assert "Usage: refine runner" in out

    calls: list[object] = []
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

    print("[ok] Typer CLI dispatch preserves commands, aliases, and ps options")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
