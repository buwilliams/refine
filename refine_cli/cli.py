"""argparse-based CLI for refine.

Subcommands:
- init    — write refine.toml + gaps/ in a chosen volume root,
            then write the active-app binding for this checkout.
- install — install + start a persistent systemd --user UI backend.
- uninstall — stop + remove a persistent systemd --user UI backend.
- reset   — undo `init` in this checkout (remove binding and persistent units);
            optional --purge also deletes the active app's .refine/ data so
            you can attach a different app.
- start   — start a detached UI backend process.
- stop    — stop the UI backend.
- restart — stop then start (handy for picking up source changes).
- status  — show what's running (read-only).
- test    — run the repository's script-style test suite.
- server  — start the server component in the foreground for debugging.
- ui      — start the UI backend foreground process (supervised in normal use).
- doctor  — deeper diagnostic snapshot (config, agent CLI, git).
"""
from __future__ import annotations

import argparse
import os
import re
import signal
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from refine_server import config, project_registry


SYSTEMD_USER_DIR = Path.home() / ".config" / "systemd" / "user"
SETUP_UI_HOST = "0.0.0.0"
_LOGIN_PATH_CACHE: str | None = None
_LOGIN_PATH_RESOLVED = False


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="refine", description="Manage refine.")
    parser.add_argument(
        "--config", "-c",
        help="Path to refine.toml (defaults to walking up from cwd).",
    )
    sub = parser.add_subparsers(
        dest="command",
        required=True,
        metavar="{init,install,uninstall,reset,start,restart,stop,status,test,server,ui,doctor}",
    )

    p_init = sub.add_parser(
        "init",
        help="Initialize/add a target app and make it active.",
        description=(
            "Bootstraps a target app: creates <app>/.refine/refine.toml + "
            "gaps/, writes the active-app binding, records the app in "
            "the known-apps list, and prepares the checkout for `refine start` "
            "or `refine install`."
        ),
    )
    p_init.add_argument(
        "path", nargs="?", default=None,
        help="Path to the target app repo. Defaults to cwd (back-compat).",
    )
    p_init.add_argument("--force", action="store_true",
                        help="Overwrite an existing refine.toml / .refine-binding.")
    p_init.set_defaults(fn=cmd_init)

    p_install = sub.add_parser(
        "install",
        help="Install and start a persistent UI backend.",
        description=(
            "Writes, enables, and starts a systemd --user unit for this "
            "checkout. The service restarts on failure and survives terminal "
            "close. Pass a port to run multiple Refine instances on one host."
        ),
    )
    p_install.add_argument(
        "port", nargs="?", type=int, default=None,
        help="Web server port. Defaults to the configured port.",
    )
    p_install.set_defaults(fn=cmd_install)

    p_uninstall = sub.add_parser(
        "uninstall",
        help="Stop and remove a persistent UI backend.",
    )
    p_uninstall.add_argument(
        "port", nargs="?", type=int, default=None,
        help="Web server port. Defaults to the configured port.",
    )
    p_uninstall.set_defaults(fn=cmd_uninstall)

    p_reset = sub.add_parser(
        "reset",
        help="Undo `refine init` in this checkout so it can attach a different app.",
        description=(
            "Removes the .refine-binding and .refine-apps.json files from "
            "this checkout, disables + removes persistent systemd --user UI "
            "units, and (with --purge) wipes the active app's .refine/ "
            "directory. The app source tree is never touched."
        ),
    )
    p_reset.add_argument(
        "--purge", action="store_true",
        help="Also delete the active target app's .refine/ directory "
             "(gap.json files, sqlite index, .gitignore). DATA LOSS.",
    )
    p_reset.add_argument(
        "-y", "--yes", action="store_true",
        help="Skip the confirmation prompt for --purge.",
    )
    p_reset.set_defaults(fn=cmd_reset)

    p_start = sub.add_parser(
        "start",
        help="Start the UI backend.",
        description=(
            "Starts the host-native UI backend, then prints a status block. "
            "If this checkout has an installed systemd user unit, the command "
            "starts that service; otherwise it starts a detached background "
            "supervisor. The supervisor keeps the UI/control process separate "
            "from the work runner. Pass a port to run multiple Refine "
            "instances on one host."
        ),
    )
    p_start.add_argument(
        "port", nargs="?", type=int, default=None,
        help="Web server port. Defaults to the configured port.",
    )
    p_start.set_defaults(fn=cmd_start)

    p_restart = sub.add_parser(
        "restart",
        help="Stop the UI backend, then start it again.",
        description=(
            "Equivalent to `refine stop && refine start`."
        ),
    )
    p_restart.add_argument(
        "port", nargs="?", type=int, default=None,
        help="Web server port. Defaults to the configured port.",
    )
    p_restart.set_defaults(fn=cmd_restart)

    p_stop = sub.add_parser(
        "stop",
        help="Stop the UI backend.",
    )
    p_stop.add_argument(
        "port", nargs="?", type=int, default=None,
        help="Web server port. Defaults to the configured port.",
    )
    p_stop.set_defaults(fn=cmd_stop)

    p_status = sub.add_parser(
        "status",
        help="Show what's running (read-only).",
    )
    p_status.add_argument(
        "port", nargs="?", type=int, default=None,
        help="Web server port. Defaults to the configured port.",
    )
    p_status.set_defaults(fn=cmd_status)

    p_test = sub.add_parser(
        "test",
        help="Run the full test suite.",
        description=(
            "Runs every top-level tests/*_test.py script sequentially with "
            "the current Python interpreter. Returns non-zero if any test "
            "script fails."
        ),
    )
    p_test.set_defaults(fn=cmd_test)

    # Useful interactively when you want server logs in the foreground.
    p_server = sub.add_parser(
        "server",
        help="Run the server component in the foreground for debugging.",
    )
    p_server.set_defaults(fn=cmd_server_foreground)

    p_ui = sub.add_parser("ui", help="Start the UI backend in the foreground.")
    p_ui.set_defaults(fn=cmd_ui)

    p_supervisor = sub.add_parser(
        "supervisor",
        help=argparse.SUPPRESS,
    )
    p_supervisor.set_defaults(fn=cmd_supervisor)

    # Backwards-compatible aliases without advertising the old names in help.
    sub._name_parser_map["runner"] = p_server  # noqa: SLF001
    sub._name_parser_map["web"] = p_ui  # noqa: SLF001

    p_doctor = sub.add_parser("doctor", help="Print a diagnostic snapshot.")
    p_doctor.set_defaults(fn=cmd_doctor)

    args = parser.parse_args(argv)
    return args.fn(args)


# ----- init -------------------------------------------------------------------

