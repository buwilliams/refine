"""Local Refine supervisor process and IPC control plane."""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from refine_runtime import identity, ipc
from refine_runtime.manager import ResourceManager
from refine_runtime.resources import ResourceSettings, memory_limit_mb
from refine_runtime.supervisor_protocol import (
    M_ATTACH_APP,
    M_BACKEND_CALL,
    M_DETACH_APP,
    M_ENSURE_WORKER,
    M_PROCESS_LAUNCH,
    M_PROCESS_READ,
    M_PROCESS_SIGNAL,
    M_PROCESS_WAIT,
    M_PROCESS_WRITE,
    M_SHUTDOWN,
    M_STATUS,
    M_STOP_WORKER,
    M_SWITCH_APP,
    M_TARGET_APP_RUN,
    WORKER_STARTUP_TIMEOUT_SECONDS,
)
from refine_server import config, db, node_ops, project_apps, project_state, project_sync
from refine_server.backend_protocol import M_PING, M_RUNNING, M_TARGET_APP_RUN


@dataclass
class ManagedProcess:
    process_id: str
    proc: subprocess.Popen
    kind: str
    args: list[str]
    cwd: str
    resource_backend: str
    resource_isolation: str
    memory_limit_mb: int
    priority: str
    started_at: float = field(default_factory=time.monotonic)
    stdout: str = ""
    stderr: str = ""
    stdout_eof: bool = False
    stderr_eof: bool = False
    condition: threading.Condition = field(default_factory=threading.Condition)


