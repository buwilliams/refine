"""Lightweight management of interactive Chat sessions.

A Chat session = an ongoing conversation with `claude` in some directory
(client repo for standalone; the Gap's worktree for attached). Each session
has a unique refine `session_id` and a stable claude `--session-id` UUID
used to thread context across user messages.

`claude` switches to non-interactive mode whenever stdout isn't a TTY (e.g.,
piped from a subprocess), so we can't drive an interactive REPL through pipes
directly. Instead, each user message launches `claude --print` and we resume
the same claude conversation on subsequent messages with `--resume <uuid>`.

Standalone sessions auto-close after `chat_idle_timeout_seconds` of no
activity (default 300s); attached sessions stay open until the Gap finishes.
"""
from __future__ import annotations

import os
import shutil
import signal
import subprocess
import threading
import time
import uuid
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Deque


@dataclass
class ChatSession:
    session_id: str
    claude_uuid: str               # passed as `--session-id` / `--resume`
    cwd: Path
    is_standalone: bool
    last_activity_ts: float        # monotonic timestamp of last user activity
    out_lines: Deque[str] = field(default_factory=lambda: deque(maxlen=10_000))
    out_lock: threading.Lock = field(default_factory=threading.Lock)
    proc_lock: threading.Lock = field(default_factory=threading.Lock)
    proc: subprocess.Popen | None = None   # in-flight request, if any
    pump: threading.Thread | None = None
    has_sent_first: bool = False
    alive: bool = True             # cleared by stop() / supervisor
    closed_reason: str | None = None


class ChatManager:
    def __init__(self,
                 get_standalone_idle_timeout: Callable[[], int] | None = None,
                 ) -> None:
        self._lock = threading.Lock()
        self._sessions: dict[str, ChatSession] = {}
        # Resolver for the standalone-idle-timeout setting. Defaults to 300s
        # if no resolver is provided (e.g., during unit tests).
        self._get_standalone_idle_timeout = (
            get_standalone_idle_timeout or (lambda: 300)
        )
        self._supervisor_stop = threading.Event()
        self._supervisor = threading.Thread(
            target=self._supervise, name="refine-chat-supervisor", daemon=True,
        )
        self._supervisor.start()

    def shutdown(self) -> None:
        self._supervisor_stop.set()

    def start(self, cwd: Path, *, is_standalone: bool = True) -> str:
        sid = uuid.uuid4().hex[:12]
        session = ChatSession(
            session_id=sid,
            claude_uuid=str(uuid.uuid4()),
            cwd=cwd,
            is_standalone=is_standalone,
            last_activity_ts=time.monotonic(),
        )
        with self._lock:
            self._sessions[sid] = session
        return sid

    def send(self, session_id: str, text: str) -> bool:
        with self._lock:
            s = self._sessions.get(session_id)
        if not s or not s.alive:
            return False
        with s.proc_lock:
            # Reject a second message while the previous one is still running.
            if s.proc is not None and s.proc.poll() is None:
                return False
            claude = shutil.which("claude") or "claude"
            args = [claude, "--print"]
            if s.has_sent_first:
                args.extend(["--resume", s.claude_uuid])
            else:
                args.extend(["--session-id", s.claude_uuid])
            args.append(text)
            try:
                s.proc = subprocess.Popen(
                    args,
                    cwd=str(s.cwd),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    bufsize=1,
                    start_new_session=True,
                )
            except (OSError, FileNotFoundError) as e:
                with s.out_lock:
                    s.out_lines.append(f"[refine] failed to launch claude: {e}")
                return False
            s.has_sent_first = True
            s.last_activity_ts = time.monotonic()
            proc = s.proc
        s.pump = threading.Thread(
            target=self._pump_output, args=(s, proc),
            name=f"refine-chat-{session_id}", daemon=True,
        )
        s.pump.start()
        return True

    def read(self, session_id: str, *, max_lines: int = 200) -> dict:
        with self._lock:
            s = self._sessions.get(session_id)
        if not s:
            return {"alive": False, "lines": [], "session_id": session_id}
        with s.out_lock:
            lines = list(s.out_lines)
            s.out_lines.clear()
        return {
            "alive": s.alive,
            "session_id": session_id,
            "lines": lines[-max_lines:],
            "closed_reason": s.closed_reason,
        }

    def stop(self, session_id: str, *, reason: str | None = None) -> bool:
        with self._lock:
            s = self._sessions.pop(session_id, None)
        if not s:
            return False
        return self._terminate(s, reason=reason)

    def _terminate(self, s: ChatSession, *, reason: str | None) -> bool:
        s.alive = False
        if reason:
            s.closed_reason = reason
            with s.out_lock:
                s.out_lines.append(f"[refine] session closed: {reason}")
        with s.proc_lock:
            proc = s.proc
            if proc is not None and proc.poll() is None:
                try:
                    os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
                except (ProcessLookupError, PermissionError):
                    pass
                try:
                    proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        pass
        return True

    def _pump_output(self, s: ChatSession, proc: subprocess.Popen) -> None:
        if proc.stdout is None:
            return
        try:
            for line in proc.stdout:
                with s.out_lock:
                    s.out_lines.append(line.rstrip("\n"))
        except Exception:
            pass

    def _supervise(self) -> None:
        # Poll every 15s. Granularity beyond that is fine; users won't notice
        # an extra few seconds before the idle close fires.
        while not self._supervisor_stop.wait(15.0):
            try:
                timeout = max(int(self._get_standalone_idle_timeout()), 0)
            except Exception:
                timeout = 300
            if timeout <= 0:
                continue  # 0/negative disables auto-close
            now = time.monotonic()
            to_close: list[ChatSession] = []
            with self._lock:
                for sid, s in list(self._sessions.items()):
                    if not s.is_standalone or not s.alive:
                        continue
                    # Don't kill a session with an in-flight request — the user
                    # is plainly active.
                    if s.proc is not None and s.proc.poll() is None:
                        continue
                    if now - s.last_activity_ts >= timeout:
                        to_close.append(s)
                        self._sessions.pop(sid, None)
            for s in to_close:
                self._terminate(
                    s, reason=f"idle for {timeout}s — auto-closed",
                )
