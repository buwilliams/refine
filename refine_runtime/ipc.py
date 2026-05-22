"""Unix-socket JSON request/response helpers for local Refine IPC."""
from __future__ import annotations

import hashlib
import json
import os
import socket
import socketserver
import threading
from pathlib import Path
from typing import Any, Callable

from refine_server import config


RequestHandler = Callable[[str, dict[str, Any]], dict[str, Any]]


def run_dir(start: Path | None = None) -> Path:
    path = config.local_run_dir(start)
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        os.chmod(path, 0o700)
    except OSError:
        pass
    return path


def config_hash(config_path: Path | str | None = None) -> str:
    try:
        cfg = config.get() if config_path is None else config.Config.load(config_path)
        raw = str(cfg.config_path.resolve())
    except Exception:
        raw = str(config_path or Path.cwd().resolve())
    return hashlib.sha1(raw.encode("utf-8")).hexdigest()[:10]


def runner_socket_path(*, port: int, config_path: Path | str | None = None,
                       start: Path | None = None) -> Path:
    # Keep the filename short: sockaddr_un paths are small on many platforms.
    return run_dir(start) / f"r-{port}-{config_hash(config_path)}.sock"


def request(path: Path | str, method: str, params: dict[str, Any] | None = None,
            *, timeout: float = 30.0) -> dict[str, Any]:
    payload = {
        "method": method,
        "params": params or {},
    }
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(timeout)
        sock.connect(str(path))
        f = sock.makefile("rwb")
        f.write(json.dumps(payload, separators=(",", ":")).encode("utf-8") + b"\n")
        f.flush()
        line = f.readline()
    if not line:
        raise RuntimeError("runner closed the IPC connection without a response")
    body = json.loads(line.decode("utf-8"))
    if not isinstance(body, dict):
        raise RuntimeError("runner returned a non-object response")
    if not body.get("ok"):
        error = body.get("error") if isinstance(body.get("error"), dict) else {}
        code = str(error.get("code") or "internal")
        message = str(error.get("message") or "runner request failed")
        details = error.get("details")
        raise IpcError(code, message, str(details) if details is not None else None)
    result = body.get("result")
    return result if isinstance(result, dict) else {"value": result}


class IpcError(Exception):
    def __init__(self, code: str, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details


class ThreadingUnixServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    allow_reuse_address = True


class JsonRequestHandler(socketserver.StreamRequestHandler):
    dispatcher: RequestHandler

    def handle(self) -> None:
        line = self.rfile.readline()
        if not line:
            return
        try:
            req = json.loads(line.decode("utf-8"))
            if not isinstance(req, dict):
                raise ValueError("request must be an object")
            method = str(req.get("method") or "")
            if not method:
                raise ValueError("method is required")
            params = req.get("params") if isinstance(req.get("params"), dict) else {}
            result = self.dispatcher(method, params)
            resp = {"ok": True, "result": result}
        except Exception as e:  # noqa: BLE001 - IPC must return structured errors.
            resp = {
                "ok": False,
                "error": {
                    "code": e.__class__.__name__,
                    "message": str(e) or repr(e),
                },
            }
        self.wfile.write(json.dumps(resp, separators=(",", ":")).encode("utf-8") + b"\n")


class IpcServer:
    def __init__(self, path: Path | str, dispatcher: RequestHandler) -> None:
        self.path = Path(path)
        self.dispatcher = dispatcher
        self._server: ThreadingUnixServer | None = None
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if self.path.exists():
            self.path.unlink()

        dispatcher = self.dispatcher

        class Handler(JsonRequestHandler):
            pass

        Handler.dispatcher = staticmethod(dispatcher)  # type: ignore[method-assign]
        self._server = ThreadingUnixServer(str(self.path), Handler)
        try:
            os.chmod(self.path, 0o600)
        except OSError:
            pass
        self._thread = threading.Thread(
            target=self._server.serve_forever,
            name="refine-ipc-server",
            daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()
            self._server = None
        if self._thread is not None and self._thread.is_alive():
            self._thread.join(timeout=2.0)
        self._thread = None
        try:
            self.path.unlink()
        except FileNotFoundError:
            pass
