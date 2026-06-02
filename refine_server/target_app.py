"""Target-application management.

The AI agent is used to discover a structured configuration for the
client app. Runtime start/stop/status is deterministic: the runner
executes saved shell commands on the host and evaluates configured
checks.
"""
from __future__ import annotations

import json
import os
import shutil
import socket
import subprocess
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from . import git_ops, perf_metrics
from .agent_cli import extract_final_text, get_spec, resolve_binary
from .chat_mgr import _chat_env, _merge_paths, _user_login_path
from refine_runtime.supervised_process import supervised_popen


_TAIL_LIMIT = 8000

# Refine writes the generated bodies into this script (repo-relative) and the
# saved start/stop/rebuild/status commands all invoke it. This gives every
# target app a single, consistent management entry point that operators can
# read and edit.
MANAGE_SCRIPT_RELPATH = ".refine/manage-app.sh"
MANAGE_COMMANDS = {
    "start_command": "./.refine/manage-app.sh start",
    "stop_command": "./.refine/manage-app.sh stop",
    "rebuild_command": "./.refine/manage-app.sh rebuild",
    "status_command": "./.refine/manage-app.sh status",
}

_GENERATE_PROMPT = """\
Analyse this codebase and design how to start, stop, rebuild, and check the
status of THIS application from the command line.

Refine will write your answer into a shell script at `.refine/manage-app.sh`
and then run `./.refine/manage-app.sh start|stop|rebuild|status` directly in a
non-interactive bash shell from the repository root. You only design the body
of each command — Refine adds the `#!/usr/bin/env bash` header, timestamped
STDOUT logging, repo-root path resolution, and the start|stop|rebuild|status
dispatch around your snippets.

Determine the best management approach by inspecting the repo. Prefer whatever
this project actually uses; the most common stacks are:
- Docker / docker compose (Dockerfile, compose.yaml, docker-compose.yml)
- Node / npm / pnpm / yarn (package.json scripts)
- Python / uv / pip / poetry (pyproject.toml, requirements.txt, manage.py)
- Make / Procfile / framework CLIs (Django, Rails, Go, etc.)

Return ONLY a JSON object with these keys:
{
  "summary": "one short line naming the detected stack and chosen approach",
  "helpers": "optional bash run once before the command: shared variables or functions (e.g. PORT=3000; PIDFILE=.refine/run/app.pid). Empty string if none.",
  "start": "bash that starts the app and returns promptly; background long-running servers and redirect their logs, do not block",
  "stop": "bash that stops the app and is idempotent (succeeds even if nothing is running)",
  "rebuild": "bash that rebuilds or prepares generated artifacts for review without starting a long-running dev server",
  "status": "bash that exits 0 only when the app is running/healthy and non-zero otherwise",
  "env": {"NAME": "value"},
  "start_timeout_seconds": 120,
  "stop_timeout_seconds": 60,
  "rebuild_timeout_seconds": 300,
  "status_timeout_seconds": 10,
  "http_check_url": "optional URL for web apps, used as an extra health probe",
  "tcp_check_host": "optional host for an extra TCP probe",
  "tcp_check_port": "optional port for an extra TCP probe",
  "notes": "short warnings or rationale for the operator"
}

Rules for the bash snippets:
- Plain bash that runs under `set -uo pipefail`. Multiple lines are fine.
- Log meaningful progress with echo/printf so failures are debuggable; Refine
  already timestamps and captures STDOUT.
- Prefer project-native commands discovered from package.json scripts, Makefile
  targets, pyproject.toml, Dockerfile, compose files, Procfile, or the README.
- For `start`, never block forever: use a process manager, `docker compose up -d`,
  or background the process (e.g. `nohup ... > .refine/run/app.log 2>&1 &`).
- Reference paths relative to the repository root.
- Do not add a shebang and do not redefine the start|stop|rebuild|status
  dispatch — only provide the command bodies.
- Output the JSON object only: no markdown fences, comments, or prose around it.
"""


