"""Lightweight management of interactive Chat sessions.

A Chat session = an ongoing conversation with `claude` in some directory
(client repo for standalone; the Gap's worktree for attached). Each session
has a unique refine `session_id`; the underlying claude session ID is
discovered from claude's own stream-json output on the first send and reused
via `--resume <id>` for subsequent messages — this avoids depending on the
`--session-id` flag, which older claude binaries don't recognize.

`claude` switches to non-interactive mode whenever stdout isn't a TTY (e.g.,
piped from a subprocess), so we can't drive an interactive REPL through pipes
directly. Instead, each user message launches `claude --print` with
`--output-format=stream-json --verbose` and we parse the structured events
back into chat output.

Standalone sessions auto-close after `chat_idle_timeout_seconds` of no
activity (default 300s); attached sessions stay open until the Gap finishes.
"""
from __future__ import annotations

import json
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


def _chat_env() -> dict[str, str]:
    """Build the environment chat subprocesses run with.

    Chat is supposed to behave like the host CLI `claude` the user runs in
    a terminal — which means OAuth via `~/.claude/`, not an API key. If a
    stale or invalid `ANTHROPIC_API_KEY` / `CLAUDE_API_KEY` leaks in from
    the runner's process env, claude prefers that key and produces
    "Invalid API key · Please run /login" instead of using the login the
    user already has. Strip those vars so we always fall back to the
    host's logged-in auth.
    """
    env = os.environ.copy()
    for key in ("ANTHROPIC_API_KEY", "CLAUDE_API_KEY"):
        env.pop(key, None)
    return env


