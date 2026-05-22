"""Auto-resolve a merge conflict by spawning an agent CLI subprocess
in the host's main client repo, mid-merge.

Called by `verify_op` when `git merge` produces conflict markers. The
agent reads the unresolved files, integrates both sides, stages the
fixes, and exits. Refine then commits the merge so the `Refine Gap:`
trailer lands on a real two-parent merge commit (so the Changes screen
can still Undo it).

If the agent leaves anything unresolved, accidentally aborts the
merge, or runs longer than the configured caps, we fall back to the
spec's original behavior: leave the merge state in place and surface
the conflict for human resolution.
"""
from __future__ import annotations

import json
import os
import signal
import sqlite3
import subprocess
import threading
import time
from pathlib import Path

from refine_server import db
from refine_runtime.manager import ResourceManager
from refine_runtime.resources import ResourceSettings

from . import agent_cli, git_ops
from .chat_mgr import _chat_env


# Caps for the resolver subprocess. Wider than the original 120s/600s:
# real conflicts (e.g. a multi-hunk merge on a single file) routinely
# burn >2 min of thinking time between the initial Read and the first
# Edit, with no stdout events to keep `last_chunk_at` warm. Killing on
# idle there means the resolver never actually edits anything. We
# default to 10 min idle / 30 min hard which is generous for focused
# resolution but still catches a true runaway.
_IDLE_SECONDS = 600.0
_HARD_CAP_SECONDS = 1800.0