def config_from_settings(settings: dict[str, str]) -> dict[str, Any]:
    """Normalize target-app settings into a runtime config dict."""
    env_raw = (settings.get("target_app_env_json") or "{}").strip() or "{}"
    try:
        env_obj = json.loads(env_raw)
    except json.JSONDecodeError:
        env_obj = {}
    if not isinstance(env_obj, dict):
        env_obj = {}
    return {
        "start_command": (settings.get("target_app_start_command") or "").strip(),
        "stop_command": (settings.get("target_app_stop_command") or "").strip(),
        "rebuild_command": (settings.get("target_app_rebuild_command") or "").strip(),
        "status_command": (settings.get("target_app_status_command") or "").strip(),
        "cwd": (settings.get("target_app_cwd") or "").strip(),
        "env": {str(k): str(v) for k, v in env_obj.items()},
        "start_timeout_seconds": _int_setting(settings, "target_app_start_timeout_seconds", 120),
        "stop_timeout_seconds": _int_setting(settings, "target_app_stop_timeout_seconds", 60),
        "rebuild_timeout_seconds": _int_setting(settings, "target_app_rebuild_timeout_seconds", 300),
        "status_timeout_seconds": _int_setting(settings, "target_app_status_timeout_seconds", 10),
        "log_path": (settings.get("target_app_log_path") or "").strip(),
        "http_check_url": (
            settings.get("target_app_http_check_url")
            or settings.get("target_app_health_url")
            or ""
        ).strip(),
        "tcp_check_host": (settings.get("target_app_tcp_check_host") or "").strip(),
        "tcp_check_port": (settings.get("target_app_tcp_check_port") or "").strip(),
        "process_check_command": (settings.get("target_app_process_check_command") or "").strip(),
        "legacy_start_instructions": (settings.get("target_app_start_instructions") or "").strip(),
        "legacy_stop_instructions": (settings.get("target_app_stop_instructions") or "").strip(),
        "root": "",
    }


def _int_setting(settings: dict[str, str], key: str, default: int) -> int:
    try:
        n = int(settings.get(key) or default)
    except (TypeError, ValueError):
        return default
    return max(1, min(n, 3600))


def run_operation(
    kind: str,
    config: dict[str, Any],
    *,
    cancel_event: Any | None = None,
) -> dict[str, Any]:
    """Run start/stop/status and any configured verification checks."""
    started_at = _now_iso()
    if kind not in ("start", "stop", "rebuild", "status"):
        return {"ok": False, "message": f"unknown operation: {kind}"}
    command = (config.get(f"{kind}_command") or "").strip()
    if kind == "status":
        checks = run_checks(config, cancel_event=cancel_event)
        state = state_from_checks(checks)
        return {
            "ok": checks["configured"] and state == "running",
            "kind": kind,
            "state": state,
            "message": checks["message"],
            "started_at": started_at,
            "finished_at": _now_iso(),
            "checks_configured": checks["configured"],
            "checks": checks["checks"],
        }
    if not command:
        return noop_operation(kind, started_at=started_at)

    timeout = int(config.get(f"{kind}_timeout_seconds") or 60)
    cmd_result = run_command(
        command, config=config, timeout=timeout, cancel_event=cancel_event,
    )
    if not cmd_result["ok"]:
        return {
            **cmd_result, "kind": kind, "state": "failed",
            "started_at": started_at, "finished_at": _now_iso(),
            "message": cmd_result["message"],
            "checks": [],
        }

    checks = wait_for_lifecycle_checks(
        kind, config, timeout=timeout, cancel_event=cancel_event,
    )
    if checks.get("cancelled"):
        return {
            **cmd_result,
            "kind": kind,
            "state": "failed",
            "ok": False,
            "message": checks["message"],
            "started_at": started_at,
            "finished_at": _now_iso(),
            "checks_configured": False,
            "checks": [],
            "cancelled": True,
        }
    state = state_after_lifecycle(kind, checks)
    ok = state in ("running", "stopped", "unknown")
    msg = checks["message"] if checks["configured"] else f"{kind} command completed"
    if state == "degraded":
        ok = False
    return {
        **cmd_result,
        "kind": kind,
        "state": state,
        "ok": ok,
        "message": msg,
        "started_at": started_at,
        "finished_at": _now_iso(),
        "checks_configured": checks["configured"],
        "checks": checks["checks"],
    }


