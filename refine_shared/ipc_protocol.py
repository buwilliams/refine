"""IPC protocol between refine-web (Docker) and refine-runner (host).

Wire format: line-delimited JSON over Unix domain socket. Each request is one
JSON object on its own line; each response is one JSON object on its own line.

Request envelope: {"id": str, "method": str, "params": {...}}
Response envelope: {"id": str, "ok": bool, "result"?: {...}, "error"?: {...}}
Error: {"code": str, "message": str, "details"?: str}

A blank line is ignored. Connection is one-shot for now (simple).
"""
from __future__ import annotations

import os
from typing import Final

DEFAULT_SOCKET_PATH: Final = os.environ.get(
    "REFINE_RUNNER_SOCKET", "/var/run/refine/runner.sock"
)

# Method names
M_PING = "ping"
M_PREFLIGHT = "preflight"                # runner runs claude pre-flight
M_LAUNCH = "launch"                       # launch a CLI subprocess for a Gap's latest round
M_CANCEL = "cancel"                       # kill a Gap's running subprocess
M_VERIFY = "verify"                       # run merge+push for a Gap (review→done)
M_CREATE_GAP = "create_gap"               # webapp asks runner to create gap.json (writer ownership)
M_APPEND_ROUND = "append_round"           # human submitted a new round → gap.json patch
M_EDIT_ROUND = "edit_round"               # webapp asks runner to edit the latest round
M_LOG_APPEND = "log_append"               # append a {datetime, severity, category, message} entry
M_DELETE_GAP = "delete_gap"               # remove gap.json + worktree as appropriate
M_CHAT_START = "chat_start"               # spawn an interactive `claude` subprocess in a worktree
M_CHAT_INPUT = "chat_input"               # feed input line to running chat
M_CHAT_READ = "chat_read"                 # drain queued output lines + liveness
M_CHAT_STOP = "chat_stop"                 # end a chat session
M_RUNNING = "running"                     # query: which subprocesses are running
M_DIAGNOSTICS = "diagnostics"             # last-contact, recent IPC errors


def envelope_ok(req_id: str, result: dict | None = None) -> dict:
    return {"id": req_id, "ok": True, "result": result or {}}


def envelope_err(req_id: str, code: str, message: str, details: str | None = None) -> dict:
    err: dict = {"code": code, "message": message}
    if details is not None:
        err["details"] = details
    return {"id": req_id, "ok": False, "error": err}
