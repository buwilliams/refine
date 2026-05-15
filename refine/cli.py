"""argparse-based CLI for refine.

Subcommands:
- init    — write refine.toml + run/ + gaps/ in a chosen volume root,
            then write + enable a systemd --user unit for the runner.
- reset   — undo `init` in this clone (stop services, disable unit,
            remove binding); optional --purge also deletes the client's
            .refine/ data so you can re-`init` against a different repo.
- start   — bring up runner (systemd) + webapp (docker compose).
- stop    — stop runner + webapp.
- restart — stop then start (handy for picking up source changes).
- status  — show what's running (read-only).
- runner  — start the runner daemon in-process (used by the systemd unit
            and for interactive debugging; not the daily verb).
- web     — start the webapp (rarely invoked directly; Docker wraps it).
- doctor  — deeper diagnostic snapshot (config, IPC, agent CLI, git).
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path

from refine_shared import config, project_registry


SYSTEMD_USER_DIR = Path.home() / ".config" / "systemd" / "user"
COMPOSE_SERVICE = "refine-web"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="refine", description="Manage refine.")
    parser.add_argument(
        "--config", "-c",
        help="Path to refine.toml (defaults to walking up from cwd).",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_init = sub.add_parser(
        "init",
        help="Initialize refine for a client repo and bind this refine clone to it.",
        description=(
            "Bootstraps a client repo: creates <client>/.refine/refine.toml + "
            "run/ + gaps/, writes a .refine-binding + .env, and registers a "
            "systemd --user unit so the runner survives terminal close."
        ),
    )
    p_init.add_argument(
        "path", nargs="?", default=None,
        help="Path to the client repo. Defaults to cwd (back-compat).",
    )
    p_init.add_argument("--force", action="store_true",
                        help="Overwrite an existing refine.toml / .refine-binding / unit file.")
    p_init.set_defaults(fn=cmd_init)

    p_reset = sub.add_parser(
        "reset",
        help="Undo `refine init` in this clone so it can be pointed at a different repo.",
        description=(
            "Stops the runner + webapp, removes the .refine-binding and .env "
            "files from this clone, disables + removes the systemd --user "
            "unit, and (with --purge) wipes the bound client repo's .refine/ "
            "directory. The client repo itself is never touched."
        ),
    )
    p_reset.add_argument(
        "--purge", action="store_true",
        help="Also delete the bound client repo's .refine/ directory "
             "(gap.json files, sqlite index, run/, .gitignore). DATA LOSS.",
    )
    p_reset.add_argument(
        "-y", "--yes", action="store_true",
        help="Skip the confirmation prompt for --purge.",
    )
    p_reset.set_defaults(fn=cmd_reset)

    p_start = sub.add_parser(
        "start",
        help="Start runner + webapp.",
        description=(
            "Rebuilds the web image if source files are newer than the image, "
            "brings the webapp up via docker compose, starts the runner via "
            "systemd --user, and prints a status block."
        ),
    )
    p_start.add_argument(
        "--rebuild", action="store_true",
        help="Force a `docker compose build` before starting.",
    )
    p_start.add_argument(
        "--no-rebuild", action="store_true",
        help="Skip the rebuild staleness check entirely.",
    )
    p_start.set_defaults(fn=cmd_start)

    p_restart = sub.add_parser(
        "restart",
        help="Stop runner + webapp, then start them again.",
        description=(
            "Equivalent to `refine stop && refine start`. Accepts the same "
            "rebuild flags as `start` for the post-stop bring-up."
        ),
    )
    p_restart.add_argument(
        "--rebuild", action="store_true",
        help="Force a `docker compose build` between stop and start.",
    )
    p_restart.add_argument(
        "--no-rebuild", action="store_true",
        help="Skip the rebuild staleness check on the post-stop start.",
    )
    p_restart.set_defaults(fn=cmd_restart)

    p_stop = sub.add_parser(
        "stop",
        help="Stop runner + webapp.",
    )
    p_stop.set_defaults(fn=cmd_stop)

    p_status = sub.add_parser(
        "status",
        help="Show what's running (read-only).",
    )
    p_status.set_defaults(fn=cmd_status)

    # The systemd unit invokes this directly. Also useful interactively when
    # you want runner logs in the foreground.
    p_runner = sub.add_parser(
        "runner",
        help="Run the runner daemon in the foreground (used by the systemd unit).",
    )
    p_runner.set_defaults(fn=cmd_runner_foreground)

    p_web = sub.add_parser("web", help="Start the webapp (in-process; Docker wraps it).")
    p_web.set_defaults(fn=cmd_web)

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
    env_written = result.get("env_path")
    unit_path = result.get("unit_path")
    print(f"Wrote {cfg_path}")
    print(f"Created directories: {target}/run, {target}/gaps")
    if binding_written:
        print(f"Bound this refine source dir → {client_repo}")
        print(f"Wrote {binding_written}")
        print(f"Wrote {env_written}")
    if unit_path:
        print(f"Installed systemd unit: {unit_path}")
    print()
    print("Next steps:")
    if binding_written:
        print(f"  uv run refine start          # webapp + runner, one command")
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
        print("`refine init` from inside a refine source dir (so docker compose and")
        print("the systemd unit can be wired up).")
    return 0


def _is_refine_source_dir(p: Path) -> bool:
    """Heuristic: cwd is a refine source dir if it has pyproject.toml and a `refine/` package."""
    return (p / "pyproject.toml").is_file() and (p / "refine" / "cli.py").is_file()


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
) -> dict[str, Path | bool]:
    """Create/bind a client repo using the same files as `refine init`.

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
    env_written = None
    unit_path = None
    if _is_refine_source_dir(clone_dir):
        binding_path = clone_dir / config.BINDING_FILENAME
        if binding_path.exists() and not force:
            raise config.ConfigError(
                f"{binding_path} already exists (use --force to rebind)"
            )
        binding_written = config.write_binding(clone_dir, client_repo)
        env_written = clone_dir / ".env"
        env_written.write_text(
            f"# Auto-generated by `refine init`. Read by docker-compose.\n"
            f"REFINE_CLIENT_REFINE_DIR={target}\n",
            encoding="utf-8",
        )
        if install_unit:
            unit_path = _write_and_enable_unit(clone_dir, client_repo, force=force)
        project_registry.upsert_app(clone_dir, client_repo, make_current=True)

    return {
        "client_repo": client_repo,
        "volume_root": target,
        "config_path": cfg_path,
        "binding_path": binding_written,
        "env_path": env_written,
        "unit_path": unit_path,
        "git_initialized": git_initialized,
        "config_created": config_created,
    }


