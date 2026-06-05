"""Popen-compatible handles backed by the supervisor process broker."""
from __future__ import annotations

import io
import os
import signal
import subprocess
import time
from pathlib import Path
from typing import Mapping, Sequence

from refine_runtime import ipc
from refine_runtime.supervisor_protocol import (
    M_PROCESS_LAUNCH,
    M_PROCESS_READ,
    M_PROCESS_WRITE,
    M_PROCESS_SIGNAL,
    M_PROCESS_WAIT,
)


class SupervisorProcessStdout(io.TextIOBase):
    def __init__(self, proc: "SupervisorProcess") -> None:
        self._proc = proc
        self._buf = ""

    def readable(self) -> bool:
        return True

    def read(self, size: int = -1) -> str:
        while size < 0 or len(self._buf) < size:
            chunk, eof = self._proc._read_stdout(timeout=30.0)
            if chunk:
                self._buf += chunk
                continue
            if eof:
                break
        if size is None or size < 0:
            out, self._buf = self._buf, ""
            return out
        out, self._buf = self._buf[:size], self._buf[size:]
        return out

    def readline(self, size: int = -1) -> str:
        while "\n" not in self._buf:
            chunk, eof = self._proc._read_stdout(timeout=30.0)
            if chunk:
                self._buf += chunk
                continue
            if eof:
                break
        if size is not None and size >= 0:
            newline_at = self._buf.find("\n")
            end = min(size, newline_at + 1 if newline_at >= 0 else len(self._buf))
        else:
            newline_at = self._buf.find("\n")
            end = newline_at + 1 if newline_at >= 0 else len(self._buf)
        out, self._buf = self._buf[:end], self._buf[end:]
        return out

    def __iter__(self):
        return self

    def __next__(self) -> str:
        line = self.readline()
        if line == "":
            raise StopIteration
        return line


class SupervisorProcess:
    """Small Popen facade for worker code that streams supervisor output."""

    def __init__(
        self,
        *,
        socket_path: str,
        process_id: str,
        pid: int,
        resource_backend: str = "",
        resource_isolation: str = "",
    ) -> None:
        self.socket_path = socket_path
        self.process_id = process_id
        self.pid = pid
        self.resource_backend = resource_backend
        self.resource_isolation = resource_isolation
        self.returncode: int | None = None
        self.stdout = SupervisorProcessStdout(self)
        self.stderr = None
        self.stdin = None
        self._stdout_cursor = 0

    def poll(self) -> int | None:
        result = ipc.request(
            self.socket_path,
            M_PROCESS_WAIT,
            {"process_id": self.process_id, "timeout": 0},
            timeout=5.0,
        )
        if result.get("exited"):
            self.returncode = _int_or_none(result.get("returncode"))
        return self.returncode

    def wait(self, timeout: float | None = None) -> int:
        result = ipc.request(
            self.socket_path,
            M_PROCESS_WAIT,
            {"process_id": self.process_id, "timeout": timeout},
            timeout=(timeout or 30.0) + 5.0,
        )
        if not result.get("exited"):
            raise subprocess.TimeoutExpired(self.process_id, timeout)
        self.returncode = int(result.get("returncode") or 0)
        return self.returncode

    def communicate(self, input: str | None = None, timeout: float | None = None):  # noqa: A002
        if input:
            ipc.request(
                self.socket_path,
                M_PROCESS_WRITE,
                {"process_id": self.process_id, "data": input},
                timeout=5.0,
            )
        deadline = None if timeout is None else time.monotonic() + timeout
        chunks: list[str] = []
        while True:
            remaining = None if deadline is None else max(0.0, deadline - time.monotonic())
            if remaining is not None and remaining <= 0:
                raise subprocess.TimeoutExpired(self.process_id, timeout)
            chunk, eof = self._read_stdout(timeout=min(1.0, remaining) if remaining is not None else 1.0)
            if chunk:
                chunks.append(chunk)
            if eof:
                break
        self.wait(timeout=0)
        return "".join(chunks), ""

    def terminate(self) -> None:
        self.send_signal(signal.SIGTERM)

    def kill(self) -> None:
        self.send_signal(signal.SIGKILL)

    def send_signal(self, sig: int | signal.Signals) -> None:
        ipc.request(
            self.socket_path,
            M_PROCESS_SIGNAL,
            {"process_id": self.process_id, "signal": int(sig)},
            timeout=5.0,
        )

    def _read_stdout(self, *, timeout: float) -> tuple[str, bool]:
        result = ipc.request(
            self.socket_path,
            M_PROCESS_READ,
            {
                "process_id": self.process_id,
                "stream": "stdout",
                "cursor": self._stdout_cursor,
                "timeout": timeout,
            },
            timeout=max(timeout, 0.1) + 5.0,
        )
        data = str(result.get("data") or "")
        self._stdout_cursor = int(result.get("cursor") or self._stdout_cursor + len(data))
        if result.get("returncode") is not None:
            self.returncode = _int_or_none(result.get("returncode"))
        return data, bool(result.get("eof"))


def supervised_popen(
    args: Sequence[str],
    *,
    cwd: Path,
    env: Mapping[str, str],
    kind: str,
    stdin: object | None = subprocess.DEVNULL,
    stdout: object | None = subprocess.PIPE,
    stderr: object | None = subprocess.STDOUT,
    text: bool = True,
    bufsize: int = 1,
    fallback_manager=None,
):
    socket_path = (
        None if os.environ.get("REFINE_IN_SUPERVISOR") else os.environ.get("REFINE_SUPERVISOR_SOCKET")
    )
    if not socket_path:
        if fallback_manager is None:
            raise RuntimeError("REFINE_SUPERVISOR_SOCKET is required for process launch")
        return fallback_manager.popen(
            args,
            cwd=cwd,
            env=env,
            kind=kind,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            bufsize=bufsize,
        )
    try:
        result = ipc.request(
            socket_path,
            M_PROCESS_LAUNCH,
            {
                "args": list(args),
                "cwd": str(cwd),
                "env": dict(env),
                "kind": kind,
                "stdin": "pipe" if stdin == subprocess.PIPE else "devnull",
                "stdout": "pipe" if stdout == subprocess.PIPE else "inherit",
                "stderr": "stdout" if stderr == subprocess.STDOUT else "pipe",
                "text": bool(text),
                "bufsize": int(bufsize),
            },
            timeout=30.0,
        )
    except Exception:
        if fallback_manager is None:
            raise
        return fallback_manager.popen(
            args,
            cwd=cwd,
            env=env,
            kind=kind,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            bufsize=bufsize,
        )
    return SupervisorProcess(
        socket_path=socket_path,
        process_id=str(result["process_id"]),
        pid=int(result["pid"]),
        resource_backend=str(result.get("resource_backend") or ""),
        resource_isolation=str(result.get("resource_isolation") or ""),
    )


def _int_or_none(value: object) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None
