"""Unix-domain-socket IPC server.

Wire format: line-delimited JSON (one request per line, one response per line).
The webapp connects, sends one request, reads one response, closes.

Handlers run on a per-connection thread.
"""
from __future__ import annotations

import json
import os
import socket
import sqlite3
import threading
import time
from pathlib import Path
from typing import Callable

from refine_shared.ipc_protocol import envelope_err, envelope_ok


class IpcServer:
    def __init__(self, socket_path: str, dispatch: Callable[[str, dict], dict]) -> None:
        self.socket_path = socket_path
        self.dispatch = dispatch
        self._sock: socket.socket | None = None
        self._thread: threading.Thread | None = None
        self._stop = threading.Event()
        self._diag_lock = threading.Lock()
        self._last_contact: str | None = None
        self._recent_errors: list[str] = []

    def start(self) -> None:
        self._stop.clear()
        Path(self.socket_path).parent.mkdir(parents=True, exist_ok=True)
        # remove stale socket
        try:
            os.unlink(self.socket_path)
        except FileNotFoundError:
            pass
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.bind(self.socket_path)
        os.chmod(self.socket_path, 0o660)
        s.listen(64)
        s.settimeout(0.5)
        self._sock = s
        self._thread = threading.Thread(target=self._accept_loop, name="refine-ipc",
                                        daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._sock:
            try:
                self._sock.close()
            except OSError:
                pass
        try:
            os.unlink(self.socket_path)
        except FileNotFoundError:
            pass

    def diagnostics(self) -> dict:
        with self._diag_lock:
            return {
                "socket_path": self.socket_path,
                "last_contact_at": self._last_contact,
                "recent_errors": list(self._recent_errors[-10:]),
            }

    def _accept_loop(self) -> None:
        assert self._sock is not None
        while not self._stop.is_set():
            try:
                conn, _ = self._sock.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            t = threading.Thread(target=self._handle_conn, args=(conn,),
                                 name="refine-ipc-conn", daemon=True)
            t.start()

    def _handle_conn(self, conn: socket.socket) -> None:
        with conn:
            conn.settimeout(30.0)
            try:
                buf = b""
                while b"\n" not in buf:
                    chunk = conn.recv(65536)
                    if not chunk:
                        return
                    buf += chunk
                line, _, rest = buf.partition(b"\n")
                with self._diag_lock:
                    self._last_contact = _iso_now()
                try:
                    req = json.loads(line.decode("utf-8"))
                except Exception as e:
                    resp = envelope_err("", "bad_json", f"invalid JSON: {e!r}")
                    conn.sendall((json.dumps(resp) + "\n").encode("utf-8"))
                    return
                req_id = req.get("id", "")
                method = req.get("method", "")
                params = req.get("params") or {}
                try:
                    result = self.dispatch(method, params)
                    resp = envelope_ok(req_id, result)
                except KeyError as e:
                    resp = envelope_err(req_id, "unknown_method", str(e))
                except ValueError as e:
                    resp = envelope_err(req_id, "bad_request", str(e))
                except Exception as e:
                    with self._diag_lock:
                        self._recent_errors.append(f"{_iso_now()} {method}: {e!r}")
                    resp = envelope_err(req_id, "internal", repr(e))
                conn.sendall((json.dumps(resp) + "\n").encode("utf-8"))
            except socket.timeout:
                pass
            except Exception as e:
                with self._diag_lock:
                    self._recent_errors.append(f"{_iso_now()} (conn err): {e!r}")


def _iso_now() -> str:
    from refine_shared.gaps import now_iso
    return now_iso()
