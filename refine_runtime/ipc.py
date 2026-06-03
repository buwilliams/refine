"""Unix-socket JSON request/response helpers for local Refine IPC."""
from __future__ import annotations

import hashlib
import errno
import json
import os
import select
import socket
import socketserver
import threading
import time
from pathlib import Path
from typing import Any, Callable

from refine_server import config


RequestHandler = Callable[[str, dict[str, Any]], dict[str, Any]]


def run_dir(start: Path | None = None, *, port: int | str | None = None) -> Path:
    path = config.local_run_dir(start, port=port)
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
    return run_dir(start, port=port) / f"r-{config_hash(config_path)}.sock"


def supervisor_socket_path(port: int, start: Path | None = None) -> Path:
    # One supervisor owns all local process lifecycle for a checkout/port.
    return run_dir(start, port=port) / "s.sock"


def request(path: Path | str, method: str, params: dict[str, Any] | None = None,
            *, timeout: float = 30.0) -> dict[str, Any]:
    payload = {
        "method": method,
        "params": params or {},
    }
    wire_payload = (
        json.dumps(payload, separators=(",", ":")).encode("utf-8") + b"\n"
    )
    deadline = time.monotonic() + timeout
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.setblocking(False)
        _connect_unix(sock, str(path), deadline)
        _send_all(sock, wire_payload, deadline)
        line = _recv_line(sock, deadline)
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


_CONNECT_IN_PROGRESS = {
    errno.EAGAIN,
    errno.EALREADY,
    errno.EINPROGRESS,
    errno.EWOULDBLOCK,
}


def _connect_unix(sock: socket.socket, path: str, deadline: float) -> None:
    err = sock.connect_ex(path)
    if err in (0, errno.EISCONN):
        return
    if err not in _CONNECT_IN_PROGRESS:
        raise OSError(err, os.strerror(err))
    _wait_for_socket(sock, deadline, write=True)
    err = sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR)
    if err not in (0, errno.EISCONN):
        raise OSError(err, os.strerror(err))


def _send_all(sock: socket.socket, payload: bytes, deadline: float) -> None:
    view = memoryview(payload)
    while view:
        try:
            sent = sock.send(view)
        except BlockingIOError:
            _wait_for_socket(sock, deadline, write=True)
            continue
        if sent == 0:
            raise RuntimeError("runner IPC socket closed while sending request")
        view = view[sent:]


def _recv_line(sock: socket.socket, deadline: float) -> bytes:
    chunks: list[bytes] = []
    while True:
        try:
            chunk = sock.recv(65536)
        except BlockingIOError:
            _wait_for_socket(sock, deadline, write=False)
            continue
        if not chunk:
            break
        chunks.append(chunk)
        if b"\n" in chunk:
            break
    data = b"".join(chunks)
    line, _sep, _rest = data.partition(b"\n")
    return line


def _wait_for_socket(
    sock: socket.socket,
    deadline: float,
    *,
    write: bool,
) -> None:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError("runner IPC request timed out")
    rlist = [] if write else [sock]
    wlist = [sock] if write else []
    ready_r, ready_w, _ready_x = select.select(rlist, wlist, [], remaining)
    if not ready_r and not ready_w:
        raise TimeoutError("runner IPC request timed out")


class IpcError(Exception):
    def __init__(self, code: str, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.details = details


class ThreadingUnixServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    allow_reuse_address = True
    request_queue_size = 128


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
        self._socket_stat: os.stat_result | None = None

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
        try:
            self._socket_stat = self.path.stat()
        except OSError:
            self._socket_stat = None
        self._thread = threading.Thread(
            target=self._server.serve_forever,
            name="refine-ipc-server",
            daemon=True,
        )
        self._thread.start()

    def path_current(self) -> bool:
        if self._server is None or self._socket_stat is None:
            return False
        try:
            current = self.path.stat()
        except FileNotFoundError:
            return False
        except OSError:
            return False
        return (
            current.st_ino == self._socket_stat.st_ino
            and current.st_dev == self._socket_stat.st_dev
        )

    def ensure_available(self) -> bool:
        """Rebind when our Unix-socket pathname was removed externally."""
        if self._server is None:
            return False
        if self.path_current():
            return True
        if self.path.exists():
            # Another process replaced the pathname. Do not unlink it here.
            return False
        self.stop()
        self.start()
        return self.path_current()

    def stop(self) -> None:
        server = self._server
        if server is not None:
            self._server = None
            server.shutdown()
            server.server_close()
        if self._thread is not None and self._thread.is_alive():
            self._thread.join(timeout=2.0)
        self._thread = None
        try:
            current = self.path.stat()
            if (
                self._socket_stat is not None
                and current.st_ino == self._socket_stat.st_ino
                and current.st_dev == self._socket_stat.st_dev
            ):
                self.path.unlink()
        except FileNotFoundError:
            pass
        finally:
            self._socket_stat = None
