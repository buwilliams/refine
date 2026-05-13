"""Synchronous Unix-socket IPC client. One request per connection."""
from __future__ import annotations

import json
import socket
import threading
import uuid
from typing import Any

from refine_shared.ipc_protocol import default_socket_path

_lock = threading.Lock()


class IpcError(Exception):
    def __init__(self, code: str, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details


class RunnerClient:
    def __init__(self, socket_path: str | None = None) -> None:
        self.socket_path = socket_path or default_socket_path()

    def call(self, method: str, params: dict | None = None,
             *, timeout: float = 30.0) -> dict:
        req_id = uuid.uuid4().hex[:12]
        envelope = {"id": req_id, "method": method, "params": params or {}}
        data = (json.dumps(envelope) + "\n").encode("utf-8")
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(timeout)
            try:
                s.connect(self.socket_path)
            except (FileNotFoundError, ConnectionRefusedError, PermissionError) as e:
                raise IpcError("runner_unreachable",
                               f"Host runner unreachable: {self.socket_path}",
                               str(e)) from e
            s.sendall(data)
            buf = b""
            while b"\n" not in buf:
                chunk = s.recv(65536)
                if not chunk:
                    break
                buf += chunk
        line, _, _ = buf.partition(b"\n")
        if not line:
            raise IpcError("empty_response", "Runner returned an empty response")
        try:
            resp = json.loads(line.decode("utf-8"))
        except Exception as e:
            raise IpcError("bad_response", f"Invalid response JSON: {e!r}") from e
        if not resp.get("ok"):
            err = resp.get("error") or {}
            raise IpcError(err.get("code", "unknown"),
                           err.get("message", "Unknown error"),
                           err.get("details"))
        return resp.get("result") or {}

    def ping(self) -> dict:
        return self.call("ping")

    def is_reachable(self) -> bool:
        try:
            self.ping()
            return True
        except IpcError:
            return False


_singleton: RunnerClient | None = None


def get_client() -> RunnerClient:
    global _singleton
    if _singleton is None:
        _singleton = RunnerClient()
    return _singleton