def noop_operation(kind: str, *, started_at: str | None = None) -> dict[str, Any]:
    started = started_at or _now_iso()
    return {
        "ok": True,
        "noop": True,
        "kind": kind,
        "state": "unknown",
        "command": "",
        "cwd": "",
        "exit_code": 0,
        "stdout_tail": "",
        "stderr_tail": "",
        "started_at": started,
        "finished_at": _now_iso(),
        "checks_configured": False,
        "checks": [],
        "message": f"No {kind} command configured; {kind} treated as a no-op.",
    }


def wait_for_lifecycle_checks(
    kind: str,
    config: dict[str, Any],
    timeout: int,
    *,
    cancel_event: Any | None = None,
) -> dict[str, Any]:
    """Poll checks after start/stop until the requested lifecycle state settles."""
    deadline = time.monotonic() + max(1, timeout)
    if _cancel_requested(cancel_event):
        return _cancelled_checks()
    last = run_checks(config, cancel_event=cancel_event)
    if not last["configured"]:
        return last
    if kind not in ("start", "stop"):
        return last
    while True:
        state = state_after_lifecycle(kind, last)
        if (kind == "start" and state == "running") or (
            kind == "stop" and state == "stopped"
        ):
            return last
        if time.monotonic() >= deadline:
            return last
        time.sleep(0.5)
        if _cancel_requested(cancel_event):
            return _cancelled_checks()
        last = run_checks(config, cancel_event=cancel_event)


def run_checks(config: dict[str, Any], *, cancel_event: Any | None = None) -> dict[str, Any]:
    if _cancel_requested(cancel_event):
        return _cancelled_checks()
    checks: list[dict[str, Any]] = []
    status_command = (config.get("status_command") or "").strip()
    if status_command:
        res = run_command(
            status_command, config=config,
            timeout=int(config.get("status_timeout_seconds") or 10),
            cancel_event=cancel_event,
        )
        checks.append({
            "type": "command", "label": "Status command",
            "ok": bool(res["ok"]), "message": res["message"],
            "exit_code": res.get("exit_code"),
            "stdout_tail": res.get("stdout_tail", ""),
            "stderr_tail": res.get("stderr_tail", ""),
        })

    http_url = (config.get("http_check_url") or "").strip()
    if http_url:
        checks.append({"type": "http", "label": "HTTP check",
                       **http_health(http_url, timeout=5.0)})

    host = (config.get("tcp_check_host") or "").strip()
    port = (config.get("tcp_check_port") or "").strip()
    if host and port:
        checks.append({"type": "tcp", "label": "TCP check",
                       **tcp_health(host, port, timeout=5.0)})

    proc_command = (config.get("process_check_command") or "").strip()
    if proc_command:
        res = run_command(
            proc_command, config=config,
            timeout=int(config.get("status_timeout_seconds") or 10),
            cancel_event=cancel_event,
        )
        checks.append({
            "type": "process", "label": "Process check",
            "ok": bool(res["ok"]), "message": res["message"],
            "exit_code": res.get("exit_code"),
            "stdout_tail": res.get("stdout_tail", ""),
            "stderr_tail": res.get("stderr_tail", ""),
        })

    if not checks:
        return {"configured": False, "ok": False, "checks": [],
                "message": "No status checks configured."}
    passed = sum(1 for c in checks if c.get("ok"))
    return {
        "configured": True,
        "ok": passed == len(checks),
        "checks": checks,
        "message": f"{passed}/{len(checks)} checks passed",
    }


def state_from_checks(checks: dict[str, Any]) -> str:
    if not checks.get("configured"):
        return "unknown"
    results = checks.get("checks") or []
    passed = sum(1 for c in results if c.get("ok"))
    if passed == len(results):
        return "running"
    if passed > 0:
        return "degraded"
    return "stopped"


def state_after_lifecycle(kind: str, checks: dict[str, Any]) -> str:
    if not checks.get("configured"):
        return "running" if kind == "start" else "stopped"
    state = state_from_checks(checks)
    if kind == "start":
        return "running" if state == "running" else "degraded"
    if kind == "stop":
        return "stopped" if state == "stopped" else "degraded"
    if kind == "rebuild":
        return state
    return state


