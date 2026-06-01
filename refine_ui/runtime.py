"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

from refine_server import config, db
from refine_server.backend_protocol import M_PING, M_RUNNING
from refine_runtime import identity

from .poller import SqlitePoller

_poller: SqlitePoller | None = None
_runner = None
_runner_proc: subprocess.Popen | None = None
_runner_lock = threading.Lock()
_loaded_config_path: Path | None = None


class _SocketRunnerClient:
    def __init__(self, socket_path: str) -> None:
        self.socket_path = socket_path

    def call(
        self,
        method: str,
        params: dict | None = None,
        *,
        timeout: float = 30.0,
    ) -> dict:
        from refine_runtime import ipc

        return ipc.request(self.socket_path, method, params or {}, timeout=timeout)

    def status_snapshot(self) -> dict:
        return self.call(M_RUNNING, {}, timeout=5.0)

    def shutdown(self) -> None:
        return None


def load_configured(
    path: Path | str | None = None,
    *,
    start_poller: bool = True,
    start_runner: bool = True,
    migrate: bool = False,
) -> config.Config:
    """Load config, initialize SQLite, and ensure background services run."""
    global _loaded_config_path
    requested_path = Path(path).resolve() if path is not None else config.find_config()
    cfg_preview = config.Config.load(path)
    from refine_server import project_state

    schema = project_state.schema_status(cfg_preview.volume_root)
    if (
        not schema.get("compatible")
        and schema.get("migration_required")
        and (
            project_state.migration_requires_manual(schema)
            or not project_state.empty_refine_state(cfg_preview.volume_root)
        )
    ):
        if not migrate or project_state.migration_requires_manual(schema):
            raise config.ConfigError(
                f"{project_state.migration_block_message(schema)} "
                f"{project_state.migration_block_details(schema)}"
            )
    if not schema.get("compatible") and not schema.get("migration_required"):
        raise config.ConfigError(
            "Project schema is not supported by this Refine version."
        )
    if schema.get("migration_required") and migrate:
        project_state.ensure_initialized(migrate=True, root=cfg_preview.volume_root)
    if (_loaded_config_path is not None and requested_path is not None
            and requested_path.resolve() != _loaded_config_path):
        stop_all()
    cfg = config.get(path=path, reload=True)
    os.environ[config.ENV_CONFIG_PATH] = str(cfg.config_path)
    os.environ["REFINE_RUNNER_SOCKET"] = str(_runner_socket_path(cfg))
    os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
    _loaded_config_path = cfg.config_path
    db.init_db()
    conn = db.connect(cfg.sqlite_path)
    try:
        project_state.ensure_initialized(conn, migrate=migrate, root=cfg.volume_root)
    finally:
        conn.close()
    project_state.resume_agents_for_startup()
    if start_poller:
        ensure_poller()
    if start_runner:
        ensure_runner()
    return cfg


def ensure_poller() -> None:
    global _poller
    if _poller is not None:
        return
    _poller = SqlitePoller(interval=1.0)
    _poller.start()


def stop_poller() -> None:
    global _poller
    if _poller is None:
        return
    _poller.stop()
    _poller = None


def ensure_runner():
    global _runner, _runner_proc
    with _runner_lock:
        if _runner is not None:
            return _runner

        cfg = config.get(reload=True)
        socket_path = os.environ.get("REFINE_RUNNER_SOCKET") or str(_runner_socket_path(cfg))
        os.environ["REFINE_RUNNER_SOCKET"] = socket_path
        os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
        socket = Path(socket_path)
        if socket.exists():
            if not _can_adopt_runner_socket(socket, _runner_proc):
                _terminate_workers_for_socket(socket)
                _unlink_quietly(socket)
                _runner_proc = _start_external_runner(cfg, socket)
        elif _runner_proc is not None and _runner_proc.poll() is not None:
            _runner_proc = _start_external_runner(cfg, socket)
        else:
            _runner_proc = _start_external_runner(cfg, socket)

        _runner = _SocketRunnerClient(socket_path)
        return _runner


def stop_runner() -> None:
    global _runner, _runner_proc
    if _runner is None:
        proc = _runner_proc
    else:
        _runner.shutdown()
        proc = _runner_proc
    _runner = None
    if proc is None:
        return
    _runner_proc = None
    if proc.poll() is not None:
        return
    try:
        proc.terminate()
    except OSError:
        return
    deadline = time.time() + 5
    while time.time() < deadline:
        if proc.poll() is not None:
            return
        time.sleep(0.1)
    try:
        proc.kill()
    except OSError:
        pass


def runner_call(
    method: str,
    params: dict | None = None,
    *,
    timeout: float = 30.0,
) -> dict:
    runner = ensure_runner()
    if isinstance(runner, _SocketRunnerClient):
        return runner.call(method, params or {}, timeout=timeout)
    return runner.call(method, params or {})