def _write_and_enable_unit(clone_dir: Path, client_repo: Path, *, force: bool) -> Path:
    """Write ~/.config/systemd/user/<unit>.service, daemon-reload, enable.

    Refuses if a unit by the same name already points at a different clone,
    unless --force is given (in which case it's overwritten).
    """
    unit_name = config.unit_name_for(clone_dir)
    unit_path = SYSTEMD_USER_DIR / f"{unit_name}.service"
    SYSTEMD_USER_DIR.mkdir(parents=True, exist_ok=True)

    if unit_path.exists() and not force:
        existing_wd = _grep_first(unit_path.read_text(encoding="utf-8"), "WorkingDirectory=")
        if existing_wd and existing_wd != str(clone_dir):
            raise _InitError(
                f"systemd unit {unit_name} already exists for a different clone:\n"
                f"  existing WorkingDirectory: {existing_wd}\n"
                f"  this clone:                {clone_dir}\n"
                f"Use --force to overwrite, or rename one of the clones."
            )

    uv = shutil.which("uv")
    if uv is None:
        raise _InitError(
            "could not find `uv` on PATH; install it before running `refine init` "
            "(the systemd unit needs an absolute path to invoke it)."
        )

    unit_body = (
        "# Auto-generated by `refine init`. Do not edit by hand — re-run\n"
        "# `refine init --force` to regenerate.\n"
        "[Unit]\n"
        f"Description=refine runner — {clone_dir} → {client_repo}\n"
        "After=network.target\n"
        "\n"
        "[Service]\n"
        "Type=simple\n"
        f"WorkingDirectory={clone_dir}\n"
        f"ExecStart={uv} run refine runner\n"
        "Restart=on-failure\n"
        "RestartSec=2s\n"
        # KillMode=process so the runner's child processes — most
        # importantly, any nohup'd target application the agent
        # backgrounded — survive `systemctl stop` instead of getting
        # SIGTERM'd as cgroup members. The runner only signals its
        # own pid; child processes are deliberately orphaned.
        "KillMode=process\n"
        # Cap systemd's own stop window so it doesn't sit there for the
        # default 90s if the runner hangs. The runner's shutdown path
        # is non-blocking by design (all worker threads are daemons),
        # so anything past a few seconds is a real bug worth surfacing.
        "TimeoutStopSec=30s\n"
        "\n"
        "[Install]\n"
        "WantedBy=default.target\n"
    )
    unit_path.write_text(unit_body, encoding="utf-8")

    rc, out = _systemctl("daemon-reload")
    if rc != 0:
        raise _InitError(f"systemctl --user daemon-reload failed: {out.strip()}")
    rc, out = _systemctl("enable", unit_name)
    if rc != 0:
        raise _InitError(f"systemctl --user enable {unit_name} failed: {out.strip()}")

    return unit_path


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
        from refine_web.__main__ import main as web_main
        return web_main()

    clone, unit = _resolve_clone_and_unit_or_exit()
    _load_config_or_exit(args)
    cfg = config.get()

    if args.rebuild:
        rc = _docker_compose(clone, "build")
        if rc != 0:
            return rc
    elif not args.no_rebuild and _image_is_stale(clone):
        print("Web image is stale (source newer than image) — rebuilding…")
        rc = _docker_compose(clone, "build")
        if rc != 0:
            return rc

    print("Starting webapp (docker compose up -d)…")
    rc = _docker_compose(clone, "up", "-d")
    if rc != 0:
        return rc

    if not _wait_for_port(cfg.web_host, cfg.web_port, timeout=20.0):
        print(
            f"refine start: webapp did not start listening on "
            f"{cfg.web_host}:{cfg.web_port} within 20s. "
            f"Check `docker compose logs {COMPOSE_SERVICE}`.",
            file=sys.stderr,
        )
        return 1

    print(f"Starting runner (systemctl --user start {unit})…")
    rc, out = _systemctl("start", unit)
    if rc != 0:
        print(f"refine start: {out.strip()}", file=sys.stderr)
        return 1

    if not _wait_for_socket(cfg.runner_socket, timeout=10.0):
        print(
            f"refine start: runner did not bind {cfg.runner_socket} within 10s. "
            f"Check `journalctl --user -u {unit} -n 200`.",
            file=sys.stderr,
        )
        return 1

    _print_status_block(clone, unit, cfg)
    return 0