@dataclass
class ChatSession:
    session_id: str                # refine-internal id
    cwd: Path
    is_standalone: bool
    last_activity_ts: float        # monotonic timestamp of last user activity
    # Discovered from claude's stream-json `system init` event after the
    # first send; reused via `--resume` to thread context.
    claude_session_id: str | None = None
    out_lines: Deque[str] = field(default_factory=lambda: deque(maxlen=10_000))
    out_lock: threading.Lock = field(default_factory=threading.Lock)
    proc_lock: threading.Lock = field(default_factory=threading.Lock)
    proc: subprocess.Popen | None = None   # in-flight request, if any
    pump: threading.Thread | None = None
    alive: bool = True             # cleared by stop() / supervisor
    closed_reason: str | None = None
    # Context text to prepend to the user's first message (used by attached
    # chats to seed the conversation with the Gap's context). Cleared after
    # the first send.
    pending_priming_text: str | None = None


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

    def start(self, cwd: Path, *, is_standalone: bool = True,
              priming_prompt: str | None = None,
              priming_intro: str | None = None) -> str:
        sid = uuid.uuid4().hex[:12]
        session = ChatSession(
            session_id=sid,
            cwd=cwd,
            is_standalone=is_standalone,
            last_activity_ts=time.monotonic(),
            pending_priming_text=priming_prompt or None,
        )
        if priming_intro:
            with session.out_lock:
                session.out_lines.append(priming_intro)
        with self._lock:
            self._sessions[sid] = session
        # If we have a Gap-context priming prompt, eagerly inject it into
        # claude's session memory in a background subprocess. This way the
        # user's first real message resumes a context-aware session instead
        # of having to carry the priming text bundled into the prompt.
        if priming_prompt:
            self._inject_priming(session, priming_prompt)
        return sid

    def _inject_priming(self, s: ChatSession, priming_text: str) -> None:
        """Run claude with the priming text as its prompt, capture the
        session id from the stream-json `system init` event, then discard
        the rest of the output (the user shouldn't see claude's reply to
        the priming itself)."""
        def runner() -> None:
            claude = shutil.which("claude") or "claude"
            args = [claude, "--print",
                    "--output-format=stream-json", "--verbose",
                    priming_text]
            try:
                proc = subprocess.Popen(
                    args,
                    cwd=str(s.cwd),
                    env=_chat_env(),
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    bufsize=1,
                    start_new_session=True,
                )
            except (OSError, FileNotFoundError) as e:
                with s.out_lock:
                    s.out_lines.append(
                        f"[refine] could not load Gap context: {e}"
                    )
                return
            with s.proc_lock:
                s.proc = proc
            s.last_activity_ts = time.monotonic()
            # Use the same JSON-buffered consumer as user-facing chat, but
            # ask it to suppress assistant text so claude's reply to the
            # priming doesn't leak into the user-visible chat buffer.
            if proc.stdout is not None:
                self._consume_chat_output(
                    s, proc.stdout, suppress_assistant=True,
                )
            try:
                proc.wait(timeout=120)
            except Exception:
                pass
            with s.proc_lock:
                if s.proc is proc:
                    s.proc = None
            if s.claude_session_id:
                # Context is now resident in claude's session; the lazy
                # fallback in send() is no longer needed.
                s.pending_priming_text = None
                with s.out_lock:
                    s.out_lines.append(
                        "[refine] Gap context injected — ready."
                    )
            else:
                # Eager injection failed; keep the priming text around so
                # send() can prepend it to the user's first message instead.
                with s.out_lock:
                    s.out_lines.append(
                        "[refine] Eager context injection didn't return a "
                        "session id; your first message will include the "
                        "context inline."
                    )
        threading.Thread(
            target=runner,
            name=f"refine-chat-prime-{s.session_id}",
            daemon=True,
        ).start()

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
            # `--output-format=stream-json` (with required `--verbose`) emits
            # structured events we parse in `_pump_output`, including the
            # `system init` event that carries claude's session id.
            args = [claude, "--print",
                    "--output-format=stream-json", "--verbose"]
            # On the first send of an attached chat, prepend the Gap context
            # so claude has the full picture in one shot. The user only sees
            # their own text echoed back in the UI.
            effective_prompt = text
            if s.pending_priming_text and not s.claude_session_id:
                effective_prompt = (
                    f"{s.pending_priming_text}\n\n---\n\n"
                    f"Now, my first question:\n{text}"
                )
                s.pending_priming_text = None
            if s.claude_session_id:
                args.extend(["--resume", s.claude_session_id])
            args.append(effective_prompt)
            try:
                s.proc = subprocess.Popen(
                    args,
                    cwd=str(s.cwd),
                    env=_chat_env(),
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
        with s.proc_lock:
            in_flight = s.proc is not None and s.proc.poll() is None
        return {
            "alive": s.alive,
            "session_id": session_id,
            "lines": lines[-max_lines:],
            "closed_reason": s.closed_reason,
            "in_flight": in_flight,
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
        """Parse claude's `--output-format=stream-json` events into chat lines.

        Different claude versions format stream-json differently — some emit
        one compact JSON event per line, others pretty-print each event over
        many lines. We use `json.JSONDecoder.raw_decode` to walk a growing
        buffer and extract every complete JSON object regardless of where
        the newlines fall.

        Event shapes we recognize:

        - Newer: `{"type":"system","subtype":"init","session_id":...}`,
                 `{"type":"assistant","message":{"content":[...]}}`,
                 `{"type":"result","is_error":bool,...}`.
        - Older: `{"role":"assistant","content":[{"type":"text",...}], ...}`
                 followed by `{"role":"system","cost_usd":...}`.

        Anything that doesn't look like JSON is passed through verbatim
        (line-by-line) so plain-text CLI errors still reach the user.
        """
        if proc.stdout is None:
            return
        self._consume_chat_output(s, proc.stdout, suppress_assistant=False)

    def _consume_chat_output(self, s: ChatSession, stream,
                              *, suppress_assistant: bool) -> None:
        decoder = json.JSONDecoder()
        buf = ""
        try:
            while True:
                chunk = stream.read(4096)
                if not chunk:
                    break
                buf += chunk
                buf = self._drain_json_objects(
                    s, decoder, buf, suppress_assistant=suppress_assistant,
                )
        except Exception:
            pass
        # Flush any remaining non-JSON tail (e.g., a trailing diagnostic line
        # that never got a newline). Only emit when we're not suppressing
        # output (priming) — priming would surface a stray fragment as noise.
        tail = buf.strip()
        if tail and not suppress_assistant:
            for line in tail.splitlines():
                line = line.rstrip()
                if line:
                    with s.out_lock:
                        s.out_lines.append(line)

    def _drain_json_objects(self, s: ChatSession, decoder: json.JSONDecoder,
                             buf: str, *, suppress_assistant: bool) -> str:
        """Pull complete JSON objects out of `buf`, dispatch them, and
        return whatever tail couldn't be parsed yet. Non-JSON lines are
        emitted verbatim (so plain-text errors still reach the user)."""
        i = 0
        n = len(buf)
        while i < n:
            # Skip whitespace separators between JSON objects.
            while i < n and buf[i] in " \t\r\n":
                i += 1
            if i >= n:
                break
            ch = buf[i]
            if ch in "{[":
                try:
                    evt, j = decoder.raw_decode(buf, i)
                except json.JSONDecodeError:
                    # Object is incomplete; wait for more bytes.
                    break
                self._handle_stream_event(
                    s, evt, suppress_assistant=suppress_assistant,
                )
                i = j
                continue
            # Non-JSON content (e.g., "Error: ..." or "error: unknown option").
            nl = buf.find("\n", i)
            if nl == -1:
                # Incomplete line at end of buffer; wait for newline.
                break
            line = buf[i:nl].rstrip("\r")
            if line.strip() and not suppress_assistant:
                with s.out_lock:
                    s.out_lines.append(line)
            i = nl + 1
        return buf[i:]

    def _handle_stream_event(self, s: ChatSession, evt: dict,
                              *, suppress_assistant: bool = False) -> None:
        if not isinstance(evt, dict):
            return
        t = evt.get("type")
        role = evt.get("role")

        # ---- session id capture -----------------------------------------
        if t == "system" and evt.get("subtype") == "init":
            sid = evt.get("session_id")
            if sid and not s.claude_session_id:
                s.claude_session_id = sid
            return

        # ---- assistant message (new wrapped shape) ----------------------
        if t == "assistant":
            message = evt.get("message") or {}
            self._emit_assistant_content(
                s, message.get("content") or [],
                suppress_assistant=suppress_assistant,
            )
            return

        # ---- result event ------------------------------------------------
        if t == "result":
            if evt.get("is_error"):
                err = (evt.get("error") or evt.get("result")
                       or "Claude returned an error.")
                with s.out_lock:
                    s.out_lines.append(f"[refine] {err}")
            return

        # ---- older bare shape: top-level assistant message --------------
        if role == "assistant" and isinstance(evt.get("content"), list):
            self._emit_assistant_content(
                s, evt["content"],
                suppress_assistant=suppress_assistant,
            )
            return

        # ---- older bare shape: trailing system stats block --------------
        if role == "system":
            # `{"role":"system","cost_usd":0,...}` is metadata-only — ignore.
            return

        # Unknown stream_event / delta / other event types: silently drop.

    def _emit_assistant_content(self, s: ChatSession, blocks: list,
                                  *, suppress_assistant: bool) -> None:
        if suppress_assistant:
            return
        chunks: list[str] = []
        for block in blocks:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "text":
                text = block.get("text") or ""
                if text:
                    chunks.append(text)
        if not chunks:
            return
        with s.out_lock:
            # Visually separate consecutive turns with a blank line.
            if s.out_lines and s.out_lines[-1]:
                s.out_lines.append("")
            for chunk in chunks:
                for ln in chunk.split("\n"):
                    s.out_lines.append(ln)

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