def cmd_init(args: argparse.Namespace) -> int:
    cwd = Path.cwd().resolve()
    client_repo = Path(args.path).expanduser().resolve() if args.path else cwd

    try:
        result = bootstrap_client_repo(
            client_repo,
            clone_dir=cwd,
            force=args.force,
            create=False,
            init_git=False,
            reuse_existing_config=False,
            install_unit=False,
        )
    except (config.ConfigError, _InitError) as e:
        print(f"refine init: {e}", file=sys.stderr)
        return 1

    cfg_path = result["config_path"]
    target = result["volume_root"]
    binding_written = result.get("binding_path")
    ui_unit_path = result.get("ui_unit_path")
    print(f"Wrote {cfg_path}")
    print(f"Created directories: {target}/run, {target}/gaps")
    if binding_written:
        print(f"Set active target app → {client_repo}")
        print(f"Wrote {binding_written}")
    print()
    print("Next steps:")
    if binding_written:
        print(f"  uv run refine start          # background UI backend + server")
        print(f"  uv run refine install        # persistent service, auto-restarts")
        print(f"  uv run refine status         # check it's healthy")
        print(f"  uv run refine stop           # tear it all down")
    else:
        print(f"  cd {client_repo}")
        print(f"  refine doctor                 # sanity check the config")
        print()
        print("Note: full refine start/stop/status/install orchestration requires running")
        print("`refine init` from inside a refine source dir so the systemd user")
        print("units can be wired up.")
    return 0


def cmd_test(_args: argparse.Namespace) -> int:
    root = Path(__file__).resolve().parents[1]
    tests_dir = root / "tests"
    tests = sorted(tests_dir.glob("*_test.py"))
    if not tests:
        print(f"refine test: no tests found in {tests_dir}", file=sys.stderr)
        return 1

    failures: list[Path] = []
    env = os.environ.copy()
    root_path = str(root)
    env["PYTHONPATH"] = (
        root_path if not env.get("PYTHONPATH")
        else f"{root_path}{os.pathsep}{env['PYTHONPATH']}"
    )
    print(f"Running {len(tests)} test scripts")
    for test in tests:
        rel = test.relative_to(root)
        print(f"\n=== {rel} ===", flush=True)
        result = subprocess.run([sys.executable, str(test)], cwd=str(root), env=env)
        if result.returncode != 0:
            failures.append(rel)

    if failures:
        print("\nFAILED:")
        for failed in failures:
            print(f"  {failed}")
        return 1
    print("\nALL TESTS OK")
    return 0


def _is_refine_source_dir(p: Path) -> bool:
    """Heuristic: cwd is a refine source dir if it has pyproject.toml and refine_cli."""
    return (p / "pyproject.toml").is_file() and (p / "refine_cli" / "cli.py").is_file()


class _InitError(Exception):
    """Surface a clean error message from init helpers."""


def bootstrap_client_repo(
    client_repo: Path,
    *,
    clone_dir: Path,
    force: bool,
    create: bool,
    init_git: bool,
    reuse_existing_config: bool,
    install_unit: bool,
) -> dict[str, Path | bool | None]:
    """Create or attach a target app using the same files as `refine init`.

    `refine init` calls this with strict preconditions. The web UI calls it
    with `create=True` and `init_git=True` so a first-run user can point refine
    at a new path without preparing the repo manually.
    """
    clone_dir = clone_dir.resolve()
    client_repo = client_repo.expanduser().resolve()

    if client_repo.exists() and not client_repo.is_dir():
        raise config.ConfigError(f"not a directory: {client_repo}")
    if not client_repo.exists():
        if not create:
            raise config.ConfigError(f"not a directory: {client_repo}")
        client_repo.mkdir(parents=True)

    git_dir = client_repo / ".git"
    git_initialized = False
    if not git_dir.exists():
        if not init_git:
            raise config.ConfigError(
                f"not a git repository: {client_repo}\n"
                "  Run `git init` inside it first, or pass a path to an existing git repo."
            )
        git = shutil.which("git")
        if git is None:
            raise _InitError("could not find `git` on PATH; install git or choose an existing git repo")
        out = subprocess.run([git, "init", "-q"], cwd=str(client_repo),
                             capture_output=True, text=True, timeout=30)
        if out.returncode != 0:
            raise _InitError((out.stderr or out.stdout or "git init failed").strip())
        git_initialized = True

    target = client_repo / ".refine"
    cfg_path = target / config.CONFIG_FILENAME
    config_created = False
    if cfg_path.exists() and reuse_existing_config:
        (target / "gaps").mkdir(parents=True, exist_ok=True)
        config.ensure_refine_gitignore(target)
    else:
        cfg_path = config.write_defaults(target, force=force)
        config_created = True

    binding_written = None
    ui_unit_path = None
    if _is_refine_source_dir(clone_dir):
        binding_path = clone_dir / config.BINDING_FILENAME
        if binding_path.exists() and not force:
            raise config.ConfigError(
                f"{binding_path} already exists (use --force to rebind)"
            )
        binding_written = config.write_binding(clone_dir, client_repo)
        _remove_legacy_docker_artifacts(clone_dir)
        if install_unit:
            ui_unit_path = _write_and_enable_ui_unit(clone_dir, client_repo, force=force)
        project_registry.upsert_app(clone_dir, client_repo, make_current=True)

    return {
        "client_repo": client_repo,
        "volume_root": target,
        "config_path": cfg_path,
        "binding_path": binding_written,
        "ui_unit_path": ui_unit_path,
        "git_initialized": git_initialized,
        "config_created": config_created,
    }


