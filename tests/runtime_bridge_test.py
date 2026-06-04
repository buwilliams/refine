"""UI-to-runner bridge behavior for the supervisor runtime."""
from __future__ import annotations

import errno
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    from refine_runtime import ipc
    from refine_runtime import identity
    from refine_ui import runtime

    env_keys = (
        "REFINE_CONFIG_PATH",
        "REFINE_UI_PORT",
        "REFINE_UI_SCOPE",
        "REFINE_RUN_DIR",
        "REFINE_SUPERVISOR_SOCKET",
        "REFINE_RUNNER_SOCKET",
        "REFINE_NO_INPROCESS_RUNNER",
        "REFINE_TEST_INPROCESS_BACKEND",
    )
    saved_env = {key: os.environ.get(key) for key in env_keys}
    for key in env_keys:
        os.environ.pop(key, None)
    os.environ["REFINE_UI_PORT"] = "19081"
    os.environ["REFINE_UI_SCOPE"] = "19081"
    tmp, client_repo = make_client_repo("refine-runtime-bridge-")
    conn = init_refine(client_repo)
    conn.close()
    runtime.load_configured(
        client_repo / ".refine" / "refine.toml",
        start_poller=False,
        start_runner=False,
    )
    os.environ.pop("REFINE_TEST_INPROCESS_BACKEND", None)

    original_request = ipc.request
    original_socket_factory = ipc.socket.socket
    original_select = ipc.select.select
    original_supervisor_request = runtime._supervisor_request  # type: ignore[attr-defined]
    original_runner = runtime._runner  # type: ignore[attr-defined]
    calls: list[tuple[str, str, dict, float]] = []
    supervisor_path = str(Path(__file__).resolve().parents[1] / "run" / "19081" / "s.sock")

    def fake_supervisor_request(method, params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
        calls.append((supervisor_path, method, params or {}, timeout))
        if method == "status":
            return {
                "worker": {"pid": 1234, "socket_path": "/internal/worker.sock"},
                "worker_snapshot": {"runner_reachable": True},
            }
        return {"ok": True}

    try:
        runtime._supervisor_request = fake_supervisor_request  # type: ignore[attr-defined]
        assert runtime.runner_call("slow_method", {"x": 1}, timeout=123.0) == {"ok": True}
        assert calls[-1][1] == "backend_call"
        assert calls[-1][2] == {
            "config_path": str(client_repo / ".refine" / "refine.toml"),
            "local_node_id": "default",
            "method": "slow_method",
            "params": {"x": 1},
            "timeout": 123.0,
        }

        os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
        info = runtime.backend_info()
        assert info["process_model"] == "supervisor"
        assert info["transport"] == "unix_socket"
        assert info["ui_controls_runner_lifecycle"] is False
        assert info["in_process_runner_allowed"] is False
        assert "REFINE_RUNNER_SOCKET" not in os.environ
        assert info["source_fingerprint"] == identity.SOURCE_FINGERPRINT
        assert info["refine_version"] == identity.REFINE_VERSION
        assert "local_node_id" in info

        class BusyFakeSocket:
            def __init__(self) -> None:
                self.sent = b""
                self.send_attempts = 0
                self.recv_attempts = 0

            def __enter__(self):  # noqa: ANN204
                return self

            def __exit__(self, *_args) -> None:  # noqa: ANN002
                return None

            def setblocking(self, _blocking: bool) -> None:
                return None

            def connect_ex(self, _path: str) -> int:
                return errno.EAGAIN

            def getsockopt(self, *_args) -> int:  # noqa: ANN002
                return 0

            def send(self, data) -> int:  # noqa: ANN001
                self.send_attempts += 1
                if self.send_attempts == 1:
                    raise BlockingIOError(errno.EAGAIN, "Resource temporarily unavailable")
                chunk = bytes(data)
                self.sent += chunk
                return len(chunk)

            def recv(self, _size: int) -> bytes:
                self.recv_attempts += 1
                if self.recv_attempts == 1:
                    raise BlockingIOError(errno.EAGAIN, "Resource temporarily unavailable")
                return b'{"ok":true,"result":{"pong":true}}\n'

        runtime._supervisor_request = original_supervisor_request  # type: ignore[attr-defined]
        ipc.request = original_request  # type: ignore[assignment]
        busy_socket = BusyFakeSocket()
        ipc.socket.socket = lambda *_args, **_kwargs: busy_socket  # type: ignore[assignment]
        ipc.select.select = lambda r, w, x, _timeout=None: (r, w, x)  # type: ignore[assignment]
        assert ipc.request("/tmp/refine.sock", "ping", {}, timeout=1.0) == {"pong": True}
        assert busy_socket.send_attempts == 2, busy_socket.send_attempts
        assert busy_socket.recv_attempts == 2, busy_socket.recv_attempts
        assert b'"method":"ping"' in busy_socket.sent, busy_socket.sent

        os.environ.pop("REFINE_RUNNER_SOCKET", None)
        os.environ.pop("REFINE_NO_INPROCESS_RUNNER", None)
        info = runtime.backend_info()
        assert info["process_model"] == "supervisor"
        assert info["transport"] == "unix_socket"
        assert info["ui_controls_runner_lifecycle"] is False
        assert info["in_process_runner_allowed"] is False
    finally:
        ipc.request = original_request  # type: ignore[assignment]
        ipc.socket.socket = original_socket_factory  # type: ignore[assignment]
        ipc.select.select = original_select  # type: ignore[assignment]
        runtime._supervisor_request = original_supervisor_request  # type: ignore[attr-defined]
        runtime._runner = original_runner  # type: ignore[attr-defined]
        runtime.stop_all()
        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        cleanup_tmp(tmp)

    print("runtime bridge tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
