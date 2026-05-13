"""Lightweight management of interactive Chat sessions.

A Chat session = a `claude` subprocess running in some directory (client repo
for standalone; the Gap's worktree for attached). Each session has a unique
session_id assigned by the runner; the webapp streams input lines and pulls
output. Sessions do NOT count toward the parallel-run cap.
"""
from __future__ import annotations

import os
import queue
import shutil
import signal
import subprocess
import threading
import uuid
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Deque


@dataclass
class ChatSession:
    session_id: str
    cwd: Path
    proc: subprocess.Popen
    out_lines: Deque[str] = field(default_factory=lambda: deque(maxlen=10_000))
    out_lock: threading.Lock = field(default_factory=threading.Lock)
    pump: threading.Thread | None = None


class ChatManager:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._sessions: dict[str, ChatSession] = {}

    def start(self, cwd: Path) -> str:
        claude = shutil.which("claude") or "claude"
        proc = subprocess.Popen(
            [claude],
            cwd=str(cwd),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
            start_new_session=True,
        )
        sid = uuid.uuid4().hex[:12]
        session = ChatSession(session_id=sid, cwd=cwd, proc=proc)
        with self._lock:
            self._sessions[sid] = session
        session.pump = threading.Thread(
            target=self._pump_output, args=(session,),
            name=f"refine-chat-{sid}", daemon=True,
        )
        session.pump.start()
        return sid

    def send(self, session_id: str, text: str) -> bool:
        with self._lock:
            s = self._sessions.get(session_id)
        if not s or s.proc.poll() is not None or not s.proc.stdin:
            return False
        try:
            s.proc.stdin.write(text + "\n")
            s.proc.stdin.flush()
            return True
        except (BrokenPipeError, OSError):
            return False

    def read(self, session_id: str, *, max_lines: int = 200) -> dict:
        with self._lock:
            s = self._sessions.get(session_id)
        if not s:
            return {"alive": False, "lines": [], "session_id": session_id}
        with s.out_lock:
            lines = list(s.out_lines)
            s.out_lines.clear()
        return {
            "alive": s.proc.poll() is None,
            "session_id": session_id,
            "lines": lines[-max_lines:],
        }

    def stop(self, session_id: str) -> bool:
        with self._lock:
            s = self._sessions.pop(session_id, None)
        if not s:
            return False
        try:
            os.killpg(os.getpgid(s.proc.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
        try:
            s.proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(s.proc.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        return True

    def _pump_output(self, s: ChatSession) -> None:
        assert s.proc.stdout is not None
        try:
            for line in s.proc.stdout:
                with s.out_lock:
                    s.out_lines.append(line.rstrip("\n"))
        except Exception:
            pass