def backend_info() -> dict:
    socket_path = os.environ.get("REFINE_RUNNER_SOCKET") or ""
    return {
        "process_model": "supervisor",
        "transport": "unix_socket",
        "socket_path": socket_path,
        "source_fingerprint": identity.SOURCE_FINGERPRINT,
        "refine_version": identity.REFINE_VERSION,
        "in_process_runner_allowed": False,
        "runner_client_loaded": _runner is not None,
        "ui_controls_runner_lifecycle": True,
    }


def _runner_socket_path(cfg: config.Config) -> Path:
    from refine_runtime import ipc

    try:
        port = int(os.environ.get("REFINE_UI_PORT") or cfg.web_port)
    except ValueError:
        port = cfg.web_port
    return ipc.runner_socket_path(port=port, config_path=cfg.config_path)


def _start_external_runner(cfg: config.Config, socket: Path) -> subprocess.Popen:
    env = os.environ.copy()
    env[config.ENV_CONFIG_PATH] = str(cfg.config_path)
    env["REFINE_RUNNER_SOCKET"] = str(socket)
    env["REFINE_NO_INPROCESS_RUNNER"] = "1"
    env["REFINE_PARENT_PID"] = str(os.getpid())
    env.setdefault("PYTHONUNBUFFERED", "1")
    proc = subprocess.Popen(
        [sys.executable, "-m", "refine_runtime.worker"],
        cwd=str(Path.cwd()),
        stdin=subprocess.DEVNULL,
        stdout=None,
        stderr=None,
        env=env,
    )
    _wait_for_runner_socket(socket, proc)
    return proc


def _wait_for_runner_socket(path: Path, proc: subprocess.Popen) -> None:
    deadline = time.time() + 20
    while time.time() < deadline:
        if path.exists() and _can_adopt_runner_socket(path, proc):
            return
        if proc.poll() is not None:
            raise config.ConfigError("Backend runner exited before opening its socket.")
        time.sleep(0.1)
    raise config.ConfigError(f"Backend runner socket did not appear: {path}")


def _can_adopt_runner_socket(
    path: Path,
    proc: subprocess.Popen | None,
) -> bool:
    try:
        from refine_runtime import ipc

        ping = ipc.request(path, M_PING, {}, timeout=1.0)
    except Exception:
        return False
    if ping.get("source_fingerprint") != identity.SOURCE_FINGERPRINT:
        return False
    if ping.get("refine_version") != identity.REFINE_VERSION:
        return False
    pid = _int_or_none(ping.get("pid"))
    if proc is not None and proc.poll() is None and pid == proc.pid:
        return True
    parent_pid = _int_or_none(ping.get("parent_pid"))
    expected_parent_pid = _int_or_none(ping.get("expected_parent_pid"))
    return (
        parent_pid == os.getpid()
        and expected_parent_pid in (None, os.getpid())
    )


def _int_or_none(value: object) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _unlink_quietly(path: Path) -> None:
    try:
        path.unlink()
    except FileNotFoundError:
        pass


def _terminate_workers_for_socket(socket: Path) -> None:
    target = str(socket)
    pids: list[int] = []
    proc_root = Path("/proc")
    if not proc_root.exists():
        return
    for path in proc_root.iterdir():
        if not path.name.isdigit():
            continue
        pid = int(path.name)
        if pid == os.getpid():
            continue
        try:
            raw = (path / "environ").read_bytes()
        except OSError:
            continue
        env = raw.split(b"\0")
        if f"REFINE_RUNNER_SOCKET={target}".encode("utf-8") in env:
            pids.append(pid)
    for sig in (signal.SIGTERM, signal.SIGKILL):
        remaining: list[int] = []
        for pid in pids:
            try:
                os.kill(pid, sig)
                remaining.append(pid)
            except ProcessLookupError:
                continue
            except OSError:
                remaining.append(pid)
        pids = remaining
        deadline = time.time() + 2
        while pids and time.time() < deadline:
            pids = [pid for pid in pids if _pid_alive(pid)]
            if pids:
                time.sleep(0.05)


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False


def runner_status_snapshot() -> dict:
    """Best-effort live runner state for read-only UI summaries.

    Unlike `runner_call()`, this does not start the runner and does not route
    through the backend dispatcher/cache check. The dashboard can still render
    cached SQLite data quickly when the runner is busy or unavailable.
    """
    runner = _runner
    if runner is None:
        return {
            "runner_reachable": False,
            "backend": backend_info(),
            "pid": None,
            "running": [],
            "chat": [],
            "merger": None,
            "governance": None,
            "target_app_rebuild": None,
        }
    try:
        snap = runner.status_snapshot()
    except Exception:
        return {
            "runner_reachable": False,
            "backend": backend_info(),
            "pid": None,
            "running": [],
            "chat": [],
            "merger": None,
            "governance": None,
            "target_app_rebuild": None,
        }
    return {"runner_reachable": True, "backend": backend_info(), **snap}


def stop_all() -> None:
    stop_runner()
    stop_poller()


def detach_configured() -> None:
    """Stop project-scoped services and return this process to setup mode."""
    global _loaded_config_path
    stop_all()
    os.environ.pop(config.ENV_CONFIG_PATH, None)
    config.clear_cache()
    _loaded_config_path = None
