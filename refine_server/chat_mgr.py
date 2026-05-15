"""Lightweight management of interactive Chat sessions.

A Chat session = an ongoing conversation with the selected provider CLI in
some directory (client repo for standalone; the Gap's worktree for attached).
Each session has a unique refine `session_id`; the underlying provider
session ID is discovered from structured output on the first send and reused
for subsequent messages.

Each user message launches a non-interactive CLI turn and we parse the
structured events back into chat output.

Standalone sessions auto-close after `chat_idle_timeout_seconds` of no
activity (default 300s); attached sessions stay open until the Gap finishes.
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import threading
import time
import uuid
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Deque

from . import agent_cli


# Env vars that, if set, would make provider CLIs use API-key auth, inherit
# another agent's session context, or otherwise diverge from the host CLI's
# login session. Strip them before spawn so agent subprocesses behave like
# the user's interactive CLI in a clean terminal — e.g. `claude login` /
# `codex login`.
# How long to wait for the provider CLI to actually exit after it emits its
# terminal `result` event. Past this point, something is almost
# certainly holding the stdio pipes open — typically a backgrounded
# bash subprocess the agent spawned ("python -m http.server &"). We
# SIGTERM the whole process group so the chat can accept the next
# message instead of hanging in-flight forever. Mirrors the agent
# runner's _RESULT_EXIT_GRACE_SECONDS in subprocess_mgr.py.
_RESULT_EXIT_GRACE_SECONDS = 10.0

# Per-turn stdout-idle watchdog. If the provider CLI produces no output
# for this long while it's still running, we treat the turn as wedged
# and SIGTERM the process group. Catches the case where the agent
# kicked off a backgrounded bash and then never emitted a `result`
# event — the result-grace watchdog above can't fire because no
# result ever arrives. 60s is generous enough for legitimate thinking
# (structured CLIs stream events as they work, including tool_use / tool_result
# pairs) but tight enough that the chat self-heals quickly.
_TURN_STDOUT_IDLE_SECONDS = 60.0


_AUTH_OVERRIDE_VARS = (
    # API-key family
    "ANTHROPIC_API_KEY",
    "CLAUDE_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "OPENAI_API_KEY",
    "OPENAI_ORG_ID",
    "OPENAI_ORGANIZATION",
    "OPENAI_PROJECT",
    # Endpoint redirects that would aim a valid key at the wrong service
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_VERSION",
    "OPENAI_BASE_URL",
    "OPENAI_API_BASE",
    "OPENAI_API_HOST",
    # Alternate cloud providers — force the user's Anthropic OAuth login
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    # Inherited Claude-Code-agent context that would make claude treat the
    # subprocess as part of *another* session's auth context
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS",
    "CODEX_CI",
    "CODEX_MANAGED_BY_NPM",
    "CODEX_THREAD_ID",
    "AI_AGENT",
    "CLAUDE_EFFORT",
)


_login_path_cache: str | None = None
_login_path_resolved = False


def _user_login_path() -> str | None:
    """Capture the PATH the user's interactive login shell sees.

    The systemd --user manager runs the runner with a minimal PATH that
    typically lacks `~/.local/bin`, `~/.npm-global/bin`, `/opt/homebrew/bin`,
    and other host-specific bin dirs the user has set up — so
    `shutil.which("claude")` lands on a stale system-wide binary (or
    nothing) instead of the Claude Code CLI the user actually logged into.

    Rather than hardcode any particular install location (which would
    break on macOS, NixOS, asdf/mise setups, etc.), we ask the user's
    interactive login shell exactly once for its PATH. Whatever that
    shell prints is what `claude` / `codex` resolve against in
    interactive use — matching it makes agent subprocesses behave like
    the user's terminal.
    """
    global _login_path_cache, _login_path_resolved
    if _login_path_resolved:
        return _login_path_cache
    _login_path_resolved = True
    shell = os.environ.get("SHELL") or "/bin/bash"
    try:
        out = subprocess.run(
            [shell, "-lic", "printf %s \"$PATH\""],
            capture_output=True, text=True, timeout=5,
        )
        if out.returncode != 0 or not (out.stdout or "").strip():
            out = subprocess.run(
                [shell, "-lc", "printf %s \"$PATH\""],
                capture_output=True, text=True, timeout=5,
            )
    except Exception:
        return None
    path = (out.stdout or "").strip()
    if out.returncode == 0 and path:
        _login_path_cache = path
    return _login_path_cache


def _chat_env() -> dict[str, str]:
    """Build the environment agent subprocesses run with.

    Strip API-key and inherited agent vars so provider CLIs use the
    user's normal login session. For all providers, override PATH with
    the user's interactive-login-shell PATH (cached after the first
    call) so `claude`, `codex`, or `gemini` resolve the same way they
    do in the user's terminal.
    """
    env = os.environ.copy()
    for key in _AUTH_OVERRIDE_VARS:
        env.pop(key, None)
    login_path = _user_login_path()
    if login_path:
        env["PATH"] = login_path
    return env


def _resolve_agent(provider: str | None,
                   env: dict[str, str]) -> tuple[agent_cli.CliSpec, str]:
    spec = agent_cli.get_spec(provider)
    return spec, agent_cli.resolve_binary(spec, env)


@dataclass
class ChatSession:
    session_id: str                # refine-internal id
    cwd: Path
    is_standalone: bool
    last_activity_ts: float        # monotonic timestamp of last user activity
    provider: str = "claude"
    # Discovered from provider structured output after the first send;
    # reused by provider-specific resume args to thread context.
    provider_session_id: str | None = None
    out_lines: Deque[str] = field(default_factory=lambda: deque(maxlen=10_000))
    out_lock: threading.Lock = field(default_factory=threading.Lock)
    proc_lock: threading.Lock = field(default_factory=threading.Lock)
    proc: subprocess.Popen | None = None   # in-flight request, if any
    pump: threading.Thread | None = None
    # PIDs of in-flight procs we've already armed a result-watchdog for.
    # Stored as ints (not the Popen) so the dataclass stays hashable-free
    # and we don't pin procs after they exit.
    watchdog_armed_pids: set[int] = field(default_factory=set)
    # Monotonic timestamp of the last stdout chunk from the current
    # in-flight proc; updated in `_consume_chat_output` per read. The
    # idle-watchdog reads this to decide when to SIGTERM a wedged turn.
    last_chunk_at: float = 0.0
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
        with self._lock:
            sessions = list(self._sessions.values())
            self._sessions.clear()
        for session in sessions:
            self._terminate(session, reason="shutdown")

    def start(self, cwd: Path, *, is_standalone: bool = True,
              provider: str | None = None,
              priming_prompt: str | None = None,
              priming_intro: str | None = None) -> str:
        sid = uuid.uuid4().hex[:12]
        session = ChatSession(
            session_id=sid,
            cwd=cwd,
            is_standalone=is_standalone,
            provider=agent_cli.get_spec(provider).name,
            last_activity_ts=time.monotonic(),
            pending_priming_text=priming_prompt or None,
        )
        if priming_intro:
            with session.out_lock:
                session.out_lines.append(priming_intro)
        with self._lock:
            self._sessions[sid] = session
        # If we have a Gap-context priming prompt, eagerly inject it into
        # the provider's session memory in a background subprocess. This way the
        # user's first real message resumes a context-aware session instead
        # of having to carry the priming text bundled into the prompt.
        if priming_prompt:
            self._inject_priming(session, priming_prompt)
        return sid

    def _inject_priming(self, s: ChatSession, priming_text: str) -> None:
        """Run the provider with the priming text as its prompt, capture the
        session id from structured output, then discard
        the rest of the output (the user shouldn't see the agent's reply to
        the priming itself)."""
        def runner() -> None:
            env = _chat_env()
            spec, binary = _resolve_agent(s.provider, env)
            args = spec.chat_args(binary, priming_text, cwd=s.cwd)
            try:
                proc = subprocess.Popen(
                    args,
                    cwd=str(s.cwd),
                    env=env,
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
            # ask it to suppress assistant text so the priming reply doesn't
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
            if s.provider_session_id:
                # Context is now resident in the provider's session; the lazy
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
            env = _chat_env()
            spec, binary = _resolve_agent(s.provider, env)
            # On the first send of an attached chat, prepend the Gap context
            # so the agent has the full picture in one shot. The user only sees
            # their own text echoed back in the UI.
            effective_prompt = text
            if s.pending_priming_text and not s.provider_session_id:
                effective_prompt = (
                    f"{s.pending_priming_text}\n\n---\n\n"
                    f"Now, my first question:\n{text}"
                )
                s.pending_priming_text = None
            args = spec.chat_args(
                binary, effective_prompt,
                session_id=s.provider_session_id,
                cwd=s.cwd,
            )
            try:
                s.proc = subprocess.Popen(
                    args,
                    cwd=str(s.cwd),
                    env=env,
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
                        f"[refine] failed to launch {spec.binary}: {e}"
                    )
                return False
            s.last_activity_ts = time.monotonic()
            proc = s.proc
            s.last_chunk_at = time.monotonic()
        s.pump = threading.Thread(
            target=self._pump_output, args=(s, proc),
            name=f"refine-chat-{session_id}", daemon=True,
        )
        s.pump.start()
        # Per-turn idle watchdog: catches the case where the agent streams
        # an assistant message, kicks off a backgrounded subprocess,
        # and never emits a `result` event — the result-grace watchdog
        # only arms on a real `result`, so without this fallback the
        # chat hangs in-flight forever.
        threading.Thread(
            target=self._idle_watchdog, args=(s, proc),
            name=f"refine-chat-idle-{session_id}", daemon=True,
        ).start()
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
        """Parse provider structured events into chat lines.

        Different provider versions format JSON differently — some emit
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

        Codex `exec --json` item events are also recognized. Anything that
        doesn't look like JSON is passed through verbatim
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
                # Liveness signal for the idle-watchdog: any byte from
                # any provider output resets the wedged-turn countdown.
                s.last_chunk_at = time.monotonic()
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
        for key in ("session_id", "conversation_id", "thread_id"):
            sid = evt.get(key)
            if sid and not s.provider_session_id:
                s.provider_session_id = str(sid)
                break
        if t == "system" and evt.get("subtype") == "init":
            sid = evt.get("session_id")
            if sid and not s.provider_session_id:
                s.provider_session_id = sid
            return

        # ---- Codex JSONL item events ------------------------------------
        item = evt.get("item") if isinstance(evt.get("item"), dict) else None
        if item is not None:
            sid = item.get("session_id") or item.get("conversation_id")
            if sid and not s.provider_session_id:
                s.provider_session_id = str(sid)
            item_type = item.get("type")
            text = item.get("text") or item.get("content")
            if item_type in ("agent_message", "assistant_message") and text:
                if not suppress_assistant:
                    self._emit_text(s, str(text))
                return
            if item_type == "error" or t == "error":
                err = item.get("error") or evt.get("error") or text \
                    or "Codex returned an error."
                with s.out_lock:
                    s.out_lines.append(f"[refine] {err}")
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
                       or "Agent returned an error.")
                with s.out_lock:
                    s.out_lines.append(f"[refine] {err}")
            # Once the agent has logically finished, give the CLI a short
            # grace to exit; if a backgrounded subprocess (HTTP server,
            # file watcher, …) is keeping the stdio pipes open, kill
            # the process group so the chat doesn't hang in-flight.
            self._arm_result_watchdog(s)
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
        self._emit_text(s, "\n".join(chunks))

    def _emit_text(self, s: ChatSession, text: str) -> None:
        with s.out_lock:
            # Visually separate consecutive turns with a blank line.
            if s.out_lines and s.out_lines[-1]:
                s.out_lines.append("")
            for ln in text.split("\n"):
                s.out_lines.append(ln)

    def _arm_result_watchdog(self, s: ChatSession) -> None:
        """Schedule a one-shot watchdog for the currently in-flight chat
        proc: wait the grace period, then SIGTERM the process group if
        the CLI hasn't exited on its own. Idempotent per-proc — repeated
        result events (shouldn't happen, but defensive) don't double-arm."""
        with s.proc_lock:
            proc = s.proc
            if proc is None or proc.poll() is not None:
                return
            pid = proc.pid
            if pid in s.watchdog_armed_pids:
                return
            s.watchdog_armed_pids.add(pid)
        threading.Thread(
            target=self._result_watchdog,
            args=(s, proc),
            name=f"refine-chat-result-{s.session_id}",
            daemon=True,
        ).start()

    def _idle_watchdog(self, s: ChatSession,
                         proc: subprocess.Popen) -> None:
        """Per-turn fallback for when no `result` event ever arrives.

        Polls every 5s. If the proc is still alive and we've gone
        `_TURN_STDOUT_IDLE_SECONDS` without a stdout chunk, SIGTERM
        the process group so the chat can recover. Exits as soon as
        the proc itself exits — no work to do.
        """
        while proc.poll() is None:
            if (time.monotonic() - s.last_chunk_at
                    > _TURN_STDOUT_IDLE_SECONDS):
                try:
                    os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
                except (ProcessLookupError, PermissionError):
                    return
                try:
                    proc.wait(timeout=5.0)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
                    except (ProcessLookupError, PermissionError):
                        pass
                with s.out_lock:
                    s.out_lines.append(
                        "[refine] Turn went idle on stdout — terminated to "
                        "free the chat (likely a backgrounded subprocess "
                        "that didn't detach).",
                    )
                return
            time.sleep(5.0)

    def _result_watchdog(self, s: ChatSession,
                          proc: subprocess.Popen) -> None:
        # Brief grace for the provider CLI to exit on its own.
        try:
            proc.wait(timeout=_RESULT_EXIT_GRACE_SECONDS)
            return  # clean exit
        except subprocess.TimeoutExpired:
            pass
        # Still alive after grace — SIGTERM the whole process group so
        # backgrounded children (http.server, watchers, …) come down too.
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            return
        try:
            proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        with s.out_lock:
            s.out_lines.append(
                "[refine] Backgrounded subprocess terminated to free the chat.",
            )

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