def _write_and_enable_ui_unit(
    clone_dir: Path,
    client_repo: Path,
    *,
    force: bool,
    runner_unit_name: str | None = None,
    host: str = "0.0.0.0",
    port: int = 8080,
) -> Path:
    """Write the UI backend systemd user unit, daemon-reload, and enable it.

    Refuses if a unit by the same name already points at a different checkout,
    unless --force is given (in which case it's overwritten).
    """
    runner_unit = runner_unit_name or config.unit_name_for(clone_dir)
    ui_unit = _ui_unit_name(runner_unit, port)
    ui_unit_path = SYSTEMD_USER_DIR / f"{ui_unit}.service"
    SYSTEMD_USER_DIR.mkdir(parents=True, exist_ok=True)
    _remove_legacy_runtime_units(runner_unit)

    if not force:
        if ui_unit_path.exists():
            existing_wd = _grep_first(ui_unit_path.read_text(encoding="utf-8"), "WorkingDirectory=")
            if existing_wd and existing_wd != str(clone_dir):
                raise _InitError(
                    f"systemd unit {ui_unit} already exists for a different checkout:\n"
                    f"  existing WorkingDirectory: {existing_wd}\n"
                    f"  this checkout:             {clone_dir}\n"
                    f"Use --force to overwrite, or rename one of the checkouts."
                )

    uv = _find_host_command("uv")
    if uv is None:
        raise _InitError(
            "could not find `uv` on PATH or the login-shell PATH; install it "
            "before running `refine init` (the systemd unit needs an absolute "
            "path to invoke it)."
        )
    captured_env = dict(os.environ)
    if "PATH" not in captured_env:
        login_path = _user_login_path()
        if login_path:
            captured_env["PATH"] = login_path

    ui_body = (
        "# Auto-generated by `refine init`. Do not edit by hand — re-run\n"
        "# `refine init --force` to regenerate.\n"
        "[Unit]\n"
        f"Description=refine UI backend — {clone_dir} → {client_repo}\n"
        "After=network.target\n"
        "\n"
        "[Service]\n"
        "Type=simple\n"
        f"WorkingDirectory={clone_dir}\n"
        f"{_systemd_environment_lines(captured_env)}"
        f"Environment=REFINE_UI_HOST={host}\n"
        f"Environment=REFINE_UI_PORT={port}\n"
        f"Environment=REFINE_UI_SCOPE={port}\n"
        f"Environment=REFINE_CONFIG_PATH={client_repo / '.refine' / config.CONFIG_FILENAME}\n"
        f"ExecStart={uv} run refine supervisor\n"
        "Restart=on-failure\n"
        "RestartSec=2s\n"
        "TimeoutStopSec=30s\n"
        "\n"
        "[Install]\n"
        "WantedBy=default.target\n"
    )
    ui_unit_path.write_text(ui_body, encoding="utf-8")

    rc, out = _systemctl("daemon-reload")
    if rc != 0:
        raise _InitError(f"systemctl --user daemon-reload failed: {out.strip()}")
    rc, out = _systemctl("enable", ui_unit)
    if rc != 0:
        raise _InitError(f"systemctl --user enable {ui_unit} failed: {out.strip()}")

    return ui_unit_path


def _grep_first(text: str, prefix: str) -> str | None:
    for line in text.splitlines():
        if line.startswith(prefix):
            return line[len(prefix):].strip()
    return None


# ----- start / stop / status -------------------------------------------------

def cmd_install(args: argparse.Namespace) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()
    port = _effective_port(args, cfg)
    _sync_bound_project_registry(clone, cfg)
    try:
        ui_unit_path = _write_and_enable_ui_unit(
            clone,
            cfg.client_repo,
            force=True,
            runner_unit_name=unit,
            host=cfg.web_host,
            port=port,
        )
    except _InitError as e:
        print(f"refine install: {e}", file=sys.stderr)
        return 1
    ui_unit = _ui_unit_name(unit, port)
    print(f"Installed UI backend unit: {ui_unit_path}")
    print(f"Starting persistent UI backend (systemctl --user start {ui_unit})...")
    rc, out = _systemctl("start", ui_unit)
    if rc != 0:
        print(f"refine install: {out.strip()}", file=sys.stderr)
        return 1
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        print(
            f"refine install: UI backend did not start listening on "
            f"{cfg.web_host}:{port} within 20s. "
            f"Check `journalctl --user -u {ui_unit} -n 200`.",
            file=sys.stderr,
        )
        return 1
    _print_status_block(clone, unit, cfg, port=port)
    return 0


def cmd_uninstall(args: argparse.Namespace) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()
    port = _effective_port(args, cfg)
    ui_unit = _ui_unit_name(unit, port)
    print(f"Stopping persistent UI backend (systemctl --user stop {ui_unit})...")
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"  (systemctl: {out.strip()})", file=sys.stderr)
    rc, out = _systemctl("disable", ui_unit)
    if rc != 0:
        print(f"  (systemctl disable {ui_unit}: {out.strip()})", file=sys.stderr)
    unit_path = SYSTEMD_USER_DIR / f"{ui_unit}.service"
    if unit_path.exists():
        unit_path.unlink()
        print(f"Removed {unit_path}")
        _systemctl("daemon-reload")
    else:
        print(f"No unit file found at {unit_path}")
    _remove_legacy_runtime_units(unit)
    return 0


def cmd_start(args: argparse.Namespace) -> int:
    setup_clone = _setup_source_dir()
    if setup_clone is not None:
        port = _effective_port(args, None)
        print("No refine project is attached yet.")
        print(
            "Starting the host-native setup UI at "
            f"http://{_displayable_host(SETUP_UI_HOST)}:{port}"
        )
        print("Use the browser to create or attach a target app path.")
        if _port_open(SETUP_UI_HOST, port):
            print("Setup UI is already reachable.")
            return 0
        try:
            pid = _start_background_ui(setup_clone, None, host=SETUP_UI_HOST, port=port)
        except _InitError as e:
            print(f"refine start: {e}", file=sys.stderr)
            return 1
        if not _wait_for_port(SETUP_UI_HOST, port, timeout=20.0):
            print(
                f"refine start: setup UI did not start listening on "
                f"{SETUP_UI_HOST}:{port} within 20s.",
                file=sys.stderr,
            )
            return 1
        print(f"Started setup UI in background (pid {pid}).")
        return 0

    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()
    _ensure_sqlite_schema(cfg)
    port = _runtime_action_port(args, clone, cfg, unit)
    _sync_bound_project_registry(clone, cfg)
    ui_unit = _installed_ui_unit(unit, port)
    if ui_unit is not None:
        return _start_systemd_ui(clone, unit, cfg, port)
    if _port_open(cfg.web_host, port):
        print(f"UI backend is already reachable on port {port}.")
        _print_status_block(clone, unit, cfg, port=port)
        return 0
    try:
        pid = _start_background_ui(clone, cfg, host=cfg.web_host, port=port)
    except _InitError as e:
        print(f"refine start: {e}", file=sys.stderr)
        return 1

    print(f"Starting UI backend in background on port {port} (pid {pid})...")
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        log_path = _runtime_log_path(clone, cfg, port)
        print(
            f"refine start: UI backend did not start listening on "
            f"{cfg.web_host}:{port} within 20s. "
            f"Check `{log_path}`.",
            file=sys.stderr,
        )
        return 1

    _print_status_block(clone, unit, cfg, port=port)
    return 0


