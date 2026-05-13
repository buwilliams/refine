"""Spawn and supervise `claude --print` subprocesses.

Per spec:
- One fresh CLI invocation per unaddressed round (no session resume).
- Idle timeout (primary stuck-detector) — kill if no stdout/stderr for N seconds.
- Hard wall-clock cap (ultimate stop-gap) — kill if total runtime exceeds N seconds.
- Stream stdout/stderr → round logs[] (runner appends, via gap_writer).
"""
from __future__ import annotations

import os
import shutil
import signal
import sqlite3
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from refine_shared import activity, db
from refine_shared.gaps import now_iso

from . import gap_writer  # local module; sole owner of gap.json writes


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
    killed_reason: str | None = None
    finished: threading.Event = None  # type: ignore[assignment]

    def __post_init__(self) -> None:
        if self.finished is None:
            self.finished = threading.Event()


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
        on_finished: Callable[[str, int, str | None], None] | None = None,
    ) -> int:
        """Spawn a `claude --print` subprocess in the Gap's worktree.

        Returns the PID. on_finished(gap_id, exit_code, killed_reason) is invoked
        from the supervisor thread when the subprocess exits.
        """
        claude_path = shutil.which("claude") or "claude"
        env = os.environ.copy()
        # The agent inherits ~/.claude auth on the host. PATH and HOME come along.
        proc = subprocess.Popen(
            [claude_path, "--print", "--dangerously-skip-permissions", prompt],
            cwd=str(cwd),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,  # line-buffered
            start_new_session=True,  # so we can kill the process group
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

    def cancel(self, gap_id: str) -> bool:
        """Kill the running subprocess for a Gap, if any."""
        with self._lock:
            h = self._runs.get(gap_id)
        if not h:
            return False
        self._kill(h, "cancel")
        return True

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
        on_finished: Callable[[str, int, str | None], None] | None,
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
                # Sleep briefly; check again.
                if h.finished.wait(timeout=2.0):
                    break  # already finished
            exit_code = h.proc.wait()
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
                on_finished(h.gap_id, exit_code, h.killed_reason)
            except Exception as e:  # pragma: no cover — defensive
                activity.append(
                    self._get_conn(),
                    message=f"on_finished callback raised: {e!r}",
                    severity="error", category="cli",
                    gap_id=h.gap_id, actor="runner",
                )

    def _drain_stdout(self, h: RunHandle) -> None:
        assert h.proc.stdout is not None
        buf: list[str] = []
        try:
            for line in h.proc.stdout:
                h.last_output = time.monotonic()
                line = line.rstrip("\n")
                if not line:
                    continue
                # Update the run row's last_output_at periodically (not every line).
                buf.append(line)
                if len(buf) >= 20:
                    self._flush_lines(h, buf)
                    buf = []
        finally:
            if buf:
                self._flush_lines(h, buf)
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

    def _flush_lines(self, h: RunHandle, lines: list[str]) -> None:
        message = "\n".join(lines)
        # Append as one log entry to the round's logs[]
        try:
            gap_writer.append_round_log(
                gap_id=h.gap_id,
                round_idx=h.round_idx,
                severity="info",
                category="cli",
                message=message[:200],  # keep summary short
                details=message if len(message) > 200 else None,
            )
        except Exception:
            pass
        # touch last_output_at
        try:
            conn = self._get_conn()
            with db.transaction(conn):
                conn.execute(
                    "UPDATE runs SET last_output_at = ? WHERE gap_id = ? AND finished_at IS NULL",
                    (now_iso(), h.gap_id),
                )
        except Exception:
            pass

    def _kill(self, h: RunHandle, reason: str) -> None:
        h.killed_reason = reason
        try:
            # Kill the whole process group (start_new_session=True at spawn)
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