def run_command(command: str, *, config: dict[str, Any],
                timeout: int, cancel_event: Any | None = None) -> dict[str, Any]:
    cwd = resolve_cwd_for(config, config.get("cwd") or "")
    env = _command_env(config.get("env") if isinstance(config.get("env"), dict) else {})
    if _cancel_requested(cancel_event):
        return _cancelled_command(command, str(cwd))
    try:
        shell = _shell_path()
        proc = supervised_popen(
            [shell, "-lc", command],
            cwd=str(cwd),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            kind="target-app",
            fallback_manager=_DirectProcessManager(),
        )
        deadline = time.monotonic() + max(1, timeout)
        while True:
            if _cancel_requested(cancel_event):
                stdout, stderr = _terminate_command(proc)
                return {
                    "ok": False, "command": command, "cwd": str(cwd),
                    "exit_code": None,
                    "stdout_tail": _tail(stdout or ""),
                    "stderr_tail": _tail(stderr or ""),
                    "message": "command cancelled",
                    "cancelled": True,
                }
            wait_for = max(0.0, min(0.2, deadline - time.monotonic()))
            if wait_for <= 0:
                stdout, stderr = _terminate_command(proc)
                return {
                    "ok": False, "command": command, "cwd": str(cwd),
                    "exit_code": None,
                    "stdout_tail": _tail(stdout or ""),
                    "stderr_tail": _tail(stderr or ""),
                    "message": f"command timed out after {timeout}s",
                }
            try:
                stdout, stderr = proc.communicate(timeout=wait_for)
                break
            except subprocess.TimeoutExpired:
                continue
    except subprocess.TimeoutExpired as e:
        return {
            "ok": False, "command": command, "cwd": str(cwd),
            "exit_code": None,
            "stdout_tail": _tail(e.stdout or ""),
            "stderr_tail": _tail(e.stderr or ""),
            "message": f"command timed out after {timeout}s",
        }
    except (OSError, FileNotFoundError) as e:
        return {
            "ok": False, "command": command, "cwd": str(cwd),
            "exit_code": None, "stdout_tail": "", "stderr_tail": str(e),
            "message": f"could not launch command: {e}",
        }
    stderr = stderr or ""
    stdout = stdout or ""
    msg = "completed" if proc.returncode == 0 else (
        stderr.strip().splitlines()[-1] if stderr.strip()
        else f"command exited {proc.returncode}"
    )
    return {
        "ok": proc.returncode == 0,
        "command": command,
        "cwd": str(cwd),
        "exit_code": proc.returncode,
        "stdout_tail": _tail(stdout),
        "stderr_tail": _tail(stderr),
        "message": msg,
    }


def _cancel_requested(cancel_event: Any | None) -> bool:
    return bool(cancel_event is not None and cancel_event.is_set())


def _cancelled_checks() -> dict[str, Any]:
    return {
        "configured": False,
        "ok": False,
        "checks": [],
        "message": "operation cancelled",
        "cancelled": True,
    }


def _cancelled_command(command: str, cwd: str) -> dict[str, Any]:
    return {
        "ok": False,
        "command": command,
        "cwd": cwd,
        "exit_code": None,
        "stdout_tail": "",
        "stderr_tail": "",
        "message": "command cancelled",
        "cancelled": True,
    }


def _terminate_command(proc: subprocess.Popen) -> tuple[str, str]:
    try:
        if proc.poll() is None:
            try:
                proc.terminate()
            except (ProcessLookupError, OSError):
                pass
        try:
            stdout, stderr = proc.communicate(timeout=3.0)
        except subprocess.TimeoutExpired:
            if proc.poll() is None:
                try:
                    proc.kill()
                except (ProcessLookupError, OSError):
                    pass
            stdout, stderr = proc.communicate(timeout=1.0)
        return stdout or "", stderr or ""
    except Exception:
        return "", ""


class _DirectProcessManager:
    def popen(self, args, *, cwd, env, kind, stdin, stdout, stderr, text, bufsize):  # noqa: ANN001, ARG002
        return subprocess.Popen(
            list(args),
            cwd=str(cwd),
            env=dict(env),
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            text=text,
            bufsize=bufsize,
            start_new_session=True,
        )


def _shell_path() -> str:
    return shutil.which("bash") or "/bin/bash"


def resolve_cwd(cwd_setting: str) -> Path:
    return resolve_cwd_for({}, cwd_setting)


def resolve_cwd_for(config: dict[str, Any], cwd_setting: str) -> Path:
    root_raw = str(config.get("root") or "").strip()
    root = Path(root_raw) if root_raw else git_ops.client_repo_path()
    cwd = (cwd_setting or "").strip()
    if not cwd:
        return root
    p = Path(cwd)
    return p if p.is_absolute() else root / p


