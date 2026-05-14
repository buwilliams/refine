"""Target-application management — one-shot agent invocations.

The operator writes plain-language prompts that, when sent to a
Standalone Claude Code agent in the client repo, bring the target
application up or take it down. Refine doesn't own the running
process — the prompt does (e.g. it should background the app with
`nohup … &` or hand off to systemd so the process survives the
agent's exit). Refine learns whether the app is alive by polling a
configured health URL.

Three call shapes:

  - `run_instructions(prompt)` — fire a Standalone agent with the
    operator's start/stop prompt. Long-running (agents typically
    take a minute or two to rebuild + relaunch). Stdout/stderr are
    captured and returned for the activity feed.

  - `generate_instructions(kind)` — ask the agent to analyse the
    repo and *write* a start or stop prompt. Returns the generated
    text so the webapp can store it as a setting (the operator
    reviews and tweaks before saving).

  - `http_health(url, timeout)` — issue a single GET against the
    configured health URL and report ok/not-ok.

Shares env/PATH plumbing with chat_mgr so the agent uses the same
OAuth login as the operator's interactive `claude` does.
"""
from __future__ import annotations

import subprocess
import urllib.error
import urllib.request
from typing import Any

from refine_shared import db

from . import git_ops
from .agent_cli import get_spec, resolve_binary
from .chat_mgr import _chat_env, _resolve_claude


_GENERATE_PROMPT = """\
Analyse this codebase and write a plain-language instruction prompt
that another autonomous agent can follow to {action} the application
locally.

Constraints on the prompt you produce:
  - Address it to "you" (the autonomous agent that will read it).
  - Be specific about the actual commands to run — discover them by
    reading package.json scripts, Makefile targets, pyproject.toml,
    Dockerfile, README, etc.{extra}
  - The agent runs in the repo root with no interactive TTY. It must
    not block — use backgrounding (e.g. `nohup <cmd> >/tmp/app.log 2>&1 &`)
    or detach via systemd / a process manager that's already configured.
  - Cover the {action} steps end-to-end{rebuild_clause}.

Output ONLY the prompt text — no preamble, no markdown fences, no
commentary. The output will be passed verbatim to another agent.
"""

_START_EXTRA = " Identify install/build commands and run them first if dependencies might be stale."
_STOP_EXTRA = " Identify the running process by port, PID file, or process name."
_REBUILD_START = " (install/rebuild if needed, then start in the background, then verify it's listening)"
_REBUILD_STOP = " (find the running process, terminate it gracefully, confirm it's gone)"


def _agent_command(prompt: str) -> tuple[list[str], dict[str, str]]:
    """Build the `claude --print …` argv for a one-shot Standalone agent run.

    Mirrors the env scrubbing chat_mgr does so the subprocess uses the
    operator's interactive OAuth login, not whatever env vars happen to
    be inherited from the runner process.
    """
    env = _chat_env()
    claude = _resolve_claude(env)
    # --dangerously-skip-permissions is required so the agent can run
    # the shell commands the prompt describes without an interactive
    # confirmation step (no TTY in this code path).
    args = [claude, "--print", "--dangerously-skip-permissions", prompt]
    return args, env


def run_instructions(prompt: str, *, timeout: float = 600.0) -> dict[str, Any]:
    """Run a start/stop prompt as a Standalone agent in the client repo.

    Returns `{ok, stdout, stderr, exit_code, message}`. `ok` is True iff
    the subprocess exited 0; the caller decides what to do with the
    transcript (logging, transition state, etc.).
    """
    prompt = (prompt or "").strip()
    if not prompt:
        return {"ok": False, "stdout": "", "stderr": "",
                "exit_code": None,
                "message": "No instructions configured."}
    args, env = _agent_command(prompt)
    cwd = git_ops.client_repo_path()
    try:
        out = subprocess.run(
            args, capture_output=True, text=True,
            timeout=timeout, env=env, cwd=str(cwd),
        )
    except subprocess.TimeoutExpired as e:
        return {"ok": False, "stdout": (e.stdout or ""),
                "stderr": (e.stderr or ""), "exit_code": None,
                "message": f"agent timed out after {int(timeout)}s"}
    except (OSError, FileNotFoundError) as e:
        return {"ok": False, "stdout": "", "stderr": str(e),
                "exit_code": None,
                "message": f"could not launch agent: {e}"}
    ok = out.returncode == 0
    msg = "completed" if ok else (
        (out.stderr or "").strip().splitlines()[-1] if out.stderr
        else f"agent exited {out.returncode}"
    )
    return {
        "ok": ok,
        "stdout": out.stdout or "",
        "stderr": out.stderr or "",
        "exit_code": out.returncode,
        "message": msg,
    }


def generate_instructions(kind: str, *, timeout: float = 300.0) -> dict[str, Any]:
    """Use the agent to write a start or stop prompt based on the codebase.

    Returns `{ok, text, message}`. On success, `text` holds the prompt
    body for the operator to review and save.
    """
    if kind not in ("start", "stop"):
        return {"ok": False, "text": "", "message": f"unknown kind: {kind!r}"}
    action = "start" if kind == "start" else "stop"
    extra = _START_EXTRA if kind == "start" else _STOP_EXTRA
    rebuild = _REBUILD_START if kind == "start" else _REBUILD_STOP
    meta_prompt = _GENERATE_PROMPT.format(
        action=action, extra=extra, rebuild_clause=rebuild,
    )
    result = run_instructions(meta_prompt, timeout=timeout)
    if not result["ok"]:
        return {"ok": False, "text": "", "message": result["message"]}
    # Strip leading/trailing whitespace + accidental ``` fences.
    text = (result["stdout"] or "").strip()
    if text.startswith("```"):
        # Drop the first fence line and any trailing fence line.
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    if not text:
        return {"ok": False, "text": "",
                "message": "agent produced no instructions"}
    return {"ok": True, "text": text, "message": "generated"}


def http_health(url: str, *, timeout: float = 5.0) -> dict[str, Any]:
    """Issue a single GET against `url` and return `{ok, status, message}`.

    Any 2xx is success. Network errors, timeouts, or 4xx/5xx are
    failures. The caller logs the message; we don't write to SQLite.
    """
    url = (url or "").strip()
    if not url:
        return {"ok": False, "status": None, "message": "no health URL configured"}
    req = urllib.request.Request(url, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            code = getattr(resp, "status", resp.getcode())
            ok = 200 <= code < 300
            return {
                "ok": ok, "status": code,
                "message": f"HTTP {code}" if ok else f"HTTP {code} (not 2xx)",
            }
    except urllib.error.HTTPError as e:
        return {"ok": False, "status": e.code,
                "message": f"HTTP {e.code}"}
    except urllib.error.URLError as e:
        return {"ok": False, "status": None,
                "message": f"unreachable: {e.reason}"}
    except (TimeoutError, OSError) as e:
        return {"ok": False, "status": None,
                "message": f"health check error: {e}"}