def cmd_stop(args: argparse.Namespace) -> int:
    setup_clone = _setup_source_dir()
    if setup_clone is not None:
        port = _runtime_action_port(args, setup_clone, None)
        stopped = _stop_background_ui(setup_clone, None, port)
        if stopped:
            print(f"Stopped setup UI backend on port {port}.")
        else:
            print(f"No background setup UI backend was running on port {port}.")
        return 0

    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()
    port = _runtime_action_port(args, clone, cfg, unit)
    ui_unit = _installed_ui_unit(unit, port)
    if ui_unit is not None:
        return _stop_systemd_ui(clone, unit, cfg, port)
    stopped = _stop_background_ui(clone, cfg, port)
    if stopped:
        print(f"Stopped UI backend on port {port}.")
    else:
        print(f"No background UI backend was running on port {port}.")
    return 0


def cmd_restart(args: argparse.Namespace) -> int:
    """`refine stop && refine start` — picks up source changes the
    running host processes haven't loaded yet without forcing the operator
    to run two commands."""
    setup_clone = _setup_source_dir()
    if setup_clone is not None:
        port = _runtime_action_port(args, setup_clone, None)
        restart_args = argparse.Namespace(**vars(args))
        restart_args.port = port
        rc = cmd_stop(restart_args)
        if rc != 0:
            return rc
        print()
        return cmd_start(restart_args)

    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()
    _ensure_sqlite_schema(cfg)
    port = _runtime_action_port(args, clone, cfg, unit)
    ui_unit = _installed_ui_unit(unit, port)
    if ui_unit is not None:
        return _restart_systemd_ui(clone, unit, cfg, port)
    restart_args = argparse.Namespace(**vars(args))
    restart_args.port = port
    rc = cmd_stop(restart_args)
    if rc != 0:
        return rc
    print()
    return cmd_start(restart_args)


def cmd_reset(args: argparse.Namespace) -> int:
    """Undo `refine init` in this checkout.

    The reverse of init: remove persistent systemd UI units, delete
    `.refine-binding` and `.refine-apps.json` from the checkout, and optionally
    purge the active app's `.refine/` directory. Leaves the app source tree
    untouched.

    After this, the checkout is fresh and can be attached to any other
    target app.
    """
    cwd = Path.cwd().resolve()
    binding_path = cwd / config.BINDING_FILENAME
    if not binding_path.exists():
        print(
            f"refine reset: no {config.BINDING_FILENAME} in {cwd} — nothing to reset.\n"
            f"  (run this from a refine source dir that was previously init'd)",
            file=sys.stderr,
        )
        return 1

    unit = config.read_binding_unit(binding_path) or config.unit_name_for(cwd)
    try:
        client_repo = config.read_binding(binding_path)
    except config.ConfigError:
        client_repo = None
    client_refine_dir = (client_repo / ".refine") if client_repo else None

    # Purge confirmation up front (refuse to silently delete data).
    if args.purge:
        if not client_refine_dir or not client_refine_dir.is_dir():
            print("refine reset: --purge: no client .refine/ directory to delete.")
        elif not args.yes:
            print(f"--purge will DELETE {client_refine_dir}")
            print("This removes ALL gap data, the sqlite index, and run state.")
            try:
                answer = input("Type 'yes' to confirm: ").strip().lower()
            except EOFError:
                answer = ""
            if answer != "yes":
                print("Aborted.")
                return 1

    # 1. Stop + remove persistent units (best-effort — keep going if already down).
    print("Stopping persistent UI backend units...")
    removed_units = False
    for unit_name in _unit_names_for_reset(unit):
        rc, out = _systemctl("stop", unit_name)
        if rc != 0:
            print(f"  (systemctl stop {unit_name}: {out.strip()})", file=sys.stderr)
        rc, out = _systemctl("disable", unit_name)
        if rc != 0:
            # If it wasn't enabled or doesn't exist, that's fine.
            print(f"  (systemctl disable {unit_name}: {out.strip()})", file=sys.stderr)
        unit_path = SYSTEMD_USER_DIR / f"{unit_name}.service"
        if unit_path.exists():
            unit_path.unlink()
            removed_units = True
            print(f"Removed {unit_path}")
    if removed_units:
        _systemctl("daemon-reload")

    # 3. Remove binding + known-app registry from the checkout.
    binding_path.unlink()
    print(f"Removed {binding_path}")
    _remove_legacy_docker_artifacts(cwd, verbose=True)
    registry_path = project_registry.registry_path(cwd)
    if registry_path.exists():
        registry_path.unlink()
        print(f"Removed {registry_path}")

    # 4. Optional: purge the target app's .refine/ directory.
    if args.purge and client_refine_dir and client_refine_dir.is_dir():
        shutil.rmtree(client_refine_dir)
        print(f"Removed {client_refine_dir}")

    print()
    print("Reset complete. To attach a target app:")
    print(f"  cd {cwd}")
    print(f"  uv run refine init <path/to/target-app>")
    if client_refine_dir and client_refine_dir.is_dir() and not args.purge:
        print()
        print(f"The previous app's refine data is preserved at:")
        print(f"  {client_refine_dir}")
        print("Re-running `refine init` against that path will pick it up again.")
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    setup_clone = _setup_source_dir()
    if setup_clone is not None:
        for port in _status_ports(args, setup_clone, None):
            _print_setup_status_block(setup_clone, port=port)
        return 0

    clone, unit = _resolve_clone_and_unit_or_exit()
    try:
        cfg = config.get(path=args.config) if args.config else config.get()
    except config.ConfigError as e:
        print(f"refine status: {e}", file=sys.stderr)
        return 1
    _sync_bound_project_registry(clone, cfg)
    for port in _status_ports(args, clone, cfg, unit):
        _print_status_block(clone, unit, cfg, port=port)
    return 0


def _print_status_block(clone: Path, unit: str, cfg: "config.Config", *,
                        port: int | None = None) -> None:
    effective_port = port or cfg.web_port
    ui_unit = _ui_unit_name(unit, effective_port)
    web_up = _port_open(cfg.web_host, effective_port)
    process_pid = _running_pid(clone, cfg, effective_port)
    display_cfg = _running_config(process_pid) or cfg
    service_active = _systemctl_is_active(ui_unit)

    print()
    print(_bold("refine"))
    print(f"  checkout: {clone}")
    print(f"  app:      {display_cfg.client_repo}")
    print(f"  ui:       {_dot((process_pid is not None or service_active) and web_up)} "
          f"http {'reachable' if web_up else 'unreachable'} at "
          f"http://{_displayable_host(cfg.web_host)}:{effective_port}")
    print(f"  process:  {_dot(process_pid is not None)} "
          f"{'pid ' + str(process_pid) if process_pid is not None else 'not running'}")
    print(f"  service:  {_dot(service_active)} systemd unit `{ui_unit}` "
          f"({'active' if service_active else 'inactive'})")
    print(f"  server:   {_dot((process_pid is not None or service_active) and web_up)} "
          "supervisor-managed UI + runner worker")
    print(f"  logs:     {_runtime_log_path(clone, display_cfg, effective_port)}")
    print(f"  journal:  journalctl --user -u {ui_unit} -f")
    print(f"  stop:     uv run refine stop {effective_port}")
    print()