def _command_env(overrides: dict[str, str]) -> dict[str, str]:
    env = os.environ.copy()
    login_path = _user_login_path()
    path = _merge_paths(login_path, env.get("PATH"))
    if path:
        env["PATH"] = path
    for k, v in overrides.items():
        if k:
            env[str(k)] = str(v)
    return env


def _tail(text: str, limit: int = _TAIL_LIMIT) -> str:
    return text[-limit:] if len(text) > limit else text


def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def generate_config(provider: str | None = None,
                    timeout: float = 300.0) -> dict[str, Any]:
    """Ask the selected AI agent to produce structured target-app config."""
    metric_start = perf_metrics.now()
    env = _chat_env()
    spec = get_spec(provider)
    binary = resolve_binary(spec, env)
    cwd = git_ops.client_repo_path()
    args = spec.one_shot_args(
        binary, _GENERATE_PROMPT, cwd=cwd,
        json_output=spec.output_format == "codex_json",
    )
    try:
        out = subprocess.run(
            args, capture_output=True, text=True, timeout=timeout,
            env=env, cwd=str(cwd),
        )
    except subprocess.TimeoutExpired as e:
        perf_metrics.record(
            "ai.target_app_generate",
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=spec.name,
            bytes_in=len(_GENERATE_PROMPT.encode("utf-8", errors="replace")),
            bytes_out=len(str(e.stdout or "").encode("utf-8", errors="replace")),
            details={"error": "timeout", "timeout": timeout},
        )
        return {"ok": False, "config": {}, "message": f"agent timed out after {int(timeout)}s",
                "raw": (e.stdout or "")}
    except (OSError, FileNotFoundError) as e:
        perf_metrics.record(
            "ai.target_app_generate",
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=spec.name,
            bytes_in=len(_GENERATE_PROMPT.encode("utf-8", errors="replace")),
            details={"error": repr(e)[:1000]},
        )
        return {"ok": False, "config": {}, "message": f"could not launch agent: {e}",
                "raw": ""}
    if out.returncode != 0:
        perf_metrics.record(
            "ai.target_app_generate",
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=spec.name,
            bytes_in=len(_GENERATE_PROMPT.encode("utf-8", errors="replace")),
            bytes_out=len(((out.stdout or "") + (out.stderr or "")).encode("utf-8", errors="replace")),
            details={"returncode": out.returncode},
        )
        return {"ok": False, "config": {}, "message": (
            (out.stderr or "").strip().splitlines()[-1]
            if (out.stderr or "").strip() else f"agent exited {out.returncode}"
        ), "raw": (out.stdout or out.stderr or "")}
    raw = _last_agent_text(out.stdout or "").strip()
    parsed = _parse_json_object(raw)
    if parsed is None:
        perf_metrics.record(
            "ai.target_app_generate",
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            success=False,
            provider=spec.name,
            bytes_in=len(_GENERATE_PROMPT.encode("utf-8", errors="replace")),
            bytes_out=len(raw.encode("utf-8", errors="replace")),
            details={"error": "invalid_json"},
        )
        return {"ok": False, "config": {}, "message": "agent did not return a JSON object",
                "raw": raw}
    config = normalize_generated_config(parsed)
    script_rel = MANAGE_SCRIPT_RELPATH
    script_error = ""
    try:
        write_manage_script(build_manage_script(parsed), root=cwd)
    except OSError as e:
        script_error = f"could not write {script_rel}: {e}"
    summary = _one_line(parsed.get("summary") or "")
    lead = (
        script_error
        or f"Wrote {script_rel}" + (f" ({summary})" if summary else "") + "."
    )
    config["notes"] = (f"{lead} {config['notes']}".strip()
                       if config.get("notes") else lead)
    perf_metrics.record(
        "ai.target_app_generate",
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        provider=spec.name,
        bytes_in=len(_GENERATE_PROMPT.encode("utf-8", errors="replace")),
        bytes_out=len(raw.encode("utf-8", errors="replace")),
    )
    return {
        "ok": True,
        "config": config,
        "message": "generated",
        "raw": raw,
        "script_path": script_rel,
    }


