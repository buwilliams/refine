"""Target-application management.

The AI agent is used to discover a structured configuration for the
client app. Runtime start/stop/status is deterministic: the runner
executes saved shell commands on the host and evaluates configured
checks.
"""
from __future__ import annotations

import json
import os
import socket
import subprocess
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from . import git_ops
from .agent_cli import get_spec, resolve_binary
from .chat_mgr import _chat_env, _user_login_path


_TAIL_LIMIT = 8000

_GENERATE_PROMPT = """\
Analyse this codebase and produce target-application management
configuration for Refine.

Refine will NOT send prose to an agent at runtime. It will run the
commands you provide directly in a non-interactive shell from the
configured working directory.

Return ONLY a JSON object with these keys:
{
  "start_command": "one-line shell command that starts the app and returns promptly",
  "stop_command": "one-line shell command that stops the app and returns promptly",
  "status_command": "one-line shell command; exit 0 only when the app is healthy/running",
  "cwd": "repo-relative working directory, or empty string for repo root",
  "env": {"NAME": "value"},
  "start_timeout_seconds": 120,
  "stop_timeout_seconds": 60,
  "status_timeout_seconds": 10,
  "log_path": "optional repo-relative or absolute log path",
  "http_check_url": "optional URL for web apps",
  "tcp_check_host": "optional host for TCP checks",
  "tcp_check_port": "optional port for TCP checks",
  "process_check_command": "optional one-line shell command; exit 0 when expected process exists",
  "notes": "short warnings or rationale for the operator"
}

Rules:
- Commands must be single-line CLI commands, not numbered lists.
- Prefer project-native commands discovered from package.json scripts,
  Makefile targets, pyproject.toml, Dockerfile, README, compose files,
  Procfile, or similar sources.
- The start command must not block forever. Use an existing process
  manager, docker compose detach mode, or backgrounding with logging.
- The stop command must be idempotent when practical.
- The status command is required unless no reliable CLI check exists.
- Use optional HTTP/TCP/process checks only when they add confidence.
- Do not include markdown, comments, or prose outside the JSON object.
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
        "status_command": (settings.get("target_app_status_command") or "").strip(),
        "cwd": (settings.get("target_app_cwd") or "").strip(),
        "env": {str(k): str(v) for k, v in env_obj.items()},
        "start_timeout_seconds": _int_setting(settings, "target_app_start_timeout_seconds", 120),
        "stop_timeout_seconds": _int_setting(settings, "target_app_stop_timeout_seconds", 60),
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
    }


def _int_setting(settings: dict[str, str], key: str, default: int) -> int:
    try:
        n = int(settings.get(key) or default)
    except (TypeError, ValueError):
        return default
    return max(1, min(n, 3600))


def run_operation(kind: str, config: dict[str, Any]) -> dict[str, Any]:
    """Run start/stop/status and any configured verification checks."""
    started_at = _now_iso()
    if kind not in ("start", "stop", "status"):
        return {"ok": False, "message": f"unknown operation: {kind}"}
    command = (config.get(f"{kind}_command") or "").strip()
    if kind == "status":
        checks = run_checks(config)
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
        return {"ok": False, "kind": kind, "state": "failed",
                "started_at": started_at, "finished_at": _now_iso(),
                "message": f"No {kind} command configured."}

    timeout = int(config.get(f"{kind}_timeout_seconds") or 60)
    cmd_result = run_command(command, config=config, timeout=timeout)
    if not cmd_result["ok"]:
        return {
            **cmd_result, "kind": kind, "state": "failed",
            "started_at": started_at, "finished_at": _now_iso(),
            "message": cmd_result["message"],
            "checks": [],
        }

    checks = run_checks(config)
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


def run_checks(config: dict[str, Any]) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []
    status_command = (config.get("status_command") or "").strip()
    if status_command:
        res = run_command(
            status_command, config=config,
            timeout=int(config.get("status_timeout_seconds") or 10),
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
    return state


def run_command(command: str, *, config: dict[str, Any],
                timeout: int) -> dict[str, Any]:
    cwd = resolve_cwd(config.get("cwd") or "")
    env = _command_env(config.get("env") if isinstance(config.get("env"), dict) else {})
    try:
        out = subprocess.run(
            ["bash", "-lc", command],
            cwd=str(cwd),
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
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
    stderr = out.stderr or ""
    stdout = out.stdout or ""
    msg = "completed" if out.returncode == 0 else (
        stderr.strip().splitlines()[-1] if stderr.strip()
        else f"command exited {out.returncode}"
    )
    return {
        "ok": out.returncode == 0,
        "command": command,
        "cwd": str(cwd),
        "exit_code": out.returncode,
        "stdout_tail": _tail(stdout),
        "stderr_tail": _tail(stderr),
        "message": msg,
    }


def resolve_cwd(cwd_setting: str) -> Path:
    root = git_ops.client_repo_path()
    cwd = (cwd_setting or "").strip()
    if not cwd:
        return root
    p = Path(cwd)
    return p if p.is_absolute() else root / p


def _command_env(overrides: dict[str, str]) -> dict[str, str]:
    env = os.environ.copy()
    login_path = _user_login_path()
    if login_path:
        env["PATH"] = login_path
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
        return {"ok": False, "config": {}, "message": f"agent timed out after {int(timeout)}s",
                "raw": (e.stdout or "")}
    except (OSError, FileNotFoundError) as e:
        return {"ok": False, "config": {}, "message": f"could not launch agent: {e}",
                "raw": ""}
    if out.returncode != 0:
        return {"ok": False, "config": {}, "message": (
            (out.stderr or "").strip().splitlines()[-1]
            if (out.stderr or "").strip() else f"agent exited {out.returncode}"
        ), "raw": (out.stdout or out.stderr or "")}
    raw = _last_agent_text(out.stdout or "").strip()
    parsed = _parse_json_object(raw)
    if parsed is None:
        return {"ok": False, "config": {}, "message": "agent did not return a JSON object",
                "raw": raw}
    config = normalize_generated_config(parsed)
    return {"ok": True, "config": config, "message": "generated", "raw": raw}


def normalize_generated_config(obj: dict[str, Any]) -> dict[str, Any]:
    env = obj.get("env") if isinstance(obj.get("env"), dict) else {}
    return {
        "start_command": _one_line(obj.get("start_command") or ""),
        "stop_command": _one_line(obj.get("stop_command") or ""),
        "status_command": _one_line(obj.get("status_command") or ""),
        "cwd": str(obj.get("cwd") or "").strip(),
        "env": {str(k): str(v) for k, v in env.items()},
        "start_timeout_seconds": _positive_int(obj.get("start_timeout_seconds"), 120),
        "stop_timeout_seconds": _positive_int(obj.get("stop_timeout_seconds"), 60),
        "status_timeout_seconds": _positive_int(obj.get("status_timeout_seconds"), 10),
        "log_path": str(obj.get("log_path") or "").strip(),
        "http_check_url": str(obj.get("http_check_url") or "").strip(),
        "tcp_check_host": str(obj.get("tcp_check_host") or "").strip(),
        "tcp_check_port": str(obj.get("tcp_check_port") or "").strip(),
        "process_check_command": _one_line(obj.get("process_check_command") or ""),
        "notes": str(obj.get("notes") or "").strip(),
    }


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
    """Extract final assistant text from Codex JSONL, or return plain stdout."""
    if not stdout.lstrip().startswith("{"):
        return stdout
    last = ""
    for line in stdout.splitlines():
        try:
            evt = json.loads(line)
        except json.JSONDecodeError:
            continue
        item = evt.get("item") if isinstance(evt.get("item"), dict) else {}
        text = item.get("text") or evt.get("text")
        typ = item.get("type") or evt.get("type")
        if text and typ in ("agent_message", "assistant_message",
                            "item.completed"):
            last = str(text)
    return last or stdout


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