def _print_setup_status_block(clone: Path, *, port: int) -> None:
    web_up = _port_open(SETUP_UI_HOST, port)
    process_pid = _running_pid(clone, None, port)
    print()
    print(_bold("refine"))
    print(f"  checkout: {clone}")
    print("  app:      setup mode")
    print(f"  ui:       {_dot(process_pid is not None and web_up)} "
          f"http {'reachable' if web_up else 'unreachable'} at "
          f"http://{_displayable_host(SETUP_UI_HOST)}:{port}")
    print(f"  process:  {_dot(process_pid is not None)} "
          f"{'pid ' + str(process_pid) if process_pid is not None else 'not running'}")
    print(f"  logs:     {_runtime_log_path(clone, None, port)}")
    print(f"  stop:     uv run refine stop {port}")
    print()


def _running_config(pid: int | None) -> "config.Config | None":
    if pid is None:
        return None
    cfg_path = _pid_env_value(pid, config.ENV_CONFIG_PATH)
    if not cfg_path:
        return None
    try:
        return config.Config.load(cfg_path)
    except config.ConfigError:
        return None


# ----- server / ui ------------------------------------------------------------

def cmd_server_foreground(args: argparse.Namespace) -> int:
    """Run the server component in the foreground for debugging.

    The production path is `refine supervisor`, which keeps the UI/control
    process separate from the work runner.
    """
    _load_config_or_exit(args)
    from refine_server.__main__ import main as server_main
    return server_main()


def cmd_ui(args: argparse.Namespace) -> int:
    from refine_ui.__main__ import main as ui_main
    return ui_main()


# ----- doctor -----------------------------------------------------------------

def cmd_doctor(args: argparse.Namespace) -> int:
    cfg_path = args.config
    try:
        cfg = config.get(path=cfg_path) if cfg_path else config.get()
    except config.ConfigError as e:
        print(_red("No refine configuration found."))
        print(f"  {e}")
        print()
        print("Run `refine init <target-app>` from the refine checkout to create one.")
        return 1

    print(_section("Configuration"))
    _kv("config file",   cfg.config_path)
    _kv("volume root",   cfg.volume_root)
    _kv("target app",    cfg.client_repo)
    _kv("web host:port", f"{cfg.web_host}:{cfg.web_port}")

    print(_section("Volume root"))
    sqlite_present = cfg.sqlite_path.is_file()
    _kv("index.sqlite",  f"{cfg.sqlite_path} ({'present' if sqlite_present else 'missing'})")
    gap_count = _count_gap_files(cfg.gaps_dir)
    _kv("gaps/ files",   f"{gap_count} gap.json file(s)")

    print(_section("Agent CLI"))
    agent_cli = _configured_agent_cli(cfg.sqlite_path)
    cli_path = shutil.which(agent_cli) or "(not on PATH)"
    _kv("provider", agent_cli)
    _kv(f"{agent_cli} path", cli_path)
    ok, msg = _cli_version(cli_path, agent_cli)
    _kv(f"{agent_cli} --version", _bool(ok))
    if not ok:
        _kv("error", msg or "")

    print(_section("Client repo / git"))
    repo = cfg.client_repo
    if not (repo / ".git").exists():
        _kv("git", _red("not a git repository"))
    else:
        branch = _git(repo, "symbolic-ref", "--quiet", "--short", "HEAD")
        upstream = _git(repo, "rev-parse", "--abbrev-ref", f"{branch.strip()}@{{upstream}}") if branch.strip() else None
        dirty = _git(repo, "status", "--porcelain")
        _kv("branch", branch.strip() or _red("detached HEAD"))
        _kv("upstream", (upstream or "").strip() or _red("no upstream"))
        _kv("clean", _bool(not (dirty or "").strip()))

    print()
    ok_all = reachable and ok and (repo / ".git").exists()
    print(_green("doctor: ok") if ok_all else _red("doctor: issues to address (see above)"))
    binding = config.find_binding()
    if binding is not None:
        _sync_bound_project_registry(binding.parent.resolve(), cfg)
    return 0 if ok_all else 1


# ----- helpers ----------------------------------------------------------------

def _sync_bound_project_registry(clone: Path, cfg: "config.Config") -> None:
    """Migrate an old single-app binding into the clone-local app registry."""
    if not _is_refine_source_dir(clone):
        return
    try:
        project_registry.upsert_app(clone, cfg.client_repo, make_current=True)
    except (OSError, config.ConfigError):
        # Registry migration is best-effort; startup/status should not fail
        # because the source checkout is unexpectedly read-only.
        pass


def _effective_port(args: argparse.Namespace, cfg: "config.Config | None") -> int:
    default_port = cfg.web_port if cfg is not None else 8080
    raw_port = getattr(args, "port", None)
    port = int(raw_port if raw_port is not None else default_port)
    if port <= 0 or port > 65535:
        raise SystemExit(f"refine: invalid port {port}")
    return port


def _start_background_ui(
    clone: Path,
    cfg: "config.Config | None",
    *,
    host: str,
    port: int,
) -> int:
    live = _running_pid(clone, cfg, port)
    if live is not None:
        return live
    pid_path = _runtime_pid_path(clone, cfg, port)
    log_path = _runtime_log_path(clone, cfg, port)
    pid_path.parent.mkdir(parents=True, exist_ok=True)
    log_path.parent.mkdir(parents=True, exist_ok=True)

    uv = _find_host_command("uv")
    if uv is None:
        raise _InitError(
            "could not find `uv` on PATH or the login-shell PATH; install it "
            "before running `refine start`."
        )

    env = os.environ.copy()
    env["REFINE_UI_HOST"] = host
    env["REFINE_UI_PORT"] = str(port)
    env["REFINE_UI_SCOPE"] = str(port)
    if cfg is not None:
        env["REFINE_CONFIG_PATH"] = str(cfg.config_path)
    env.setdefault("PYTHONUNBUFFERED", "1")
    command = [uv, "run", "refine", "ui" if cfg is None else "supervisor"]
    with log_path.open("ab") as log:
        proc = subprocess.Popen(
            command,
            cwd=str(clone),
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            env=env,
            start_new_session=True,
        )
    pid_path.write_text(f"{proc.pid}\n", encoding="utf-8")
    return proc.pid


def cmd_supervisor(args: argparse.Namespace) -> int:  # noqa: ARG001
    from refine_runtime.supervisor import main as supervisor_main

    return supervisor_main()