def cmd_stop(args: argparse.Namespace) -> int:
    clone, unit = _resolve_clone_and_unit_or_exit()

    print(f"Stopping runner (systemctl --user stop {unit})…")
    rc, out = _systemctl("stop", unit)
    if rc != 0:
        # Non-fatal: maybe it wasn't running. Continue to compose down.
        print(f"  (systemctl: {out.strip()})", file=sys.stderr)
        if rc == 124:
            # The wrapper's own timeout fired. Most common cause: the
            # installed unit predates `KillMode=process`, so systemd is
            # waiting for child processes (the target-app agent's
            # nohup'd subprocess, for example) before it considers the
            # unit stopped. Point the operator at the fix.
            print(
                "  Hint: the unit may be waiting for nohup'd child processes "
                "to exit. Re-run `refine init --force` to regenerate the "
                "systemd unit with KillMode=process, then try stopping "
                "again. If the runner is wedged in the meantime, force-kill "
                f"it with `systemctl --user kill -s SIGKILL {unit}`.",
                file=sys.stderr,
            )

    print("Stopping webapp (docker compose down)…")
    rc = _docker_compose(clone, "down")
    if rc != 0:
        return rc

    print("Stopped.")
    return 0


def cmd_restart(args: argparse.Namespace) -> int:
    """`refine stop && refine start` — picks up source changes the
    running processes haven't loaded yet (runner Python code, webapp
    image rebuilds) without forcing the operator to run two commands."""
    rc = cmd_stop(args)
    if rc != 0:
        return rc
    print()
    return cmd_start(args)


