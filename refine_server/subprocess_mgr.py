"""Spawn and supervise agent CLI subprocesses.

Per spec:
- One fresh CLI invocation per unaddressed round (no session resume).
- Idle timeout (primary stuck-detector) — kill if no stdout/stderr for N seconds.
- Hard wall-clock cap (ultimate stop-gap) — kill if total runtime exceeds N seconds.
- Stream stdout/stderr → per-Gap round log file (runner appends, via gap_writer).
"""
from __future__ import annotations

import json
import os
import shutil
import signal
import sqlite3
import subprocess
import threading
import time
from collections import deque
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from refine_server import activity, db, perf_metrics
from refine_server.gaps import now_iso
from refine_runtime.manager import ResourceManager
from refine_runtime.resources import ResourceSettings

from . import gap_writer  # local module; sole owner of gap.json writes


# How long to wait for Claude to exit after it emits its `result` event
# before SIGTERMing the process group. Long enough for normal cleanup,
# short enough that the user doesn't watch the dashboard stall when the
# agent kicked off a backgrounded HTTP server (or similar) that's now
# keeping its stdio pipes open indefinitely.
_RESULT_EXIT_GRACE_SECONDS = 10.0


def _summarize_agent_event(evt: dict) -> list[str]:
    """Translate one parsed `--output-format=stream-json` event into a
    list of round-log entries (one per logical thing the event
    represents). Returns an empty list for events we deliberately drop
    (granular `stream_event` deltas, successful `result`, etc.)."""
    if not isinstance(evt, dict):
        return []
    t = evt.get("type")

    if t == "system":
        sub = evt.get("subtype")
        if sub == "init":
            sid = (evt.get("session_id") or "")[:8]
            model = evt.get("model") or "?"
            return [f"[session] init — model={model} session={sid}"]
        # status / requesting / etc. are too granular for the round log.
        return []

    if t == "assistant":
        msg = evt.get("message") or {}
        out: list[str] = []
        for block in msg.get("content") or []:
            bt = block.get("type")
            if bt == "text":
                text = (block.get("text") or "").strip()
                if text:
                    out.append(text)
            elif bt == "tool_use":
                name = block.get("name") or "tool"
                inp = block.get("input") or {}
                out.append(f"[{name}] {_summarize_tool_input(name, inp)}")
        return out

    if t == "user":
        msg = evt.get("message") or {}
        out = []
        for block in msg.get("content") or []:
            if block.get("type") != "tool_result":
                continue
            err = bool(block.get("is_error"))
            content = block.get("content")
            summary = _summarize_tool_result(content)
            if not summary:
                continue
            prefix = "[tool err]" if err else "[tool result]"
            out.append(f"{prefix} {summary[:160]}")
        return out

    if t == "result":
        if evt.get("is_error"):
            err = evt.get("error") or evt.get("result") or "error"
            return [f"[result error] {err}"]
        # successful result text is already in the last `assistant` event
        return []

    # `stream_event` (deltas, message_start/stop, etc.) are too granular —
    # silently drop. `last_output` still ticks per line so idle stays live.
    return []


def _summarize_codex_event(evt: dict) -> list[str]:
    """Translate one Codex `exec --json` event into round-log entries.

    Codex's JSONL event names have changed across releases, so this parser
    keys mostly off the embedded `item.type` shape and falls back to common
    top-level fields.
    """
    if not isinstance(evt, dict):
        return []
    out: list[str] = []
    t = str(evt.get("type") or "")
    item = evt.get("item") if isinstance(evt.get("item"), dict) else {}
    it = str(item.get("type") or evt.get("item_type") or "")

    if t.endswith("started") and ("session_id" in evt or "id" in evt):
        sid = str(evt.get("session_id") or evt.get("id") or "")[:8]
        if "session" in t and sid:
            return [f"[session] init — provider=codex session={sid}"]

    text = (
        item.get("text") or item.get("content") or item.get("message")
        or evt.get("text") or evt.get("message")
    )
    if it in ("agent_message", "assistant_message", "message") and text:
        out.extend(str(text).strip().splitlines())

    command = (
        item.get("command") or item.get("cmd") or item.get("input")
        or evt.get("command")
    )
    if it in ("tool_call", "function_call", "local_shell_call",
              "command_execution", "exec_command") and command:
        first = str(command).splitlines()[0]
        out.append(f"[tool] {first[:160]}")

    result = (
        item.get("output") or item.get("result") or item.get("content")
        or evt.get("output")
    )
    if it in ("tool_result", "function_call_output",
              "local_shell_call_output", "command_output") and result:
        first = _first_nonempty_line(result)
        if first:
            out.append(f"[tool result] {first[:160]}")

    if t == "error" or it == "error":
        err = evt.get("error") or item.get("error") or text or "Codex error"
        out.append(f"[error] {str(err)[:200]}")

    return [s for s in out if s]


