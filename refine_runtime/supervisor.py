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
    cfg_path = _supervisor_config_path()
    sock = ipc.runner_socket_path(port=port, config_path=cfg_path)
    env = os.environ.copy()
    if cfg_path:
        os.environ[config.ENV_CONFIG_PATH] = cfg_path
        env[config.ENV_CONFIG_PATH] = cfg_path
    env["REFINE_SUPERVISOR_PID"] = str(os.getpid())
    env["REFINE_RUNNER_SOCKET"] = str(sock)
    env["REFINE_NO_INPROCESS_RUNNER"] = "1"
    env.setdefault("PYTHONUNBUFFERED", "1")
    resource_settings = _load_resource_settings(cfg_path)
    resources = ResourceManager(resource_settings)

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
        return

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

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
            time.sleep(0.5)
    finally:
        _terminate(ui)
        deadline = time.time() + 5
        while time.time() < deadline:
            live = [
                p for p in (ui,)
                if p is not None and p.poll() is None
            ]
            if not live:
                break
            time.sleep(0.1)
        _kill(ui)
    return 0


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