def normalize_generated_config(obj: dict[str, Any]) -> dict[str, Any]:
    """Map a generated analysis into saved target-app settings.

    The start/stop/rebuild/status commands always point at the generated
    `.refine/manage-app.sh` wrapper (operators may override them later). The
    working directory is pinned to the repo root so the relative script path
    resolves; the script itself cd's to the repo root regardless.
    """
    env = obj.get("env") if isinstance(obj.get("env"), dict) else {}
    return {
        "start_command": MANAGE_COMMANDS["start_command"],
        "stop_command": MANAGE_COMMANDS["stop_command"],
        "rebuild_command": MANAGE_COMMANDS["rebuild_command"],
        "status_command": MANAGE_COMMANDS["status_command"],
        "cwd": "",
        "env": {str(k): str(v) for k, v in env.items()},
        "start_timeout_seconds": _positive_int(obj.get("start_timeout_seconds"), 120),
        "stop_timeout_seconds": _positive_int(obj.get("stop_timeout_seconds"), 60),
        "rebuild_timeout_seconds": _positive_int(obj.get("rebuild_timeout_seconds"), 300),
        "status_timeout_seconds": _positive_int(obj.get("status_timeout_seconds"), 10),
        "log_path": str(obj.get("log_path") or "").strip(),
        "http_check_url": str(obj.get("http_check_url") or "").strip(),
        "tcp_check_host": str(obj.get("tcp_check_host") or "").strip(),
        "tcp_check_port": str(obj.get("tcp_check_port") or "").strip(),
        # Status is handled by the wrapper's `status` subcommand, so we do not
        # configure a redundant separate process check here.
        "process_check_command": "",
        "notes": str(obj.get("notes") or "").strip(),
    }


_SCRIPT_DEFAULT_BODIES = {
    "start": 'log "no start command was generated; nothing to do"',
    "stop": 'log "no stop command was generated; nothing to do"',
    "rebuild": 'log "no rebuild command was generated; nothing to do"',
    # Default status is conservative: report "not running" rather than claim
    # health we cannot verify.
    "status": ('log "no status command was generated; reporting not running"\n'
               'return 1'),
}


def _indent_block(text: str, prefix: str) -> str:
    lines = str(text or "").splitlines()
    return "\n".join((prefix + line) if line.strip() else line for line in lines)


def build_manage_script(obj: dict[str, Any]) -> str:
    """Assemble the `.refine/manage-app.sh` contents from a generated analysis.

    Refine owns the harness — shebang, timestamped STDOUT logging, repo-root
    resolution, and the start|stop|rebuild|status dispatch — so logging and
    structure are guaranteed regardless of what the agent returns. The agent
    only supplies the per-command bodies and optional shared `helpers`.
    """
    summary = _one_line(obj.get("summary") or "") or "auto-detected"
    helpers = str(obj.get("helpers") or "").strip()
    bodies = {}
    for name in ("start", "stop", "rebuild", "status"):
        snippet = str(obj.get(name) or "").strip() or _SCRIPT_DEFAULT_BODIES[name]
        bodies[name] = _indent_block(snippet, "  ")
    helpers_block = (
        f"# Shared setup from analysis.\n{helpers}"
        if helpers else "# (no shared setup)"
    )
    template = _MANAGE_SCRIPT_TEMPLATE
    return (
        template
        .replace("__SUMMARY__", summary)
        .replace("__HELPERS__", helpers_block)
        .replace("__START__", bodies["start"])
        .replace("__STOP__", bodies["stop"])
        .replace("__REBUILD__", bodies["rebuild"])
        .replace("__STATUS__", bodies["status"])
    )


def write_manage_script(content: str, *, root: Path | None = None) -> Path:
    """Write (overwrite) the manage-app.sh wrapper and mark it executable."""
    base = Path(root) if root is not None else git_ops.client_repo_path()
    path = base / MANAGE_SCRIPT_RELPATH
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    try:
        path.chmod(0o755)
    except OSError:
        pass
    return path


