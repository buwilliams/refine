"""Web process runtime state shared by the entry point and project API."""
from __future__ import annotations

import os
import threading
import time
from pathlib import Path

from refine_server import config, db, project_state
from refine_server.backend_protocol import M_RUNNING
from refine_runtime import identity
from refine_runtime.supervisor_protocol import (
    M_DETACH_APP,
    M_ENSURE_WORKER,
    M_STATUS,
    M_STOP_WORKER,
    WORKER_STARTUP_TIMEOUT_SECONDS,
)

from .poller import SqlitePoller

_poller: SqlitePoller | None = None
_runner = None
_runner_lock = threading.Lock()
_loaded_config_path: Path | None = None
_worker_pid: int | None = None


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


class _InProcessRunnerClient:
    def __init__(self) -> None:
        from refine_server.runner import Runner

        self.runner = Runner()

    def call(
        self,
        method: str,
        params: dict | None = None,
        *,
        timeout: float = 30.0,  # noqa: ARG002
    ) -> dict:
        return self.runner.call(method, params or {})

    def status_snapshot(self) -> dict:
        return self.runner.call(M_RUNNING, {})

    def shutdown(self) -> None:
        self.runner.shutdown()


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
    os.environ.setdefault(config.ENV_UI_PORT, str(cfg_preview.web_port))
    os.environ.setdefault(config.ENV_UI_SCOPE, str(cfg_preview.web_port))
    os.environ.setdefault(
        config.ENV_RUN_DIR,
        str(config.local_run_dir(port=cfg_preview.web_port)),
    )
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
    os.environ[config.ENV_LOCAL_NODE_ID] = project_state.local_node_id(
        root=cfg.volume_root,
    )
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
    global _runner, _worker_pid
    with _runner_lock:
        if _runner is not None:
            return _runner

        cfg = config.get(reload=True)
        supervisor_socket = _supervisor_socket_path(cfg)
        try:
            result = _supervisor_request(
                M_ENSURE_WORKER,
                {"config_path": str(cfg.config_path)},
                timeout=WORKER_STARTUP_TIMEOUT_SECONDS + 15.0,
            )
        except config.ConfigError:
            if os.environ.get("REFINE_TEST_INPROCESS_BACKEND") != "1":
                raise
            _runner = _InProcessRunnerClient()
            _worker_pid = os.getpid()
            return _runner
        socket_path = str(
            result.get("worker_socket")
            or result.get("socket_path")
            or os.environ.get("REFINE_RUNNER_SOCKET")
            or _runner_socket_path(cfg)
        )
        os.environ["REFINE_RUNNER_SOCKET"] = socket_path
        os.environ["REFINE_SUPERVISOR_SOCKET"] = str(supervisor_socket)
        os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
        _worker_pid = _int_or_none(result.get("worker_pid"))
        _runner = _SocketRunnerClient(socket_path)
        return _runner


def stop_runner() -> None:
    global _runner, _worker_pid
    runner = _runner
    _runner = None
    if runner is not None:
        try:
            runner.shutdown()
        except Exception:
            pass
    try:
        _supervisor_request(M_STOP_WORKER, {}, timeout=10.0)
    except Exception:
        pass
    _worker_pid = None


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
    try:
        cfg = config.get(reload=False)
    except config.ConfigError:
        cfg = None
    supervisor_socket = str(_supervisor_socket_path(cfg))
    worker_pid = _worker_pid
    try:
        status = _supervisor_request(M_STATUS, {}, timeout=2.0)
        worker = status.get("worker") if isinstance(status.get("worker"), dict) else {}
        worker_pid = _int_or_none(worker.get("pid")) or worker_pid
        socket_path = str(worker.get("socket_path") or socket_path)
    except Exception:
        pass
    return {
        "process_model": "supervisor",
        "transport": "unix_socket",
        "socket_path": socket_path,
        "worker_socket_path": socket_path,
        "supervisor_socket_path": supervisor_socket,
        "worker_pid": worker_pid,
        "source_fingerprint": identity.SOURCE_FINGERPRINT,
        "refine_version": identity.REFINE_VERSION,
        "local_node_id": _local_node_id_or_empty(),
        "in_process_runner_allowed": False,
        "runner_client_loaded": _runner is not None,
        "ui_controls_runner_lifecycle": False,
    }


def _runner_socket_path(cfg: config.Config) -> Path:
    from refine_runtime import ipc

    try:
        port = int(os.environ.get("REFINE_UI_PORT") or cfg.web_port)
    except ValueError:
        port = cfg.web_port
    return ipc.runner_socket_path(port=port, config_path=cfg.config_path)


def _supervisor_socket_path(cfg: config.Config | None) -> Path:
    from refine_runtime import ipc

    try:
        port = int(os.environ.get("REFINE_UI_PORT") or (cfg.web_port if cfg else 8080))
    except ValueError:
        port = cfg.web_port if cfg else 8080
    raw = os.environ.get("REFINE_SUPERVISOR_SOCKET")
    return Path(raw) if raw else ipc.supervisor_socket_path(port)


def _supervisor_request(
    method: str,
    params: dict | None = None,
    *,
    timeout: float = 30.0,
) -> dict:
    from refine_runtime import ipc

    cfg = config.get(reload=True)
    socket_path = _supervisor_socket_path(cfg)
    os.environ["REFINE_SUPERVISOR_SOCKET"] = str(socket_path)
    try:
        return ipc.request(socket_path, method, params or {}, timeout=timeout)
    except Exception as e:
        raise config.ConfigError(
            f"Refine supervisor is not reachable at {socket_path}: {e}"
        ) from e


def _int_or_none(value: object) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _local_node_id_or_empty() -> str:
    try:
        return project_state.local_node_id()
    except Exception:
        return ""


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
    """Stop project-scoped services and return this process to no-app state."""
    global _loaded_config_path
    try:
        _supervisor_request(M_DETACH_APP, {}, timeout=10.0)
    except Exception:
        stop_all()
    else:
        stop_poller()
    os.environ.pop(config.ENV_CONFIG_PATH, None)
    config.clear_cache()
    _loaded_config_path = None
