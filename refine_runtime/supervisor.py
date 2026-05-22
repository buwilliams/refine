"""Local Refine supervisor process."""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from pathlib import Path

from refine_runtime import ipc
from refine_runtime.manager import ResourceManager
from refine_runtime.resources import ResourceSettings
from refine_server import config, db


def main() -> int:
    host = os.environ.get("REFINE_UI_HOST", "0.0.0.0")
    port = int(os.environ.get("REFINE_UI_PORT", "8080"))
    cfg_path = os.environ.get(config.ENV_CONFIG_PATH)
    sock = ipc.runner_socket_path(port=port, config_path=cfg_path)
    env = os.environ.copy()
    env["REFINE_RUNNER_SOCKET"] = str(sock)
    env["REFINE_NO_INPROCESS_RUNNER"] = "1"
    env.setdefault("PYTHONUNBUFFERED", "1")
    resource_settings = _load_resource_settings(cfg_path)
    resources = ResourceManager(resource_settings)

    worker: subprocess.Popen | None = None
    ui: subprocess.Popen | None = None
    stopping = False

    def _terminate(proc: subprocess.Popen | None) -> None:
        if proc is None or proc.poll() is not None:
            return
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        except OSError:
            try:
                proc.terminate()
            except OSError:
                pass

    def _kill(proc: subprocess.Popen | None) -> None:
        if proc is None or proc.poll() is not None:
            return
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except OSError:
            try:
                proc.kill()
            except OSError:
                pass

    def _on_signal(signum, _frame):  # noqa: ANN001
        nonlocal stopping
        sys.stderr.write(f"\n[refine-supervisor] caught signal {signum}, shutting down\n")
        stopping = True
        _terminate(ui)
        _terminate(worker)

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

    if cfg_path:
        worker = subprocess.Popen(
            [sys.executable, "-m", "refine_runtime.worker"],
            cwd=str(Path.cwd()),
            env=env,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
        )
        _wait_for_socket(sock, worker)

    ui = resources.popen(
        [sys.executable, "-m", "refine_cli", "ui"],
        cwd=Path.cwd(),
        env=env,
        kind="ui",
        stdin=subprocess.DEVNULL,
        stdout=None,
        stderr=None,
    )

    try:
        while not stopping:
            if ui.poll() is not None:
                if stopping:
                    break
                sys.stderr.write("[refine-supervisor] UI exited; shutting down\n")
                return ui.returncode or 1
            if worker is not None and worker.poll() is not None:
                sys.stderr.write("[refine-supervisor] runner exited; shutting down UI\n")
                _terminate(ui)
                return worker.returncode or 1
            time.sleep(0.5)
    finally:
        _terminate(ui)
        _terminate(worker)
        deadline = time.time() + 5
        while time.time() < deadline:
            live = [
                p for p in (ui, worker)
                if p is not None and p.poll() is None
            ]
            if not live:
                break
            time.sleep(0.1)
        _kill(ui)
        _kill(worker)
    return 0


def _wait_for_socket(path: Path, proc: subprocess.Popen) -> None:
    deadline = time.time() + 20
    while time.time() < deadline:
        if path.exists():
            return
        if proc.poll() is not None:
            raise SystemExit(proc.returncode or 1)
        time.sleep(0.1)
    raise SystemExit(f"runner socket did not appear: {path}")


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


if __name__ == "__main__":
    raise SystemExit(main())
