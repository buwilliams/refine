"""argparse-based CLI for refine.

Subcommands:
- init    — write refine.toml + run/ + gaps/ in a chosen volume root,
            then write + enable a systemd --user unit for the UI backend.
- reset   — undo `init` in this checkout (stop service, disable unit,
            remove binding); optional --purge also deletes the active app's
            .refine/ data so you can attach a different app.
- start   — bring up the host-native UI backend.
- stop    — stop the UI backend.
- restart — stop then start (handy for picking up source changes).
- status  — show what's running (read-only).
- test    — run the repository's script-style test suite.
- server  — start the server component in the foreground for debugging.
- ui      — start the UI backend in-process (used by the systemd unit).
- doctor  — deeper diagnostic snapshot (config, agent CLI, git).
"""
from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from refine_server import config, project_registry


SYSTEMD_USER_DIR = Path.home() / ".config" / "systemd" / "user"
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
        metavar="{init,reset,start,restart,stop,status,test,server,ui,doctor}",
    )

    p_init = sub.add_parser(
        "init",
        help="Initialize/add a target app and make it active.",
        description=(
            "Bootstraps a target app: creates <app>/.refine/refine.toml + "
            "run/ + gaps/, writes the active-app binding, records the app in "
            "the known-apps list, and registers a systemd --user unit for the "
            "host-native UI backend."
        ),
    )
    p_init.add_argument(
        "path", nargs="?", default=None,
        help="Path to the target app repo. Defaults to cwd (back-compat).",
    )
    p_init.add_argument("--force", action="store_true",
                        help="Overwrite an existing refine.toml / .refine-binding / unit file.")
    p_init.set_defaults(fn=cmd_init)

    p_reset = sub.add_parser(
        "reset",
        help="Undo `refine init` in this checkout so it can attach a different app.",
        description=(
            "Stops the UI backend, removes the .refine-binding and "
            ".refine-apps.json files from this checkout, disables + removes "
            "the systemd --user UI unit, and (with --purge) wipes the active "
            "app's .refine/ directory. The app source tree is never touched."
        ),
    )
    p_reset.add_argument(
        "--purge", action="store_true",
        help="Also delete the active target app's .refine/ directory "
             "(gap.json files, sqlite index, run/, .gitignore). DATA LOSS.",
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
            "Starts the host-native UI backend via systemd --user, then "
            "prints a status block. The backend owns the runner in-process."
        ),
    )
    p_start.set_defaults(fn=cmd_start)

    p_restart = sub.add_parser(
        "restart",
        help="Stop the UI backend, then start it again.",
        description=(
            "Equivalent to `refine stop && refine start`."
        ),
    )
    p_restart.set_defaults(fn=cmd_restart)

    p_stop = sub.add_parser(
        "stop",
        help="Stop the UI backend.",
    )
    p_stop.set_defaults(fn=cmd_stop)

    p_status = sub.add_parser(
        "status",
        help="Show what's running (read-only).",
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
            install_unit=True,
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
    if ui_unit_path:
        print(f"Installed UI backend unit: {ui_unit_path}")
    print()
    print("Next steps:")
    if binding_written:
        print(f"  uv run refine start          # UI backend + server, one command")
        print(f"  uv run refine status         # check it's healthy")
        print(f"  uv run refine stop           # tear it all down")
        print()
        print("To survive logout / reboot:")
        print(f"  loginctl enable-linger $USER  # one-time, sudo-prompts as needed")
    else:
        print(f"  cd {client_repo}")
        print(f"  refine doctor                 # sanity check the config")
        print()
        print("Note: full refine start/stop/status orchestration requires running")
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
    print(f"Running {len(tests)} test scripts")
    for test in tests:
        rel = test.relative_to(root)
        print(f"\n=== {rel} ===", flush=True)
        result = subprocess.run([sys.executable, str(test)], cwd=str(root))
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
        (target / "run").mkdir(parents=True, exist_ok=True)
        (target / "gaps").mkdir(parents=True, exist_ok=True)
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
) -> Path:
    """Write the UI backend systemd user unit, daemon-reload, and enable it.

    Refuses if a unit by the same name already points at a different checkout,
    unless --force is given (in which case it's overwritten).
    """
    runner_unit = runner_unit_name or config.unit_name_for(clone_dir)
    ui_unit = _ui_unit_name(runner_unit)
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
        f"ExecStart={uv} run refine ui\n"
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