def attempt_auto_resolve(
    conn: sqlite3.Connection,
    gap_id: str,
    *,
    branch: str,
    target: str,
    merge_message: str,
    actor: str,
    log: callable,
) -> dict:
    """Returns `{"ok": True}` when the merge is resolved and committed,
    or `{"ok": False, "message": str, "details": str?}` when the
    operator needs to intervene. On failure, the caller should leave
    the merge state intact so the human can resolve manually.

    `log(message, *, severity, category, details=None)` is the existing
    verify_op `_log` partial — keeps every step audit-trailed against
    the Gap's latest round + the global activity feed.
    """
    repo = git_ops.client_repo_path()
    files = git_ops.unmerged_paths()
    if not files:
        # No conflicts to resolve — caller mis-routed us here.
        return {"ok": False, "message":
                "auto-resolve called with no unmerged paths"}

    prompt = _build_prompt(branch=branch, target=target, files=files)

    env = _chat_env()
    spec = agent_cli.get_spec(db.get_setting(conn, "agent_cli"))
    bin_path = agent_cli.resolve_binary(spec, env)
    log(f"Attempting to auto-resolve merge conflict in {len(files)} "
        f"file{'' if len(files) == 1 else 's'} via {spec.display_name}...",
        severity="info", category="git")
    try:
        manager = ResourceManager(ResourceSettings.from_settings(db.list_settings(conn)))
        proc = manager.popen(
            spec.agent_args(bin_path, prompt, cwd=repo),
            cwd=repo,
            env=env,
            kind="conflict-resolver",
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except (OSError, FileNotFoundError) as e:
        return {"ok": False,
                "message": f"could not launch {spec.binary} for auto-resolve: {e}"}

    _stream_and_supervise(proc, log, output_format=spec.output_format)

    return _finalize(merge_message=merge_message, log=log)


# ---- internals --------------------------------------------------------------


def _build_prompt(*, branch: str, target: str, files: list[str]) -> str:
    file_list = "\n".join(f"  - {f}" for f in files)
    return f"""You are resolving a git merge conflict in this repository.

A merge is in progress: branch `{branch}` is being merged into `{target}`.
The following files have unresolved conflict markers:

{file_list}

For EACH file in the list above:
  1. Read the file and locate the `<<<<<<<`, `=======`, `>>>>>>>` markers.
  2. Decide the correct merged content by considering both sides:
     - `{branch}` represents an implemented Gap that the team has reviewed
       and approved — its behavior changes should be preserved.
     - `{target}` represents the current state of the project.
     - Default to integrating BOTH changes. If they conflict semantically,
       prefer the incoming branch's intent (it's the resolved Gap).
  3. Replace the entire conflict block (`<<<<<<<` through `>>>>>>>`,
     inclusive) with the resolved content.
  4. Run `git add <file>` to stage the resolved file.

When every file in the list above has its conflicts resolved AND staged,
your task is complete. Print a one-line confirmation and exit.

CRITICAL CONSTRAINTS — failure to follow these will be flagged:
  - Do NOT run `git commit`. Refine will commit after you finish so the
    merge commit message carries the right `Refine Gap:` trailer.
  - Do NOT run `git merge --abort`, `git reset`, or `git checkout --` on
    any of these files. Those discard the merge.
  - Do NOT modify files that aren't in the list above.
  - Do NOT introduce new files.
  - If a conflict is genuinely unresolvable (the two sides contradict each
    other and you can't pick a sensible merge), stop and explain — leave
    the file with markers in place.
"""


def _stream_and_supervise(proc: subprocess.Popen, log, *,
                          output_format: str) -> None:
    """Read structured output from the agent, log assistant text + tool calls,
    and SIGTERM the pgroup on idle/hard-cap. Returns when the proc has
    exited (cleanly or by our hand)."""
    started = time.monotonic()
    last_output = {"t": started}
    done = threading.Event()

    def drain() -> None:
        try:
            assert proc.stdout is not None
            for raw in proc.stdout:
                last_output["t"] = time.monotonic()
                line = raw.rstrip("\n")
                if not line:
                    continue
                try:
                    evt = json.loads(line)
                except json.JSONDecodeError:
                    log(f"[auto-resolve] {line[:200]}",
                        severity="info", category="git")
                    continue
                for entry in _summarize_event(evt, output_format=output_format):
                    log(entry, severity="info", category="git")
        finally:
            done.set()

    t = threading.Thread(target=drain, daemon=True)
    t.start()

    killed: str | None = None
    while proc.poll() is None:
        now = time.monotonic()
        if now - started > _HARD_CAP_SECONDS:
            killed = "hard_cap"
            break
        if now - last_output["t"] > _IDLE_SECONDS:
            killed = "idle"
            break
        if done.wait(timeout=2.0):
            break

    if killed:
        log(f"Auto-resolve agent killed: {killed}",
            severity="warn", category="git")
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
    try:
        proc.wait(timeout=5.0)
    except subprocess.TimeoutExpired:
        pass
    t.join(timeout=2.0)


def _summarize_event(evt: dict, *, output_format: str) -> list[str]:
    """One-line summaries for the gap.json log — same shape as the
    Gap-runner's stream-json translator, but trimmed for our scope."""
    if not isinstance(evt, dict):
        return []
    if output_format == "codex_json":
        from .subprocess_mgr import _summarize_codex_event
        return [f"[auto-resolve] {s[:200]}" for s in _summarize_codex_event(evt)]

    t = evt.get("type")
    if t == "assistant":
        msg = evt.get("message") or {}
        out: list[str] = []
        for block in msg.get("content") or []:
            bt = block.get("type")
            if bt == "text":
                text = (block.get("text") or "").strip()
                if text:
                    out.append(f"[auto-resolve] {text[:200]}")
            elif bt == "tool_use":
                name = block.get("name") or "tool"
                inp = block.get("input") or {}
                out.append(f"[auto-resolve] [{name}] "
                           f"{_short_tool_input(name, inp)}")
        return out
    if t == "result" and evt.get("is_error"):
        err = evt.get("error") or evt.get("result") or "error"
        return [f"[auto-resolve] result error: {str(err)[:200]}"]
    return []


def _short_tool_input(name: str, inp: dict) -> str:
    if not isinstance(inp, dict):
        return ""
    if name == "Bash":
        cmd = (inp.get("command") or "").splitlines()
        return cmd[0][:160] if cmd else "(empty)"
    if name in ("Read", "Edit", "Write"):
        return inp.get("file_path") or "(?)"
    return ""


def _finalize(*, merge_message: str, log) -> dict:
    """Inspect the post-agent state and either commit the merge or
    return a failure with enough detail to surface in the round log."""
    remaining = git_ops.unmerged_paths()
    if remaining:
        msg = (f"Auto-resolve left {len(remaining)} file"
               f"{'' if len(remaining) == 1 else 's'} unresolved — "
               f"falling back to human resolution")
        log(msg, severity="error", category="git",
            details="\n".join(remaining))
        return {"ok": False, "message": msg,
                "details": "\n".join(remaining)}

    op = git_ops.in_progress_op()
    if op and op[0] == "merge":
        cr = git_ops.commit_pending_merge(merge_message)
        if not cr.ok:
            msg = ("Auto-resolve cleared all conflicts but the commit "
                   "failed")
            log(msg, severity="error", category="git", details=cr.stderr)
            return {"ok": False, "message": msg, "details": cr.stderr}
        log("Auto-resolved and committed the merge",
            severity="info", category="git")
        return {"ok": True}

    # MERGE_HEAD gone and no unmerged paths — the agent either committed
    # itself (despite our instructions) or aborted/reset. Check HEAD.
    parents = git_ops.head_parents()
    if len(parents) >= 2:
        log("Auto-resolve agent committed the merge itself — accepting",
            severity="warn", category="git")
        return {"ok": True}
    msg = ("Auto-resolve dropped the merge state without producing a "
           "merge commit — likely an `abort` or `reset`. Falling back to "
           "human resolution; refine will re-attempt the merge on the "
           "next Verify.")
    log(msg, severity="error", category="git")
    return {"ok": False, "message": msg}