class Supervisor:
    def __init__(
        self,
        *,
        host: str,
        port: int,
        cfg_path: str | None,
        pid_path: Path | str | None = None,
    ) -> None:
        self.host = host
        self.port = port
        self.cfg_path = cfg_path
        self.run_dir = ipc.run_dir(port=port)
        self.socket_path = ipc.supervisor_socket_path(port)
        self.pid_path = Path(pid_path) if pid_path is not None else self.run_dir / "supervisor.pid"
        self.runner_socket = self._runner_socket_path(cfg_path)
        self.resource_settings = _load_resource_settings(cfg_path)
        self.resources = ResourceManager(self.resource_settings)
        self.capabilities = self.resources.capabilities()
        self.ui: subprocess.Popen | None = None
        self.worker: subprocess.Popen | None = None
        self.worker_socket: Path | None = None
        self._stopping = threading.Event()
        self._lock = threading.RLock()
        self._processes: dict[str, ManagedProcess] = {}
        self._server = ipc.IpcServer(self.socket_path, self.dispatch)

    def start(self) -> None:
        self._server.start()
        self._write_pid_file()
        self._start_ui()

    def run(self) -> int:
        try:
            while not self._stopping.is_set():
                with self._lock:
                    ui = self.ui
                if ui is not None and ui.poll() is not None:
                    sys.stderr.write("[refine-supervisor] UI exited; shutting down\n")
                    return ui.returncode or 1
                time.sleep(0.5)
        finally:
            self.shutdown()
        return 0

    def dispatch(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        handlers = {
            M_STATUS: self.status,
            M_SHUTDOWN: self._h_shutdown,
            M_BACKEND_CALL: self._h_backend_call,
            M_ATTACH_APP: self._h_attach_app,
            M_SWITCH_APP: self._h_switch_app,
            M_DETACH_APP: self._h_detach_app,
            M_ENSURE_WORKER: self._h_ensure_worker,
            M_STOP_WORKER: self._h_stop_worker,
            M_TARGET_APP_RUN: self._h_target_app_run,
            M_PROCESS_LAUNCH: self._h_process_launch,
            M_PROCESS_WRITE: self._h_process_write,
            M_PROCESS_READ: self._h_process_read,
            M_PROCESS_SIGNAL: self._h_process_signal,
            M_PROCESS_WAIT: self._h_process_wait,
        }
        handler = handlers.get(method)
        if handler is None:
            raise KeyError(method)
        return handler(params)

    def status(self, _params: dict[str, Any] | None = None) -> dict[str, Any]:
        with self._lock:
            worker_snapshot = self._worker_snapshot_locked()
            processes = [
                {
                    "process_id": p.process_id,
                    "pid": p.proc.pid,
                    "kind": p.kind,
                    "cwd": p.cwd,
                    "running": p.proc.poll() is None,
                    "returncode": p.proc.poll(),
                    "resource_backend": p.resource_backend,
                    "resource_isolation": p.resource_isolation,
                    "memory_limit_mb": p.memory_limit_mb,
                    "priority": p.priority,
                    "elapsed_seconds": int(time.monotonic() - p.started_at),
                }
                for p in self._processes.values()
            ]
            ui_pid = self.ui.pid if self.ui is not None and self.ui.poll() is None else None
            worker_pid = (
                self.worker.pid
                if self.worker is not None and self.worker.poll() is None
                else None
            )
            cfg_path = self.cfg_path
            worker_socket = str(self.worker_socket) if self.worker_socket else ""
        app: dict[str, Any] = {}
        if cfg_path:
            try:
                cfg = config.Config.load(cfg_path)
                app = {
                    "config_path": str(cfg.config_path),
                    "client_repo": str(cfg.client_repo),
                    "volume_root": str(cfg.volume_root),
                }
            except Exception:
                app = {"config_path": cfg_path}
        return {
            "supervisor_pid": os.getpid(),
            "port": self.port,
            "run_dir": str(self.run_dir),
            "supervisor_socket": str(self.socket_path),
            "active_config_path": cfg_path or "",
            "active_app": app,
            "ui": {"pid": ui_pid},
            "worker": {"pid": worker_pid, "socket_path": worker_socket},
            "managed_processes": processes,
            "resource_backend": {
                "name": self.capabilities.name,
                "isolation": self.capabilities.isolation,
                "enforced": self.capabilities.enforced,
                "details": self.capabilities.details,
            },
            "worker_snapshot": worker_snapshot,
        }

    def shutdown(self) -> None:
        self._stopping.set()
        with self._lock:
            processes = list(self._processes.values())
            worker = self.worker
            ui = self.ui
        for managed in processes:
            self._terminate(managed.proc)
        self._terminate(worker)
        self._terminate(ui)
        deadline = time.time() + 8
        while time.time() < deadline:
            live = [
                proc for proc in [*(p.proc for p in processes), worker, ui]
                if proc is not None and proc.poll() is None
            ]
            if not live:
                break
            time.sleep(0.1)
        for managed in processes:
            self._kill(managed.proc)
        self._kill(worker)
        self._kill(ui)
        self._server.stop()
        self._unlink_pid_file()

    def _write_pid_file(self) -> None:
        self.pid_path.parent.mkdir(parents=True, exist_ok=True)
        self.pid_path.write_text(f"{os.getpid()}\n", encoding="utf-8")

    def _unlink_pid_file(self) -> None:
        try:
            raw = self.pid_path.read_text(encoding="utf-8").strip()
        except OSError:
            return
        if raw == str(os.getpid()):
            try:
                self.pid_path.unlink()
            except FileNotFoundError:
                pass

    def _start_ui(self) -> None:
        env = os.environ.copy()
        if self.cfg_path:
            env[config.ENV_CONFIG_PATH] = self.cfg_path
        env["REFINE_UI_HOST"] = self.host
        env["REFINE_UI_PORT"] = str(self.port)
        env["REFINE_UI_SCOPE"] = str(self.port)
        env[config.ENV_RUN_DIR] = str(self.run_dir)
        env["REFINE_SUPERVISOR_PID"] = str(os.getpid())
        env["REFINE_SUPERVISOR_SOCKET"] = str(self.socket_path)
        env.pop("REFINE_RUNNER_SOCKET", None)
        env["REFINE_NO_INPROCESS_RUNNER"] = "1"
        env.setdefault("PYTHONUNBUFFERED", "1")
        self.ui = self.resources.popen(
            [sys.executable, "-m", "refine_cli", "ui"],
            cwd=Path.cwd(),
            env=env,
            kind="ui",
            stdin=subprocess.DEVNULL,
            stdout=None,
            stderr=None,
        )

    def _h_shutdown(self, _params: dict[str, Any]) -> dict[str, Any]:
        threading.Thread(target=self.shutdown, name="refine-supervisor-shutdown", daemon=True).start()
        return {"shutting_down": True, "supervisor_pid": os.getpid()}

    def _h_backend_call(self, params: dict[str, Any]) -> dict[str, Any]:
        method = str(params.get("method") or "").strip()
        if not method:
            raise ValueError("method is required")
        call_params = params.get("params") if isinstance(params.get("params"), dict) else {}
        timeout = float(params.get("timeout") or 30.0)
        if method == M_TARGET_APP_RUN:
            return self._h_target_app_run(call_params)
        self._h_ensure_worker({
            "config_path": str(params.get("config_path") or self.cfg_path or ""),
        })
        with self._lock:
            worker_socket = self.worker_socket
        if worker_socket is None:
            raise config.ConfigError("Backend runner is not available.")
        try:
            return ipc.request(worker_socket, method, call_params, timeout=timeout)
        except (FileNotFoundError, ConnectionRefusedError, RuntimeError):
            with self._lock:
                self._stop_worker_locked()
                self.worker_socket = None
            self._h_ensure_worker({
                "config_path": str(params.get("config_path") or self.cfg_path or ""),
            })
            with self._lock:
                worker_socket = self.worker_socket
            if worker_socket is None:
                raise config.ConfigError("Backend runner is not available.")
            return ipc.request(worker_socket, method, call_params, timeout=timeout)

    def _h_attach_app(self, params: dict[str, Any]) -> dict[str, Any]:
        body = params.get("body") if isinstance(params.get("body"), dict) else {}
        clone_dir = Path(str(params.get("clone_dir") or Path.cwd())).resolve()
        code, payload = project_apps.attach_project(
            body,
            clone_dir=clone_dir,
            port=self.port,
            load_configured=self._load_project_attach_configured,
            current_client_repo=self._current_client_repo,
            loaded_client_repo=self._loaded_client_repo,
            prepare_current_project_for_switch=self._prepare_current_project_for_switch,
            commit_refine_state=self._commit_refine_state,
            node_summary=self._node_summary,
        )
        return {"http_status": code, "body": payload}

    def _h_switch_app(self, params: dict[str, Any]) -> dict[str, Any]:
        cfg_path = str(params.get("config_path") or "").strip()
        if not cfg_path:
            raise ValueError("config_path is required")
        config.Config.load(cfg_path)
        with self._lock:
            changed = self.cfg_path != cfg_path
            if changed:
                self._stop_worker_locked()
                self.cfg_path = cfg_path
                self.runner_socket = self._runner_socket_path(cfg_path)
                self.resource_settings = _load_resource_settings(cfg_path)
                self.resources = ResourceManager(self.resource_settings)
                self.capabilities = self.resources.capabilities()
        result = {"switched": changed, **self._h_ensure_worker({"config_path": cfg_path})}
        return result

    def _h_detach_app(self, _params: dict[str, Any]) -> dict[str, Any]:
        with self._lock:
            self._stop_worker_locked()
            self.cfg_path = None
            self.worker_socket = None
        return {"detached": True}

    def _h_ensure_worker(self, params: dict[str, Any]) -> dict[str, Any]:
        cfg_path = str(params.get("config_path") or self.cfg_path or "").strip()
        if not cfg_path:
            raise RuntimeError("No Refine app is attached")
        cfg = config.Config.load(cfg_path)
        socket_path = self._runner_socket_path(str(cfg.config_path))
        existing_worker: subprocess.Popen | None = None
        with self._lock:
            if (
                self.worker is not None
                and self.worker.poll() is None
                and self.worker_socket == socket_path
            ):
                existing_worker = self.worker
        if existing_worker is not None and self._can_ping_worker(socket_path):
            with self._lock:
                if self.worker is existing_worker:
                    return self._worker_result()

        with self._lock:
            if (
                self.worker is not None
                and self.worker.poll() is None
                and self.worker_socket == socket_path
                and self._can_ping_worker(socket_path)
            ):
                return self._worker_result()
            self._stop_worker_locked()
            env = os.environ.copy()
            env[config.ENV_CONFIG_PATH] = str(cfg.config_path)
            env["REFINE_UI_PORT"] = str(self.port)
            env["REFINE_UI_SCOPE"] = str(self.port)
            env[config.ENV_RUN_DIR] = str(self.run_dir)
            env["REFINE_SUPERVISOR_SOCKET"] = str(self.socket_path)
            env["REFINE_RUNNER_SOCKET"] = str(socket_path)
            env["REFINE_NO_INPROCESS_RUNNER"] = "1"
            env["REFINE_PARENT_PID"] = str(os.getpid())
            env[config.ENV_LOCAL_NODE_ID] = project_state.local_node_id(
                root=cfg.volume_root,
            )
            env.setdefault("PYTHONUNBUFFERED", "1")
            self.cfg_path = str(cfg.config_path)
            self.runner_socket = socket_path
            self.worker_socket = socket_path
            self.worker = self.resources.popen(
                [sys.executable, "-m", "refine_runtime.worker"],
                cwd=Path.cwd(),
                env=env,
                kind="worker",
                stdin=subprocess.DEVNULL,
                stdout=None,
                stderr=None,
            )
            worker = self.worker

        try:
            self._wait_for_worker_socket(socket_path, worker)
        except Exception:
            with self._lock:
                if self.worker is worker:
                    self._stop_worker_locked()
                    self.worker_socket = None
            raise

        with self._lock:
            if self.worker is not worker:
                raise config.ConfigError("Backend runner was replaced before opening its socket.")
            return self._worker_result()

    def _h_stop_worker(self, _params: dict[str, Any]) -> dict[str, Any]:
        with self._lock:
            stopped = self._stop_worker_locked()
        return {"stopped": stopped}

    def _load_project_attach_configured(
        self,
        config_path: Path,
        _start_poller: bool,
        start_runner: bool,
        migrate: bool,
        port: int,
    ) -> config.Config:
        cfg = config.get(path=str(config_path), reload=True, port=port)
        db.init_db(cfg.sqlite_path)
        conn = db.connect(cfg.sqlite_path)
        try:
            status = project_state.ensure_initialized(
                conn,
                migrate=migrate,
                root=cfg.volume_root,
            )
            if not status.get("compatible"):
                raise config.ConfigError(project_state.migration_block_details(status))
            project_state.rebuild_sqlite_cache(conn)
        finally:
            conn.close()
        project_state.resume_agents_for_startup()
        with self._lock:
            changed = self.cfg_path != str(cfg.config_path)
            if changed:
                self._stop_worker_locked()
            self.cfg_path = str(cfg.config_path)
            self.runner_socket = self._runner_socket_path(str(cfg.config_path))
            self.resource_settings = _load_resource_settings(str(cfg.config_path))
            self.resources = ResourceManager(self.resource_settings)
            self.capabilities = self.resources.capabilities()
        if start_runner:
            self._h_ensure_worker({"config_path": str(cfg.config_path)})
        return cfg

    def _current_client_repo(self) -> Path | None:
        try:
            return config.get(reload=True, port=self.port).client_repo
        except config.ConfigError:
            return None

    def _loaded_client_repo(self) -> Path | None:
        if not self.cfg_path:
            return None
        try:
            return config.Config.load(self.cfg_path).client_repo
        except (OSError, config.ConfigError):
            return None

    def _prepare_current_project_for_switch(self, current_repo: Path | None) -> dict[str, Any]:
        warnings: list[str] = []
        if current_repo is None:
            return {"warnings": warnings}
        with self._lock:
            self._stop_worker_locked()
            self.worker_socket = None
        self._commit_refine_state(current_repo)
        dirty = self._git_stdout(current_repo, ["status", "--porcelain"])
        if dirty.strip():
            raise project_apps.SwitchBlocked(
                "Current app has uncommitted changes.",
                (
                    "Commit, stash, or discard changes in the current app before switching:\n"
                    + dirty.strip()
                ),
            )
        return {"warnings": warnings}

    def _commit_refine_state(self, repo: Path) -> None:
        config.ensure_refine_gitignore(repo / ".refine")
        dirty_refine = self._git_stdout(repo, ["status", "--porcelain", "--", ".refine"])
        if not dirty_refine.strip():
            return
        repo_cfg = config.Config.load(repo / ".refine" / config.CONFIG_FILENAME)
        db.init_db(repo_cfg.sqlite_path)
        conn = db.connect(repo_cfg.sqlite_path)
        try:
            result = project_sync.commit_and_push_refine_state(
                conn,
                actor="refine",
                cwd=repo,
                state_message="refine: sync project state before switch",
                rebuild_cache=True,
            )
        finally:
            conn.close()
        if result.get("ok"):
            return
        raise project_apps.SwitchBlocked(
            "Could not commit current app Refine state.",
            str(result.get("details") or result.get("message") or "git commit failed").strip(),
        )

    def _git_stdout(self, repo: Path, args: list[str]) -> str:
        out = subprocess.run(
            ["git", *args],
            cwd=str(repo),
            capture_output=True,
            text=True,
            timeout=30,
        )
        if out.returncode != 0:
            raise project_apps.SwitchBlocked(
                "Could not inspect current app git state.",
                (out.stderr or out.stdout or f"git {' '.join(args)} failed").strip(),
            )
        return out.stdout

    def _node_summary(self) -> dict[str, Any]:
        try:
            return node_ops.summary()
        except Exception:
            return {"nodes": [], "active_node_id": ""}

    def _h_target_app_run(self, params: dict[str, Any]) -> dict[str, Any]:
        from refine_server import target_app

        old = os.environ.get("REFINE_IN_SUPERVISOR")
        os.environ["REFINE_IN_SUPERVISOR"] = "1"
        try:
            kind = str(params.get("kind") or "")
            cfg = params.get("config") if isinstance(params.get("config"), dict) else {}
            return target_app.run_operation(kind, cfg)
        finally:
            if old is None:
                os.environ.pop("REFINE_IN_SUPERVISOR", None)
            else:
                os.environ["REFINE_IN_SUPERVISOR"] = old

    def _h_process_launch(self, params: dict[str, Any]) -> dict[str, Any]:
        args = params.get("args")
        if not isinstance(args, list) or not all(isinstance(i, str) for i in args):
            raise ValueError("args must be a list of strings")
        cwd = Path(str(params.get("cwd") or Path.cwd()))
        env_raw = params.get("env") if isinstance(params.get("env"), dict) else {}
        env = {str(k): str(v) for k, v in env_raw.items()}
        env.setdefault("REFINE_UI_PORT", str(self.port))
        env.setdefault("REFINE_UI_SCOPE", str(self.port))
        env.setdefault(config.ENV_RUN_DIR, str(self.run_dir))
        kind = str(params.get("kind") or "process")
        stdin = subprocess.PIPE if params.get("stdin") == "pipe" else subprocess.DEVNULL
        stdout = subprocess.PIPE if params.get("stdout") != "inherit" else None
        stderr = subprocess.STDOUT if params.get("stderr") == "stdout" else subprocess.PIPE
        capabilities = self.resources.capabilities()
        proc = self.resources.popen(
            args,
            cwd=cwd,
            env=env,
            kind=kind,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=bool(params.get("text", True)),
            bufsize=int(params.get("bufsize") or 1),
        )
        process_id = uuid.uuid4().hex
        managed = ManagedProcess(
            process_id=process_id,
            proc=proc,
            kind=kind,
            args=list(args),
            cwd=str(cwd),
            resource_backend=capabilities.name,
            resource_isolation=capabilities.isolation,
            memory_limit_mb=memory_limit_mb(self.resource_settings, kind),
            priority=self.resource_settings.worker_cpu_priority,
        )
        with self._lock:
            self._processes[process_id] = managed
        if proc.stdout is not None:
            threading.Thread(
                target=self._capture_stream,
                args=(managed, "stdout", proc.stdout),
                name=f"refine-supervisor-out-{process_id[:8]}",
                daemon=True,
            ).start()
        if proc.stderr is not None and proc.stderr is not proc.stdout:
            threading.Thread(
                target=self._capture_stream,
                args=(managed, "stderr", proc.stderr),
                name=f"refine-supervisor-err-{process_id[:8]}",
                daemon=True,
            ).start()
        return {
            "process_id": process_id,
            "pid": proc.pid,
            "resource_backend": managed.resource_backend,
            "resource_isolation": managed.resource_isolation,
            "memory_limit_mb": managed.memory_limit_mb,
            "priority": managed.priority,
        }

    def _h_process_write(self, params: dict[str, Any]) -> dict[str, Any]:
        managed = self._managed(params)
        data = str(params.get("data") or "")
        if managed.proc.stdin is None:
            return {"written": 0, "stdin": False}
        managed.proc.stdin.write(data)
        managed.proc.stdin.flush()
        return {"written": len(data), "stdin": True}

    def _h_process_read(self, params: dict[str, Any]) -> dict[str, Any]:
        managed = self._managed(params)
        stream = "stderr" if params.get("stream") == "stderr" else "stdout"
        cursor = max(0, int(params.get("cursor") or 0))
        timeout = max(0.0, float(params.get("timeout") or 0))
        deadline = time.monotonic() + timeout
        with managed.condition:
            while True:
                data = managed.stderr if stream == "stderr" else managed.stdout
                eof = managed.stderr_eof if stream == "stderr" else managed.stdout_eof
                if len(data) > cursor or eof or managed.proc.poll() is not None:
                    break
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                managed.condition.wait(timeout=remaining)
            data = managed.stderr if stream == "stderr" else managed.stdout
            chunk = data[cursor:]
            new_cursor = cursor + len(chunk)
            eof = managed.stderr_eof if stream == "stderr" else managed.stdout_eof
        return {
            "data": chunk,
            "cursor": new_cursor,
            "eof": bool(eof and new_cursor >= len(data)),
            "returncode": managed.proc.poll(),
        }

    def _h_process_signal(self, params: dict[str, Any]) -> dict[str, Any]:
        managed = self._managed(params)
        sig = int(params.get("signal") or signal.SIGTERM)
        self._signal(managed.proc, sig)
        return {"signaled": True, "signal": sig}

    def _h_process_wait(self, params: dict[str, Any]) -> dict[str, Any]:
        managed = self._managed(params)
        timeout_raw = params.get("timeout")
        timeout = None if timeout_raw is None else max(0.0, float(timeout_raw))
        try:
            returncode = managed.proc.wait(timeout=timeout)
            exited = True
        except subprocess.TimeoutExpired:
            returncode = managed.proc.poll()
            exited = False
        with managed.condition:
            managed.condition.notify_all()
        return {"exited": exited, "returncode": returncode}

    def _capture_stream(self, managed: ManagedProcess, stream: str, pipe) -> None:  # noqa: ANN001
        try:
            while True:
                chunk = pipe.read(4096)
                if not chunk:
                    break
                with managed.condition:
                    if stream == "stderr":
                        managed.stderr += chunk
                    else:
                        managed.stdout += chunk
                    managed.condition.notify_all()
        finally:
            with managed.condition:
                if stream == "stderr":
                    managed.stderr_eof = True
                else:
                    managed.stdout_eof = True
                managed.condition.notify_all()

    def _managed(self, params: dict[str, Any]) -> ManagedProcess:
        process_id = str(params.get("process_id") or "")
        with self._lock:
            managed = self._processes.get(process_id)
        if managed is None:
            raise KeyError(f"unknown process_id: {process_id}")
        return managed

    def _worker_result(self) -> dict[str, Any]:
        return {
            "started": True,
            "worker_pid": self.worker.pid if self.worker is not None else None,
            "worker_socket": str(self.worker_socket or ""),
            "socket_path": str(self.worker_socket or ""),
            "resource_backend": self.capabilities.name,
            "resource_isolation": self.capabilities.isolation,
        }

    def _worker_snapshot_locked(self) -> dict[str, Any]:
        socket_path = self.worker_socket
        if socket_path is None or self.worker is None or self.worker.poll() is not None:
            return {"runner_reachable": False}
        try:
            return {"runner_reachable": True, **ipc.request(socket_path, M_RUNNING, {}, timeout=2.0)}
        except Exception as e:
            return {"runner_reachable": False, "error": str(e)}

    def _can_ping_worker(self, socket_path: Path) -> bool:
        try:
            ping = ipc.request(socket_path, M_PING, {}, timeout=1.0)
        except Exception:
            return False
        return (
            ping.get("source_fingerprint") == identity.SOURCE_FINGERPRINT
            and ping.get("refine_version") == identity.REFINE_VERSION
        )

    def _wait_for_worker_socket(self, path: Path, proc: subprocess.Popen) -> None:
        deadline = time.time() + WORKER_STARTUP_TIMEOUT_SECONDS
        while time.time() < deadline:
            if path.exists() and self._can_ping_worker(path):
                return
            if proc.poll() is not None:
                raise config.ConfigError("Backend runner exited before opening its socket.")
            time.sleep(0.1)
        raise config.ConfigError(f"Backend runner socket did not appear: {path}")

    def _stop_worker_locked(self) -> bool:
        worker = self.worker
        self.worker = None
        if worker is None:
            return False
        self._terminate(worker)
        try:
            worker.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            self._kill(worker)
        return True

    def _runner_socket_path(self, cfg_path: str | None) -> Path:
        return ipc.runner_socket_path(port=self.port, config_path=cfg_path)

    def _terminate(self, proc: subprocess.Popen | None) -> None:
        if proc is None or proc.poll() is not None:
            return
        self._signal(proc, signal.SIGTERM)

    def _kill(self, proc: subprocess.Popen | None) -> None:
        if proc is None or proc.poll() is not None:
            return
        self._signal(proc, signal.SIGKILL)

    def _signal(self, proc: subprocess.Popen, sig: int | signal.Signals) -> None:
        try:
            os.killpg(os.getpgid(proc.pid), int(sig))
        except OSError:
            try:
                proc.send_signal(int(sig))
            except OSError:
                pass


def main() -> int:
    config.load_dotenv()
    host = os.environ.get("REFINE_UI_HOST", "0.0.0.0")
    port = int(os.environ.get("REFINE_UI_PORT", "8080"))
    supervisor = Supervisor(host=host, port=port, cfg_path=_supervisor_config_path())

    def _on_signal(signum, _frame):  # noqa: ANN001
        sys.stderr.write(f"\n[refine-supervisor] caught signal {signum}, shutting down\n")
        supervisor.shutdown()

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

    supervisor.start()
    sys.stderr.write(f"[refine-supervisor] listening on {supervisor.socket_path}\n")
    return supervisor.run()


def _load_resource_settings(cfg_path: str | None) -> ResourceSettings:
    if not cfg_path:
        return ResourceSettings()
    try:
        config.get(path=cfg_path, reload=True)
        conn = db.connect()
        try:
            settings = db.list_settings(conn)
        finally:
            conn.close()
        return ResourceSettings.from_settings(settings)
    except Exception as e:
        sys.stderr.write(
            f"[refine-supervisor] using default resource settings: {e}\n"
        )
        return ResourceSettings()


def _supervisor_config_path() -> str | None:
    raw = os.environ.get(config.ENV_CONFIG_PATH)
    try:
        if raw:
            return str(config.Config.load(raw).config_path)
        found = config.find_config()
        if found is None:
            return None
        return str(config.Config.load(found).config_path)
    except config.ConfigError as e:
        sys.stderr.write(f"[refine-supervisor] no usable config: {e}\n")
        return None


if __name__ == "__main__":
    raise SystemExit(main())