def cmd_reset(args: argparse.Namespace) -> int:
    """Undo `refine init` in this clone.

    The reverse of init: stop services, disable + remove the systemd unit,
    delete `.refine-binding` and `.env` from the clone dir, and optionally
    purge the client's `.refine/` directory. Leaves the client repo's
    source tree untouched.

    After this, the clone is fresh and can be re-`init`'d against any
    other client repo.
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
            print("This removes ALL gap data, the sqlite index, and the runner socket.")
            try:
                answer = input("Type 'yes' to confirm: ").strip().lower()
            except EOFError:
                answer = ""
            if answer != "yes":
                print("Aborted.")
                return 1

    # 1. Stop services (best-effort — keep going if they were already down).
    print(f"Stopping runner (systemctl --user stop {unit})…")
    rc, out = _systemctl("stop", unit)
    if rc != 0:
        print(f"  (systemctl: {out.strip()})", file=sys.stderr)
    print("Stopping webapp (docker compose down)…")
    _docker_compose(cwd, "down")

    # 2. Disable + remove the systemd unit.
    rc, out = _systemctl("disable", unit)
    if rc != 0:
        # If it wasn't enabled or doesn't exist, that's fine.
        print(f"  (systemctl disable: {out.strip()})", file=sys.stderr)
    unit_path = SYSTEMD_USER_DIR / f"{unit}.service"
    if unit_path.exists():
        unit_path.unlink()
        print(f"Removed {unit_path}")
        _systemctl("daemon-reload")

    # 3. Remove binding + .env + known-app registry from the clone.
    binding_path.unlink()
    print(f"Removed {binding_path}")
    env_path = cwd / ".env"
    if env_path.exists():
        env_path.unlink()
        print(f"Removed {env_path}")
    registry_path = project_registry.registry_path(cwd)
    if registry_path.exists():
        registry_path.unlink()
        print(f"Removed {registry_path}")

    # 4. Optional: purge the client repo's .refine/ directory.
    if args.purge and client_refine_dir and client_refine_dir.is_dir():
        shutil.rmtree(client_refine_dir)
        print(f"Removed {client_refine_dir}")

    print()
    print("Reset complete. To bind this clone to a different repo:")
    print(f"  cd {cwd}")
    print(f"  uv run refine init <path/to/new-client-repo>")
    if client_refine_dir and client_refine_dir.is_dir() and not args.purge:
        print()
        print(f"The previous client's data is preserved at:")
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
    _print_status_block(clone, unit, cfg)
    return 0


def _print_status_block(clone: Path, unit: str, cfg: "config.Config") -> None:
    web_up = _port_open(cfg.web_host, cfg.web_port)
    sock_up = _ipc_ping_quick(cfg.runner_socket)
    runner_active = _systemctl_is_active(unit)

    print()
    print(_bold("refine"))
    print(f"  clone:    {clone}")
    print(f"  client:   {cfg.client_repo}")
    print(f"  web:      {_dot(web_up)} http://{_displayable_host(cfg.web_host)}:{cfg.web_port}")
    print(f"  runner:   {_dot(runner_active and sock_up)} systemd unit `{unit}` "
          f"({'active' if runner_active else 'inactive'}, "
          f"socket {'reachable' if sock_up else 'unreachable'})")
    print(f"  logs:     journalctl --user -u {unit} -f")
    print(f"  stop:     uv run refine stop")
    print()


# ----- runner / web -----------------------------------------------------------

def cmd_runner_foreground(args: argparse.Namespace) -> int:
    """Run the runner daemon in-process. Used by the systemd unit.

    No fork/setsid/pidfile dance — systemd owns the lifecycle. Stdout/stderr
    flow to journald, which is where logs are read from.
    """
    _load_config_or_exit(args)
    from refine_runner.__main__ import main as runner_main
    return runner_main()


def cmd_web(args: argparse.Namespace) -> int:
    from refine_web.__main__ import main as web_main
    return web_main()


# ----- doctor -----------------------------------------------------------------

def cmd_doctor(args: argparse.Namespace) -> int:
    cfg_path = args.config
    try:
        cfg = config.get(path=cfg_path) if cfg_path else config.get()
    except config.ConfigError as e:
        print(_red("No refine configuration found."))
        print(f"  {e}")
        print()
        print("Run `refine init` in the client repo to create one.")
        return 1

    print(_section("Configuration"))
    _kv("config file",   cfg.config_path)
    _kv("volume root",   cfg.volume_root)
    _kv("client repo",   cfg.client_repo)
    _kv("runner socket", cfg.runner_socket)
    _kv("web host:port", f"{cfg.web_host}:{cfg.web_port}")

    print(_section("Volume root"))
    sqlite_present = cfg.sqlite_path.is_file()
    _kv("index.sqlite",  f"{cfg.sqlite_path} ({'present' if sqlite_present else 'missing'})")
    gap_count = _count_gap_files(cfg.gaps_dir)
    _kv("gaps/ files",   f"{gap_count} gap.json file(s)")
    _kv("runner socket dir", cfg.runner_socket.parent)

    print(_section("Host runner"))
    reachable, ping_msg = _ipc_ping(cfg.runner_socket)
    _kv("socket reachable", _bool(reachable))
    if not reachable:
        _kv("error", ping_msg or "")
    else:
        _kv("ping ok", "pong")

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
    return 0 if ok_all else 1


# ----- helpers ----------------------------------------------------------------

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
            "refine: no .refine-binding in scope. Run `refine init <client-repo>` "
            "from a refine source dir first.",
            file=sys.stderr,
        )
        sys.exit(1)
    clone = binding.parent.resolve()
    unit = config.read_binding_unit(binding) or config.unit_name_for(clone)
    return clone, unit


def _docker_compose(clone: Path, *args: str) -> int:
    cmd = ["docker", "compose", *args]
    try:
        return subprocess.run(cmd, cwd=str(clone)).returncode
    except FileNotFoundError:
        print("refine: `docker` not found on PATH.", file=sys.stderr)
        return 1


def _image_is_stale(clone: Path) -> bool:
    """True if any tracked source file is newer than the web image.

    If the image doesn't exist yet, treat as stale (we need to build).
    """
    image = _compose_image_id(clone)
    if image is None:
        return True
    created = _image_created_epoch(image)
    if created is None:
        return True
    watched = [
        clone / "Dockerfile",
        clone / "pyproject.toml",
        clone / "refine",
        clone / "refine_shared",
        clone / "refine_runner",
        clone / "refine_web",
    ]
    for p in watched:
        if _newest_mtime(p) > created:
            return True
    return False


def _compose_image_id(clone: Path) -> str | None:
    try:
        out = subprocess.run(
            ["docker", "compose", "images", "-q", COMPOSE_SERVICE],
            cwd=str(clone),
            capture_output=True, text=True, timeout=10,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    line = out.stdout.strip().splitlines()
    return line[0].strip() if line else None


def _image_created_epoch(image: str) -> float | None:
    try:
        out = subprocess.run(
            ["docker", "image", "inspect", image, "--format", "{{.Created}}"],
            capture_output=True, text=True, timeout=10,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    if out.returncode != 0:
        return None
    raw = out.stdout.strip()
    from datetime import datetime, timezone
    stem = raw.split(".")[0].rstrip("Z")
    try:
        dt = datetime.strptime(stem, "%Y-%m-%dT%H:%M:%S").replace(tzinfo=timezone.utc)
    except ValueError:
        return None
    return dt.timestamp()


def _newest_mtime(p: Path) -> float:
    try:
        if p.is_file():
            return p.stat().st_mtime
        if p.is_dir():
            newest = 0.0
            for child in p.rglob("*"):
                if child.is_file():
                    try:
                        m = child.stat().st_mtime
                    except OSError:
                        continue
                    if m > newest:
                        newest = m
            return newest
    except OSError:
        pass
    return 0.0


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
        with socket.create_connection((target, port), timeout=1.0):
            return True
    except OSError:
        return False


def _wait_for_socket(path: Path, *, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if path.exists() and _ipc_ping_quick(path):
            return True
        time.sleep(0.2)
    return False


def _ipc_ping_quick(socket_path: Path) -> bool:
    ok, _ = _ipc_ping(socket_path)
    return ok


def _ipc_ping(socket_path: Path) -> tuple[bool, str | None]:
    if not socket_path.exists():
        return False, f"socket file not found: {socket_path}"
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(3.0)
            s.connect(str(socket_path))
            envelope = {"id": "doctor", "method": "ping", "params": {}}
            s.sendall((json.dumps(envelope) + "\n").encode("utf-8"))
            buf = b""
            while b"\n" not in buf:
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
        line, _, _ = buf.partition(b"\n")
        resp = json.loads(line.decode("utf-8"))
        if resp.get("ok") and resp.get("result", {}).get("pong"):
            return True, None
        return False, f"unexpected response: {resp}"
    except (ConnectionRefusedError, PermissionError, FileNotFoundError) as e:
        return False, repr(e)
    except Exception as e:
        return False, repr(e)


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