def cmd_start(args: argparse.Namespace) -> int:
    binding = config.find_binding()
    if binding is None and config.find_config() is None and _is_refine_source_dir(Path.cwd()):
        print("No refine project is attached yet.")
        print("Starting the host-native setup UI at http://127.0.0.1:8080")
        print("Use the browser to create or attach a target app path.")
        from refine_ui.__main__ import main as ui_main
        return ui_main()

    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()
    _sync_bound_project_registry(clone, cfg)
    try:
        _ensure_host_unit_installed(clone, cfg, unit)
    except _InitError as e:
        print(f"refine start: {e}", file=sys.stderr)
        return 1
    ui_unit = _ui_unit_name(unit)

    print(f"Starting UI backend (systemctl --user start {ui_unit})…")
    rc, out = _systemctl("start", ui_unit)
    if rc != 0:
        print(f"refine start: {out.strip()}", file=sys.stderr)
        return 1

    if not _wait_for_port(cfg.web_host, cfg.web_port, timeout=20.0):
        print(
            f"refine start: UI backend did not start listening on "
            f"{cfg.web_host}:{cfg.web_port} within 20s. "
            f"Check `journalctl --user -u {ui_unit} -n 200`.",
            file=sys.stderr,
        )
        return 1

    _print_status_block(clone, unit, cfg)
    return 0


def cmd_stop(args: argparse.Namespace) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()
    ui_unit = _ui_unit_name(unit)

    print(f"Stopping UI backend (systemctl --user stop {ui_unit})…")
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"  (systemctl: {out.strip()})", file=sys.stderr)
    legacy_unit = _legacy_pre_ui_unit_name(unit)
    rc, out = _systemctl("stop", legacy_unit)
    if rc != 0:
        print(f"  (legacy systemctl: {out.strip()})", file=sys.stderr)

    print("Stopped.")
    return 0


def cmd_restart(args: argparse.Namespace) -> int:
    """`refine stop && refine start` — picks up source changes the
    running host processes haven't loaded yet without forcing the operator
    to run two commands."""
    rc = cmd_stop(args)
    if rc != 0:
        return rc
    print()
    return cmd_start(args)


def cmd_reset(args: argparse.Namespace) -> int:
    """Undo `refine init` in this checkout.

    The reverse of init: stop service, disable + remove the systemd UI unit,
    delete `.refine-binding` and `.refine-apps.json` from the checkout, and
    optionally purge the active app's `.refine/` directory. Leaves the app
    source tree untouched.

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
    ui_unit = _ui_unit_name(unit)
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

    # 1. Stop service (best-effort — keep going if it was already down).
    print(f"Stopping UI backend (systemctl --user stop {ui_unit})…")
    rc, out = _systemctl("stop", ui_unit)
    if rc != 0:
        print(f"  (systemctl: {out.strip()})", file=sys.stderr)

    # 2. Disable + remove the UI unit. Also remove legacy runtime units if
    # present from older installs.
    removed_units = False
    for unit_name in (unit, _legacy_pre_ui_unit_name(unit), ui_unit):
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
    clone, unit = _resolve_clone_and_unit_or_exit()
    try:
        cfg = config.get(path=args.config) if args.config else config.get()
    except config.ConfigError as e:
        print(f"refine status: {e}", file=sys.stderr)
        return 1
    _sync_bound_project_registry(clone, cfg)
    _print_status_block(clone, unit, cfg)
    return 0


def _print_status_block(clone: Path, unit: str, cfg: "config.Config") -> None:
    ui_unit = _ui_unit_name(unit)
    web_up = _port_open(cfg.web_host, cfg.web_port)
    web_active = _systemctl_is_active(ui_unit)

    print()
    print(_bold("refine"))
    print(f"  checkout: {clone}")
    print(f"  app:      {cfg.client_repo}")
    print(f"  ui:       {_dot(web_active and web_up)} systemd unit `{ui_unit}` "
          f"({'active' if web_active else 'inactive'}, "
          f"http {'reachable' if web_up else 'unreachable'} at "
          f"http://{_displayable_host(cfg.web_host)}:{cfg.web_port})")
    print(f"  server:   {_dot(web_active and web_up)} in-process with UI backend")
    print(f"  logs:     journalctl --user -u {ui_unit} -f")
    print(f"  stop:     uv run refine stop")
    print()


# ----- server / ui ------------------------------------------------------------

def cmd_server_foreground(args: argparse.Namespace) -> int:
    """Run the server component in the foreground for debugging.

    The production path is `refine ui`, which owns the runner in-process.
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


def _ui_unit_name(runner_unit: str) -> str:
    return f"{runner_unit}-ui"


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
    ui_unit = _ui_unit_name(runner_unit)
    ui_path = SYSTEMD_USER_DIR / f"{ui_unit}.service"
    _remove_legacy_runtime_units(runner_unit)
    if ui_path.exists():
        return
    _write_and_enable_ui_unit(clone, cfg.client_repo, force=True, runner_unit_name=runner_unit)


def _remove_legacy_runtime_units(runner_unit: str) -> None:
    for unit_name in (runner_unit, _legacy_pre_ui_unit_name(runner_unit)):
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