def _summarize_tool_input(name: str, inp: dict) -> str:
    """Short, human-readable description of a tool_use input block."""
    if not isinstance(inp, dict):
        return ""
    if name == "Bash":
        cmd = (inp.get("command") or "").splitlines()
        return cmd[0][:160] if cmd else "(empty)"
    if name in ("Read", "Edit", "Write", "NotebookEdit"):
        return inp.get("file_path") or "(?)"
    if name in ("Glob", "Grep"):
        return inp.get("pattern") or "(?)"
    if name == "TodoWrite":
        todos = inp.get("todos") or []
        return f"{len(todos)} todo{'' if len(todos) == 1 else 's'}"
    if name == "Task":
        return (inp.get("description")
                or inp.get("prompt", "")[:80]
                or "(task)")
    try:
        return json.dumps(inp, ensure_ascii=False)[:120]
    except Exception:
        return "?"


def _summarize_tool_result(content) -> str:
    """First useful line of a tool_result block — content is either a
    string or a list of {type:text, text:…} blocks."""
    if isinstance(content, str):
        for ln in content.splitlines():
            ln = ln.strip()
            if ln:
                return ln
        return ""
    if isinstance(content, list):
        for block in content:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "text":
                text = (block.get("text") or "").strip()
                if text:
                    return text.splitlines()[0]
    return ""


def _first_nonempty_line(value) -> str:
    if isinstance(value, list):
        for block in value:
            if isinstance(block, dict):
                text = block.get("text") or block.get("content")
                if text:
                    found = _first_nonempty_line(text)
                    if found:
                        return found
            elif block:
                found = _first_nonempty_line(str(block))
                if found:
                    return found
        return ""
    for ln in str(value).splitlines():
        ln = ln.strip()
        if ln:
            return ln
    return ""


@dataclass
class RunHandle:
    gap_id: str
    round_idx: int
    proc: subprocess.Popen
    started_at: float
    idle_window: int       # seconds; 0 disables
    hard_cap: int          # seconds; 0 disables
    last_output: float
    cwd: Path              # worktree path
    base_ref: str          # commit before the run, for "no commits produced" detection
    output_format: str = "plain"
    provider: str = ""
    killed_reason: str | None = None
    finished: threading.Event = None  # type: ignore[assignment]
    # Set to time.monotonic() when stream-json emits a `result` event.
    # The supervisor uses this to bound how long we wait for Claude to
    # actually exit after it's logically done — if it's still alive a
    # short grace period later (because a backgrounded subprocess is
    # holding its stdio pipes open), we SIGTERM the process group.
    result_seen_at: float | None = None
    # Mirrors `result.is_error` from the stream-json terminal event:
    # True  → agent reported success (regardless of whether it produced
    #         commits — e.g., when actual already matched target),
    # False → agent reported failure,
    # None  → no `result` event observed (e.g., subprocess crashed).
    # Plumbed into the outcome classifier so a clean exit with the
    # agent's own success signal doesn't get demoted to `failed`
    # solely because nothing changed in the worktree.
    agent_reported_success: bool | None = None
    recent_output: deque[str] | None = None
    log_entries: int = 0
    truncated_bytes: int = 0
    skipped_bytes: int = 0
    last_log_metric_at: float = 0.0

    def __post_init__(self) -> None:
        if self.finished is None:
            self.finished = threading.Event()
        if self.recent_output is None:
            self.recent_output = deque(maxlen=40)


