"""UI-to-runner bridge behavior for the supervisor runtime."""
from __future__ import annotations

import errno
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def main() -> int:
    from refine_runtime import ipc
    from refine_ui import api
    from refine_ui import runtime
    from refine_ui.backend_client import BackendClient

    original_request = ipc.request
    original_socket_factory = ipc.socket.socket
    original_select = ipc.select.select
    original_runner = runtime._runner  # type: ignore[attr-defined]
    original_socket = os.environ.get("REFINE_RUNNER_SOCKET")
    original_no_inprocess = os.environ.get("REFINE_NO_INPROCESS_RUNNER")
    calls: list[tuple[str, str, dict, float]] = []

    def fake_request(path, method, params=None, *, timeout=30.0):  # noqa: ANN001, ANN202
        calls.append((str(path), method, params or {}, timeout))
        return {"ok": True}

    try:
        ipc.request = fake_request  # type: ignore[assignment]
        runtime._runner = runtime._SocketRunnerClient("/tmp/refine-runner.sock")  # type: ignore[attr-defined]
        assert runtime.runner_call("slow_method", {"x": 1}, timeout=123.0) == {"ok": True}
        assert calls[-1] == (
            "/tmp/refine-runner.sock",
            "slow_method",
            {"x": 1},
            123.0,
        )

        client = BackendClient()
        assert client.call("bulk_method", {"ids": [1, 2]}, timeout=77.0) == {"ok": True}
        assert calls[-1] == (
            "/tmp/refine-runner.sock",
            "bulk_method",
            {"ids": [1, 2]},
            77.0,
        )

        os.environ["REFINE_RUNNER_SOCKET"] = "/tmp/refine-runner.sock"
        os.environ["REFINE_NO_INPROCESS_RUNNER"] = "1"
        info = runtime.backend_info()
        assert info["process_model"] == "supervisor"
        assert info["transport"] == "unix_socket"
        assert info["ui_controls_runner_lifecycle"] is True
        assert info["in_process_runner_allowed"] is False

        status, body = api.backend_diagnostics()
        assert status == 200
        assert body["reachable"] is True
        assert body["backend"]["process_model"] == "supervisor"

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
        assert info["ui_controls_runner_lifecycle"] is True
        assert info["in_process_runner_allowed"] is False
    finally:
        ipc.request = original_request  # type: ignore[assignment]
        ipc.socket.socket = original_socket_factory  # type: ignore[assignment]
        ipc.select.select = original_select  # type: ignore[assignment]
        runtime._runner = original_runner  # type: ignore[attr-defined]
        if original_socket is None:
            os.environ.pop("REFINE_RUNNER_SOCKET", None)
        else:
            os.environ["REFINE_RUNNER_SOCKET"] = original_socket
        if original_no_inprocess is None:
            os.environ.pop("REFINE_NO_INPROCESS_RUNNER", None)
        else:
            os.environ["REFINE_NO_INPROCESS_RUNNER"] = original_no_inprocess

    print("runtime bridge tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