def _stop_background_ui(clone: Path, cfg: "config.Config | None", port: int) -> bool:
    pid_path = _runtime_pid_path(clone, cfg, port)
    pid = _running_pid(clone, cfg, port)
    if pid is None:
        return False
    try:
        pgid = os.getpgid(pid)
    except ProcessLookupError:
        _unlink_quietly(pid_path)
        return False
    try:
        os.killpg(pgid, signal.SIGTERM)
    except ProcessLookupError:
        _unlink_quietly(pid_path)
        return False
    except OSError:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            _unlink_quietly(pid_path)
            return False
    deadline = time.time() + 10
    while time.time() < deadline:
        if not _pid_alive(pid):
            _unlink_quietly(pid_path)
            return True
        time.sleep(0.2)
    try:
        os.killpg(pgid, signal.SIGKILL)
    except OSError:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
    _unlink_quietly(pid_path)
    return True


def _start_systemd_ui(clone: Path, unit: str, cfg: "config.Config", port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Starting persistent UI backend (systemctl --user start {ui_unit})...")
    rc, out = _systemctl("start", ui_unit)
    if rc != 0:
        print(f"refine start: systemctl --user start {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    _unlink_quietly(_runtime_pid_path(clone, cfg, port))
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        print(
            f"refine start: systemd unit {ui_unit} did not start listening on "
            f"{cfg.web_host}:{port} within 20s.",
            file=sys.stderr,
        )
        return 1
    _print_status_block(clone, unit, cfg, port=port)
    return 0


def _stop_systemd_ui(clone: Path, unit: str, cfg: "config.Config", port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Stopping persistent UI backend (systemctl --user stop {ui_unit})...")
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"refine stop: systemctl --user stop {ui_unit} failed: {out.strip()}",
              file=sys.stderr)
        return 1
    _unlink_quietly(_runtime_pid_path(clone, cfg, port))
    print(f"Stopped persistent UI backend on port {port}.")
    return 0


def _restart_systemd_ui(clone: Path, unit: str, cfg: "config.Config", port: int) -> int:
    ui_unit = _ui_unit_name(unit, port)
    print(f"Restarting persistent UI backend (systemctl --user restart {ui_unit})...")
    rc, out = _systemctl("restart", ui_unit)
    if rc != 0:
        print(
            f"refine restart: systemctl --user restart {ui_unit} failed: {out.strip()}",
            file=sys.stderr,
        )
        return 1
    _unlink_quietly(_runtime_pid_path(clone, cfg, port))
    if not _wait_for_port(cfg.web_host, port, timeout=20.0):
        print(
            f"refine restart: systemd unit {ui_unit} did not start listening on "
            f"{cfg.web_host}:{port} within 20s.",
            file=sys.stderr,
        )
        return 1
    _print_status_block(clone, unit, cfg, port=port)
    return 0


def _running_pid(clone: Path, cfg: "config.Config | None", port: int) -> int | None:
    pid_path = _runtime_pid_path(clone, cfg, port)
    pid = _read_pid(pid_path)
    if pid is not None and _pid_alive(pid):
        return pid
    if pid is not None:
        _unlink_quietly(pid_path)

    host = cfg.web_host if cfg is not None else SETUP_UI_HOST
    listener = _refine_ui_listener_pid(clone, host, port)
    if listener is not None and _pid_alive(listener):
        return listener
    return None


def _runtime_pid_path(clone: Path, cfg: "config.Config | None", port: int) -> Path:
    return _runtime_dir(clone, cfg) / f"ui-{port}.pid"


def _runtime_log_path(clone: Path, cfg: "config.Config | None", port: int) -> Path:
    return _runtime_dir(clone, cfg) / f"ui-{port}.log"


def _runtime_dir(clone: Path, cfg: "config.Config | None") -> Path:
    return config.local_run_dir(clone)


def _read_pid(path: Path) -> int | None:
    try:
        return int(path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def _refine_ui_listener_pid(clone: Path, host: str, port: int) -> int | None:
    for pid in _listener_pids(port):
        if _pid_matches_refine_ui(pid, clone):
            return pid
    return None


def _listener_pids(port: int) -> list[int]:
    pids: list[int] = []
    lsof = shutil.which("lsof")
    if lsof:
        try:
            out = subprocess.run(
                [lsof, "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fp"],
                check=False,
                capture_output=True,
                text=True,
                timeout=2,
            ).stdout
            for line in out.splitlines():
                if line.startswith("p") and line[1:].isdigit():
                    pids.append(int(line[1:]))
        except (OSError, subprocess.TimeoutExpired):
            pass
    if pids:
        return pids

    ss = shutil.which("ss")
    if not ss:
        return []
    try:
        out = subprocess.run(
            [ss, "-ltnp", f"sport = :{port}"],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    return [int(m.group(1)) for m in re.finditer(r"pid=(\d+)", out)]


def _pid_matches_refine_ui(pid: int, clone: Path) -> bool:
    cmdline = _pid_cmdline(pid)
    if not cmdline:
        return False
    if "refine" not in cmdline or re.search(r"(?:^|\s)ui(?:\s|$)", cmdline) is None:
        return False
    cwd = _pid_cwd(pid)
    return cwd is not None and cwd == clone.resolve()


def _owned_refine_ui_ports(clone: Path) -> list[int]:
    ports: set[int] = set()
    for pid, listen_port in _listener_port_pids():
        if not _pid_matches_refine_ui(pid, clone):
            continue
        env_port = _pid_env_value(pid, "REFINE_UI_PORT")
        if env_port and env_port.isdigit():
            ports.add(int(env_port))
        else:
            ports.add(listen_port)
    return sorted(ports)


def _status_ports(args: argparse.Namespace, clone: Path,
                  cfg: "config.Config | None",
                  unit: str | None = None) -> list[int]:
    if getattr(args, "port", None) is not None:
        return [_effective_port(args, cfg)]
    ports = set(_runtime_pid_ports(clone, cfg))
    ports.update(_owned_refine_ui_ports(clone))
    if unit is not None:
        ports.update(_installed_ui_unit_ports(unit))
    if not ports:
        ports.add(_effective_port(args, cfg))
    return sorted(ports)


def _runtime_pid_ports(clone: Path, cfg: "config.Config | None") -> list[int]:
    run_dir = _runtime_dir(clone, cfg)
    ports: set[int] = set()
    try:
        entries = list(run_dir.glob("ui-*.pid"))
    except OSError:
        return []
    for path in entries:
        m = re.fullmatch(r"ui-(\d+)\.pid", path.name)
        if not m:
            continue
        port = int(m.group(1))
        if 0 < port <= 65535:
            ports.add(port)
    return sorted(ports)


def _listener_port_pids() -> list[tuple[int, int]]:
    pairs: list[tuple[int, int]] = []
    lsof = shutil.which("lsof")
    if lsof:
        try:
            out = subprocess.run(
                [lsof, "-nP", "-iTCP", "-sTCP:LISTEN", "-Fp", "-Fn"],
                check=False,
                capture_output=True,
                text=True,
                timeout=2,
            ).stdout
        except (OSError, subprocess.TimeoutExpired):
            out = ""
        pid: int | None = None
        for line in out.splitlines():
            if line.startswith("p") and line[1:].isdigit():
                pid = int(line[1:])
                continue
            if pid is None or not line.startswith("n"):
                continue
            m = re.search(r":(\d+)(?:\s|$)", line)
            if m:
                pairs.append((pid, int(m.group(1))))
        if pairs:
            return pairs

    ss = shutil.which("ss")
    if not ss:
        return []
    try:
        out = subprocess.run(
            [ss, "-ltnp"],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return []
    for line in out.splitlines():
        pid_match = re.search(r"pid=(\d+)", line)
        port_match = re.search(r":(\d+)\s+", line)
        if pid_match and port_match:
            pairs.append((int(pid_match.group(1)), int(port_match.group(1))))
    return pairs


def _runtime_action_port(args: argparse.Namespace, clone: Path,
                         cfg: "config.Config | None",
                         unit: str | None = None) -> int:
    configured = _effective_port(args, cfg)
    if getattr(args, "port", None) is not None:
        return configured
    if _running_pid(clone, cfg, configured) is not None:
        return configured
    if unit is not None and _installed_ui_unit(unit, configured) is not None:
        return configured
    live_ports = [p for p in _owned_refine_ui_ports(clone) if p != configured]
    if len(live_ports) == 1:
        return live_ports[0]
    if unit is not None:
        installed_ports = [p for p in _installed_ui_unit_ports(unit) if p != configured]
        if len(installed_ports) == 1:
            return installed_ports[0]
    return configured


def _pid_cmdline(pid: int) -> str:
    proc_path = Path("/proc") / str(pid) / "cmdline"
    try:
        raw = proc_path.read_bytes()
    except OSError:
        raw = b""
    if raw:
        return raw.replace(b"\0", b" ").decode("utf-8", "replace").strip()
    try:
        out = subprocess.run(
            ["ps", "-p", str(pid), "-o", "command="],
            check=False,
            capture_output=True,
            text=True,
            timeout=2,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return out.strip()


def _pid_cwd(pid: int) -> Path | None:
    try:
        return Path(f"/proc/{pid}/cwd").resolve(strict=True)
    except OSError:
        return None


def _pid_env_value(pid: int, key: str) -> str | None:
    try:
        raw = (Path("/proc") / str(pid) / "environ").read_bytes()
    except OSError:
        return None
    prefix = f"{key}=".encode("utf-8")
    for item in raw.split(b"\0"):
        if item.startswith(prefix):
            return item[len(prefix):].decode("utf-8", "replace")
    return None


def _setup_source_dir() -> Path | None:
    cwd = Path.cwd().resolve()
    if (
        config.find_binding(cwd) is None
        and config.find_config(cwd) is None
        and _is_refine_source_dir(cwd)
    ):
        return cwd
    return None


def _unlink_quietly(path: Path) -> None:
    try:
        path.unlink()
    except OSError:
        pass


def _unit_names_for_reset(unit: str) -> list[str]:
    names = [unit, _legacy_pre_ui_unit_name(unit), f"{unit}-ui"]
    try:
        names.extend(path.stem for path in SYSTEMD_USER_DIR.glob(f"{unit}-*-ui.service"))
    except OSError:
        pass
    out: list[str] = []
    for name in names:
        if name not in out:
            out.append(name)
    return out


def _installed_ui_unit(unit: str, port: int) -> str | None:
    ui_unit = _ui_unit_name(unit, port)
    if (SYSTEMD_USER_DIR / f"{ui_unit}.service").exists():
        return ui_unit
    return None


def _installed_ui_unit_ports(unit: str) -> list[int]:
    ports: set[int] = set()
    pattern = re.compile(rf"^{re.escape(unit)}-(\d+)-ui\.service$")
    try:
        paths = list(SYSTEMD_USER_DIR.glob(f"{unit}-*-ui.service"))
    except OSError:
        return []
    for path in paths:
        m = pattern.fullmatch(path.name)
        if m:
            ports.add(int(m.group(1)))
    return sorted(ports)


def _ui_unit_name(runner_unit: str, port: int) -> str:
    return f"{runner_unit}-{port}-ui"


def _legacy_pre_ui_unit_name(runner_unit: str) -> str:
    return f"{runner_unit}-web"


def _find_host_command(name: str) -> str | None:
    """Resolve a host command using the current PATH, then the user's login PATH."""
    direct = shutil.which(name)
    if direct:
        return direct
    login_path = _user_login_path()
    if login_path:
        return shutil.which(name, path=login_path)
    return None


def _systemd_environment_lines(env: dict[str, str]) -> str:
    return "".join(
        _systemd_environment_line(name, value)
        for name, value in sorted(env.items())
        if _valid_environment_name(name)
    )


def _valid_environment_name(name: str) -> bool:
    return bool(re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name or ""))


def _systemd_environment_line(name: str, value: str | None) -> str:
    if value is None:
        return ""
    escaped = (
        value
        .replace("\\", "\\\\")
        .replace("\n", "\\n")
        .replace("\r", "\\r")
        .replace('"', '\\"')
        .replace("%", "%%")
    )
    return f'Environment="{name}={escaped}"\n'


def _user_login_path() -> str | None:
    """Return the PATH an interactive login shell sees.

    systemd --user services often run with a minimal PATH that misses uv
    installs in ~/.local/bin, ~/.cargo/bin, asdf/mise shims, Homebrew, etc.
    Project setup may run inside the host-native UI service, so resolving uv
    must match the operator's terminal rather than systemd's stripped env.
    """
    global _LOGIN_PATH_CACHE, _LOGIN_PATH_RESOLVED
    if _LOGIN_PATH_RESOLVED:
        return _LOGIN_PATH_CACHE
    _LOGIN_PATH_RESOLVED = True
    shell = os.environ.get("SHELL") or "/bin/bash"
    for flag in ("-lic", "-lc"):
        try:
            out = subprocess.run(
                [shell, flag, 'printf %s "$PATH"'],
                capture_output=True, text=True, timeout=5,
            )
        except Exception:
            continue
        path = (out.stdout or "").strip()
        if out.returncode == 0 and path:
            _LOGIN_PATH_CACHE = path
            break
    return _LOGIN_PATH_CACHE


def _ensure_host_unit_installed(clone: Path, cfg: "config.Config", runner_unit: str) -> None:
    ui_unit = _ui_unit_name(runner_unit, cfg.web_port)
    ui_path = SYSTEMD_USER_DIR / f"{ui_unit}.service"
    _remove_legacy_runtime_units(runner_unit)
    if ui_path.exists():
        return
    _write_and_enable_ui_unit(
        clone, cfg.client_repo, force=True, runner_unit_name=runner_unit,
        host=cfg.web_host, port=cfg.web_port,
    )


def _remove_legacy_runtime_units(runner_unit: str) -> None:
    for unit_name in (runner_unit, _legacy_pre_ui_unit_name(runner_unit), f"{runner_unit}-ui"):
        _remove_unit(unit_name)


def _remove_unit(unit_name: str) -> None:
    unit_path = SYSTEMD_USER_DIR / f"{unit_name}.service"
    if not unit_path.exists():
        return
    _systemctl("stop", unit_name)
    _systemctl("disable", unit_name)
    try:
        unit_path.unlink()
    except OSError:
        return
    _systemctl("daemon-reload")


def _remove_legacy_docker_artifacts(clone: Path, *, verbose: bool = False) -> None:
    env_path = clone / ".env"
    try:
        text = env_path.read_text(encoding="utf-8")
    except OSError:
        text = ""
    if text and "REFINE_CLIENT_REFINE_DIR=" in text:
        env_path.unlink()
        if verbose:
            print(f"Removed legacy {env_path}")
    current_link = clone / ".refine-current"
    if current_link.is_symlink() or current_link.exists():
        current_link.unlink()
        if verbose:
            print(f"Removed legacy {current_link}")

def _load_config_or_exit(args: argparse.Namespace) -> None:
    try:
        if args.config:
            config.get(path=args.config)
        else:
            config.get()
    except config.ConfigError as e:
        print(f"refine: {e}", file=sys.stderr)
        sys.exit(1)


def _ensure_sqlite_schema(cfg: "config.Config") -> None:
    """Apply lightweight SQLite schema migrations before runtime handoff.

    The UI backend initializes SQLite too, but start/restart can delegate to a
    systemd unit that was installed from a different checkout. Running schema
    setup from the invoking CLI makes cache-table migrations immediate after a
    pull, before the service process starts serving requests.
    """
    from refine_server import db

    db.init_db(cfg.sqlite_path)


def _resolve_clone_and_unit_or_exit() -> tuple[Path, str]:
    """Find the binding for the current cwd and return (clone_dir, unit_name).

    start/stop/status only make sense when run from a bound refine source dir.
    """
    binding = config.find_binding()
    if binding is None:
        print(
            "refine: no .refine-binding in scope. Run `refine init <target-app>` "
            "from a refine source dir first.",
            file=sys.stderr,
        )
        sys.exit(1)
    clone = binding.parent.resolve()
    unit = config.read_binding_unit(binding) or config.unit_name_for(clone)
    return clone, unit


def _systemctl(*args: str) -> tuple[int, str]:
    # systemd's TimeoutStopSec defaults to 90s and our unit template caps
    # it to 30s. The wrapper has to give systemd at least its full stop
    # window or else we report a false-positive "timed out" — which is
    # what `refine stop` used to do on units the agent had spawned child
    # processes for.
    cmd = args[0] if args else ""
    timeout = 60 if cmd in ("stop", "start", "restart") else 15
    try:
        out = subprocess.run(
            ["systemctl", "--user", *args],
            capture_output=True, text=True, timeout=timeout,
        )
    except FileNotFoundError:
        return 127, "systemctl not found (systemd --user required)"
    except subprocess.TimeoutExpired:
        return 124, "systemctl timed out"
    return out.returncode, (out.stderr or out.stdout)


def _systemctl_is_active(unit: str) -> bool:
    rc, _ = _systemctl("is-active", "--quiet", unit)
    return rc == 0


def _wait_for_port(host: str, port: int, *, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _port_open(host, port):
            return True
        time.sleep(0.2)
    return False


def _port_open(host: str, port: int) -> bool:
    target = "127.0.0.1" if host in ("0.0.0.0", "::") else host
    try:
        with urllib.request.urlopen(f"http://{target}:{port}/api/project/status", timeout=1.0):
            return True
    except (OSError, urllib.error.URLError):
        return False


def _configured_agent_cli(sqlite_path: Path) -> str:
    if not sqlite_path.is_file():
        return "claude"
    try:
        import sqlite3
        conn = sqlite3.connect(str(sqlite_path))
        row = conn.execute(
            "SELECT value FROM settings WHERE key = 'agent_cli'",
        ).fetchone()
        conn.close()
        value = (row[0] if row else "claude") or "claude"
        value = str(value).strip().lower()
        return value if value in ("claude", "codex", "gemini") else "claude"
    except Exception:
        return "claude"


def _cli_version(cli_path: str, binary: str) -> tuple[bool, str | None]:
    try:
        out = subprocess.run(
            [cli_path, "--version"],
            capture_output=True, text=True, timeout=10,
        )
        if out.returncode == 0:
            return True, out.stdout.strip()
        return False, (out.stderr.strip() or out.stdout.strip()
                       or f"exit code {out.returncode}")
    except FileNotFoundError as e:
        return False, repr(e)
    except subprocess.TimeoutExpired:
        return False, f"{binary} --version timed out (10s)"
    except Exception as e:
        return False, repr(e)


def _git(cwd: Path, *args: str) -> str:
    try:
        out = subprocess.run(
            ["git", *args], cwd=str(cwd),
            capture_output=True, text=True, timeout=10,
        )
        return out.stdout
    except Exception:
        return ""


def _count_gap_files(gaps_dir: Path) -> int:
    if not gaps_dir.is_dir():
        return 0
    return sum(1 for _ in gaps_dir.rglob("gap.json"))


def _displayable_host(h: str) -> str:
    return "localhost" if h in ("0.0.0.0", "::") else h


def _section(title: str) -> str:
    return f"\n{_bold(title)}\n" + "-" * len(title)


def _kv(label: str, value) -> None:  # noqa: ANN001
    print(f"  {label:<22} {value}")


def _bool(b: bool) -> str:
    return _green("yes") if b else _red("no")


def _dot(b: bool) -> str:
    return _green("●") if b else _red("○")


def _bold(s: str) -> str:
    return f"\033[1m{s}\033[0m" if sys.stdout.isatty() else s


def _red(s: str) -> str:
    return f"\033[31m{s}\033[0m" if sys.stdout.isatty() else s


def _green(s: str) -> str:
    return f"\033[32m{s}\033[0m" if sys.stdout.isatty() else s