class SubprocessManager:
    """Tracks running subprocesses per Gap, enforces idle/hard caps, captures output."""

    def __init__(self, get_conn: Callable[[], sqlite3.Connection]) -> None:
        self._get_conn = get_conn
        self._lock = threading.Lock()
        self._runs: dict[str, RunHandle] = {}  # gap_id -> RunHandle

    # --- public api -----------------------------------------------------------

    def launch(
        self,
        *,
        gap_id: str,
        round_idx: int,
        prompt: str,
        cwd: Path,
        base_ref: str,
        idle_window: int,
        hard_cap: int,
        on_finished: Callable[..., None] | None = None,
    ) -> int:
        """Spawn an agent CLI subprocess in the Gap's worktree.

        Returns the PID. on_finished(gap_id, exit_code, killed_reason,
        agent_reported_success, failure_text) is invoked from the supervisor
        thread when the subprocess exits.
        """
        # Reuse the same env + PATH plumbing the chat subprocess uses:
        # strip provider API-key override vars for CLI login auth, and
        # resolve the selected provider binary via the user's interactive
        # login-shell PATH rather than systemd-user's stripped PATH.
        from .chat_mgr import _chat_env
        from . import agent_cli
        env = _chat_env()
        # The CLI to drive is operator-configurable (claude / codex /
        # gemini); we look up the binary on the user's interactive-
        # login PATH that `_chat_env` already provides. Stream-json
        # parsing in `_drain_stdout` is gated on `spec.output_format`.
        spec = agent_cli.get_spec(
            db.get_setting(self._get_conn(), "agent_cli")
        )
        bin_path = agent_cli.resolve_binary(spec, env)
        args = spec.agent_args(bin_path, prompt, cwd=cwd)
        settings = db.list_settings(self._get_conn())
        manager = ResourceManager(ResourceSettings.from_settings(settings))
        proc = manager.popen(
            args,
            cwd=cwd,
            env=env,
            kind="agent",
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,  # line-buffered
        )
        now = time.monotonic()
        handle = RunHandle(
            gap_id=gap_id,
            round_idx=round_idx,
            proc=proc,
            started_at=now,
            idle_window=idle_window,
            hard_cap=hard_cap,
            last_output=now,
            cwd=cwd,
            base_ref=base_ref,
            output_format=spec.output_format,
            provider=spec.name,
        )
        with self._lock:
            self._runs[gap_id] = handle

        # Record `runs` row for observability + restart reconciliation.
        conn = self._get_conn()
        with db.transaction(conn):
            conn.execute(
                "INSERT INTO runs (gap_id, round_idx, started_at, pid, status, last_output_at) "
                "VALUES (?, ?, ?, ?, 'running', ?)",
                (gap_id, round_idx, now_iso(), proc.pid, now_iso()),
            )

        t = threading.Thread(
            target=self._supervise,
            args=(handle, on_finished),
            name=f"refine-run-{gap_id}",
            daemon=True,
        )
        t.start()
        return proc.pid

    def cancel(self, gap_id: str, reason: str = "cancel") -> bool:
        """Kill the running subprocess for a Gap, if any."""
        with self._lock:
            h = self._runs.get(gap_id)
        if not h:
            return False
        self._kill(h, reason)
        return True

    def cancel_all(self, reason: str = "shutdown") -> int:
        """Kill every running Gap subprocess and return how many were signaled."""
        with self._lock:
            handles = list(self._runs.values())
        for h in handles:
            self._kill(h, reason)
        return len(handles)

    def is_running(self, gap_id: str) -> bool:
        with self._lock:
            return gap_id in self._runs

    def running_snapshot(self) -> list[dict]:
        out = []
        now = time.monotonic()
        with self._lock:
            for h in self._runs.values():
                out.append({
                    "gap_id": h.gap_id,
                    "round_idx": h.round_idx,
                    "pid": h.proc.pid,
                    "elapsed_seconds": int(now - h.started_at),
                    "idle_seconds": int(now - h.last_output),
                })
        return out

    # --- internals ------------------------------------------------------------

    def _supervise(
        self,
        h: RunHandle,
        on_finished: Callable[..., None] | None,
    ) -> None:
        # Reader thread for stdout (stderr is merged in)
        reader = threading.Thread(
            target=self._drain_stdout,
            args=(h,),
            name=f"refine-out-{h.gap_id}",
            daemon=True,
        )
        reader.start()

        try:
            while h.proc.poll() is None:
                now = time.monotonic()
                if h.hard_cap and (now - h.started_at) > h.hard_cap:
                    self._kill(h, "hard_cap")
                    break
                if h.idle_window and (now - h.last_output) > h.idle_window:
                    self._kill(h, "idle")
                    break
                # Bound the post-result wait. Once the agent has emitted
                # its terminal `result` event, give Claude a short grace
                # period to exit cleanly; if it doesn't, a backgrounded
                # subprocess (HTTP server, watcher, etc.) is almost
                # certainly holding the stdio pipes open. SIGTERM the
                # whole process group so the run wraps up and the Gap
                # can move on to `review`.
                if (h.result_seen_at is not None
                        and (now - h.result_seen_at) > _RESULT_EXIT_GRACE_SECONDS):
                    self._kill(h, "result_grace")
                    break
                # Sleep briefly; check again.
                if h.finished.wait(timeout=2.0):
                    break  # already finished
            exit_code = h.proc.wait()
            if (h.output_format == "codex_json" and exit_code == 0
                    and h.agent_reported_success is None):
                h.agent_reported_success = True
        except Exception:
            exit_code = -1

        reader.join(timeout=2.0)

        # Finalize the run record + remove from active set.
        with self._lock:
            self._runs.pop(h.gap_id, None)
        conn = self._get_conn()
        with db.transaction(conn):
            conn.execute(
                "UPDATE runs SET finished_at = ?, status = ?, failure_category = ? "
                "WHERE gap_id = ? AND finished_at IS NULL",
                (
                    now_iso(),
                    "finished" if h.killed_reason is None else "killed",
                    h.killed_reason,
                    h.gap_id,
                ),
            )

        h.finished.set()
        if on_finished is not None:
            try:
                on_finished(h.gap_id, exit_code, h.killed_reason,
                            h.agent_reported_success, self._failure_text(h))
            except Exception as e:  # pragma: no cover — defensive
                activity.append(
                    self._get_conn(),
                    message=f"on_finished callback raised: {e!r}",
                    severity="error", category="cli",
                    gap_id=h.gap_id, actor="runner",
                )
        elapsed = max(0.001, time.monotonic() - h.started_at)
        perf_metrics.record(
            "agent_log_append",
            conn=self._get_conn(),
            elapsed_ms=elapsed * 1000.0,
            success=h.killed_reason is None,
            gap_id=h.gap_id,
            provider=h.provider,
            rows_returned=h.log_entries,
            bytes_out=h.truncated_bytes,
            details={
                "round_idx": h.round_idx,
                "entries_per_sec": round(h.log_entries / elapsed, 3),
                "total_entries": h.log_entries,
                "truncated_bytes": h.truncated_bytes,
                "skipped_bytes": h.skipped_bytes,
                "killed_reason": h.killed_reason or "",
            },
        )
        perf_metrics.record(
            "ai.agent_run",
            conn=self._get_conn(),
            elapsed_ms=elapsed * 1000.0,
            success=(h.killed_reason is None and exit_code == 0),
            gap_id=h.gap_id,
            provider=h.provider,
            details={
                "round_idx": h.round_idx,
                "exit_code": exit_code,
                "killed_reason": h.killed_reason or "",
                "agent_reported_success": h.agent_reported_success,
            },
        )

    def _drain_stdout(self, h: RunHandle) -> None:
        """Translate structured events from the agent into round-log lines.

        With `--output-format=stream-json --verbose`, each stdout line is
        a JSON event: `system/init`, `assistant` (with text + tool_use
        blocks), `user` (with tool_result blocks), `result`, plus noisy
        `stream_event` deltas. We pick the meaningful ones and emit one
        round-log entry per. Non-JSON lines (CLI errors before/after
        stream-json) pass through verbatim.

        When a `result` event arrives, stamp `h.result_seen_at` so the
        supervisor can SIGTERM the process group if Claude doesn't
        actually exit shortly after — happens when the agent kicked off
        a backgrounded subprocess that's holding the stdio pipes open.
        """
        assert h.proc.stdout is not None
        try:
            for raw in h.proc.stdout:
                h.last_output = time.monotonic()
                line = raw.rstrip("\n")
                if not line:
                    continue
                if h.output_format == "plain":
                    # Gemini / any plain-output CLI: line passthrough.
                    # passthrough. The line goes verbatim into the
                    # round log; idle / hard-cap still work via the
                    # `last_output` tick above. No `result` event, so
                    # `agent_reported_success` stays None — outcome
                    # classification falls back to exit-code-only.
                    self._write_log_entry(h, line)
                    continue
                try:
                    evt = json.loads(line)
                except json.JSONDecodeError:
                    # Wasn't JSON — could be a CLI error or plain stderr
                    # message merged in via stderr=STDOUT.
                    self._write_log_entry(h, line)
                    continue
                if h.output_format == "codex_json":
                    summaries = _summarize_codex_event(evt)
                    if isinstance(evt, dict):
                        t = str(evt.get("type") or "")
                        item = evt.get("item") if isinstance(evt.get("item"), dict) else {}
                        if t == "error" or item.get("type") == "error":
                            h.agent_reported_success = False
                        elif t in ("turn.completed", "session.completed",
                                   "exec.completed"):
                            h.agent_reported_success = True
                else:
                    summaries = _summarize_agent_event(evt)
                for s in summaries:
                    if s:
                        self._write_log_entry(h, s)
                if h.output_format != "plain" and not summaries:
                    h.skipped_bytes += len(line.encode("utf-8", errors="replace"))
                if (h.output_format == "claude_json"
                        and isinstance(evt, dict)
                        and evt.get("type") == "result"):
                    h.result_seen_at = time.monotonic()
                    h.agent_reported_success = not bool(evt.get("is_error"))
        finally:
            # final last_output_at touch
            try:
                conn = self._get_conn()
                with db.transaction(conn):
                    conn.execute(
                        "UPDATE runs SET last_output_at = ? WHERE gap_id = ? AND finished_at IS NULL",
                        (now_iso(), h.gap_id),
                    )
            except Exception:
                pass

    def _write_log_entry(self, h: RunHandle, message: str) -> None:
        """One round-log entry per call + a `last_output_at` bump."""
        self._remember_output(h, message)
        raw_bytes = len(message.encode("utf-8", errors="replace"))
        stored = message[:200]
        stored_bytes = len(stored.encode("utf-8", errors="replace"))
        if raw_bytes > stored_bytes:
            h.truncated_bytes += raw_bytes - stored_bytes
        h.log_entries += 1
        try:
            gap_writer.append_round_log(
                gap_id=h.gap_id,
                round_idx=h.round_idx,
                severity="info",
                category="cli",
                message=stored,
                details=message if len(message) > 200 else None,
            )
        except Exception:
            pass
        now = time.monotonic()
        if h.last_log_metric_at == 0.0:
            h.last_log_metric_at = now
        elif h.log_entries % 50 == 0 or (now - h.last_log_metric_at) >= 30.0:
            elapsed = max(0.001, now - h.started_at)
            perf_metrics.record(
                "agent_log_append",
                conn=self._get_conn(),
                elapsed_ms=elapsed * 1000.0,
                gap_id=h.gap_id,
                provider=h.provider,
                rows_returned=h.log_entries,
                bytes_out=h.truncated_bytes,
                details={
                    "round_idx": h.round_idx,
                    "entries_per_sec": round(h.log_entries / elapsed, 3),
                    "total_entries": h.log_entries,
                    "truncated_bytes": h.truncated_bytes,
                    "skipped_bytes": h.skipped_bytes,
                    "partial": True,
                },
            )
            h.last_log_metric_at = now
        try:
            conn = self._get_conn()
            with db.transaction(conn):
                conn.execute(
                    "UPDATE runs SET last_output_at = ? WHERE gap_id = ? AND finished_at IS NULL",
                    (now_iso(), h.gap_id),
                )
        except Exception:
            pass

    def _remember_output(self, h: RunHandle, message: str) -> None:
        text = (message or "").strip()
        if text and h.recent_output is not None:
            h.recent_output.append(text[:1000])

    def _failure_text(self, h: RunHandle) -> str:
        if not h.recent_output:
            return ""
        return "\n".join(h.recent_output)

    def _kill(self, h: RunHandle, reason: str) -> None:
        h.killed_reason = reason
        try:
            # Kill the whole process group created by the resource backend.
            os.killpg(os.getpgid(h.proc.pid), signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            pass
        # Give it 5s, then SIGKILL
        try:
            h.proc.wait(timeout=5.0)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(h.proc.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