_MANAGE_SCRIPT_TEMPLATE = """\
#!/usr/bin/env bash
#
# .refine/manage-app.sh - generated by Refine "Generate with AI".
#
# A consistent entry point Refine uses to manage the target application:
#
#     ./.refine/manage-app.sh start     # start the app, return promptly
#     ./.refine/manage-app.sh stop      # stop the app (idempotent)
#     ./.refine/manage-app.sh rebuild   # rebuild generated artifacts
#     ./.refine/manage-app.sh status    # exit 0 only when running/healthy
#
# Regenerated whenever you click "Generate with AI". Edit it freely - Refine
# only calls the four subcommands above, and you can override the saved
# commands in Settings > Node > Application.
#
# Detected stack: __SUMMARY__

set -uo pipefail

# --- logging -----------------------------------------------------------------
# Every step logs to STDOUT with a UTC timestamp so Refine's process logs and
# the status panel show exactly what ran and how it exited.
_ts()  { date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date; }
log()  { printf '[manage-app %s] %s\\n' "$(_ts)" "$*"; }
run()  { log "RUN: $*"; "$@"; local rc=$?; log "EXIT ${rc}: $*"; return "$rc"; }

# Resolve paths relative to this script so it works from any working directory.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" >/dev/null 2>&1 && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"
cd "$APP_DIR" || { log "FATAL: cannot cd to $APP_DIR"; exit 1; }

CMD="${1:-}"
log "begin: cmd='${CMD}' app_dir='${APP_DIR}' user='$(id -un 2>/dev/null || echo '?')'"

# --- shared setup (from analysis) --------------------------------------------
__HELPERS__

# --- commands (from analysis) ------------------------------------------------
do_start() {
  log "starting application"
__START__
}

do_stop() {
  log "stopping application"
__STOP__
}

do_rebuild() {
  log "rebuilding application"
__REBUILD__
}

do_status() {
  log "checking application status"
__STATUS__
}

rc=0
case "$CMD" in
  start)   do_start;   rc=$? ;;
  stop)    do_stop;    rc=$? ;;
  rebuild) do_rebuild; rc=$? ;;
  status)  do_status;  rc=$? ;;
  ""|-h|--help|help)
    log "usage: manage-app.sh {start|stop|rebuild|status}"
    rc=2 ;;
  *)
    log "ERROR: unknown command '${CMD}'"
    log "usage: manage-app.sh {start|stop|rebuild|status}"
    rc=2 ;;
esac

log "done: cmd='${CMD}' exit=${rc}"
exit "$rc"
"""


def _one_line(value: Any) -> str:
    return " ".join(str(value or "").strip().splitlines()).strip()


def _positive_int(value: Any, default: int) -> int:
    try:
        n = int(value)
    except (TypeError, ValueError):
        return default
    return max(1, min(n, 3600))


def _parse_json_object(text: str) -> dict[str, Any] | None:
    stripped = text.strip()
    if stripped.startswith("```"):
        lines = stripped.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        stripped = "\n".join(lines).strip()
    try:
        obj = json.loads(stripped)
    except json.JSONDecodeError:
        start = stripped.find("{")
        end = stripped.rfind("}")
        if start < 0 or end <= start:
            return None
        try:
            obj = json.loads(stripped[start:end + 1])
        except json.JSONDecodeError:
            return None
    return obj if isinstance(obj, dict) else None


def _last_agent_text(stdout: str) -> str:
    return extract_final_text(stdout)


def http_health(url: str, *, timeout: float = 5.0) -> dict[str, Any]:
    url = (url or "").strip()
    if not url:
        return {"ok": False, "status": None, "message": "no HTTP URL configured"}
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
        return {"ok": False, "status": e.code, "message": f"HTTP {e.code}"}
    except urllib.error.URLError as e:
        return {"ok": False, "status": None,
                "message": f"unreachable: {e.reason}"}
    except (TimeoutError, OSError) as e:
        return {"ok": False, "status": None,
                "message": f"HTTP check error: {e}"}


def tcp_health(host: str, port: str, *, timeout: float = 5.0) -> dict[str, Any]:
    try:
        port_int = int(port)
    except (TypeError, ValueError):
        return {"ok": False, "message": f"invalid TCP port: {port}"}
    try:
        with socket.create_connection((host, port_int), timeout=timeout):
            pass
    except OSError as e:
        return {"ok": False, "message": f"TCP connect failed: {e}"}
    return {"ok": True, "message": f"TCP {host}:{port_int} reachable"}


# Back-compat wrapper used by old call sites/tests.
def http_health_legacy(url: str, *, timeout: float = 5.0) -> dict[str, Any]:
    return http_health(url, timeout=timeout)
